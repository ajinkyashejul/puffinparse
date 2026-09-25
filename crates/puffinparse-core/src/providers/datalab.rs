//! Datalab (datalab.to) Convert API — the hosted Marker pipeline.
//!
//! Flow: `POST /api/v1/convert` (multipart `file` or `file_url`, `mode`, `output_format`) →
//! `{request_id, request_check_url}` → poll the check URL until `status == "complete"` →
//! optionally download `result_url` (EU results are not returned inline).
//!
//! `/api/v1/marker` is the deprecated predecessor of `/api/v1/convert`; PuffinParse uses the current
//! endpoint, so `use_llm` / `force_ocr` / `format_lines` are replaced by `mode`.

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::providers::unstructured::{html_table_to_markdown, html_to_text, polygon_bbox};
use crate::types::{Block, BlockType, DocumentInput, DocumentRequest, OutputFormat, Page, ParseResponse, Usage};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME: &str = "datalab";
const ENV_KEY: &str = "DATALAB_API_KEY";
const ENV_BASE: &str = "DATALAB_BASE_URL";
const DEFAULT_BASE: &str = "https://www.datalab.to";
const CONVERT_PATH: &str = "/api/v1/convert";
/// Datalab's hard ceiling on pages per request; used to close an open-ended page range.
const MAX_PAGES: u32 = 7000;

#[derive(Debug, Default, Clone, Copy)]
pub struct Datalab;

#[async_trait]
impl Provider for Datalab {
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

        // 1. Submit.
        let submitted: Value = http::with_retry(NAME, retry, &deadline, || {
            let mut form = reqwest::multipart::Form::new();
            for (k, v) in &fields {
                form = form.text(k.clone(), v.clone());
            }
            form = match (&data, &request.input) {
                (Some(bytes), input) => form.part("file", provider::file_part(bytes.clone(), input)),
                (None, DocumentInput::Url { url }) => form.text("file_url", url.clone()),
                (None, _) => unreachable!("load_bytes returns bytes for non-URL inputs"),
            };
            let rb = client
                .post(format!("{base}{CONVERT_PATH}"))
                .header("X-API-Key", &api_key)
                .header("accept", "application/json")
                .timeout(deadline.request_timeout())
                .multipart(form);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await?;
        let initial: InitialResponse = serde_json::from_value(submitted.clone())?;
        if initial.success == Some(false) {
            return Err(job_error(initial.error.as_deref().unwrap_or("conversion request rejected"), None));
        }
        let request_id = initial
            .request_id
            .clone()
            .ok_or_else(|| Error::provider("convert response carried no request_id").with_provider(NAME))?;
        let check_url = check_url(&base, initial.request_check_url.as_deref(), &request_id);
        tracing::debug!(request_id = %request_id, "datalab: conversion submitted");

        // 2. Poll. A finished job is `status == "complete"`; failures also surface as
        //    `success == false` with `status` still "complete".
        let polled: Value = http::poll_until(NAME, &deadline, Duration::from_secs(2), Duration::from_secs(10), || {
            let rb = client.get(&check_url).header("X-API-Key", &api_key).timeout(deadline.request_timeout());
            async move {
                let body: Value = http::read_json(NAME, rb.send().await?).await?;
                let status = body.get("status").and_then(Value::as_str).unwrap_or_default();
                let failed = body.get("success").and_then(Value::as_bool) == Some(false);
                Ok(if status == "complete" || status == "failed" || failed { Some(body) } else { None })
            }
        })
        .await
        .map_err(|e| e.with_job_id(request_id.clone()))?;

        // 3. EU (and other regional) results arrive through a signed URL instead of inline.
        let merged = match polled.get("result_url").and_then(Value::as_str) {
            Some(url) if !url.is_empty() => {
                // The signed URL authorises by itself; sending the API key to it is not allowed.
                let rb = client.get(url).timeout(deadline.request_timeout());
                let downloaded: Value = http::read_json(NAME, rb.send().await?).await?;
                merge_result(downloaded, &polled)
            }
            _ => polled,
        };

        let result: ConvertResult = serde_json::from_value(merged.clone())?;
        if result.success == Some(false) || result.status.as_deref() == Some("failed") {
            return Err(job_error(result.error.as_deref().unwrap_or("conversion failed"), Some(request_id.clone())));
        }

        let raw = if request.include_raw { Some(merged) } else { None };
        let mut resp = normalize(&result, request.output, model);
        resp.provider_job_id = Some(request_id);
        resp.raw = raw;
        Ok(resp)
    }
}

/// Classify a job-level failure. The page-concurrency ceiling is reported in the result body
/// rather than as HTTP 429, so it is recognised here and stays fallback/retry-eligible.
fn job_error(message: &str, job_id: Option<String>) -> Error {
    let kind =
        if message.to_ascii_lowercase().contains("rate limit") { ErrorKind::RateLimit } else { ErrorKind::Provider };
    let mut e = Error::new(kind, message.to_string()).with_provider(NAME);
    if let Some(id) = job_id {
        e = e.with_job_id(id);
    }
    e
}

/// Re-host the returned check URL on the configured base so `base_url` overrides keep working.
fn check_url(base: &str, returned: Option<&str>, request_id: &str) -> String {
    let path = returned
        .and_then(|u| url::Url::parse(u).ok())
        .map(|u| u.path().to_string())
        .unwrap_or_else(|| format!("{CONVERT_PATH}/{request_id}"));
    format!("{base}{path}")
}

/// Merge the polling response into the downloaded result: the poll carries the freshest billing
/// and score fields, the download carries the document content.
fn merge_result(mut downloaded: Value, polled: &Value) -> Value {
    if let (Some(target), Some(source)) = (downloaded.as_object_mut(), polled.as_object()) {
        for (k, v) in source {
            if !v.is_null() {
                target.insert(k.clone(), v.clone());
            }
        }
    }
    downloaded
}

/// Multipart text fields for the convert call.
fn form_fields(request: &DocumentRequest, model: &str) -> Result<Vec<(String, String)>> {
    let mode = match model {
        "fast" | "balanced" | "accurate" => model,
        other => return Err(Error::unsupported_model(format!("datalab: unknown mode '{other}'"))),
    };
    let mut fields: Vec<(String, String)> = vec![
        ("mode".into(), mode.into()),
        // Both formats in one conversion: `json` carries block types and boxes, `markdown`
        // carries Marker's own page rendering. One request, one page charge.
        ("output_format".into(), "json,markdown".into()),
        ("paginate".into(), "true".into()),
    ];
    if let Some(pages) = &request.pages {
        // `page_range` is 0-based; ours is 1-based.
        let spec: Vec<String> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) if e == s => format!("{}", s - 1),
                Some(e) => format!("{}-{}", s - 1, e - 1),
                None => format!("{}-{}", s - 1, MAX_PAGES - 1),
            })
            .collect();
        fields.push(("page_range".into(), spec.join(",")));
    }
    if let Some(Value::Object(opts)) = &request.provider_options {
        for (k, v) in opts {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Null => continue,
                // `additional_config` and friends are JSON strings on the wire.
                other => other.to_string(),
            };
            fields.retain(|(key, _)| key != k);
            fields.push((k.clone(), s));
        }
    }
    Ok(fields)
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct InitialResponse {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    request_check_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ConvertResult {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub markdown: Option<String>,
    /// Marker's block tree. `anyOf: object | string` on the wire, so it is kept as a `Value`.
    #[serde(default)]
    pub json: Option<Value>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub page_count: Option<u32>,
    #[serde(default)]
    pub parse_quality_score: Option<f64>,
    #[serde(default)]
    pub cost_breakdown: Option<Value>,
    #[serde(default)]
    pub checkpoint_id: Option<String>,
}

/// One node of Marker's block tree (`Document` → `Page` → blocks → …).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct JsonBlock {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub block_type: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub polygon: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    pub children: Option<Vec<JsonBlock>>,
}

impl JsonBlock {
    fn points(&self) -> Vec<[f64; 2]> {
        self.polygon.iter().flatten().filter(|p| p.len() >= 2).map(|p| [p[0], p[1]]).collect()
    }

    /// Page size from the page block's polygon (Marker pages start at the origin).
    fn page_dims(&self) -> Option<(f64, f64)> {
        let pts = self.points();
        let w = pts.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
        let h = pts.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
        (w > 0.0 && h > 0.0).then_some((w, h))
    }
}

/// `result.json` is the `Document` block; it may arrive as an object or as a JSON string.
pub(crate) fn document_block(value: &Value) -> Option<JsonBlock> {
    match value {
        Value::String(s) => serde_json::from_str(s).ok(),
        other => serde_json::from_value(other.clone()).ok(),
    }
}

// ---- normalisation -----------------------------------------------------------------------------

/// Marker block types (`marker/schema/__init__.py`) mapped onto the unified vocabulary.
/// Marker has no `Title` type — the document title is a `SectionHeader` rendered as `<h1>`, which
/// is mapped to `title` to match the other providers.
fn map_block_type(t: &str, html: &str) -> BlockType {
    match t {
        "Text" | "TextInlineMath" | "Handwriting" | "Form" | "Code" | "Reference" | "Span" | "Line" => BlockType::Text,
        "SectionHeader" => {
            if heading_level(html) == Some(1) {
                BlockType::Title
            } else {
                BlockType::SectionHeader
            }
        }
        "Title" => BlockType::Title,
        "ListItem" | "ListGroup" => BlockType::List,
        "Table" | "TableGroup" | "TableCell" => BlockType::Table,
        "Figure" | "FigureGroup" | "Picture" | "PictureGroup" => BlockType::Figure,
        "Caption" => BlockType::Caption,
        "Footnote" => BlockType::Footnote,
        "PageHeader" => BlockType::Header,
        "PageFooter" => BlockType::Footer,
        "Equation" => BlockType::Formula,
        _ => BlockType::Other,
    }
}

/// `"/page/10/SectionHeader/3"` → page index `10` (0-based, in the original document).
fn page_index_from_id(id: &str) -> Option<u32> {
    let mut parts = id.split('/').filter(|s| !s.is_empty());
    while let Some(p) = parts.next() {
        if p == "page" {
            return parts.next().and_then(|n| n.parse().ok());
        }
    }
    None
}

fn heading_level(html: &str) -> Option<usize> {
    let lower = html.to_ascii_lowercase();
    (1..=6).find(|l| lower.contains(&format!("<h{l}")))
}

/// Best-effort HTML → markdown for one block. Marker's own markdown is used for page text; this
/// keeps `Block.content` readable and close to it.
fn block_markdown(block_type: BlockType, html: &str) -> String {
    let text = html_to_text(html);
    match block_type {
        BlockType::Table => html_table_to_markdown(html).unwrap_or_else(|| html.trim().to_string()),
        BlockType::Title | BlockType::SectionHeader => {
            let level = heading_level(html).unwrap_or(if block_type == BlockType::Title { 1 } else { 2 });
            format!("{} {text}", "#".repeat(level))
        }
        BlockType::List => {
            if html.to_ascii_lowercase().contains("<li") {
                list_items(html).join("\n")
            } else {
                format!("- {text}")
            }
        }
        BlockType::Formula => {
            if text.is_empty() {
                text
            } else {
                format!("$${text}$$")
            }
        }
        _ => text,
    }
}

fn list_items(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut items = Vec::new();
    let mut idx = 0usize;
    while let Some(start) = lower[idx..].find("<li") {
        let open = idx + start;
        let Some(gt) = html[open..].find('>').map(|p| open + p + 1) else { break };
        let end = lower[gt..].find("</li").map(|p| gt + p).unwrap_or(html.len());
        let item = html_to_text(&html[gt..end]);
        if !item.is_empty() {
            items.push(format!("- {item}"));
        }
        idx = end + 1;
        if idx >= html.len() {
            break;
        }
    }
    items
}

/// Flatten a page's block tree: groups whose HTML only references children (`<content-ref>`) are
/// replaced by those children, so every emitted block carries real content.
fn collect_blocks(
    block: &JsonBlock,
    page_number: u32,
    dims: Option<(f64, f64)>,
    fmt: OutputFormat,
    out: &mut Vec<Block>,
) {
    let html = block.html.clone().unwrap_or_default();
    let children = block.children.as_deref().unwrap_or_default();
    if html.contains("<content-ref") && !children.is_empty() {
        for child in children {
            collect_blocks(child, page_number, dims, fmt, out);
        }
        return;
    }
    let block_type = map_block_type(&block.block_type, &html);
    let markdown = block_markdown(block_type, &html);
    let text = html_to_text(&html);
    let content = match fmt {
        OutputFormat::Markdown => markdown,
        OutputFormat::Text => text.clone(),
    };
    if content.trim().is_empty() {
        return;
    }
    out.push(Block {
        block_type,
        content,
        text: Some(text),
        bbox: dims.and_then(|(w, h)| polygon_bbox(&block.points(), w, h)),
        confidence: None,
        page_number,
    });
}

/// Split `paginate=true` markdown. Marker writes `\n\n{<page_id>}` + 48 dashes + `\n\n` *before*
/// each page, with `page_id` the 0-based index in the original document.
pub(crate) fn split_paginated_markdown(md: &str) -> BTreeMap<u32, String> {
    let mut pages: BTreeMap<u32, String> = BTreeMap::new();
    let mut current: Option<u32> = None;
    let mut buf = String::new();
    for line in md.lines() {
        match page_marker(line.trim()) {
            Some(id) => {
                if let Some(n) = current.take() {
                    pages.insert(n, buf.trim().to_string());
                }
                buf.clear();
                current = Some(id + 1);
            }
            None => {
                buf.push_str(line);
                buf.push('\n');
            }
        }
    }
    if let Some(n) = current {
        pages.insert(n, buf.trim().to_string());
    }
    pages
}

/// `{7}------…` → `Some(7)`.
fn page_marker(line: &str) -> Option<u32> {
    let rest = line.strip_prefix('{')?;
    let (id, dashes) = rest.split_once('}')?;
    let dashes = dashes.trim();
    if dashes.len() >= 8 && dashes.chars().all(|c| c == '-') {
        id.parse().ok()
    } else {
        None
    }
}

pub(crate) fn normalize(result: &ConvertResult, fmt: OutputFormat, model: &str) -> ParseResponse {
    let doc = result.json.as_ref().and_then(document_block);
    let md_pages = result.markdown.as_deref().map(split_paginated_markdown).unwrap_or_default();

    let mut blocks: Vec<Block> = Vec::new();
    let mut page_dims: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
    if let Some(doc) = &doc {
        for (idx, page) in doc.children.as_deref().unwrap_or_default().iter().enumerate() {
            let page_number = page.id.as_deref().and_then(page_index_from_id).map(|i| i + 1).unwrap_or(idx as u32 + 1);
            let dims = page.page_dims();
            if let Some(d) = dims {
                page_dims.insert(page_number, d);
            }
            for child in page.children.as_deref().unwrap_or_default() {
                collect_blocks(child, page_number, dims, fmt, &mut blocks);
            }
        }
    }

    let mut pages: Vec<Page> = crate::types::pages_from_blocks(blocks, &page_dims);
    // Marker's own page markdown wins over the blocks joined together.
    for p in &mut pages {
        if let Some(md) = md_pages.get(&p.page_number) {
            p.markdown = md.clone();
            p.text = crate::types::markdown_to_text(md);
        }
    }
    for (n, md) in &md_pages {
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

    let billed = result.page_count.filter(|&p| p > 0).unwrap_or(pages.len() as u32);
    let usage = Usage { pages: billed, credits: None, provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    if let Some(score) = result.parse_quality_score {
        resp.metadata.insert("datalab_parse_quality_score".into(), json!(score));
    }
    if let Some(id) = &result.checkpoint_id {
        resp.metadata.insert("datalab_checkpoint_id".into(), json!(id));
    }
    if let Some(cost) = &result.cost_breakdown {
        // Documented as "cost in cents"; the shape is not specified, so it is passed through as-is.
        resp.metadata.insert("datalab_cost_breakdown".into(), cost.clone());
    }
    if let Some(failed) = result.metadata.as_ref().and_then(|m| m.get("failed_pages")) {
        if !failed.as_array().map(|a| a.is_empty()).unwrap_or(true) {
            resp.metadata.insert("datalab_failed_pages".into(), failed.clone());
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ConvertResult {
        let raw = include_str!("../../tests/fixtures/datalab_convert.json");
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn normalizes_fixture() {
        let result = fixture();
        let resp = normalize(&result, OutputFormat::Markdown, "balanced");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages[0].page_number, 1);
        assert_eq!(resp.pages[1].page_number, 2);
        // Page markdown comes from Marker's paginated markdown, not from the blocks.
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"), "{}", resp.pages[0].markdown);
        assert!(!resp.pages[0].markdown.contains("{0}---"));
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Title);
        assert_eq!(resp.pages[0].blocks[0].content, "# Hello LiteOCR");
        let bb = resp.pages[0].blocks[0].bbox.unwrap();
        assert!((bb.x0 - 0.1).abs() < 1e-6 && bb.y1 < 0.2, "{bb:?}");
        assert_eq!(resp.pages[0].width, Some(612.0));
        // The TableGroup is flattened to its Table child and rendered as a markdown table.
        let table = resp.pages[1].blocks.iter().find(|b| b.block_type == BlockType::Table).unwrap();
        assert!(table.content.starts_with("| Item | Qty | Price |"), "{}", table.content);
        assert!(resp.pages[1].blocks.iter().any(|b| b.block_type == BlockType::List));
        assert!(resp.pages[1].blocks.iter().any(|b| b.block_type == BlockType::Footer));
        assert_eq!(resp.metadata["datalab_parse_quality_score"], json!(4.5));
        assert!(resp.markdown.contains("## Line Items"));
        assert!(resp.text.contains("Widget"));
    }

    #[test]
    fn text_output_strips_markdown() {
        let resp = normalize(&fixture(), OutputFormat::Text, "fast");
        assert!(!resp.pages[0].markdown.starts_with('#'));
        assert!(resp.text.starts_with("Hello LiteOCR"));
    }

    #[test]
    fn splits_paginated_markdown() {
        let md = format!("\n\n{{0}}{d}\n\npage one\n\n{{1}}{d}\n\npage two\n", d = "-".repeat(48));
        let pages = split_paginated_markdown(&md);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[&1], "page one");
        assert_eq!(pages[&2], "page two");
        // Unpaginated markdown yields a single page 1.
        let one = split_paginated_markdown("just text");
        assert!(one.is_empty(), "no page markers means no per-page split");
    }

    #[test]
    fn reads_page_numbers_from_block_ids() {
        assert_eq!(page_index_from_id("/page/10/SectionHeader/3"), Some(10));
        assert_eq!(page_index_from_id("/page/0/Page/0"), Some(0));
        assert_eq!(page_index_from_id("nonsense"), None);
    }

    #[test]
    fn json_field_accepts_a_string() {
        let v = json!("{\"block_type\":\"Document\",\"children\":[{\"id\":\"/page/0/Page/0\",\"block_type\":\"Page\",\"html\":\"<content-ref src='x'></content-ref>\",\"polygon\":[[0,0],[612,0],[612,792],[0,792]],\"children\":[{\"id\":\"/page/0/Text/1\",\"block_type\":\"Text\",\"html\":\"<p>hi</p>\",\"polygon\":[[10,10],[100,10],[100,30],[10,30]]}]}]}");
        let doc = document_block(&v).unwrap();
        assert_eq!(doc.block_type, "Document");
        let result = ConvertResult { json: Some(v), page_count: Some(1), ..Default::default() };
        let resp = normalize(&result, OutputFormat::Markdown, "fast");
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.pages[0].text, "hi");
    }

    #[test]
    fn form_fields_convert_pages_and_merge_options() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .pages("1-3,5,9-")
            .provider_options(json!({"output_format": "json", "extras": "table_cell_bboxes", "word_bboxes": true}));
        let f = form_fields(&req, "accurate").unwrap();
        let get = |k: &str| f.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("mode"), Some("accurate"));
        assert_eq!(get("page_range"), Some("0-2,4,8-6999"));
        assert_eq!(get("paginate"), Some("true"));
        assert_eq!(get("output_format"), Some("json"), "provider_options override our default");
        assert_eq!(f.iter().filter(|(k, _)| k == "output_format").count(), 1);
        assert_eq!(get("word_bboxes"), Some("true"));
        assert!(form_fields(&req, "marker").is_err());
    }

    #[test]
    fn check_url_is_rehosted_on_the_configured_base() {
        assert_eq!(
            check_url("https://proxy.internal", Some("https://www.datalab.to/api/v1/convert/abc"), "abc"),
            "https://proxy.internal/api/v1/convert/abc"
        );
        assert_eq!(check_url("https://www.datalab.to", None, "xyz"), "https://www.datalab.to/api/v1/convert/xyz");
    }

    #[test]
    fn merge_keeps_downloaded_content_and_fresh_poll_fields() {
        let downloaded = json!({"markdown": "body", "page_count": 2, "success": true});
        let polled = json!({"status": "complete", "markdown": null, "parse_quality_score": 4.0, "success": true});
        let merged = merge_result(downloaded, &polled);
        assert_eq!(merged["markdown"], "body", "null poll fields must not clobber the download");
        assert_eq!(merged["parse_quality_score"], 4.0);
        assert_eq!(merged["status"], "complete");
    }

    #[test]
    fn job_errors_classify_the_page_concurrency_limit() {
        assert_eq!(
            job_error("Page rate limit exceeded. Your team has 4000 pages in flight", None).kind,
            ErrorKind::RateLimit
        );
        assert_eq!(job_error("Document processing failed", None).kind, ErrorKind::Provider);
    }

    /// Live smoke test. Run with:
    /// `DATALAB_API_KEY=… cargo test -p puffinparse-core datalab -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs DATALAB_API_KEY and network"]
    async fn live_parse() {
        if std::env::var(ENV_KEY).map(|v| v.trim().is_empty()).unwrap_or(true) {
            eprintln!("skipping: {ENV_KEY} not set");
            return;
        }
        let sample =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");
        let req = DocumentRequest::from_path(sample).model("datalab/fast").timeout_secs(240.0);
        let resp = Datalab.parse(&req, "fast").await.expect("conversion succeeds");
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages.len(), 2);
        assert!(!resp.markdown.trim().is_empty());
        assert!(resp.pages.iter().any(|p| !p.blocks.is_empty()));
    }
}
