//! Extend (extend.ai) document parsing.
//!
//! Flow: upload (multipart `POST /files/upload`) unless the input is a URL → `POST /parse_runs`
//! → poll `GET /parse_runs/{id}` until `PROCESSED` / `FAILED`.
//! API version pinned via the mandatory `x-extend-api-version` header.

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, OcrProvider};
use crate::types::{BBox, Block, BlockType, DocumentInput, OcrRequest, OcrResponse, OutputFormat, Page, Usage};
use crate::util::deep_merge;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME: &str = "extend";
pub const API_VERSION: &str = "2026-02-09";
const ENV_KEY: &str = "EXTEND_API_KEY";
const ENV_BASE: &str = "EXTEND_BASE_URL";
const DEFAULT_BASE: &str = "https://api.extend.ai";

#[derive(Debug, Default, Clone, Copy)]
pub struct Extend;

#[async_trait]
impl OcrProvider for Extend {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn ocr(&self, request: &OcrRequest, model: &str) -> Result<OcrResponse> {
        let api_key = provider::resolve_api_key(request, ENV_KEY, NAME)?;
        let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
        let deadline = Deadline::new(request.timeout_secs);
        let retry = Retry::new(request.max_retries);
        let client = http::client();
        let headers = |rb: reqwest::RequestBuilder| {
            let rb = rb.bearer_auth(&api_key).header("x-extend-api-version", API_VERSION);
            match request.option("workspace_id").and_then(Value::as_str) {
                Some(ws) => rb.header("x-extend-workspace-id", ws),
                None => rb,
            }
        };

        // 1. File reference: URL directly, or upload bytes.
        let file_ref = match provider::load_bytes(&request.input).await? {
            None => {
                let DocumentInput::Url { url } = &request.input else { unreachable!() };
                json!({ "url": url, "name": request.input.filename() })
            }
            Some(data) => {
                let upload: UploadResponse = http::with_retry(NAME, retry, &deadline, || {
                    let form =
                        reqwest::multipart::Form::new().part("file", provider::file_part(data.clone(), &request.input));
                    let rb = headers(client.post(format!("{base}/files/upload")))
                        .timeout(deadline.request_timeout())
                        .multipart(form);
                    async move { http::read_json(NAME, rb.send().await?).await }
                })
                .await?;
                tracing::debug!(file_id = %upload.id, "extend: uploaded");
                json!({ "id": upload.id })
            }
        };

        // 2. Build config.
        let body = build_body(request, model, file_ref)?;

        // 3. Create async run.
        let run: ParseRun = http::with_retry(NAME, retry, &deadline, || {
            let rb = headers(client.post(format!("{base}/parse_runs"))).timeout(deadline.request_timeout()).json(&body);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await?;
        let run_id = run.id.clone();
        tracing::debug!(run_id = %run_id, status = %run.status, "extend: run created");

        // 4. Poll.
        let run = if run.status == "PROCESSED" || run.status == "FAILED" {
            run
        } else {
            http::poll_until(NAME, &deadline, Duration::from_secs(1), Duration::from_secs(10), || {
                let rb = headers(client.get(format!("{base}/parse_runs/{run_id}"))).timeout(deadline.request_timeout());
                async move {
                    let run: ParseRun = http::read_json(NAME, rb.send().await?).await?;
                    Ok(match run.status.as_str() {
                        "PROCESSED" | "FAILED" => Some(run),
                        _ => None,
                    })
                }
            })
            .await
            .map_err(|e| e.with_job_id(run_id.clone()))?
        };

        if run.status == "FAILED" {
            let reason = run.failure_reason.clone().unwrap_or_else(|| "UNKNOWN".into());
            let msg = run.failure_message.clone().unwrap_or_default();
            let kind = match reason.as_str() {
                "OCR_ERROR" | "INTERNAL_ERROR" => ErrorKind::Provider,
                "OUT_OF_CREDITS" => ErrorKind::Authentication,
                _ => ErrorKind::BadRequest,
            };
            return Err(Error::new(kind, format!("parse run failed: {reason}: {msg}"))
                .with_provider(NAME)
                .with_job_id(run_id));
        }

        // 5. Output (inline, or via presigned URL when responseType=url was requested).
        let output = match (run.output.clone(), run.output_url.as_deref()) {
            (Some(o), _) => o,
            (None, Some(url)) => {
                let resp = client.get(url).timeout(deadline.request_timeout()).send().await?;
                http::read_json::<Output>(NAME, resp).await?
            }
            (None, None) => {
                return Err(Error::provider("run finished without output").with_provider(NAME).with_job_id(run_id))
            }
        };

        let raw = if request.include_raw { Some(serde_json::to_value(&run)?) } else { None };
        let mut resp = normalize(&run, output, request.output);
        resp.provider_job_id = Some(run_id);
        resp.raw = raw;
        Ok(resp)
    }
}

fn build_body(request: &OcrRequest, model: &str, file_ref: Value) -> Result<Value> {
    let engine = match model {
        "parse_performance" | "parse_light" | "parse_auto" => model,
        other => return Err(Error::unsupported_model(format!("extend: unknown engine '{other}'"))),
    };
    let table_format = match request.output {
        OutputFormat::Markdown => "markdown",
        OutputFormat::Text => "markdown",
    };
    let mut config = json!({
        "target": "markdown",
        "chunkingStrategy": { "type": "page" },
        "engine": engine,
        "blockOptions": { "tables": { "targetFormat": table_format } },
    });
    if let Some(pages) = &request.pages {
        let ranges: Vec<Value> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) => json!({ "start": s, "end": e }),
                None => json!({ "start": s, "end": 750 }),
            })
            .collect();
        config["advancedOptions"] = json!({ "pageRanges": ranges });
    }
    let mut body = json!({ "file": file_ref, "config": config });
    if let Some(opts) = &request.provider_options {
        // Options may target `config` directly or be given at the top level (metadata, dataRetention).
        const CONFIG_KEYS: &[&str] =
            &["target", "chunkingStrategy", "engine", "engineVersion", "blockOptions", "advancedOptions"];
        let mut patch = opts.clone();
        if let Value::Object(o) = &mut patch {
            o.remove("workspace_id");
            let mut cfg = serde_json::Map::new();
            for k in CONFIG_KEYS {
                if let Some(v) = o.remove(*k) {
                    cfg.insert((*k).to_string(), v);
                }
            }
            if !cfg.is_empty() {
                let mut merged = o.remove("config").unwrap_or_else(|| json!({}));
                deep_merge(&mut merged, &Value::Object(cfg));
                o.insert("config".into(), merged);
            }
        }
        deep_merge(&mut body, &patch);
    }
    Ok(body)
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UploadResponse {
    id: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParseRun {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub output: Option<Output>,
    #[serde(default)]
    pub output_url: Option<String>,
    #[serde(default)]
    pub metrics: Option<Metrics>,
    #[serde(default)]
    pub usage: Option<RunUsage>,
    #[serde(default)]
    pub file: Option<Value>,
    #[serde(default)]
    pub config: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Output {
    #[serde(default)]
    pub chunks: Vec<Chunk>,
    #[serde(default)]
    pub metadata: Option<OutputMetadata>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputMetadata {
    #[serde(default)]
    pub pages: Option<Vec<PageMeta>>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageMeta {
    pub number: u32,
    #[serde(default)]
    pub original_page_width: Option<f64>,
    #[serde(default)]
    pub original_page_height: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chunk {
    #[serde(rename = "type", default)]
    pub chunk_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub metadata: Option<ChunkMeta>,
    #[serde(default)]
    pub blocks: Vec<WireBlock>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChunkMeta {
    #[serde(default)]
    pub page_range: Option<PageRange>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct PageRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireBlock {
    #[serde(rename = "type", default)]
    pub block_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub metadata: Option<BlockMeta>,
    #[serde(default)]
    pub bounding_box: Option<WireBBox>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlockMeta {
    #[serde(default)]
    pub page: Option<BlockPage>,
    #[serde(default)]
    pub avg_ocr_confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct BlockPage {
    pub number: u32,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct WireBBox {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Metrics {
    #[serde(default)]
    pub page_count: Option<f64>,
    #[serde(default)]
    pub processing_time_ms: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunUsage {
    #[serde(default)]
    pub credits: Option<f64>,
}

// ---- normalisation -----------------------------------------------------------------------------

fn map_block_type(t: &str) -> BlockType {
    match t {
        "text" | "key_value" => BlockType::Text,
        "heading" => BlockType::Title,
        "section_heading" => BlockType::SectionHeader,
        "table" | "table_head" | "table_cell" => BlockType::Table,
        "figure" => BlockType::Figure,
        "formula" => BlockType::Formula,
        "header" => BlockType::Header,
        "footer" => BlockType::Footer,
        _ => BlockType::Other,
    }
}

pub(crate) fn normalize(run: &ParseRun, output: Output, fmt: OutputFormat) -> OcrResponse {
    let mut page_dims: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
    let mut page_md: BTreeMap<u32, String> = BTreeMap::new();
    let mut blocks: Vec<Block> = Vec::new();

    for chunk in &output.chunks {
        let range = chunk.metadata.as_ref().and_then(|m| m.page_range.as_ref());
        if let Some(r) = range {
            if r.start == r.end && chunk.chunk_type == "page" {
                page_md.insert(r.start, chunk.content.clone());
            }
        }
        let fallback_page = range.map(|r| r.start).unwrap_or(1);
        for b in &chunk.blocks {
            let (page_number, dims) = b
                .metadata
                .as_ref()
                .and_then(|m| m.page.as_ref())
                .map(|p| (p.number, p.width.zip(p.height)))
                .unwrap_or((fallback_page, None));
            if let Some((w, h)) = dims {
                page_dims.entry(page_number).or_insert((w, h));
            }
            let bbox = b.bounding_box.as_ref().and_then(|bb| {
                let (w, h) = page_dims.get(&page_number).copied()?;
                BBox::from_xywh(bb.left, bb.top, bb.right - bb.left, bb.bottom - bb.top, w, h)
            });
            let content = match fmt {
                OutputFormat::Markdown => b.content.clone(),
                OutputFormat::Text => crate::types::markdown_to_text(&b.content),
            };
            blocks.push(Block {
                block_type: map_block_type(&b.block_type),
                content,
                text: None,
                bbox,
                confidence: b.metadata.as_ref().and_then(|m| m.avg_ocr_confidence),
                page_number,
            });
        }
    }

    let mut pages: Vec<Page> = crate::types::pages_from_blocks(blocks, &page_dims);
    // Prefer the chunk-level page markdown (Extend's own page rendering) when available.
    for p in &mut pages {
        if let Some(md) = page_md.get(&p.page_number) {
            p.markdown = md.clone();
            p.text = crate::types::markdown_to_text(md);
        }
    }
    // Pages that only had chunk content but no blocks.
    for (n, md) in &page_md {
        if !pages.iter().any(|p| p.page_number == *n) {
            pages.push(Page {
                page_number: *n,
                width: page_dims.get(n).map(|d| d.0),
                height: page_dims.get(n).map(|d| d.1),
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

    let billed_pages = run
        .metrics
        .as_ref()
        .and_then(|m| m.page_count)
        .map(|p| p.round() as u32)
        .filter(|&p| p > 0)
        .unwrap_or(pages.len() as u32);
    let usage =
        Usage { pages: billed_pages, credits: run.usage.as_ref().and_then(|u| u.credits), provider_cost_usd: None };
    let mut resp = OcrResponse::from_pages(
        NAME,
        &format!(
            "{NAME}/{}",
            run.config.as_ref().and_then(|c| c.get("engine")).and_then(Value::as_str).unwrap_or("parse_performance")
        ),
        pages,
        usage,
    );
    if let Some(ms) = run.metrics.as_ref().and_then(|m| m.processing_time_ms) {
        resp.metadata.insert("extend_processing_time_ms".into(), json!(ms));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fixture() {
        let raw = include_str!("../../tests/fixtures/extend_parse_run.json");
        let run: ParseRun = serde_json::from_str(raw).unwrap();
        let output = run.output.clone().unwrap();
        let resp = normalize(&run, output, OutputFormat::Markdown);
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.usage.credits, Some(4.0));
        assert!(resp.pages[0].markdown.contains("# Hello LiteOCR"));
        assert!(resp.pages[0].markdown.contains("| Item | Amount |"));
        assert_eq!(resp.pages[1].page_number, 2);
        let first = &resp.pages[0].blocks[0];
        assert_eq!(first.block_type, BlockType::Title);
        let bb = first.bbox.unwrap();
        assert!(bb.x0 > 0.07 && bb.x0 < 0.08, "{bb:?}");
        assert!(bb.y1 < 0.1);
        assert!(first.confidence.unwrap() > 0.9);
        assert!(resp.markdown.contains("Reference: ABC-9876"));
        assert_eq!(resp.pages[0].width, Some(1241.0));
    }

    #[test]
    fn text_output_strips_markdown() {
        let raw = include_str!("../../tests/fixtures/extend_parse_run.json");
        let run: ParseRun = serde_json::from_str(raw).unwrap();
        let output = run.output.clone().unwrap();
        let resp = normalize(&run, output, OutputFormat::Text);
        assert!(!resp.pages[0].markdown.starts_with('#'));
        assert!(resp.text.starts_with("Hello LiteOCR"));
    }

    #[test]
    fn body_merges_provider_options_and_pages() {
        let req = OcrRequest::from_url("https://x/y.pdf")
            .pages("1-2,5")
            .provider_options(json!({"blockOptions": {"figures": {"enabled": false}}, "metadata": {"k": "v"}}));
        let body = build_body(&req, "parse_light", json!({"url": "https://x/y.pdf"})).unwrap();
        assert_eq!(body["config"]["engine"], "parse_light");
        assert_eq!(body["config"]["blockOptions"]["tables"]["targetFormat"], "markdown");
        assert_eq!(body["config"]["blockOptions"]["figures"]["enabled"], false);
        assert_eq!(body["config"]["advancedOptions"]["pageRanges"][1]["start"], 5);
        assert_eq!(body["metadata"]["k"], "v");
        assert!(build_body(&req, "nope", json!({})).is_err());
    }
}
