//! Mistral Document AI (api.mistral.ai) OCR.
//!
//! Flow: one synchronous `POST /v1/ocr` — there is no job queue. Public URLs are passed straight
//! through as `document_url` / `image_url`; local files are uploaded with `POST /v1/files`
//! (`purpose=ocr`) and referenced by the signed URL from `GET /v1/files/{id}/url`, except small
//! images, which are inlined as a base64 `data:` URL.
//!
//! `extract` mode is the same call with `document_annotation_format` set to a `json_schema`
//! response format; the model returns the object as a JSON **string** in `document_annotation`.

use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::types::{
    BBox, Block, BlockType, DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, OutputFormat, Page,
    ParseResponse, Usage,
};
use crate::util::deep_merge;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub const NAME: &str = "mistral";
const ENV_KEY: &str = "MISTRAL_API_KEY";
const ENV_BASE: &str = "MISTRAL_BASE_URL";
const DEFAULT_BASE: &str = "https://api.mistral.ai";
/// Images at or below this size are inlined as a `data:` URL instead of going through
/// `POST /v1/files`. PDFs and bigger images always take the upload path (Mistral caps documents at
/// 50 MB / 1 000 pages either way).
const INLINE_IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Hours the signed URL handed to `/v1/ocr` stays valid (API default is 24, range 1–168).
const SIGNED_URL_EXPIRY_HOURS: u32 = 1;

#[derive(Debug, Default, Clone, Copy)]
pub struct Mistral;

#[async_trait]
impl Provider for Mistral {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let (wire, raw) = run_ocr(request, model, None).await?;
        let mut resp = normalize(&wire, request.output, model);
        if request.include_raw {
            resp.raw = Some(raw);
        }
        Ok(resp)
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let annotation =
            Annotation { schema: &request.schema, prompt: request.instructions.as_deref().filter(|s| !s.is_empty()) };
        let (wire, raw) = run_ocr(&request.document, model, Some(annotation)).await?;
        let data = decode_annotation(&wire)?;
        let mut resp = ExtractResponse::new(NAME, &format!("{NAME}/{model}"), data, usage(&wire));
        resp.metadata.insert("mistral_model".into(), json!(wire.model));
        if request.citations {
            // Mistral returns no per-field provenance for document annotations.
            resp.metadata.insert("mistral_citations_unsupported".into(), json!(true));
        }
        if request.document.include_raw {
            resp.raw = Some(raw);
        }
        Ok(resp)
    }
}

/// A `document_annotation_format` request (extract mode).
#[derive(Debug)]
struct Annotation<'a> {
    schema: &'a Value,
    prompt: Option<&'a str>,
}

/// Resolve the input, POST `/v1/ocr`, and return the typed payload plus the untouched JSON.
async fn run_ocr(
    request: &DocumentRequest,
    model: &str,
    annotation: Option<Annotation<'_>>,
) -> Result<(OcrWire, Value)> {
    let api_key = provider::resolve_api_key(request, ENV_KEY, NAME)?;
    let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
    let deadline = Deadline::new(request.timeout_secs);
    let retry = Retry::new(request.max_retries);
    let client = http::client();

    let document = document_chunk(request, &api_key, &base, &deadline, retry).await?;
    let body = build_body(request, model, document, annotation.as_ref())?;

    let raw: Value = http::with_retry(NAME, retry, &deadline, || {
        let rb =
            client.post(format!("{base}/v1/ocr")).bearer_auth(&api_key).timeout(deadline.request_timeout()).json(&body);
        async move { http::read_json(NAME, rb.send().await?).await }
    })
    .await?;

    let wire: OcrWire = serde_json::from_value(raw.clone()).map_err(|e| {
        Error::provider(format!("unexpected response shape: {e}; body starts: {}", http::snippet(&raw.to_string())))
            .with_provider(NAME)
    })?;
    Ok((wire, raw))
}

/// Build the `document` chunk: URLs pass through, local files are uploaded (or inlined if small images).
async fn document_chunk(
    request: &DocumentRequest,
    api_key: &str,
    base: &str,
    deadline: &Deadline,
    retry: Retry,
) -> Result<Value> {
    let Some(data) = provider::load_bytes(&request.input).await? else {
        let DocumentInput::Url { url } = &request.input else { unreachable!() };
        return Ok(url_chunk(url, &request.input));
    };
    if is_image(&request.input) && data.len() <= INLINE_IMAGE_MAX_BYTES {
        return Ok(json!({ "type": "image_url", "image_url": data_url(&request.input.mime_type(), &data) }));
    }

    let client = http::client();
    let upload: UploadedFile = http::with_retry(NAME, retry, deadline, || {
        let form = reqwest::multipart::Form::new()
            .text("purpose", "ocr")
            .part("file", provider::file_part(data.clone(), &request.input));
        let rb = client
            .post(format!("{base}/v1/files"))
            .bearer_auth(api_key)
            .timeout(deadline.request_timeout())
            .multipart(form);
        async move { http::read_json(NAME, rb.send().await?).await }
    })
    .await?;
    tracing::debug!(file_id = %upload.id, "mistral: uploaded");

    let signed: SignedUrl = http::with_retry(NAME, retry, deadline, || {
        let rb = client
            .get(format!("{base}/v1/files/{}/url?expiry={SIGNED_URL_EXPIRY_HOURS}", upload.id))
            .bearer_auth(api_key)
            .header("accept", "application/json")
            .timeout(deadline.request_timeout());
        async move { http::read_json(NAME, rb.send().await?).await }
    })
    .await?;
    Ok(json!({ "type": "document_url", "document_url": signed.url, "document_name": request.input.filename() }))
}

fn url_chunk(url: &str, input: &DocumentInput) -> Value {
    if is_image(input) {
        json!({ "type": "image_url", "image_url": url })
    } else {
        json!({ "type": "document_url", "document_url": url, "document_name": input.filename() })
    }
}

fn is_image(input: &DocumentInput) -> bool {
    input.mime_type().starts_with("image/")
}

fn data_url(mime: &str, data: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64_encode(data))
}

fn build_body(
    request: &DocumentRequest,
    model: &str,
    document: Value,
    annotation: Option<&Annotation<'_>>,
) -> Result<Value> {
    let mut body = json!({
        "model": api_model(model)?,
        "document": document,
        "include_image_base64": false,
    });
    if let Some(pages) = &request.pages {
        body["pages"] = Value::Array(zero_based_pages(pages)?);
    }
    if let Some(a) = annotation {
        if !a.schema.is_object() {
            return Err(Error::input("mistral: extract schema must be a JSON Schema object"));
        }
        body["document_annotation_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": "document_annotation",
                "schema": a.schema,
                // Mistral's strict mode requires a closed schema; only claim it when the caller's
                // schema already says `additionalProperties: false`.
                "strict": a.schema.get("additionalProperties") == Some(&Value::Bool(false)),
            },
        });
        if let Some(prompt) = a.prompt {
            body["document_annotation_prompt"] = json!(prompt);
        }
    }
    if let Some(opts) = &request.provider_options {
        deep_merge(&mut body, opts);
    }
    Ok(body)
}

/// PuffinParse model name → Mistral API model id.
fn api_model(model: &str) -> Result<&'static str> {
    match model {
        "ocr-latest" => Ok("mistral-ocr-latest"),
        "ocr-4-1" => Ok("mistral-ocr-4-1"),
        "ocr-4-0" => Ok("mistral-ocr-4-0"),
        "ocr-2512" => Ok("mistral-ocr-2512"),
        other => Err(Error::unsupported_model(format!("mistral: unknown model '{other}'"))),
    }
}

/// PuffinParse's 1-based `"1-3,7"` → Mistral's 0-based `[0,1,2,6]`.
fn zero_based_pages(spec: &str) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for (start, end) in crate::util::parse_page_ranges(spec)? {
        let end = end.ok_or_else(|| {
            Error::input(format!(
                "mistral: open-ended page range in '{spec}' is not supported — give an explicit end (e.g. \"{start}-20\")"
            ))
        })?;
        for p in start..=end {
            let n = p - 1;
            if !out.contains(&json!(n)) {
                out.push(json!(n));
            }
        }
    }
    Ok(out)
}

fn decode_annotation(wire: &OcrWire) -> Result<Value> {
    let text = wire.document_annotation.as_deref().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| {
        Error::provider("response has no document_annotation (the model returned no structured output)")
            .with_provider(NAME)
    })?;
    serde_json::from_str(text).map_err(|e| {
        Error::provider(format!("document_annotation is not valid JSON: {e}; starts: {}", http::snippet(text)))
            .with_provider(NAME)
    })
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UploadedFile {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SignedUrl {
    url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OcrWire {
    #[serde(default)]
    pub pages: Vec<WirePage>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub document_annotation: Option<String>,
    #[serde(default)]
    pub usage_info: Option<UsageInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UsageInfo {
    #[serde(default)]
    pub pages_processed: u32,
    #[serde(default)]
    pub doc_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WirePage {
    /// 0-based page index.
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub markdown: String,
    #[serde(default)]
    pub images: Vec<WireImage>,
    #[serde(default)]
    pub dimensions: Option<Dimensions>,
    /// Present when `include_blocks` is on (the API default for OCR 4+).
    #[serde(default)]
    pub blocks: Option<Vec<WireBlock>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Dimensions {
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
    #[serde(default)]
    pub dpi: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireImage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub top_left_x: Option<f64>,
    #[serde(default)]
    pub top_left_y: Option<f64>,
    #[serde(default)]
    pub bottom_right_x: Option<f64>,
    #[serde(default)]
    pub bottom_right_y: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireBlock {
    #[serde(rename = "type", default)]
    pub block_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub top_left_x: Option<f64>,
    #[serde(default)]
    pub top_left_y: Option<f64>,
    #[serde(default)]
    pub bottom_right_x: Option<f64>,
    #[serde(default)]
    pub bottom_right_y: Option<f64>,
    #[serde(default)]
    pub confidence_scores: Option<BlockConfidence>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BlockConfidence {
    #[serde(default)]
    pub average_content_confidence_score: Option<f64>,
}

// ---- normalisation -----------------------------------------------------------------------------

/// Mistral's structural block labels → PuffinParse block types.
fn map_block_type(t: &str) -> BlockType {
    match t {
        "text" | "aside_text" => BlockType::Text,
        "title" => BlockType::Title,
        "list" => BlockType::List,
        "table" => BlockType::Table,
        "image" => BlockType::Figure,
        "equation" => BlockType::Formula,
        "caption" => BlockType::Caption,
        "header" => BlockType::Header,
        "footer" => BlockType::Footer,
        // `code`, `references`, `signature` have no unified equivalent.
        _ => BlockType::Other,
    }
}

fn bbox(x0: Option<f64>, y0: Option<f64>, x1: Option<f64>, y1: Option<f64>, dims: Option<&Dimensions>) -> Option<BBox> {
    let (w, h) = dims.map(|d| (d.width, d.height))?;
    BBox::from_xywh(x0?, y0?, x1? - x0?, y1? - y0?, w?, h?)
}

fn usage(wire: &OcrWire) -> Usage {
    let pages =
        wire.usage_info.as_ref().map(|u| u.pages_processed).filter(|&n| n > 0).unwrap_or(wire.pages.len() as u32);
    Usage { pages, credits: None, provider_cost_usd: None }
}

pub(crate) fn normalize(wire: &OcrWire, fmt: OutputFormat, model: &str) -> ParseResponse {
    let pages: Vec<Page> = wire
        .pages
        .iter()
        .map(|p| {
            let page_number = p.index.saturating_add(1);
            let dims = p.dimensions.as_ref();
            let render = |md: &str| match fmt {
                OutputFormat::Markdown => md.to_string(),
                OutputFormat::Text => crate::types::markdown_to_text(md),
            };
            let native = p.blocks.as_deref().unwrap_or(&[]);
            let blocks: Vec<Block> = if native.is_empty() {
                // No `blocks` (older models, or `include_blocks: false`): one text block carrying the
                // page markdown, plus a figure block per extracted image so the boxes are not lost.
                let mut blocks = vec![Block {
                    block_type: BlockType::Text,
                    content: render(&p.markdown),
                    text: Some(crate::types::markdown_to_text(&p.markdown)),
                    bbox: None,
                    confidence: None,
                    page_number,
                }];
                blocks.extend(p.images.iter().map(|img| Block {
                    block_type: BlockType::Figure,
                    content: format!("![{}]({})", img.id, img.id),
                    text: Some(String::new()),
                    bbox: bbox(img.top_left_x, img.top_left_y, img.bottom_right_x, img.bottom_right_y, dims),
                    confidence: None,
                    page_number,
                }));
                blocks
            } else {
                native
                    .iter()
                    .map(|b| Block {
                        block_type: map_block_type(&b.block_type),
                        content: render(&b.content),
                        text: None,
                        bbox: bbox(b.top_left_x, b.top_left_y, b.bottom_right_x, b.bottom_right_y, dims),
                        confidence: b.confidence_scores.as_ref().and_then(|c| c.average_content_confidence_score),
                        page_number,
                    })
                    .collect()
            };
            let text = crate::types::markdown_to_text(&p.markdown);
            Page {
                page_number,
                width: dims.and_then(|d| d.width),
                height: dims.and_then(|d| d.height),
                markdown: render(&p.markdown),
                text,
                blocks,
            }
        })
        .collect();

    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage(wire));
    if !wire.model.is_empty() {
        resp.metadata.insert("mistral_model".into(), json!(wire.model));
    }
    if let Some(bytes) = wire.usage_info.as_ref().and_then(|u| u.doc_size_bytes) {
        resp.metadata.insert("mistral_doc_size_bytes".into(), json!(bytes));
    }
    if let Some(dpi) = wire.pages.first().and_then(|p| p.dimensions.as_ref()).and_then(|d| d.dpi) {
        resp.metadata.insert("mistral_dpi".into(), json!(dpi));
    }
    resp
}

// ---- base64 (upload-free inlining of small images) ----------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> OcrWire {
        let raw = match name {
            "ocr" => include_str!("../../tests/fixtures/mistral_ocr.json"),
            "blocks" => include_str!("../../tests/fixtures/mistral_ocr_blocks.json"),
            "annotation" => include_str!("../../tests/fixtures/mistral_annotation.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn normalizes_fixture() {
        let wire = fixture("ocr");
        let resp = normalize(&wire, OutputFormat::Markdown, "ocr-latest");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.model, "mistral/ocr-latest");
        // `index` is 0-based on the wire, `page_number` is 1-based.
        assert_eq!(resp.pages[0].page_number, 1);
        assert_eq!(resp.pages[1].page_number, 2);
        assert_eq!(resp.pages[0].width, Some(1700.0));
        assert_eq!(resp.pages[0].height, Some(2200.0));
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"));
        assert!(resp.markdown.contains("| Item | Amount |"));
        assert!(resp.text.contains("Total: $56.78"));
        // Page 1 has no images: a single text block with no box.
        assert_eq!(resp.pages[0].blocks.len(), 1);
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Text);
        assert!(resp.pages[0].blocks[0].bbox.is_none());
        // Page 2 adds one figure block per extracted image, boxed by the page dimensions.
        assert_eq!(resp.pages[1].blocks.len(), 2);
        let fig = &resp.pages[1].blocks[1];
        assert_eq!(fig.block_type, BlockType::Figure);
        assert_eq!(fig.content, "![img-0.jpeg](img-0.jpeg)");
        let bb = fig.bbox.unwrap();
        assert!((bb.x0 - 292.0 / 1700.0).abs() < 1e-9, "{bb:?}");
        assert!((bb.y1 - 649.0 / 2200.0).abs() < 1e-9, "{bb:?}");
        assert_eq!(resp.metadata["mistral_model"], "mistral-ocr-latest");
        assert_eq!(resp.metadata["mistral_doc_size_bytes"], 30021);
        assert_eq!(resp.metadata["mistral_dpi"], 200.0);
    }

    #[test]
    fn maps_native_blocks() {
        let wire = fixture("blocks");
        let resp = normalize(&wire, OutputFormat::Markdown, "ocr-4-1");
        assert_eq!(resp.pages.len(), 1);
        let blocks = &resp.pages[0].blocks;
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].block_type, BlockType::Title);
        assert_eq!(blocks[1].block_type, BlockType::Text);
        assert_eq!(blocks[2].block_type, BlockType::Table);
        assert_eq!(blocks[3].block_type, BlockType::Figure);
        assert!(blocks[0].confidence.unwrap() > 0.98);
        let bb = blocks[0].bbox.unwrap();
        assert!((bb.x0 - 203.0 / 1700.0).abs() < 1e-9, "{bb:?}");
        assert!(bb.y1 < 0.1);
        // Page markdown still comes from `pages[].markdown`, not from concatenated blocks.
        assert!(resp.pages[0].markdown.starts_with("# Quarterly Report"));
        assert_eq!(resp.usage.pages, 1);
    }

    #[test]
    fn text_output_strips_markdown() {
        let resp = normalize(&fixture("ocr"), OutputFormat::Text, "ocr-latest");
        assert!(!resp.pages[0].markdown.starts_with('#'));
        assert!(resp.text.starts_with("Hello LiteOCR"));
        assert!(resp.pages[0].blocks[0].content.starts_with("Hello LiteOCR"));
    }

    #[test]
    fn decodes_document_annotation() {
        let wire = fixture("annotation");
        let data = decode_annotation(&wire).unwrap();
        assert_eq!(data["invoice_number"], "INV-1234");
        assert_eq!(data["total"], 56.78);
        assert_eq!(data["line_items"][0]["description"], "Widget");
        assert_eq!(usage(&wire).pages, 1);
        // A response without an annotation is a provider error, not a silent empty object.
        let mut empty = wire.clone();
        empty.document_annotation = None;
        assert!(decode_annotation(&empty).is_err());
        let mut junk = wire;
        junk.document_annotation = Some("not json".into());
        assert!(decode_annotation(&junk).is_err());
    }

    #[test]
    fn body_variants() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .pages("1-3,7")
            .provider_options(json!({"include_image_base64": true, "table_format": "html", "image_limit": 5}));
        let body = build_body(&req, "ocr-latest", url_chunk("https://x/y.pdf", &req.input), None).unwrap();
        assert_eq!(body["model"], "mistral-ocr-latest");
        assert_eq!(body["document"]["type"], "document_url");
        assert_eq!(body["document"]["document_url"], "https://x/y.pdf");
        assert_eq!(body["document"]["document_name"], "y.pdf");
        // 1-based "1-3,7" → 0-based [0,1,2,6].
        assert_eq!(body["pages"], json!([0, 1, 2, 6]));
        // provider_options win over our defaults.
        assert_eq!(body["include_image_base64"], true);
        assert_eq!(body["table_format"], "html");
        assert_eq!(body["image_limit"], 5);
        assert!(body.get("document_annotation_format").is_none());

        assert!(build_body(&req, "nope", json!({}), None).is_err());
        let open = DocumentRequest::from_url("https://x/y.pdf").pages("3-");
        assert!(build_body(&open, "ocr-latest", json!({}), None).is_err());
        assert_eq!(api_model("ocr-2512").unwrap(), "mistral-ocr-2512");
    }

    #[test]
    fn annotation_body_wraps_schema() {
        let schema = json!({
            "type": "object",
            "properties": {"total": {"type": "number"}},
            "required": ["total"],
            "additionalProperties": false,
        });
        let req = DocumentRequest::from_url("https://x/y.pdf");
        let a = Annotation { schema: &schema, prompt: Some("read the totals table") };
        let body = build_body(&req, "ocr-latest", json!({}), Some(&a)).unwrap();
        let fmt = &body["document_annotation_format"];
        assert_eq!(fmt["type"], "json_schema");
        assert_eq!(fmt["json_schema"]["name"], "document_annotation");
        assert_eq!(fmt["json_schema"]["schema"], schema);
        assert_eq!(fmt["json_schema"]["strict"], true);
        assert_eq!(body["document_annotation_prompt"], "read the totals table");
        // Open schemas cannot use Mistral's strict mode.
        let open = json!({"type": "object", "properties": {"total": {"type": "number"}}});
        let a = Annotation { schema: &open, prompt: None };
        let body = build_body(&req, "ocr-latest", json!({}), Some(&a)).unwrap();
        assert_eq!(body["document_annotation_format"]["json_schema"]["strict"], false);
        assert!(body.get("document_annotation_prompt").is_none());
    }

    #[test]
    fn image_inputs_use_the_image_chunk() {
        let img = DocumentInput::Url { url: "https://x/receipt.png".into() };
        assert_eq!(
            url_chunk("https://x/receipt.png", &img),
            json!({"type": "image_url", "image_url": "https://x/receipt.png"})
        );
        let pdf = DocumentInput::Url { url: "https://x/y.pdf".into() };
        assert_eq!(url_chunk("https://x/y.pdf", &pdf)["type"], "document_url");
        assert!(is_image(&img) && !is_image(&pdf));
        assert_eq!(data_url("image/png", b"foobar"), "data:image/png;base64,Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(&[0u8, 255, 17]), "AP8R");
    }

    // ---- live (opt-in) --------------------------------------------------------------------------

    const SAMPLE: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");

    #[tokio::test]
    #[ignore = "needs MISTRAL_API_KEY and network"]
    async fn mistral_live_parse() {
        if std::env::var(ENV_KEY).map(|v| v.is_empty()).unwrap_or(true) {
            eprintln!("skipping: {ENV_KEY} not set");
            return;
        }
        let req = DocumentRequest::from_path(SAMPLE).model("mistral/ocr-latest").timeout_secs(240.0);
        let resp = Mistral.parse(&req, "ocr-latest").await.expect("ocr succeeds");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert!(!resp.markdown.trim().is_empty());
        assert!(resp.pages.iter().all(|p| !p.blocks.is_empty()));
    }

    #[tokio::test]
    #[ignore = "needs MISTRAL_API_KEY and network"]
    async fn mistral_live_extract() {
        if std::env::var(ENV_KEY).map(|v| v.is_empty()).unwrap_or(true) {
            eprintln!("skipping: {ENV_KEY} not set");
            return;
        }
        let schema = json!({
            "type": "object",
            "properties": {"title": {"type": "string", "description": "The document title"}},
            "required": ["title"],
            "additionalProperties": false,
        });
        let req = ExtractRequest::new(
            DocumentRequest::from_path(SAMPLE).model("mistral/ocr-latest").timeout_secs(240.0),
            schema,
        );
        let resp = Mistral.extract(&req, "ocr-latest").await.expect("extract succeeds");
        assert!(resp.data.get("title").and_then(Value::as_str).is_some_and(|s| !s.is_empty()));
        assert!(resp.usage.pages >= 1);
    }
}
