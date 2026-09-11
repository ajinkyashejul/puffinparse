//! Reducto (reducto.ai) document parsing.
//!
//! Flow: `POST /upload` (multipart) unless the input is a URL → `POST /parse` (sync, default) or
//! `POST /parse_async` + `GET /job/{id}` when `provider_options.async == true`.
//! Results with `result.type == "url"` are fetched from the presigned URL.

use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::types::{BBox, Block, BlockType, DocumentInput, DocumentRequest, OutputFormat, Page, ParseResponse, Usage};
use crate::util::deep_merge;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME: &str = "reducto";
const ENV_KEY: &str = "REDUCTO_API_KEY";
const ENV_BASE: &str = "REDUCTO_BASE_URL";
const DEFAULT_BASE: &str = "https://platform.reducto.ai";

#[derive(Debug, Default, Clone, Copy)]
pub struct Reducto;

#[async_trait]
impl Provider for Reducto {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let api_key = provider::resolve_api_key(request, ENV_KEY, NAME)?;
        let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
        let deadline = Deadline::new(request.timeout_secs);
        let retry = Retry::new(request.max_retries);
        let client = http::client();

        // 1. Input reference.
        let input = match provider::load_bytes(&request.input).await? {
            None => {
                let DocumentInput::Url { url } = &request.input else { unreachable!() };
                url.clone()
            }
            Some(data) => {
                let upload: UploadResponse = http::with_retry(NAME, retry, &deadline, || {
                    let form =
                        reqwest::multipart::Form::new().part("file", provider::file_part(data.clone(), &request.input));
                    let rb = client
                        .post(format!("{base}/upload"))
                        .bearer_auth(&api_key)
                        .timeout(deadline.request_timeout())
                        .multipart(form);
                    async move { http::read_json(NAME, rb.send().await?).await }
                })
                .await?;
                tracing::debug!(file_id = %upload.file_id, "reducto: uploaded");
                upload.file_id
            }
        };

        // 2. Body.
        let use_async = request.option("async").and_then(Value::as_bool).unwrap_or(false);
        let body = build_body(request, model, &input)?;

        // 3. Parse (sync or async + poll).
        let parsed: WireParseResponse = if use_async {
            let submit: AsyncParseResponse = http::with_retry(NAME, retry, &deadline, || {
                let rb = client
                    .post(format!("{base}/parse_async"))
                    .bearer_auth(&api_key)
                    .timeout(deadline.request_timeout())
                    .json(&body);
                async move { http::read_json(NAME, rb.send().await?).await }
            })
            .await?;
            let job_id = submit.job_id.clone();
            tracing::debug!(job_id = %job_id, "reducto: async job submitted");
            let job = http::poll_until(NAME, &deadline, Duration::from_secs(1), Duration::from_secs(8), || {
                let rb = client
                    .get(format!("{base}/job/{job_id}"))
                    .bearer_auth(&api_key)
                    .timeout(deadline.request_timeout());
                async move {
                    let job: JobResponse = http::read_json(NAME, rb.send().await?).await?;
                    Ok(match job.status.as_str() {
                        "Completed" | "Failed" | "Cancelled" => Some(job),
                        _ => None,
                    })
                }
            })
            .await
            .map_err(|e| e.with_job_id(job_id.clone()))?;
            if job.status != "Completed" {
                let reason = job
                    .reason
                    .or_else(|| {
                        job.error.as_ref().and_then(|e| e.get("message")).and_then(Value::as_str).map(String::from)
                    })
                    .unwrap_or_else(|| "unknown".into());
                return Err(Error::provider(format!("job {}: {reason}", job.status))
                    .with_provider(NAME)
                    .with_job_id(job_id));
            }
            let result = job.result.ok_or_else(|| {
                Error::provider("completed job has no result").with_provider(NAME).with_job_id(job_id.clone())
            })?;
            serde_json::from_value(result).map_err(|e| {
                Error::provider(format!("unexpected job result shape: {e}")).with_provider(NAME).with_job_id(job_id)
            })?
        } else {
            http::with_retry(NAME, retry, &deadline, || {
                let rb = client
                    .post(format!("{base}/parse"))
                    .bearer_auth(&api_key)
                    .timeout(deadline.request_timeout())
                    .json(&body);
                async move { http::read_json(NAME, rb.send().await?).await }
            })
            .await?
        };

        // 4. Large results come back as a presigned URL holding the same `FullResult` object.
        let job_id = parsed.job_id.clone();
        let full = match &parsed.result {
            ParseResult::Full(f) => f.clone(),
            ParseResult::Url { url, .. } => {
                let resp = client.get(url).timeout(deadline.request_timeout()).send().await?;
                match http::read_json::<ParseResult>(NAME, resp).await? {
                    ParseResult::Full(f) => f,
                    ParseResult::Url { .. } => {
                        return Err(Error::provider("result URL returned another URL")
                            .with_provider(NAME)
                            .with_job_id(job_id))
                    }
                }
            }
        };

        let raw = if request.include_raw { Some(serde_json::to_value(&parsed)?) } else { None };
        let mut resp = normalize(&parsed, &full, request.output, model);
        resp.provider_job_id = Some(job_id);
        resp.raw = raw;
        Ok(resp)
    }
}

fn build_body(request: &DocumentRequest, model: &str, input: &str) -> Result<Value> {
    let mut body = json!({
        "input": input,
        "retrieval": { "chunking": { "chunk_mode": "page" } },
        "formatting": { "table_output_format": "md" },
        "settings": {},
    });
    match model {
        "standard" => {}
        "r-1" => body["settings"]["model"] = json!("r-1"),
        "agentic" => body["enhance"] = json!({ "agentic": [{ "scope": "text" }, { "scope": "table" }] }),
        other => return Err(Error::unsupported_model(format!("reducto: unknown model '{other}'"))),
    }
    if let Some(pages) = &request.pages {
        let ranges: Vec<Value> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) => json!({ "start": s, "end": e }),
                None => json!({ "start": s }),
            })
            .collect();
        body["settings"]["page_range"] = Value::Array(ranges);
    }
    if let Some(opts) = &request.provider_options {
        let mut patch = opts.clone();
        if let Value::Object(o) = &mut patch {
            o.remove("async");
        }
        deep_merge(&mut body, &patch);
    }
    Ok(body)
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UploadResponse {
    file_id: String,
}

#[derive(Debug, Deserialize)]
struct AsyncParseResponse {
    job_id: String,
}

#[derive(Debug, Deserialize)]
struct JobResponse {
    status: String,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireParseResponse {
    pub job_id: String,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub usage: Option<ParseUsage>,
    pub result: ParseResult,
    #[serde(default)]
    pub studio_link: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ParseUsage {
    #[serde(default)]
    pub num_pages: u32,
    #[serde(default)]
    pub credits: Option<f64>,
    #[serde(default)]
    pub credit_breakdown: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum ParseResult {
    Full(FullResult),
    Url {
        url: String,
        #[serde(default)]
        result_id: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct FullResult {
    #[serde(default)]
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Chunk {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub blocks: Vec<WireBlock>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireBlock {
    #[serde(rename = "type", default)]
    pub block_type: String,
    #[serde(default)]
    pub bbox: Option<WireBBox>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub granular_confidence: Option<GranularConfidence>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct GranularConfidence {
    #[serde(default)]
    pub parse_confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireBBox {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default = "one")]
    pub page: u32,
    #[serde(default)]
    pub original_page: Option<u32>,
}

fn one() -> u32 {
    1
}

// ---- normalisation -----------------------------------------------------------------------------

fn map_block_type(t: &str) -> BlockType {
    match t {
        "Text" | "Key Value" | "Comment" => BlockType::Text,
        "Title" => BlockType::Title,
        "Section Header" => BlockType::SectionHeader,
        "List Item" => BlockType::List,
        "Table" => BlockType::Table,
        "Figure" => BlockType::Figure,
        "Header" => BlockType::Header,
        "Footer" => BlockType::Footer,
        "Footnote" => BlockType::Footnote,
        "Caption" => BlockType::Caption,
        "Formula" | "Equation" => BlockType::Formula,
        _ => BlockType::Other,
    }
}

pub(crate) fn normalize(
    parsed: &WireParseResponse,
    full: &FullResult,
    fmt: OutputFormat,
    model: &str,
) -> ParseResponse {
    let mut blocks: Vec<Block> = Vec::new();
    // Chunk content is the provider's own markdown rendering; keep it when a chunk maps to one page.
    let mut page_md: BTreeMap<u32, String> = BTreeMap::new();
    for chunk in &full.chunks {
        let pages: std::collections::BTreeSet<u32> =
            chunk.blocks.iter().filter_map(|b| b.bbox.as_ref().map(|bb| bb.page)).collect();
        if pages.len() == 1 && !chunk.content.trim().is_empty() {
            let p = *pages.iter().next().unwrap();
            page_md
                .entry(p)
                .and_modify(|s| {
                    s.push_str("\n\n");
                    s.push_str(&chunk.content)
                })
                .or_insert_with(|| chunk.content.clone());
        }
        for b in &chunk.blocks {
            let page_number = b.bbox.as_ref().map(|bb| bb.page).unwrap_or(1);
            let bbox = b.bbox.as_ref().map(|bb| BBox::from_normalized_ltwh(bb.left, bb.top, bb.width, bb.height));
            let coarse = match b.confidence.as_deref() {
                Some("high") => Some(0.9),
                Some("low") => Some(0.5),
                _ => None,
            };
            let confidence = b.granular_confidence.as_ref().and_then(|g| g.parse_confidence).or(coarse);
            let content = match fmt {
                OutputFormat::Markdown => b.content.clone(),
                OutputFormat::Text => crate::types::markdown_to_text(&b.content),
            };
            blocks.push(Block {
                block_type: map_block_type(&b.block_type),
                content,
                text: None,
                bbox,
                confidence,
                page_number,
            });
        }
    }
    let mut pages: Vec<Page> = crate::types::pages_from_blocks(blocks, &BTreeMap::new());
    for p in &mut pages {
        if let Some(md) = page_md.get(&p.page_number) {
            p.markdown = md.clone();
            p.text = crate::types::markdown_to_text(md);
        }
    }
    for (n, md) in &page_md {
        if !pages.iter().any(|p| p.page_number == *n) {
            pages.push(Page {
                page_number: *n,
                width: None,
                height: None,
                markdown: md.clone(),
                text: crate::types::markdown_to_text(md),
                blocks: vec![],
            });
        }
    }
    if let OutputFormat::Text = fmt {
        for p in &mut pages {
            p.markdown = p.text.clone();
        }
    }
    let num_pages = parsed.usage.as_ref().map(|u| u.num_pages).filter(|&n| n > 0).unwrap_or(pages.len() as u32);
    let usage =
        Usage { pages: num_pages, credits: parsed.usage.as_ref().and_then(|u| u.credits), provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    if let Some(d) = parsed.duration {
        resp.metadata.insert("reducto_duration_s".into(), json!(d));
    }
    if let Some(link) = &parsed.studio_link {
        resp.metadata.insert("reducto_studio_link".into(), json!(link));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fixture() {
        let raw = include_str!("../../tests/fixtures/reducto_parse.json");
        let parsed: WireParseResponse = serde_json::from_str(raw).unwrap();
        let ParseResult::Full(full) = &parsed.result else { panic!("expected full") };
        let resp = normalize(&parsed, full, OutputFormat::Markdown, "standard");
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.usage.credits, Some(1.0));
        assert_eq!(resp.pages[0].blocks.len(), 3);
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Title);
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"));
        let bb = resp.pages[0].blocks[0].bbox.unwrap();
        assert!((bb.x0 - 0.1193).abs() < 1e-3 && (bb.x1 - (0.1193 + 0.2475)).abs() < 1e-3);
        assert!(resp.pages[0].blocks[0].confidence.unwrap() > 0.9);
        assert!(resp.metadata.contains_key("reducto_duration_s"));
    }

    #[test]
    fn url_result_variant_parses() {
        let v: ParseResult =
            serde_json::from_str(r#"{"type":"url","url":"https://x/y.json","result_id":"abc"}"#).unwrap();
        assert!(matches!(v, ParseResult::Url { .. }));
    }

    #[test]
    fn body_variants() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .pages("2-3")
            .provider_options(json!({"async": true, "settings": {"ocr_system": "legacy"}}));
        let b = build_body(&req, "agentic", "https://x/y.pdf").unwrap();
        assert_eq!(b["input"], "https://x/y.pdf");
        assert_eq!(b["enhance"]["agentic"][0]["scope"], "text");
        assert_eq!(b["settings"]["page_range"][0]["start"], 2);
        assert_eq!(b["settings"]["ocr_system"], "legacy");
        assert!(b.get("async").is_none());
        let b = build_body(&DocumentRequest::from_url("u"), "r-1", "u").unwrap();
        assert_eq!(b["settings"]["model"], "r-1");
        assert!(build_body(&DocumentRequest::from_url("u"), "nope", "u").is_err());
    }
}
