//! LlamaParse (LlamaCloud) document parsing.
//!
//! Flow: `POST /api/v1/parsing/upload` (multipart `file` or `input_url`, plus `tier` + `version`)
//! → poll `GET /api/v1/parsing/job/{id}` → `GET /api/v1/parsing/job/{id}/result/json`.

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, OcrProvider};
use crate::types::{BBox, Block, BlockType, DocumentInput, OcrRequest, OcrResponse, OutputFormat, Page, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

pub const NAME: &str = "llamaparse";
const ENV_KEY: &str = "LLAMA_API_KEY";
const ENV_BASE: &str = "LLAMA_BASE_URL";
const DEFAULT_BASE: &str = "https://api.cloud.llamaindex.ai";

#[derive(Debug, Default, Clone, Copy)]
pub struct LlamaParse;

#[async_trait]
impl OcrProvider for LlamaParse {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn ocr(&self, request: &OcrRequest, model: &str) -> Result<OcrResponse> {
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
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "SUCCESS" | "PARTIAL_SUCCESS" | "ERROR" | "CANCELLED")
}

/// Multipart text fields for the upload call.
fn form_fields(request: &OcrRequest, model: &str) -> Result<Vec<(String, String)>> {
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

pub(crate) fn normalize(result: &JsonResult, fmt: OutputFormat, model: &str) -> OcrResponse {
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
    let mut resp = OcrResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    if let Some(true) = meta.and_then(|m| m.job_is_cache_hit) {
        resp.metadata.insert("llamaparse_cache_hit".into(), json!(true));
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
    fn form_fields_convert_pages_and_options() {
        let req = OcrRequest::from_url("https://x/y.pdf")
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
}
