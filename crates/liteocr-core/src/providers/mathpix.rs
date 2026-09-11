//! Mathpix Convert API (`api.mathpix.com`), the STEM-focused OCR engine behind Snip.
//!
//! Two endpoints, picked by input type:
//!
//! * **Documents** (PDF, DOCX, PPTX, EPUB, …) — `POST /v3/pdf` → poll `GET /v3/pdf/{pdf_id}`
//!   until `status == "completed"` → download `GET /v3/pdf/{pdf_id}.lines.json` (per-page lines
//!   with polygons) and, for `parse`, `GET /v3/pdf/{pdf_id}.mmd` (the assembled Mathpix Markdown).
//! * **Images** (PNG, JPG, …) — `POST /v3/text`, one synchronous call returning Mathpix Markdown
//!   plus `line_data` and, in `ocr` mode, `word_data`.
//!
//! Both authenticate with the `app_id` + `app_key` header pair. **Most Mathpix errors arrive as
//! HTTP 200 with `error` / `error_info` in the body**, so every payload is inspected.

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::providers::unstructured::polygon_bbox;
use crate::types::{
    Block, BlockType, DocumentInput, DocumentRequest, Line, OutputFormat, Page, ParseResponse, TextPage, TextResponse,
    Usage, Word,
};
use crate::util::deep_merge;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::Duration;

pub const NAME: &str = "mathpix";
const ENV_APP_ID: &str = "MATHPIX_APP_ID";
const ENV_APP_KEY: &str = "MATHPIX_APP_KEY";
const ENV_BASE: &str = "MATHPIX_BASE_URL";
const DEFAULT_BASE: &str = "https://api.mathpix.com";

#[derive(Debug, Default, Clone, Copy)]
pub struct Mathpix;

#[async_trait]
impl Provider for Mathpix {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        match run(request, model, Mode::Parse).await? {
            Payload::Document { lines, mmd, pdf_id, num_pages, raw } => {
                let mut resp = normalize_document(&lines, mmd.as_deref(), request.output, model, num_pages);
                resp.provider_job_id = Some(pdf_id);
                resp.raw = request.include_raw.then_some(raw);
                Ok(resp)
            }
            Payload::Image { result, raw } => {
                let mut resp = normalize_image(&result, request.output, model);
                resp.provider_job_id = result.request_id.clone();
                resp.raw = request.include_raw.then_some(raw);
                Ok(resp)
            }
        }
    }

    /// Native OCR: Mathpix reports line polygons for documents, and line *and* word polygons for
    /// images, so the text response is built from the provider's own geometry rather than derived
    /// from [`Self::parse`].
    async fn ocr(&self, request: &DocumentRequest, model: &str) -> Result<TextResponse> {
        match run(request, model, Mode::Ocr).await? {
            Payload::Document { lines, pdf_id, num_pages, raw, .. } => {
                let mut resp = document_text(&lines, model, num_pages);
                resp.provider_job_id = Some(pdf_id);
                resp.raw = request.include_raw.then_some(raw);
                Ok(resp)
            }
            Payload::Image { result, raw } => {
                let mut resp = image_text(&result, model);
                resp.provider_job_id = result.request_id.clone();
                resp.raw = request.include_raw.then_some(raw);
                Ok(resp)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Parse,
    Ocr,
}

#[derive(Debug)]
enum Payload {
    Document { lines: LinesJson, mmd: Option<String>, pdf_id: String, num_pages: Option<u32>, raw: Value },
    Image { result: Box<TextResult>, raw: Value },
}

/// `mathpix/pdf` routes by input type: `/v3/pdf` takes documents and ebooks only, images go to
/// `/v3/text`. `mathpix/text` always uses `/v3/text`.
fn use_image_endpoint(model: &str, input: &DocumentInput) -> bool {
    model == "text" || input.mime_type().starts_with("image/")
}

async fn run(request: &DocumentRequest, model: &str, mode: Mode) -> Result<Payload> {
    if !matches!(model, "pdf" | "text") {
        return Err(Error::unsupported_model(format!("mathpix: unknown model '{model}'")));
    }
    let app_key = provider::resolve_api_key(request, ENV_APP_KEY, NAME)?;
    let app_id = resolve_app_id(request)?;
    let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
    let deadline = Deadline::new(request.timeout_secs);
    let retry = Retry::new(request.max_retries);
    let client = http::client();
    let auth =
        move |rb: reqwest::RequestBuilder| rb.header("app_id", app_id.clone()).header("app_key", app_key.clone());

    if use_image_endpoint(model, &request.input) {
        let options = image_options(request, mode);
        let data = provider::load_bytes(&request.input).await?;
        let body: Value = http::with_retry(NAME, retry, &deadline, || {
            let rb = match (&data, &request.input) {
                (Some(bytes), input) => {
                    // Multipart uploads carry every option as a stringified JSON blob.
                    let form = reqwest::multipart::Form::new()
                        .part("file", provider::file_part(bytes.clone(), input))
                        .text("options_json", options.to_string());
                    auth(client.post(format!("{base}/v3/text"))).multipart(form)
                }
                (None, DocumentInput::Url { url }) => {
                    let mut json_body = options.clone();
                    json_body["src"] = json!(url);
                    auth(client.post(format!("{base}/v3/text"))).json(&json_body)
                }
                (None, _) => unreachable!("load_bytes returns bytes for non-URL inputs"),
            };
            let rb = rb.timeout(deadline.request_timeout());
            async move {
                let body: Value = http::read_json(NAME, rb.send().await?).await?;
                check_payload(&body)?;
                Ok(body)
            }
        })
        .await?;
        let result: TextResult = serde_json::from_value(body.clone())?;
        return Ok(Payload::Image { result: Box::new(result), raw: body });
    }

    // ---- document pipeline ----
    let options = pdf_options(request)?;
    let data = provider::load_bytes(&request.input).await?;
    let submitted: Value = http::with_retry(NAME, retry, &deadline, || {
        let rb = match (&data, &request.input) {
            (Some(bytes), input) => {
                let form = reqwest::multipart::Form::new()
                    .part("file", provider::file_part(bytes.clone(), input))
                    .text("options_json", options.to_string());
                auth(client.post(format!("{base}/v3/pdf"))).multipart(form)
            }
            (None, DocumentInput::Url { url }) => {
                let mut json_body = options.clone();
                json_body["url"] = json!(url);
                auth(client.post(format!("{base}/v3/pdf"))).json(&json_body)
            }
            (None, _) => unreachable!("load_bytes returns bytes for non-URL inputs"),
        };
        let rb = rb.timeout(deadline.request_timeout());
        async move {
            let body: Value = http::read_json(NAME, rb.send().await?).await?;
            check_payload(&body)?;
            Ok(body)
        }
    })
    .await?;
    let pdf_id = submitted
        .get("pdf_id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::provider("v3/pdf response carried no pdf_id").with_provider(NAME))?
        .to_string();
    tracing::debug!(pdf_id = %pdf_id, "mathpix: document submitted");

    // Poll `status`, never `percent_done`: the latter hits 100 before the outputs are assembled.
    let status: PdfStatus = http::poll_until(NAME, &deadline, Duration::from_secs(2), Duration::from_secs(10), || {
        let rb = auth(client.get(format!("{base}/v3/pdf/{pdf_id}"))).timeout(deadline.request_timeout());
        async move {
            let body: Value = http::read_json(NAME, rb.send().await?).await?;
            let status: PdfStatus = serde_json::from_value(body.clone())?;
            match status.status.as_deref() {
                Some("completed") => Ok(Some(status)),
                Some("error") => Err(check_payload(&body)
                    .err()
                    .unwrap_or_else(|| Error::provider("document processing failed").with_provider(NAME))),
                _ => Ok(None),
            }
        }
    })
    .await
    .map_err(|e| e.with_job_id(pdf_id.clone()))?;

    let lines_body = fetch_output(client, &base, &pdf_id, "lines.json", &auth, &deadline)
        .await
        .map_err(|e| e.with_job_id(pdf_id.clone()))?;
    let raw: Value = serde_json::from_str(&lines_body)?;
    check_payload(&raw)?;
    let lines: LinesJson = serde_json::from_value(raw.clone())?;
    let mmd = match mode {
        // The assembled MMD is the document's authoritative markdown; `ocr` mode does not need it.
        Mode::Parse => Some(
            fetch_output(client, &base, &pdf_id, "mmd", &auth, &deadline)
                .await
                .map_err(|e| e.with_job_id(pdf_id.clone()))?,
        ),
        Mode::Ocr => None,
    };
    Ok(Payload::Document { lines, mmd, pdf_id, num_pages: status.num_pages, raw })
}

/// `app_id` comes from `provider_options.app_id` or `MATHPIX_APP_ID`; `app_key` follows the usual
/// `api_key` → env-var resolution.
fn resolve_app_id(request: &DocumentRequest) -> Result<String> {
    if let Some(id) = request.option("app_id").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        return Ok(id.to_string());
    }
    std::env::var(ENV_APP_ID).ok().filter(|v| !v.trim().is_empty()).ok_or_else(|| {
        Error::authentication(format!(
            "no app id for mathpix: set {ENV_APP_ID} or pass provider_options={{\"app_id\": \"…\"}}"
        ))
        .with_provider(NAME)
    })
}

/// Download one generated output (`.lines.json`, `.mmd`). A `404` whose body is the status object
/// means "not assembled yet", and a `202` means a conversion is still running; both keep polling.
async fn fetch_output<F>(
    client: &reqwest::Client,
    base: &str,
    pdf_id: &str,
    ext: &str,
    auth: &F,
    deadline: &Deadline,
) -> Result<String>
where
    F: Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
{
    http::poll_until(NAME, deadline, Duration::from_secs(1), Duration::from_secs(5), || {
        let rb = auth(client.get(format!("{base}/v3/pdf/{pdf_id}.{ext}"))).timeout(deadline.request_timeout());
        async move {
            let resp = rb.send().await?;
            let status = resp.status();
            let body = resp.text().await?;
            if status.is_success() {
                return Ok(Some(body));
            }
            if status.as_u16() == 404 || status.as_u16() == 202 {
                tracing::debug!(pdf_id, ext, status = status.as_u16(), "mathpix: output not ready yet");
                return Ok(None);
            }
            Err(Error::from_http(NAME, status.as_u16(), &body))
        }
    })
    .await
}

// ---- request bodies ---------------------------------------------------------------------------

/// Options shared by both endpoints: markdown-friendly math delimiters, plus `provider_options`.
fn common_options() -> Value {
    json!({
        "math_inline_delimiters": ["$", "$"],
        "math_display_delimiters": ["$$", "$$"],
    })
}

fn pdf_options(request: &DocumentRequest) -> Result<Value> {
    let mut options = common_options();
    if let Some(pages) = &request.pages {
        options["page_ranges"] = json!(page_ranges(pages)?);
    }
    merge_options(&mut options, request);
    Ok(options)
}

fn image_options(request: &DocumentRequest, mode: Mode) -> Value {
    let mut options = common_options();
    options["formats"] = json!(["text"]);
    options["include_line_data"] = json!(true);
    match mode {
        // `include_word_data` and `enable_document_layout` cannot be combined, so each mode gets
        // the one it needs: layout for parse, word geometry for ocr.
        Mode::Parse => options["enable_document_layout"] = json!(true),
        Mode::Ocr => options["include_word_data"] = json!(true),
    }
    merge_options(&mut options, request);
    options
}

fn merge_options(options: &mut Value, request: &DocumentRequest) {
    if let Some(opts) = &request.provider_options {
        let mut patch = opts.clone();
        if let Value::Object(o) = &mut patch {
            o.remove("app_id"); // credentials, not a request option
        }
        deep_merge(options, &patch);
    }
}

/// Mathpix `page_ranges` is 1-based like ours; an open range is closed with `-1` (the last page).
fn page_ranges(spec: &str) -> Result<String> {
    let parts: Vec<String> = crate::util::parse_page_ranges(spec)?
        .into_iter()
        .map(|(s, e)| match e {
            Some(e) if e == s => format!("{s}"),
            Some(e) => format!("{s}-{e}"),
            None => format!("{s}--1"),
        })
        .collect();
    Ok(parts.join(","))
}

// ---- error handling ---------------------------------------------------------------------------

/// Mathpix reports most failures with HTTP 200 and an `error` / `error_info` body.
fn check_payload(body: &Value) -> Result<()> {
    let info = body.get("error_info");
    let id = info.and_then(|i| i.get("id")).and_then(Value::as_str).unwrap_or_default();
    let message = info
        .and_then(|i| i.get("message"))
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .unwrap_or_default();
    if id.is_empty() && message.is_empty() {
        return Ok(());
    }
    let kind = match id {
        "http_unauthorized" | "account_disabled" | "expired_license" | "unauthorized_token_request" => {
            ErrorKind::Authentication
        }
        "http_max_requests" => ErrorKind::RateLimit,
        "sys_exception" | "connection_closed" => ErrorKind::Provider,
        // Content the engine could not read is a provider-side outcome, so a router may fall back.
        "image_no_content" | "math_confidence" | "math_syntax" | "strokes_no_content" => ErrorKind::Provider,
        // Everything else in the documented set is a request problem (`opts_*`, `pdf_*`, `image_*`).
        "" => ErrorKind::Provider,
        _ => ErrorKind::BadRequest,
    };
    let message = if id.is_empty() { message.to_string() } else { format!("{id}: {message}") };
    Err(Error::new(kind, message).with_provider(NAME))
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct PdfStatus {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    num_pages: Option<u32>,
}

/// `GET /v3/pdf/{id}.lines.json`
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct LinesJson {
    #[serde(default)]
    pub pages: Vec<LinesPage>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LinesPage {
    pub page: u32,
    #[serde(default)]
    pub page_width: Option<f64>,
    #[serde(default)]
    pub page_height: Option<f64>,
    #[serde(default)]
    pub lines: Vec<PdfLine>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct PdfLine {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(rename = "type", default)]
    pub line_type: String,
    #[serde(default)]
    pub subtype: Option<String>,
    /// Searchable plain text; empty for elements that carry no text of their own.
    #[serde(default)]
    pub text: Option<String>,
    /// Mathpix Markdown for the line, as it appears in the assembled `.mmd`.
    #[serde(default)]
    pub text_display: Option<String>,
    /// Whether the line is part of the final MMD output. Missing ⇒ treated as `true`.
    #[serde(default)]
    pub conversion_output: Option<bool>,
    /// Polygon in page pixels, `[TL, TR, BR, BL]` for axis-aligned boxes.
    #[serde(default)]
    pub cnt: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

impl PdfLine {
    fn points(&self) -> Vec<[f64; 2]> {
        points(self.cnt.as_deref())
    }

    fn kept(&self) -> bool {
        self.conversion_output.unwrap_or(true)
    }

    fn markdown(&self) -> String {
        let display = self.text_display.as_deref().unwrap_or_default().trim();
        if display.is_empty() {
            self.text.as_deref().unwrap_or_default().trim().to_string()
        } else {
            display.to_string()
        }
    }

    fn plain(&self) -> String {
        let text = self.text.as_deref().unwrap_or_default().trim();
        if text.is_empty() {
            crate::types::markdown_to_text(&self.markdown())
        } else {
            text.to_string()
        }
    }
}

/// `POST /v3/text`
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct TextResult {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub line_data: Vec<LineData>,
    #[serde(default)]
    pub word_data: Vec<WordData>,
    #[serde(default)]
    pub image_width: Option<f64>,
    #[serde(default)]
    pub image_height: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct LineData {
    #[serde(rename = "type", default)]
    pub line_type: String,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub cnt: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    pub confidence: Option<f64>,
    /// Superseded by `conversion_output`, still emitted by `/v3/text`.
    #[serde(default)]
    pub included: Option<bool>,
    #[serde(default)]
    pub conversion_output: Option<bool>,
}

impl LineData {
    fn kept(&self) -> bool {
        self.conversion_output.or(self.included).unwrap_or(true)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct WordData {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub cnt: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

fn points(cnt: Option<&[Vec<f64>]>) -> Vec<[f64; 2]> {
    cnt.unwrap_or_default().iter().filter(|p| p.len() >= 2).map(|p| [p[0], p[1]]).collect()
}

// ---- normalisation -----------------------------------------------------------------------------

/// Mathpix line types and subtypes (shared by `line_data` and PDF lines data).
fn map_line_type(t: &str, subtype: Option<&str>) -> BlockType {
    match t {
        "title" => BlockType::Title,
        "section_header" => BlockType::SectionHeader,
        "text"
        | "abstract"
        | "authors"
        | "quote"
        | "code"
        | "pseudocode"
        | "form_field"
        | "multiple_choice_block"
        | "multiple_choice_option"
        | "table_of_contents_row"
        | "table_of_contents_item"
        | "column" => BlockType::Text,
        "math" => BlockType::Formula,
        "table" | "table_cell" => BlockType::Table,
        "diagram" | "chart" => BlockType::Figure,
        "diagram_info" | "chart_info" | "figure_label" => BlockType::Caption,
        "footnote" => BlockType::Footnote,
        // Running heads, page numbers, stamps and QR codes; margin notes read as footnotes.
        "page_info" => {
            if subtype == Some("margin_note") {
                BlockType::Footnote
            } else {
                BlockType::Header
            }
        }
        _ => BlockType::Other,
    }
}

/// Drop lines excluded from the output, then drop children of kept lines so a table's cells are
/// not emitted next to the table they already belong to.
fn kept_lines(lines: &[PdfLine]) -> Vec<&PdfLine> {
    let candidates: Vec<&PdfLine> = lines.iter().filter(|l| l.kept() && !l.markdown().is_empty()).collect();
    let ids: BTreeSet<&str> = candidates.iter().filter_map(|l| l.id.as_deref()).collect();
    candidates
        .into_iter()
        .filter(|l| match l.parent_id.as_deref() {
            Some(parent) => !ids.contains(parent),
            None => true,
        })
        .collect()
}

pub(crate) fn normalize_document(
    lines: &LinesJson,
    mmd: Option<&str>,
    fmt: OutputFormat,
    model: &str,
    num_pages: Option<u32>,
) -> ParseResponse {
    let mut pages: Vec<Page> = Vec::new();
    for wp in &lines.pages {
        let dims = wp.page_width.zip(wp.page_height);
        let kept = kept_lines(&wp.lines);
        let blocks: Vec<Block> = kept
            .iter()
            .map(|l| {
                let markdown = l.markdown();
                let text = l.plain();
                Block {
                    block_type: map_line_type(&l.line_type, l.subtype.as_deref()),
                    content: match fmt {
                        OutputFormat::Markdown => markdown,
                        OutputFormat::Text => text.clone(),
                    },
                    text: Some(text),
                    bbox: dims.and_then(|(w, h)| polygon_bbox(&l.points(), w, h)),
                    confidence: l.confidence,
                    page_number: wp.page,
                }
            })
            .collect();
        let markdown =
            crate::types::join_pages(kept.iter().map(|l| l.markdown()).collect::<Vec<_>>().iter().map(String::as_str));
        let text =
            crate::types::join_pages(kept.iter().map(|l| l.plain()).collect::<Vec<_>>().iter().map(String::as_str));
        pages.push(Page {
            page_number: wp.page,
            width: wp.page_width,
            height: wp.page_height,
            markdown: match fmt {
                OutputFormat::Markdown => markdown,
                OutputFormat::Text => text.clone(),
            },
            text,
            blocks,
        });
    }
    let billed = num_pages.filter(|&p| p > 0).unwrap_or(pages.len() as u32);
    let usage = Usage { pages: billed, credits: None, provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    // The `.mmd` download is Mathpix's own rendering of the whole document; prefer it over the
    // per-page lines joined together (page markdown stays line-derived so boxes and text agree).
    if let Some(mmd) = mmd.map(str::trim).filter(|m| !m.is_empty()) {
        if fmt == OutputFormat::Markdown {
            resp.markdown = mmd.to_string();
        }
    }
    resp
}

pub(crate) fn document_text(lines: &LinesJson, model: &str, num_pages: Option<u32>) -> TextResponse {
    let pages: Vec<TextPage> = lines
        .pages
        .iter()
        .map(|wp| {
            let dims = wp.page_width.zip(wp.page_height);
            let text_lines: Vec<Line> = wp
                .lines
                .iter()
                .filter(|l| !l.plain().is_empty())
                .map(|l| Line {
                    text: l.plain(),
                    bbox: dims.and_then(|(w, h)| polygon_bbox(&l.points(), w, h)),
                    confidence: l.confidence,
                })
                .collect();
            TextPage {
                page_number: wp.page,
                width: wp.page_width,
                height: wp.page_height,
                text: text_lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"),
                lines: text_lines,
                // `/v3/pdf` has no word-level output; only `/v3/text` reports `word_data`.
                words: Vec::new(),
            }
        })
        .collect();
    let billed = num_pages.filter(|&p| p > 0).unwrap_or(pages.len() as u32);
    TextResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, Usage { pages: billed, ..Default::default() })
}

pub(crate) fn normalize_image(result: &TextResult, fmt: OutputFormat, model: &str) -> ParseResponse {
    let dims = result.image_width.zip(result.image_height);
    let blocks: Vec<Block> = result
        .line_data
        .iter()
        .filter(|l| l.kept())
        .filter_map(|l| {
            let markdown = l.text.as_deref().unwrap_or_default().trim().to_string();
            if markdown.is_empty() {
                return None;
            }
            let text = crate::types::markdown_to_text(&markdown);
            Some(Block {
                block_type: map_line_type(&l.line_type, l.subtype.as_deref()),
                content: match fmt {
                    OutputFormat::Markdown => markdown,
                    OutputFormat::Text => text.clone(),
                },
                text: Some(text),
                bbox: dims.and_then(|(w, h)| polygon_bbox(&points(l.cnt.as_deref()), w, h)),
                confidence: l.confidence,
                page_number: 1,
            })
        })
        .collect();
    // `/v3/text` treats the image as a single page, and `text` is its whole Mathpix Markdown.
    let markdown = result.text.as_deref().unwrap_or_default().trim().to_string();
    let text = crate::types::markdown_to_text(&markdown);
    let page = Page {
        page_number: 1,
        width: result.image_width,
        height: result.image_height,
        markdown: match fmt {
            OutputFormat::Markdown => markdown,
            OutputFormat::Text => text.clone(),
        },
        text,
        blocks,
    };
    let mut resp = ParseResponse::from_pages(
        NAME,
        &format!("{NAME}/{model}"),
        vec![page],
        Usage { pages: 1, ..Default::default() },
    );
    if let Some(c) = result.confidence {
        resp.metadata.insert("mathpix_confidence".into(), json!(c));
    }
    resp
}

pub(crate) fn image_text(result: &TextResult, model: &str) -> TextResponse {
    let dims = result.image_width.zip(result.image_height);
    let lines: Vec<Line> = result
        .line_data
        .iter()
        .filter_map(|l| {
            let text = crate::types::markdown_to_text(l.text.as_deref().unwrap_or_default().trim());
            (!text.is_empty()).then(|| Line {
                text,
                bbox: dims.and_then(|(w, h)| polygon_bbox(&points(l.cnt.as_deref()), w, h)),
                confidence: l.confidence,
            })
        })
        .collect();
    let words: Vec<Word> = result
        .word_data
        .iter()
        .filter_map(|w| {
            let text = w.text.as_deref().unwrap_or_default().trim().to_string();
            (!text.is_empty()).then(|| Word {
                text,
                bbox: dims.and_then(|(pw, ph)| polygon_bbox(&points(w.cnt.as_deref()), pw, ph)),
                confidence: w.confidence,
            })
        })
        .collect();
    let text = if lines.is_empty() {
        crate::types::markdown_to_text(result.text.as_deref().unwrap_or_default())
    } else {
        lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    };
    let page = TextPage { page_number: 1, width: result.image_width, height: result.image_height, text, lines, words };
    TextResponse::from_pages(NAME, &format!("{NAME}/{model}"), vec![page], Usage { pages: 1, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_fixture() -> LinesJson {
        serde_json::from_str(include_str!("../../tests/fixtures/mathpix_pdf_lines.json")).unwrap()
    }

    fn image_fixture() -> TextResult {
        serde_json::from_str(include_str!("../../tests/fixtures/mathpix_text.json")).unwrap()
    }

    #[test]
    fn normalizes_document_fixture() {
        let lines = lines_fixture();
        let mmd = "# Hello LiteOCR\n\nInvoice #1234\n\n## Line Items\n\n| Item | Qty |\n| --- | --- |\n| Widget | 2 |";
        let resp = normalize_document(&lines, Some(mmd), OutputFormat::Markdown, "pdf", Some(2));
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages[0].width, Some(1700.0));
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Title);
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"));
        let bb = resp.pages[0].blocks[0].bbox.unwrap();
        assert!((bb.x0 - 0.1).abs() < 1e-6 && bb.y1 < 0.1, "{bb:?}");
        assert_eq!(resp.pages[0].blocks[0].confidence, Some(0.998));
        // `conversion_output: false` lines (the equation number) never become blocks.
        assert!(resp.pages.iter().flat_map(|p| &p.blocks).all(|b| b.content != "(1)"));
        // Table cells are children of the table line and are not emitted twice.
        let tables: Vec<&Block> = resp.pages[1].blocks.iter().filter(|b| b.block_type == BlockType::Table).collect();
        assert_eq!(tables.len(), 1);
        assert!(tables[0].content.contains("| Widget | 2 |"));
        assert!(resp.pages[1].blocks.iter().any(|b| b.block_type == BlockType::Formula));
        assert!(resp.pages[1].blocks.iter().any(|b| b.block_type == BlockType::Header));
        // Document markdown is Mathpix's own `.mmd`, page markdown is line-derived.
        assert_eq!(resp.markdown, mmd);
        assert!(resp.text.contains("Widget"));
    }

    #[test]
    fn document_text_uses_line_geometry() {
        let resp = document_text(&lines_fixture(), "pdf", Some(2));
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert!(resp.pages[0].lines.len() >= 2);
        assert!(resp.pages[0].lines[0].bbox.is_some());
        assert!(resp.pages[0].words.is_empty(), "v3/pdf reports no word data");
        assert!(resp.text.starts_with("Hello LiteOCR"));
    }

    #[test]
    fn normalizes_image_fixture() {
        let result = image_fixture();
        let resp = normalize_image(&result, OutputFormat::Markdown, "text");
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.pages[0].width, Some(850.0));
        assert!(resp.markdown.contains("$f(x)"));
        assert_eq!(resp.pages[0].blocks.len(), 2, "the low-confidence line is excluded");
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Title);
        assert_eq!(resp.pages[0].blocks[1].block_type, BlockType::Formula);
        let bb = resp.pages[0].blocks[0].bbox.unwrap();
        assert!(bb.x0 >= 0.0 && bb.x1 <= 1.0 && bb.y0 < bb.y1, "{bb:?}");
    }

    #[test]
    fn image_text_reports_words() {
        let resp = image_text(&image_fixture(), "text");
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.pages[0].words.len(), 3);
        assert_eq!(resp.pages[0].words[0].text, "Quadratic");
        assert!(resp.pages[0].words[0].bbox.is_some());
        assert!(resp.pages[0].lines.len() >= 2);
        assert!(resp.text.contains("Quadratic"));
    }

    #[test]
    fn picks_the_endpoint_from_the_input_type() {
        let pdf = DocumentInput::Path { path: "/tmp/a.pdf".into() };
        let png = DocumentInput::Path { path: "/tmp/a.png".into() };
        assert!(!use_image_endpoint("pdf", &pdf));
        assert!(use_image_endpoint("pdf", &png), "images must go to /v3/text");
        assert!(use_image_endpoint("text", &pdf), "mathpix/text always uses /v3/text");
    }

    #[test]
    fn builds_options() {
        let req = DocumentRequest::from_url("https://x/y.pdf").pages("1-3,7,10-").provider_options(
            json!({"app_id": "secret", "include_page_breaks": true, "math_inline_delimiters": ["\\(", "\\)"]}),
        );
        let opts = pdf_options(&req).unwrap();
        assert_eq!(opts["page_ranges"], "1-3,7,10--1");
        assert_eq!(opts["include_page_breaks"], true);
        assert_eq!(opts["math_inline_delimiters"], json!(["\\(", "\\)"]), "provider_options win");
        assert!(opts.get("app_id").is_none(), "credentials are never sent as options");

        let img = image_options(&DocumentRequest::from_path("a.png"), Mode::Parse);
        assert_eq!(img["include_line_data"], true);
        assert_eq!(img["enable_document_layout"], true);
        assert!(img.get("include_word_data").is_none(), "layout and word data cannot be combined");
        let img = image_options(&DocumentRequest::from_path("a.png"), Mode::Ocr);
        assert_eq!(img["include_word_data"], true);
        assert!(img.get("enable_document_layout").is_none());
    }

    #[test]
    fn classifies_body_errors_returned_with_http_200() {
        let ok = json!({"pdf_id": "2026_09_11_abc"});
        assert!(check_payload(&ok).is_ok());
        let unauthorized =
            json!({"error": "Unauthorized", "error_info": {"id": "http_unauthorized", "message": "Unauthorized"}});
        assert_eq!(check_payload(&unauthorized).unwrap_err().kind, ErrorKind::Authentication);
        let throttled = json!({"error_info": {"id": "http_max_requests", "message": "Too many requests", "limit_name": "page_monthly_limit"}});
        assert_eq!(check_payload(&throttled).unwrap_err().kind, ErrorKind::RateLimit);
        let encrypted = json!({"error_info": {"id": "pdf_encrypted", "message": "PDF is encrypted"}});
        assert_eq!(check_payload(&encrypted).unwrap_err().kind, ErrorKind::BadRequest);
        let empty = json!({"error": "Image has no content", "error_info": {"id": "image_no_content", "message": "Image has no content"}});
        assert_eq!(check_payload(&empty).unwrap_err().kind, ErrorKind::Provider);
        let bare = json!({"error": "something went wrong"});
        let e = check_payload(&bare).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Provider);
        assert_eq!(e.message, "something went wrong");
    }

    #[test]
    fn rejects_unknown_models() {
        let req = DocumentRequest::from_path("a.pdf");
        let err = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run(&req, "ocr", Mode::Parse))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnsupportedModel);
    }

    /// Live smoke test. Run with:
    /// `MATHPIX_APP_ID=… MATHPIX_APP_KEY=… cargo test -p liteocr-core mathpix -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs MATHPIX_APP_ID + MATHPIX_APP_KEY and network"]
    async fn live_parse_and_ocr() {
        let missing =
            [ENV_APP_ID, ENV_APP_KEY].iter().any(|v| std::env::var(v).map(|s| s.trim().is_empty()).unwrap_or(true));
        if missing {
            eprintln!("skipping: {ENV_APP_ID} / {ENV_APP_KEY} not set");
            return;
        }
        let sample =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");
        let req = DocumentRequest::from_path(sample).model("mathpix/pdf").timeout_secs(240.0);
        let parsed = Mathpix.parse(&req, "pdf").await.expect("parse succeeds");
        assert_eq!(parsed.usage.pages, 2);
        assert_eq!(parsed.pages.len(), 2);
        assert!(!parsed.markdown.trim().is_empty());
        let text = Mathpix.ocr(&req, "pdf").await.expect("ocr succeeds");
        assert!(text.pages.iter().any(|p| !p.lines.is_empty()));
        assert!(text.pages.iter().flat_map(|p| &p.lines).any(|l| l.bbox.is_some()));
    }
}
