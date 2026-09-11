//! LlamaParse / LlamaExtract (LlamaCloud) document parsing and extraction.
//!
//! Parse flow: `POST /api/v1/parsing/upload` (multipart `file` or `input_url`, plus `tier` +
//! `version`) → poll `GET /api/v1/parsing/job/{id}` → `GET /api/v1/parsing/job/{id}/result/json`.
//!
//! Extract flow: `POST /api/v1/beta/files` (multipart, `purpose=extract`) → `POST /api/v2/extract`
//! with an inline `configuration.data_schema` → poll
//! `GET /api/v2/extract/{id}?expand=usage&expand=extract_metadata`.

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::types::{
    BBox, Block, BlockType, Citation, DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, FieldInfo,
    OutputFormat, Page, ParseResponse, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME: &str = "llamaparse";
const ENV_KEY: &str = "LLAMA_API_KEY";
const ENV_BASE: &str = "LLAMA_BASE_URL";
const DEFAULT_BASE: &str = "https://api.cloud.llamaindex.ai";

#[derive(Debug, Default, Clone, Copy)]
pub struct LlamaParse;

#[async_trait]
impl Provider for LlamaParse {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let api_key = provider::resolve_api_key(request, ENV_KEY, NAME)?;
        let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
        let deadline = Deadline::new(request.timeout_secs);
        let retry = Retry::new(request.max_retries);
        let client = http::client();
        let fields = form_fields(request, model)?;
        let data = provider::load_bytes(&request.input).await?;

        // 1. Upload / submit job.
        let job: ParsingJob = http::with_retry(NAME, retry, &deadline, || {
            let mut form = reqwest::multipart::Form::new();
            for (k, v) in &fields {
                form = form.text(k.clone(), v.clone());
            }
            form = match (&data, &request.input) {
                (Some(bytes), input) => form.part("file", provider::file_part(bytes.clone(), input)),
                (None, DocumentInput::Url { url }) => form.text("input_url", url.clone()),
                (None, _) => unreachable!("load_bytes returns bytes for non-URL inputs"),
            };
            let rb = client
                .post(format!("{base}/api/v1/parsing/upload"))
                .bearer_auth(&api_key)
                .header("accept", "application/json")
                .timeout(deadline.request_timeout())
                .multipart(form);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await?;
        let job_id = job.id.clone();
        tracing::debug!(job_id = %job_id, status = %job.status, "llamaparse: job created");

        // 2. Poll.
        let job = if is_terminal(&job.status) {
            job
        } else {
            http::poll_until(NAME, &deadline, Duration::from_secs(1), Duration::from_secs(5), || {
                let rb = client
                    .get(format!("{base}/api/v1/parsing/job/{job_id}"))
                    .bearer_auth(&api_key)
                    .timeout(deadline.request_timeout());
                async move {
                    let job: ParsingJob = http::read_json(NAME, rb.send().await?).await?;
                    Ok(if is_terminal(&job.status) { Some(job) } else { None })
                }
            })
            .await
            .map_err(|e| e.with_job_id(job_id.clone()))?
        };
        if !matches!(job.status.as_str(), "SUCCESS" | "PARTIAL_SUCCESS") {
            let code = job.error_code.clone().unwrap_or_default();
            let msg = job.error_message.clone().unwrap_or_default();
            let kind = if code.starts_with("INVALID") { ErrorKind::BadRequest } else { ErrorKind::Provider };
            return Err(Error::new(kind, format!("job {}: {code} {msg}", job.status).trim().to_string())
                .with_provider(NAME)
                .with_job_id(job_id));
        }

        // 3. JSON result (pages + items + metadata).
        let result: JsonResult = http::with_retry(NAME, retry, &deadline, || {
            let rb = client
                .get(format!("{base}/api/v1/parsing/job/{job_id}/result/json"))
                .bearer_auth(&api_key)
                .timeout(deadline.request_timeout());
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await
        .map_err(|e| e.with_job_id(job_id.clone()))?;

        let raw = if request.include_raw { Some(serde_json::to_value(&result)?) } else { None };
        let mut resp = normalize(&result, request.output, model);
        resp.provider_job_id = Some(job_id);
        if job.status == "PARTIAL_SUCCESS" {
            resp.metadata.insert("llamaparse_partial_success".into(), json!(true));
        }
        resp.raw = raw;
        Ok(resp)
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let doc = &request.document;
        let api_key = provider::resolve_api_key(doc, ENV_KEY, NAME)?;
        let base = provider::resolve_base_url(doc, ENV_BASE, DEFAULT_BASE);
        let deadline = Deadline::new(doc.timeout_secs);
        let retry = Retry::new(doc.max_retries);
        let client = http::client();
        let config = build_extract_config(request, model)?;

        // 1. LlamaExtract only takes a file id, so URL inputs are fetched and re-uploaded.
        let data = match provider::load_bytes(&doc.input).await? {
            Some(data) => data,
            None => {
                let DocumentInput::Url { url } = &doc.input else { unreachable!() };
                let resp = client.get(url).timeout(deadline.request_timeout()).send().await?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(Error::input(format!("could not download {url}: HTTP {status}")));
                }
                resp.bytes().await?
            }
        };
        let file: UploadedFile = http::with_retry(NAME, retry, &deadline, || {
            let form = reqwest::multipart::Form::new()
                .text("purpose", "extract")
                .part("file", provider::file_part(data.clone(), &doc.input));
            let rb = client
                .post(format!("{base}/api/v1/beta/files"))
                .bearer_auth(&api_key)
                .header("accept", "application/json")
                .timeout(deadline.request_timeout())
                .multipart(form);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await?;
        tracing::debug!(file_id = %file.id, "llamaparse: uploaded for extraction");

        // 2. Create the extraction job.
        let body = json!({ "file_input": file.id, "configuration": config });
        let job: ExtractJob = http::with_retry(NAME, retry, &deadline, || {
            let rb = client
                .post(format!("{base}/api/v2/extract"))
                .bearer_auth(&api_key)
                .timeout(deadline.request_timeout())
                .json(&body);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await?;
        let job_id = job.id.clone();
        tracing::debug!(job_id = %job_id, status = %job.status, "llamaparse: extract job created");

        // 3. Poll. `expand` is repeated once per field; usage and citations are left out without it.
        let poll_url = format!("{base}/api/v2/extract/{job_id}?expand=usage&expand=extract_metadata");
        let job = http::poll_until(NAME, &deadline, Duration::from_secs(1), Duration::from_secs(5), || {
            let rb = client.get(&poll_url).bearer_auth(&api_key).timeout(deadline.request_timeout());
            async move {
                let job: ExtractJob = http::read_json(NAME, rb.send().await?).await?;
                Ok(if is_extract_terminal(&job.status) { Some(job) } else { None })
            }
        })
        .await
        .map_err(|e| e.with_job_id(job_id.clone()))?;

        if job.status != "COMPLETED" {
            let msg = job.error_message.clone().unwrap_or_default();
            return Err(Error::new(
                ErrorKind::Provider,
                format!("extract job {}: {msg}", job.status).trim().to_string(),
            )
            .with_provider(NAME)
            .with_job_id(job_id));
        }
        let result = job.extract_result.clone().ok_or_else(|| {
            Error::provider("completed extract job has no result").with_provider(NAME).with_job_id(job_id.clone())
        })?;

        let raw = if doc.include_raw { Some(serde_json::to_value(&job)?) } else { None };
        let mut resp = normalize_extract(&job, result, model);
        resp.provider_job_id = Some(job_id);
        resp.raw = raw;
        Ok(resp)
    }
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "SUCCESS" | "PARTIAL_SUCCESS" | "ERROR" | "CANCELLED")
}

fn is_extract_terminal(status: &str) -> bool {
    matches!(status, "COMPLETED" | "FAILED" | "CANCELLED")
}

/// Credits billed per page by extract tier (used only to recover a page count when a result
/// carries no citations). Source: the `tier` field docs on the v2 extract configuration.
fn tier_credits_per_page(tier: &str) -> Option<f64> {
    match tier {
        "cost_effective" => Some(5.0),
        "agentic" => Some(15.0),
        "turbo" => Some(35.0),
        "agentic_plus" => Some(50.0),
        _ => None,
    }
}

fn build_extract_config(request: &ExtractRequest, model: &str) -> Result<Value> {
    let doc = &request.document;
    // `fast` is a parse-only tier; LlamaExtract offers cost_effective / agentic / agentic_plus / turbo.
    let tier = match model {
        "cost_effective" | "agentic" | "agentic_plus" | "turbo" => model,
        other => return Err(Error::unsupported_model(format!("llamaparse: unknown extract tier '{other}'"))),
    };
    let mut config = json!({
        "tier": tier,
        "data_schema": request.schema,
        "cite_sources": request.citations,
        "confidence_scores": request.citations,
    });
    if let Some(prompt) = request.instructions.as_deref().filter(|s| !s.trim().is_empty()) {
        config["system_prompt"] = json!(prompt);
    }
    if let Some(pages) = &doc.pages {
        // v2 `target_pages` is 1-based (unlike the 0-based `target_pages` on the parsing endpoint).
        let spec: Vec<String> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) if e == s => s.to_string(),
                Some(e) => format!("{s}-{e}"),
                None => format!("{s}-100000"),
            })
            .collect();
        config["target_pages"] = json!(spec.join(","));
    }
    if let Some(Value::Object(opts)) = &doc.provider_options {
        if let Value::Object(c) = &mut config {
            for (k, v) in opts {
                c.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(config)
}

/// Multipart text fields for the upload call.
fn form_fields(request: &DocumentRequest, model: &str) -> Result<Vec<(String, String)>> {
    let tier = match model {
        "fast" | "cost_effective" | "agentic" | "agentic_plus" => model,
        other => return Err(Error::unsupported_model(format!("llamaparse: unknown tier '{other}'"))),
    };
    let mut fields: Vec<(String, String)> = vec![("tier".into(), tier.into()), ("version".into(), "latest".into())];
    if let Some(lang) = &request.language {
        fields.push(("language".into(), lang.clone()));
    }
    if let Some(pages) = &request.pages {
        // LlamaParse's `target_pages` is 0-based; ours is 1-based.
        let spec: Vec<String> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) if e == s => format!("{}", s - 1),
                Some(e) => format!("{}-{}", s - 1, e - 1),
                None => format!("{}-100000", s - 1),
            })
            .collect();
        fields.push(("target_pages".into(), spec.join(",")));
    }
    if let Some(Value::Object(opts)) = &request.provider_options {
        for (k, v) in opts {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Null => continue,
                other => other.to_string(),
            };
            // Later entries win in multipart for scalar fields; drop our default for overrides.
            fields.retain(|(key, _)| key != k);
            fields.push((k.clone(), s));
        }
    }
    Ok(fields)
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct ParsingJob {
    id: String,
    status: String,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
}

// ---- extract wire types --------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct UploadedFile {
    id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ExtractJob {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub extract_result: Option<Value>,
    #[serde(default)]
    pub extract_metadata: Option<ExtractMetadata>,
    #[serde(default)]
    pub usage: Option<ExtractUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ExtractMetadata {
    #[serde(default)]
    pub field_metadata: Option<FieldMetadata>,
    #[serde(default)]
    pub parse_job_id: Option<String>,
    #[serde(default)]
    pub parse_tier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct FieldMetadata {
    /// Mirrors the shape of `extract_result`: objects keyed by field, arrays by position, and
    /// `{citation, confidence, …}` entries at the leaves.
    #[serde(default)]
    pub document_metadata: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ExtractUsage {
    /// Total for the job: extraction plus the parse it triggers.
    #[serde(default)]
    pub credits: Option<f64>,
    #[serde(default)]
    pub extract_credits: Option<f64>,
    #[serde(default)]
    pub parse_credits: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct WireCitation {
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    matching_text: Option<String>,
    #[serde(default)]
    bounding_boxes: Vec<CitationBox>,
    #[serde(default)]
    page_dimensions: Option<PageDimensions>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct CitationBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct PageDimensions {
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct JsonResult {
    #[serde(default)]
    pub pages: Vec<WirePage>,
    #[serde(default)]
    pub job_metadata: Option<JobMetadata>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct JobMetadata {
    #[serde(default)]
    pub job_pages: Option<u32>,
    #[serde(default)]
    pub job_credits_usage: Option<f64>,
    #[serde(default)]
    pub credits_used: Option<f64>,
    #[serde(default)]
    pub job_is_cache_hit: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WirePage {
    pub page: u32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub md: String,
    #[serde(default)]
    pub items: Vec<WireItem>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireItem {
    #[serde(rename = "type", default)]
    pub item_type: String,
    #[serde(default)]
    pub md: Option<String>,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub lvl: Option<u32>,
    #[serde(rename = "bBox", default)]
    pub bbox: Option<WireBBox>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireBBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub confidence: Option<f64>,
}

// ---- normalisation -----------------------------------------------------------------------------

fn map_item_type(t: &str, lvl: Option<u32>) -> BlockType {
    match t {
        "heading" => {
            if lvl == Some(1) {
                BlockType::Title
            } else {
                BlockType::SectionHeader
            }
        }
        "text" => BlockType::Text,
        "table" => BlockType::Table,
        "list" | "list_item" => BlockType::List,
        "figure" | "image" | "chart" => BlockType::Figure,
        "formula" | "equation" => BlockType::Formula,
        "header" => BlockType::Header,
        "footer" => BlockType::Footer,
        _ => BlockType::Other,
    }
}

pub(crate) fn normalize(result: &JsonResult, fmt: OutputFormat, model: &str) -> ParseResponse {
    let mut pages: Vec<Page> = Vec::new();
    for wp in &result.pages {
        let dims = wp.width.zip(wp.height);
        let blocks: Vec<Block> = wp
            .items
            .iter()
            .map(|it| {
                let md = it.md.clone().unwrap_or_default();
                let text = it.value.as_ref().and_then(|v| v.as_str().map(String::from));
                let content = match fmt {
                    OutputFormat::Markdown => md.clone(),
                    OutputFormat::Text => text.clone().unwrap_or_else(|| crate::types::markdown_to_text(&md)),
                };
                let bbox =
                    it.bbox.as_ref().and_then(|b| dims.and_then(|(w, h)| BBox::from_xywh(b.x, b.y, b.w, b.h, w, h)));
                Block {
                    block_type: map_item_type(&it.item_type, it.lvl),
                    content,
                    text,
                    bbox,
                    confidence: it.bbox.as_ref().and_then(|b| b.confidence),
                    page_number: wp.page,
                }
            })
            .collect();
        let text =
            if wp.text.trim().is_empty() { crate::types::markdown_to_text(&wp.md) } else { wp.text.trim().to_string() };
        let markdown = match fmt {
            OutputFormat::Markdown => wp.md.trim().to_string(),
            OutputFormat::Text => text.clone(),
        };
        pages.push(Page { page_number: wp.page, width: wp.width, height: wp.height, markdown, text, blocks });
    }
    let meta = result.job_metadata.as_ref();
    let billed = meta.and_then(|m| m.job_pages).filter(|&p| p > 0).unwrap_or(pages.len() as u32);
    // LlamaParse reports 0 credits until billing has settled; only keep a positive value.
    let credits = meta.and_then(|m| m.job_credits_usage.or(m.credits_used)).filter(|&c| c > 0.0);
    let usage = Usage { pages: billed, credits, provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    if let Some(true) = meta.and_then(|m| m.job_is_cache_hit) {
        resp.metadata.insert("llamaparse_cache_hit".into(), json!(true));
    }
    resp
}

// ---- extract normalisation ------------------------------------------------------------------------

/// Keys a `field_metadata` leaf entry may carry; anything else is a nested schema node.
const LEAF_KEYS: &[&str] = &["citation", "confidence", "extraction_confidence", "parsing_confidence", "reasoning"];

fn is_leaf(map: &serde_json::Map<String, Value>) -> bool {
    !map.is_empty() && map.keys().all(|k| LEAF_KEYS.contains(&k.as_str()))
}

/// Append one unescaped segment to a JSON pointer (RFC 6901: `~` → `~0`, `/` → `~1`).
fn push_pointer(base: &str, segment: &str) -> String {
    format!("{base}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

/// Walk `document_metadata` (which mirrors the extracted data) collecting per-field citations.
fn walk_metadata(node: &Value, pointer: &str, fields: &mut BTreeMap<String, FieldInfo>, max_page: &mut u32) {
    match node {
        Value::Object(map) if is_leaf(map) => {
            let citations: Vec<WireCitation> = map
                .get("citation")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|c| serde_json::from_value::<WireCitation>(c.clone()).ok()).collect())
                .unwrap_or_default();
            let mut out = Vec::new();
            for c in &citations {
                let page_number = c.page.unwrap_or(1);
                *max_page = (*max_page).max(page_number);
                let text = c.matching_text.clone().filter(|t| !t.is_empty());
                let dims = c.page_dimensions;
                if c.bounding_boxes.is_empty() {
                    out.push(Citation { page_number, bbox: None, text: text.clone() });
                }
                for b in &c.bounding_boxes {
                    let bbox = dims.and_then(|d| BBox::from_xywh(b.x, b.y, b.w, b.h, d.width, d.height));
                    out.push(Citation { page_number, bbox, text: text.clone() });
                }
            }
            let confidence = map.get("confidence").or_else(|| map.get("extraction_confidence")).and_then(Value::as_f64);
            if confidence.is_some() || !out.is_empty() {
                fields.insert(pointer.to_string(), FieldInfo { confidence, citations: out });
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                walk_metadata(v, &push_pointer(pointer, k), fields, max_page);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                walk_metadata(v, &push_pointer(pointer, &i.to_string()), fields, max_page);
            }
        }
        _ => {}
    }
}

pub(crate) fn normalize_extract(job: &ExtractJob, data: Value, model: &str) -> ExtractResponse {
    let mut fields = BTreeMap::new();
    let mut max_page = 0u32;
    if let Some(meta) = job.extract_metadata.as_ref().and_then(|m| m.field_metadata.as_ref()) {
        if let Some(doc_meta) = &meta.document_metadata {
            walk_metadata(doc_meta, "", &mut fields, &mut max_page);
        }
    }
    // LlamaExtract reports no page count. Prefer the highest cited page; otherwise recover it from
    // the extraction credits, which are a flat per-page rate for the tier.
    let from_credits = job
        .usage
        .as_ref()
        .and_then(|u| u.extract_credits)
        .zip(tier_credits_per_page(model))
        .map(|(c, rate)| (c / rate).round() as u32)
        .filter(|&p| p > 0);
    let pages = if max_page > 0 { max_page } else { from_credits.unwrap_or(1) };
    let usage = Usage {
        pages,
        credits: job.usage.as_ref().and_then(|u| u.credits).filter(|&c| c > 0.0),
        provider_cost_usd: None,
    };
    let mut resp = ExtractResponse::new(NAME, &format!("{NAME}/{model}"), data, usage);
    resp.fields = fields;
    if let Some(meta) = &job.extract_metadata {
        if let Some(id) = &meta.parse_job_id {
            resp.metadata.insert("llamaparse_parse_job_id".into(), json!(id));
        }
        if let Some(tier) = &meta.parse_tier {
            resp.metadata.insert("llamaparse_parse_tier".into(), json!(tier));
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fixture() {
        let raw = include_str!("../../tests/fixtures/llamaparse_result_json.json");
        let result: JsonResult = serde_json::from_str(raw).unwrap();
        let resp = normalize(&result, OutputFormat::Markdown, "agentic");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.usage.credits, None, "zero credits must not be reported");
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"));
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Title);
        assert_eq!(resp.pages[1].blocks[0].block_type, BlockType::SectionHeader);
        assert!(resp.pages[1].blocks.iter().any(|b| b.block_type == BlockType::Table && b.content.contains("| Item")));
        let bb = resp.pages[0].blocks[0].bbox.unwrap();
        assert!((bb.x0 - 0.0602).abs() < 1e-3, "{bb:?}");
        assert_eq!(resp.pages[0].width, Some(1000.0));
        assert!(resp.text.contains("Widget"));
    }

    #[test]
    fn job_without_error_fields_parses() {
        let j: ParsingJob = serde_json::from_str(r#"{"id":"x","status":"SUCCESS"}"#).unwrap();
        assert_eq!(j.error_code, None);
        let j: ParsingJob =
            serde_json::from_str(r#"{"id":"x","status":"ERROR","error_code":"E","error_message":null}"#).unwrap();
        assert_eq!(j.error_code.as_deref(), Some("E"));
    }

    #[test]
    fn normalizes_extract_fixture() {
        let raw = include_str!("../../tests/fixtures/llamaparse_extract_job.json");
        let job: ExtractJob = serde_json::from_str(raw).unwrap();
        let data = job.extract_result.clone().unwrap();
        let resp = normalize_extract(&job, data, "agentic");

        assert_eq!(resp.data["title"], "A Short History of the Harbor");
        assert_eq!(resp.data["sites"][1]["site"], "Ash Grove");
        assert_eq!(resp.model, "llamaparse/agentic");
        // Two pages: the table rows are cited on page 2.
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.usage.credits, Some(50.0));
        assert_eq!(resp.metadata["llamaparse_parse_tier"], "agentic", "parse defaults to the extract tier");
        assert!(resp.metadata.contains_key("llamaparse_parse_job_id"));

        let title = &resp.fields["/title"];
        assert!(title.confidence.unwrap() > 0.9);
        assert_eq!(title.citations[0].page_number, 1);
        assert!(title.citations[0].text.as_deref().unwrap().contains("Harbor"));
        let bb = title.citations[0].bbox.unwrap();
        assert!((bb.x0 - 36.86 / 595.2).abs() < 1e-6, "{bb:?}");
        assert!((bb.y1 - (38.94 + 24.56) / 841.92).abs() < 1e-6, "{bb:?}");
        // Array items are addressed by index.
        let cell = &resp.fields["/sites/0/samples"];
        assert_eq!(cell.citations[0].page_number, 2);
        assert!(resp.fields.contains_key("/sites/1/site"));
        assert!(!resp.fields.contains_key("/sites"), "array nodes carry no metadata of their own");
    }

    #[test]
    fn page_count_falls_back_to_credits_when_uncited() {
        let job: ExtractJob = serde_json::from_str(
            r#"{"id":"ext-1","status":"COMPLETED","extract_result":{"a":1},
                "usage":{"credits":24.0,"extract_credits":15.0,"parse_credits":9.0}}"#,
        )
        .unwrap();
        // 15 extract credits at 5 credits/page (cost_effective) ⇒ 3 pages.
        let resp = normalize_extract(&job, json!({"a": 1}), "cost_effective");
        assert_eq!(resp.usage.pages, 3);
        assert_eq!(resp.usage.credits, Some(24.0));
        assert!(resp.fields.is_empty());
        // Unknown tier ⇒ no derivation, and never zero pages.
        assert_eq!(normalize_extract(&job, json!({}), "mystery").usage.pages, 1);
    }

    #[test]
    fn metadata_leaves_are_distinguished_from_schema_nodes() {
        // A schema field literally called `confidence` must not be mistaken for a leaf entry.
        let meta = json!({
            "confidence": { "citation": [{"page": 1, "matching_text": "9/10"}], "confidence": 0.5 },
            "nested": { "deep": { "extraction_confidence": 0.25 } }
        });
        let mut fields = BTreeMap::new();
        let mut max_page = 0;
        walk_metadata(&meta, "", &mut fields, &mut max_page);
        assert_eq!(fields["/confidence"].confidence, Some(0.5));
        assert_eq!(fields["/confidence"].citations[0].text.as_deref(), Some("9/10"));
        assert_eq!(fields["/confidence"].citations[0].bbox, None);
        assert_eq!(fields["/nested/deep"].confidence, Some(0.25));
        assert_eq!(max_page, 1);
    }

    #[test]
    fn extract_config_maps_tier_pages_and_citations() {
        let doc = DocumentRequest::from_url("https://x/y.pdf").pages("1-3,5");
        let req = ExtractRequest::new(doc, json!({"type": "object", "properties": {"a": {"type": "string"}}}))
            .instructions("Assume USD")
            .citations(true);
        let c = build_extract_config(&req, "agentic_plus").unwrap();
        assert_eq!(c["tier"], "agentic_plus");
        assert_eq!(c["data_schema"]["properties"]["a"]["type"], "string", "schema is passed through verbatim");
        assert_eq!(c["system_prompt"], "Assume USD");
        assert_eq!(c["cite_sources"], true);
        assert_eq!(c["confidence_scores"], true);
        assert_eq!(c["target_pages"], "1-3,5", "v2 target_pages is 1-based");

        let plain = ExtractRequest::new(
            DocumentRequest::from_url("u").provider_options(json!({"use_reasoning": true, "tier": "turbo"})),
            json!({"type": "object"}),
        );
        let c = build_extract_config(&plain, "cost_effective").unwrap();
        assert_eq!(c["cite_sources"], false);
        assert_eq!(c["use_reasoning"], true);
        assert_eq!(c["tier"], "turbo", "provider_options win over the model default");
        assert!(c.get("system_prompt").is_none());
        assert!(build_extract_config(&plain, "fast").is_err(), "fast is a parse-only tier");
    }

    #[test]
    fn form_fields_convert_pages_and_options() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .pages("1-3,5")
            .language("de")
            .provider_options(json!({"take_screenshot": true, "version": "2026-08-19"}));
        let f = form_fields(&req, "fast").unwrap();
        let get = |k: &str| f.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("tier"), Some("fast"));
        assert_eq!(get("version"), Some("2026-08-19"));
        assert_eq!(get("target_pages"), Some("0-2,4"));
        assert_eq!(get("language"), Some("de"));
        assert_eq!(get("take_screenshot"), Some("true"));
        assert_eq!(f.iter().filter(|(k, _)| k == "version").count(), 1);
        assert!(form_fields(&req, "turbo").is_err());
    }

    // ---- live ---------------------------------------------------------------------------------

    const LIVE_DOC: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/invoice_001.png");

    #[tokio::test]
    #[ignore = "needs LLAMA_API_KEY and network"]
    async fn live_extract() {
        if std::env::var("LLAMA_API_KEY").map(|v| v.trim().is_empty()).unwrap_or(true) {
            eprintln!("skipping llamaparse live extract: LLAMA_API_KEY not set");
            return;
        }
        let schema = json!({
            "type": "object",
            "properties": {
                "invoice_number": {"type": "string"},
                "total": {"type": "string"},
                "date": {"type": "string"},
                "vendor": {"type": "string"}
            },
            "required": ["invoice_number", "total"]
        });
        let doc = DocumentRequest::from_path(LIVE_DOC).timeout_secs(240.0);
        let req = ExtractRequest::new(doc, schema)
            .instructions("Extract the invoice header fields exactly as printed.")
            .citations(true);
        let resp = LlamaParse.extract(&req, "cost_effective").await.expect("llamaparse extract succeeds");
        assert_eq!(resp.data["invoice_number"], "INV-9865");
        assert!(resp.data["total"].as_str().unwrap().contains("14,667.43"), "{}", resp.data["total"]);
        assert_eq!(resp.data["vendor"], "Cedar Ridge Supply");
        assert_eq!(resp.usage.pages, 1);
        assert!(resp.provider_job_id.is_some());
        let f = &resp.fields["/invoice_number"];
        assert_eq!(f.citations[0].page_number, 1);
        assert!(f.citations[0].bbox.is_some());
        assert!(f.confidence.unwrap() > 0.5);
    }
}
