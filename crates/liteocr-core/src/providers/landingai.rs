//! Landing AI — Agentic Document Extraction (ADE).
//!
//! Parse: `POST /v1/ade/parse` (multipart `document` **or** `document_url`, `model=dpt-2-latest`,
//! `split=page`) → `{markdown, chunks[], splits[], grounding{}, metadata{}}`.
//! Extract: ADE extracts from *Markdown*, not from a file, so `extract` mode is a two-step call —
//! parse the document, then `POST /v1/ade/extract` with that Markdown plus the JSON schema. The
//! `extraction_metadata` references chunk / table-cell ids, which are resolved back into page +
//! box citations through the parse response's `grounding` map.
//!
//! Docs: <https://docs.landing.ai/ade/ade-overview> · <https://docs.landing.ai/ade/parse>

use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::types::{
    pages_from_blocks, BBox, Block, BlockType, Citation, DocumentInput, DocumentRequest, ExtractRequest,
    ExtractResponse, FieldInfo, OutputFormat, Page, ParseResponse, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const NAME: &str = "landingai";
const ENV_KEY: &str = "LANDINGAI_API_KEY";
/// The name Landing AI's own libraries use; accepted as a fallback.
const ENV_KEY_ALT: &str = "VISION_AGENT_API_KEY";
const ENV_BASE: &str = "LANDINGAI_BASE_URL";
const DEFAULT_BASE: &str = "https://api.va.landing.ai";
const DEFAULT_EXTRACT_MODEL: &str = "extract-latest";

/// Landing AI Agentic Document Extraction.
#[derive(Debug, Default, Clone, Copy)]
pub struct LandingAi;

#[async_trait]
impl Provider for LandingAi {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let wire = run_parse(request, model).await?;
        let raw = if request.include_raw { Some(serde_json::to_value(&wire)?) } else { None };
        let mut resp = normalize(&wire, request.output, model);
        // ADE has no page-range parameter, so a `pages` selection is applied here;
        // `usage.pages` stays at the billed page count for the whole document.
        if let Some(spec) = &request.pages {
            let ranges = crate::util::parse_page_ranges(spec)?;
            resp = filter_pages(resp, &ranges);
            resp.metadata.insert("landingai_pages_filtered_client_side".into(), json!(spec));
        }
        resp.raw = raw;
        Ok(resp)
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let doc = &request.document;
        let api_key = resolve_key(doc)?;
        let base = provider::resolve_base_url(doc, ENV_BASE, DEFAULT_BASE);
        let deadline = Deadline::new(doc.timeout_secs);
        let retry = Retry::new(doc.max_retries);

        // Step 1: parse the document to Markdown (anchors included — the extractor references them).
        let parsed = run_parse(doc, model).await?;

        // Step 2: extract against the schema.
        let mut schema = request.schema.clone();
        if let (Some(instructions), Value::Object(o)) = (&request.instructions, &mut schema) {
            o.entry("description".to_string()).or_insert_with(|| json!(instructions));
        }
        let schema_str = serde_json::to_string(&schema)?;
        let extract_model =
            doc.option("extract_model").and_then(Value::as_str).unwrap_or(DEFAULT_EXTRACT_MODEL).to_string();
        let markdown = parsed.markdown.clone();
        let client = http::client();
        let extracted: ExtractWire = http::with_retry(NAME, retry, &deadline, || {
            let form = reqwest::multipart::Form::new()
                .text("markdown", markdown.clone())
                .text("schema", schema_str.clone())
                .text("model", extract_model.clone());
            let rb = authorize(client.post(format!("{base}/v1/ade/extract")), &api_key, doc)
                .timeout(deadline.request_timeout())
                .multipart(form);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await?;

        let pages = parsed.metadata.as_ref().map(|m| m.page_count).unwrap_or(0);
        let credits = match (
            parsed.metadata.as_ref().and_then(|m| m.credit_usage),
            extracted.metadata.as_ref().and_then(|m| m.credit_usage),
        ) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
        let usage = Usage { pages, credits, provider_cost_usd: None };
        let mut resp = ExtractResponse::new(NAME, &format!("{NAME}/{model}"), extracted.extraction.clone(), usage);
        let mut fields = BTreeMap::new();
        collect_fields(&extracted.extraction_metadata, "", &parsed, &mut fields);
        resp.fields = fields;
        resp.provider_job_id = extracted.metadata.as_ref().and_then(|m| m.job_id.clone());
        if let Some(err) = extracted.metadata.as_ref().and_then(|m| m.schema_violation_error.clone()) {
            resp.metadata.insert("landingai_schema_violation_error".into(), json!(err));
        }
        if let Some(v) = extracted.metadata.as_ref().and_then(|m| m.version.clone()) {
            resp.metadata.insert("landingai_extract_version".into(), json!(v));
        }
        if doc.include_raw {
            resp.raw = Some(json!({ "parse": &parsed, "extract": &extracted }));
        }
        Ok(resp)
    }
}

/// One `POST /v1/ade/parse` call, shared by `parse` and the first half of `extract`.
async fn run_parse(request: &DocumentRequest, model: &str) -> Result<ParseWire> {
    let api_key = resolve_key(request)?;
    let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
    let deadline = Deadline::new(request.timeout_secs);
    let retry = Retry::new(request.max_retries);
    let client = http::client();
    let fields = form_fields(request, model)?;
    let data = provider::load_bytes(&request.input).await?;

    let wire: ParseWire = http::with_retry(NAME, retry, &deadline, || {
        let mut form = reqwest::multipart::Form::new();
        for (k, v) in &fields {
            form = form.text(k.clone(), v.clone());
        }
        form = match (&data, &request.input) {
            (Some(bytes), input) => form.part("document", provider::file_part(bytes.clone(), input)),
            // ADE downloads remote documents itself.
            (None, DocumentInput::Url { url }) => form.text("document_url", url.clone()),
            (None, _) => unreachable!("load_bytes returns bytes for non-URL inputs"),
        };
        let rb = authorize(client.post(format!("{base}/v1/ade/parse")), &api_key, request)
            .timeout(deadline.request_timeout())
            .multipart(form);
        async move { http::read_json(NAME, rb.send().await?).await }
    })
    .await?;
    if let Some(job) = wire.metadata.as_ref().and_then(|m| m.job_id.clone()) {
        tracing::debug!(job_id = %job, "landingai: parse done");
    }
    Ok(wire)
}

/// API key: request override, then `LANDINGAI_API_KEY`, then `VISION_AGENT_API_KEY`.
fn resolve_key(request: &DocumentRequest) -> Result<String> {
    if let Some(k) = request.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        return Ok(k.to_string());
    }
    for var in [ENV_KEY, ENV_KEY_ALT] {
        if let Some(k) = std::env::var(var).ok().filter(|k| !k.trim().is_empty()) {
            return Ok(k);
        }
    }
    Err(Error::authentication(format!("no API key for {NAME}: set {ENV_KEY} (or {ENV_KEY_ALT}) or pass api_key"))
        .with_provider(NAME))
}

/// `Authorization: Bearer <key>` (what the ADE docs show). The OpenAPI security scheme is named
/// "Basic Auth", so `provider_options.auth_scheme = "basic"` switches to `Basic <key>` verbatim.
fn authorize(rb: reqwest::RequestBuilder, api_key: &str, request: &DocumentRequest) -> reqwest::RequestBuilder {
    match request.option("auth_scheme").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
        Some("basic") => rb.header("Authorization", format!("Basic {api_key}")),
        Some(other) if !other.is_empty() && other != "bearer" => {
            rb.header("Authorization", format!("{other} {api_key}"))
        }
        _ => rb.bearer_auth(api_key),
    }
}

fn form_fields(request: &DocumentRequest, model: &str) -> Result<Vec<(String, String)>> {
    let wire_model = match model {
        // `dpt-2` and `dpt-2-latest` both mean "newest DPT-2 snapshot"; pin one with
        // provider_options={"model": "dpt-2-20260410"}.
        "dpt-2" => "dpt-2-latest",
        other => return Err(Error::unsupported_model(format!("landingai: unknown model '{other}'"))),
    };
    let mut fields: Vec<(String, String)> = vec![("model".into(), wire_model.into()), ("split".into(), "page".into())];
    if let Some(Value::Object(opts)) = &request.provider_options {
        for (k, v) in opts {
            if matches!(k.as_str(), "auth_scheme" | "extract_model") {
                continue; // consumed locally
            }
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Bool(b) => b.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Null => continue,
                other => other.to_string(),
            };
            fields.retain(|(key, _)| key != k);
            fields.push((k.clone(), s));
        }
    }
    Ok(fields)
}

fn filter_pages(resp: ParseResponse, ranges: &[(u32, Option<u32>)]) -> ParseResponse {
    let keep = |p: u32| ranges.iter().any(|&(s, e)| p >= s && e.map(|e| p <= e).unwrap_or(true));
    let pages: Vec<Page> = resp.pages.into_iter().filter(|p| keep(p.page_number)).collect();
    let mut out = ParseResponse::from_pages(&resp.provider, &resp.model, pages, resp.usage);
    out.metadata = resp.metadata;
    out.provider_job_id = resp.provider_job_id;
    out
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct ParseWire {
    #[serde(default)]
    pub markdown: String,
    #[serde(default)]
    pub chunks: Vec<Chunk>,
    #[serde(default)]
    pub splits: Vec<Split>,
    /// chunk / table / table-cell id → location (and confidence for text chunks).
    #[serde(default)]
    pub grounding: BTreeMap<String, Grounding>,
    #[serde(default)]
    pub metadata: Option<ParseMetadata>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Chunk {
    #[serde(default)]
    pub markdown: String,
    /// `text` | `table` | `figure` | `marginalia` | `logo` | `card` | `attestation` | `scan_code`.
    #[serde(rename = "type", default, alias = "chunk_type")]
    pub chunk_type: String,
    #[serde(default, alias = "chunk_id")]
    pub id: Option<String>,
    #[serde(default)]
    pub grounding: Option<Groundings>,
}

/// Gen1 ADE returns one grounding object per chunk; the legacy endpoint returned a list.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum Groundings {
    // `Many` must come first: serde can also build a struct from a sequence, so an untagged
    // `One` would greedily (mis)match a list.
    Many(Vec<Grounding>),
    One(Box<Grounding>),
}

impl Groundings {
    fn first(&self) -> Option<&Grounding> {
        match self {
            Groundings::Many(v) => v.first(),
            Groundings::One(g) => Some(g),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct Grounding {
    #[serde(rename = "box", default)]
    pub bbox: Option<GroundingBox>,
    /// **Zero-indexed** page number.
    #[serde(default)]
    pub page: u32,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub grounding_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// Normalised 0–1 box, top-left origin. `l/t/r/b` are the legacy endpoint's spellings.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
pub(crate) struct GroundingBox {
    #[serde(default, alias = "l")]
    pub left: f64,
    #[serde(default, alias = "t")]
    pub top: f64,
    #[serde(default, alias = "r")]
    pub right: f64,
    #[serde(default, alias = "b")]
    pub bottom: f64,
}

impl GroundingBox {
    fn to_bbox(self) -> BBox {
        BBox {
            x0: self.left.clamp(0.0, 1.0),
            y0: self.top.clamp(0.0, 1.0),
            x1: self.right.clamp(0.0, 1.0),
            y1: self.bottom.clamp(0.0, 1.0),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Split {
    /// `full` (no `split` parameter) or `page`.
    #[serde(default)]
    pub class: String,
    /// `full`, or `page_0`, `page_1`, … (zero-indexed).
    #[serde(default)]
    pub identifier: String,
    /// Zero-indexed page numbers covered by the split.
    #[serde(default)]
    pub pages: Vec<u32>,
    #[serde(default)]
    pub markdown: String,
    #[serde(default)]
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct ParseMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default)]
    pub page_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_usage: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Present on `206 Partial Content`: zero-indexed pages that failed.
    #[serde(default)]
    pub failed_pages: Vec<u32>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct ExtractWire {
    #[serde(default)]
    pub extraction: Value,
    #[serde(default)]
    pub extraction_metadata: Value,
    #[serde(default)]
    pub metadata: Option<ExtractMetadata>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct ExtractMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_usage: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_violation_error: Option<String>,
}

// ---- normalisation -----------------------------------------------------------------------------

/// ADE chunk types → unified block types. `text` chunks carry headings as Markdown, so the
/// heading level decides between `title` and `section_header`.
fn map_chunk_type(chunk_type: &str, markdown: &str) -> BlockType {
    match chunk_type {
        "text" | "form" | "key_value" => heading_type(markdown),
        "table" => BlockType::Table,
        "figure" | "logo" | "card" | "attestation" => BlockType::Figure,
        "title" => BlockType::Title,
        "caption" => BlockType::Caption,
        "list" | "list_item" => BlockType::List,
        "page_header" | "header" => BlockType::Header,
        "page_footer" | "footer" => BlockType::Footer,
        "footnote" => BlockType::Footnote,
        "equation" | "formula" => BlockType::Formula,
        // `marginalia` mixes headers, footers and page numbers; `scan_code` is a barcode/QR.
        _ => BlockType::Other,
    }
}

fn heading_type(markdown: &str) -> BlockType {
    let line = markdown.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("<a id=")).unwrap_or("");
    let hashes = line.chars().take_while(|&c| c == '#').count();
    match hashes {
        0 => BlockType::Text,
        1 => BlockType::Title,
        _ => BlockType::SectionHeader,
    }
}

/// Drop the `<a id='…'></a>` anchors ADE injects for grounding; they are noise in the unified
/// content and the ids stay available in `raw`.
fn strip_anchors(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(start) = rest.find("<a id=") {
        let Some(end) = rest[start..].find("</a>") else { break };
        out.push_str(&rest[..start]);
        rest = &rest[start + end + "</a>".len()..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

pub(crate) fn normalize(wire: &ParseWire, fmt: OutputFormat, model: &str) -> ParseResponse {
    let blocks: Vec<Block> = wire
        .chunks
        .iter()
        .map(|c| {
            let md = strip_anchors(&c.markdown);
            let entry = c.id.as_ref().and_then(|id| wire.grounding.get(id));
            let grounding = c.grounding.as_ref().and_then(Groundings::first);
            let bbox = grounding.and_then(|g| g.bbox).or_else(|| entry.and_then(|g| g.bbox)).map(GroundingBox::to_bbox);
            // Pages are zero-indexed on the wire.
            let page = grounding.map(|g| g.page).or_else(|| entry.map(|g| g.page)).unwrap_or(0) + 1;
            let text = crate::types::markdown_to_text(&md);
            let content = match fmt {
                OutputFormat::Markdown => md.clone(),
                OutputFormat::Text => text.clone(),
            };
            Block {
                block_type: map_chunk_type(&c.chunk_type, &md),
                content,
                text: Some(text),
                bbox,
                confidence: entry.and_then(|g| g.confidence).or_else(|| grounding.and_then(|g| g.confidence)),
                page_number: page,
            }
        })
        .collect();

    let mut pages = pages_from_blocks(blocks, &BTreeMap::new());
    // `split=page` gives authoritative per-page Markdown; prefer it over joined chunks.
    for split in wire.splits.iter().filter(|s| s.class == "page") {
        let Some(page_number) = split.pages.first().map(|p| p + 1) else { continue };
        if let Some(page) = pages.iter_mut().find(|p| p.page_number == page_number) {
            let md = strip_anchors(&split.markdown);
            if !md.is_empty() {
                page.text = crate::types::markdown_to_text(&md);
                page.markdown = match fmt {
                    OutputFormat::Markdown => md,
                    OutputFormat::Text => page.text.clone(),
                };
            }
        }
    }
    if pages.is_empty() && !wire.markdown.trim().is_empty() {
        let md = strip_anchors(&wire.markdown);
        let text = crate::types::markdown_to_text(&md);
        let markdown = match fmt {
            OutputFormat::Markdown => md,
            OutputFormat::Text => text.clone(),
        };
        pages.push(Page { page_number: 1, width: None, height: None, markdown, text, blocks: Vec::new() });
    }

    let meta = wire.metadata.as_ref();
    let billed = meta.map(|m| m.page_count).filter(|&p| p > 0).unwrap_or(pages.len() as u32);
    let usage = Usage {
        pages: billed,
        credits: meta.and_then(|m| m.credit_usage).filter(|&c| c > 0.0),
        provider_cost_usd: None,
    };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    resp.provider_job_id = meta.and_then(|m| m.job_id.clone());
    if let Some(v) = meta.and_then(|m| m.version.clone()) {
        resp.metadata.insert("landingai_version".into(), json!(v));
    }
    if let Some(failed) = meta.map(|m| &m.failed_pages).filter(|f| !f.is_empty()) {
        // Reported zero-indexed; surfaced 1-based like every other page number in LiteOCR.
        let failed: Vec<u32> = failed.iter().map(|p| p + 1).collect();
        resp.metadata.insert("landingai_failed_pages".into(), json!(failed));
    }
    resp
}

/// Walk `extraction_metadata` (same shape as the schema, leaves carry `value` + `references`) and
/// turn every leaf into a `FieldInfo` keyed by JSON pointer into `data`.
fn collect_fields(node: &Value, pointer: &str, parsed: &ParseWire, out: &mut BTreeMap<String, FieldInfo>) {
    match node {
        Value::Object(map) => {
            let refs = map.get("references").or_else(|| map.get("chunk_references"));
            if let Some(Value::Array(refs)) = refs {
                let citations: Vec<Citation> = refs
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|id| {
                        let g = parsed.grounding.get(id)?;
                        Some(Citation {
                            page_number: g.page + 1,
                            bbox: g.bbox.map(GroundingBox::to_bbox),
                            text: parsed
                                .chunks
                                .iter()
                                .find(|c| c.id.as_deref() == Some(id))
                                .map(|c| strip_anchors(&c.markdown)),
                        })
                    })
                    .collect();
                let confidence = map.get("confidence").and_then(Value::as_f64);
                if !citations.is_empty() || confidence.is_some() {
                    let key = if pointer.is_empty() { "/".to_string() } else { pointer.to_string() };
                    out.insert(key, FieldInfo { confidence, citations });
                }
                return;
            }
            for (k, v) in map {
                collect_fields(v, &format!("{pointer}/{}", escape_pointer(k)), parsed, out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                collect_fields(v, &format!("{pointer}/{i}"), parsed, out);
            }
        }
        _ => {}
    }
}

/// RFC 6901 escaping for JSON-pointer segments.
fn escape_pointer(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ParseWire {
        let raw = include_str!("../../tests/fixtures/landingai_parse.json");
        serde_json::from_str(raw).unwrap()
    }

    fn extract_fixture() -> ExtractWire {
        let raw = include_str!("../../tests/fixtures/landingai_extract.json");
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn normalizes_fixture() {
        let resp = normalize(&fixture(), OutputFormat::Markdown, "dpt-2");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.usage.credits, Some(6.0));
        assert_eq!(resp.provider_job_id.as_deref(), Some("job_01HZY0PARSE"));
        assert_eq!(resp.provider, NAME);
        // Zero-indexed on the wire, 1-based in the unified response.
        assert_eq!(resp.pages[0].page_number, 1);
        assert_eq!(resp.pages[1].page_number, 2);
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Title, "`# ` heading in a text chunk");
        assert_eq!(resp.pages[0].blocks[0].content, "# Hello LiteOCR");
        assert_eq!(resp.pages[1].blocks[0].block_type, BlockType::Table);
        assert!(resp.markdown.starts_with("# Hello LiteOCR"));
        assert!(resp.markdown.contains("<td id=\"1-4\">Widget</td>"), "table HTML is kept verbatim");
        assert!(resp.text.contains("Invoice #1234"));
        assert_eq!(resp.metadata["landingai_version"], "dpt-2-20260410");
    }

    #[test]
    fn grounding_becomes_normalised_bboxes_and_confidence() {
        let resp = normalize(&fixture(), OutputFormat::Markdown, "dpt-2");
        let b = &resp.pages[0].blocks[0];
        let bb = b.bbox.unwrap();
        assert!((bb.x0 - 0.017335).abs() < 1e-5, "{bb:?}");
        assert!((bb.y1 - 0.212029).abs() < 1e-5, "{bb:?}");
        assert_eq!(b.confidence, Some(0.97));
        for p in &resp.pages {
            for b in &p.blocks {
                let bb = b.bbox.expect("every fixture chunk is grounded");
                assert!((0.0..=1.0).contains(&bb.x0) && (0.0..=1.0).contains(&bb.y1));
            }
        }
    }

    #[test]
    fn page_splits_supply_page_markdown() {
        let resp = normalize(&fixture(), OutputFormat::Markdown, "dpt-2");
        // Page 1's markdown comes from the `page_0` split, anchors stripped.
        assert!(!resp.pages[0].markdown.contains("<a id="));
        assert!(resp.pages[0].markdown.contains("Invoice #1234"));
    }

    #[test]
    fn legacy_grounding_list_and_ltrb_keys_still_parse() {
        let wire: ParseWire = serde_json::from_str(
            r#"{"markdown":"x","chunks":[{"chunk_id":"c1","chunk_type":"text","markdown":"x",
                "grounding":[{"box":{"l":0.1,"t":0.2,"r":0.3,"b":0.4},"page":1}]}],"splits":[],"grounding":{}}"#,
        )
        .unwrap();
        let resp = normalize(&wire, OutputFormat::Markdown, "dpt-2");
        assert_eq!(resp.pages[0].page_number, 2);
        let bb = resp.pages[0].blocks[0].bbox.unwrap();
        assert!((bb.x0 - 0.1).abs() < 1e-9 && (bb.y1 - 0.4).abs() < 1e-9);
    }

    #[test]
    fn chunk_types_map() {
        for (t, md, expected) in [
            ("text", "Some prose", BlockType::Text),
            ("text", "# Title", BlockType::Title),
            ("text", "### Sub", BlockType::SectionHeader),
            ("table", "<table></table>", BlockType::Table),
            ("figure", "", BlockType::Figure),
            ("logo", "", BlockType::Figure),
            ("card", "", BlockType::Figure),
            ("attestation", "", BlockType::Figure),
            ("marginalia", "", BlockType::Other),
            ("scan_code", "", BlockType::Other),
        ] {
            assert_eq!(map_chunk_type(t, md), expected, "{t} / {md}");
        }
    }

    #[test]
    fn strips_anchors() {
        assert_eq!(strip_anchors("<a id='x'></a>\n\n# T\n\ntext"), "# T\n\ntext");
        assert_eq!(strip_anchors("no anchors"), "no anchors");
        assert_eq!(strip_anchors("<a id='x'>unterminated"), "<a id='x'>unterminated");
    }

    #[test]
    fn extract_metadata_becomes_citations() {
        let parsed = fixture();
        let wire = extract_fixture();
        let mut fields = BTreeMap::new();
        collect_fields(&wire.extraction_metadata, "", &parsed, &mut fields);
        let total = fields.get("/invoice/total").expect("pointer for a nested field");
        assert_eq!(total.citations.len(), 1);
        assert_eq!(total.citations[0].page_number, 2);
        assert!(total.citations[0].bbox.is_some());
        let number = fields.get("/invoice/number").unwrap();
        assert_eq!(number.citations[0].page_number, 1);
        assert!(number.citations[0].text.as_deref().unwrap().contains("Invoice #1234"));
        let item = fields.get("/items/0/description").unwrap();
        assert_eq!(item.citations[0].page_number, 2);
        assert_eq!(wire.extraction["invoice"]["total"], 56.78);
    }

    #[test]
    fn form_fields_defaults_and_overrides() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .provider_options(json!({"model": "dpt-2-20260410", "auth_scheme": "basic", "password": "s3cret"}));
        let f = form_fields(&req, "dpt-2").unwrap();
        let get = |k: &str| f.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("model"), Some("dpt-2-20260410"));
        assert_eq!(get("split"), Some("page"));
        assert_eq!(get("password"), Some("s3cret"));
        assert_eq!(get("auth_scheme"), None);
        assert!(form_fields(&req, "dpt-3").is_err());
    }

    #[test]
    fn page_selection_is_applied_client_side() {
        let resp = normalize(&fixture(), OutputFormat::Markdown, "dpt-2");
        let filtered = filter_pages(resp, &[(2, None)]);
        assert_eq!(filtered.pages.len(), 1);
        assert_eq!(filtered.pages[0].page_number, 2);
        assert_eq!(filtered.usage.pages, 2);
    }

    #[test]
    fn errors_are_classified() {
        let e = Error::from_http(NAME, 401, r#"{"detail":"Invalid API key"}"#);
        assert_eq!(e.kind, crate::error::ErrorKind::Authentication);
        assert_eq!(e.message, "Invalid API key");
        // 402 Payment Required (out of credits) is a plain bad request for the router.
        let e = Error::from_http(NAME, 402, r#"{"detail":"Insufficient credits"}"#);
        assert_eq!(e.kind, crate::error::ErrorKind::BadRequest);
        let e = Error::from_http(NAME, 422, r#"{"detail":[{"loc":["body","schema"],"msg":"field required"}]}"#);
        assert_eq!(e.kind, crate::error::ErrorKind::BadRequest);
        assert!(e.message.contains("field required"));
    }

    /// Live smoke test. Needs `LANDINGAI_API_KEY` (or `VISION_AGENT_API_KEY`); run with
    /// `cargo test -p liteocr-core landingai -- --ignored`.
    #[tokio::test]
    #[ignore = "needs LANDINGAI_API_KEY and network"]
    async fn landingai_live() {
        if [ENV_KEY, ENV_KEY_ALT].iter().all(|v| std::env::var(v).map(|s| s.is_empty()).unwrap_or(true)) {
            eprintln!("skipping: {ENV_KEY} not set");
            return;
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");
        let req = DocumentRequest::from_path(path).timeout_secs(240.0);
        let resp = LandingAi.parse(&req, "dpt-2").await.expect("parse succeeds");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert!(!resp.markdown.trim().is_empty());

        let schema = json!({"type":"object","properties":{"total":{"type":"string","description":"invoice total"}}});
        let ex = LandingAi
            .extract(&ExtractRequest::new(req, schema).citations(true), "dpt-2")
            .await
            .expect("extract succeeds");
        assert!(ex.data.is_object());
        assert_eq!(ex.usage.pages, 2);
    }
}
