//! Open document-parsing VLMs on your own vLLM (or any OpenAI-compatible) server.
//!
//! Each registry model is a **preset** for one open-weights model: the exact full-page prompt and
//! sampling its model card / reference client uses, and the post-processing of its answer. The
//! wire format is the OpenAI chat-completions API that `vllm serve` exposes:
//!
//! `POST {VLLM_BASE_URL}/v1/chat/completions` with one user message per page:
//! `[{"type": "image_url", "image_url": {"url": "data:image/png;base64,…"}}, {"type": "text", "text": <prompt>}]`.
//!
//! These models read one page image at a time, so PDFs are rasterised locally with poppler's
//! `pdftoppm` (as the Tesseract provider does, `PDFTOPPM_CMD`) and the pages are sent concurrently.
//! Both presets answer with a JSON list of layout cells `{"bbox": [x1, y1, x2, y2], "category",
//! "text"}` in reading order, which become typed blocks with normalised boxes:
//!
//! - `infinity-parser2-flash` (`infly/Infinity-Parser2-Flash`): the `doc2json` prompt of the
//!   `infinity_parser2` package; boxes are on a 0–1000 grid; tables HTML, formulas LaTeX.
//! - `dots.mocr` (`rednote-hilab/dots.mocr`): the `prompt_layout_all_en` prompt of the `dots_mocr`
//!   package; boxes are pixels of the image after the server's `smart_resize` (factor 28,
//!   3136–11 289 600 pixels), mapped back the way `dots_mocr.utils.layout_utils.post_process_cells`
//!   does; tables HTML, formulas LaTeX, pictures carry no text.
//!
//! Configuration: `VLLM_BASE_URL` (or `base_url` on the request; default `http://localhost:8000`,
//! with or without a trailing `/v1`), optional `VLLM_API_KEY` / `api_key` sent as a bearer token
//! (`vllm serve --api-key`), and `VLLM_SERVED_MODEL` or `provider_options.served_model` when the
//! server's model name differs from the preset's. `provider_options.dpi` (PDF rasterisation),
//! `concurrency` (pages in flight, default 4) and `prompt` (replace the preset prompt) are read by
//! PuffinParse; every other key is merged into the chat-completions body. No price: your compute.

use super::paddleocr::table_html_text;
use super::vlm::{data_url, sniff_mime};
use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::providers::local::{self, FileKind, ScratchDir};
use crate::types::{markdown_to_text, BBox, Block, BlockType, DocumentRequest, OutputFormat, Page, ParseResponse, Usage};
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const NAME: &str = "vllm";
const ENV_BASE: &str = "VLLM_BASE_URL";
const ENV_KEY: &str = "VLLM_API_KEY";
const ENV_SERVED: &str = "VLLM_SERVED_MODEL";
const ENV_PDFTOPPM: &str = "PDFTOPPM_CMD";
const DEFAULT_BASE: &str = "http://localhost:8000";
const DEFAULT_CONCURRENCY: usize = 4;
/// `provider_options` keys PuffinParse consumes instead of forwarding to the server.
const OWN_OPTIONS: &[&str] = &["served_model", "dpi", "concurrency", "prompt"];

/// How a preset's answer is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Infinity-Parser2 `doc2json`: lower-case categories, boxes on a 0–1000 grid.
    Infinity,
    /// dots.mocr `prompt_layout_all_en`: DocLayNet-style categories, boxes in resized-image pixels.
    Dots,
}

/// One open model: what to send and how to read the answer.
#[derive(Debug)]
struct Preset {
    model: &'static str,
    /// Model name the server answers to with the `vllm serve` command in the docs.
    served_model: &'static str,
    family: Family,
    prompt: &'static str,
    /// Put before the prompt text (dots.mocr's client adds its image placeholder tokens).
    text_prefix: &'static str,
    dpi: u32,
    temperature: f64,
    top_p: f64,
    /// The token-limit field and value the reference client sends.
    max_tokens: (&'static str, u64),
}

/// `PROMPT_DOC2JSON`, verbatim from `infinity_parser2/prompts.py` (infly-ai/INF-MLLM, read 2026-10-08).
const INFINITY_DOC2JSON: &str = r#"
- Extract layout information from the provided PDF image.
- For each layout element, output its bbox, category, and the text content within the bbox.
- Bbox format: [x1, y1, x2, y2].
- Allowed layout categories: ['header', 'title', 'text', 'figure', 'table', 'formula', 'figure_caption', 'table_caption', 'formula_caption', 'figure_footnote', 'table_footnote', 'page_footnote', 'footer'].
- Text extraction and formatting:
  1) For 'figure', the text field must be an empty string.
  2) For 'formula', format text as LaTeX.
  3) For 'table', format text as HTML.
  4) For all other categories (e.g., text, title), format text as Markdown.
- The output text must be exactly the original text from the image, with no translation or rewriting.
- Sort all layout elements in human reading order.
- Final output must be a single JSON object.
"#;

/// `prompt_layout_all_en`, verbatim from `dots_mocr/utils/prompts.py` (rednote-hilab/dots.mocr, read 2026-10-08).
const DOTS_LAYOUT_ALL_EN: &str = r#"Please output the layout information from the PDF image, including each layout element's bbox, its category, and the corresponding text content within the bbox.

1. Bbox format: [x1, y1, x2, y2]

2. Layout Categories: The possible categories are ['Caption', 'Footnote', 'Formula', 'List-item', 'Page-footer', 'Page-header', 'Picture', 'Section-header', 'Table', 'Text', 'Title'].

3. Text Extraction & Formatting Rules:
    - Picture: For the 'Picture' category, the text field should be omitted.
    - Formula: Format its text as LaTeX.
    - Table: Format its text as HTML.
    - All Others (Text, Title, etc.): Format their text as Markdown.

4. Constraints:
    - The output text must be the original text from the image, with no translation.
    - All layout elements must be sorted according to human reading order.

5. Final Output: The entire output must be a single JSON object.
"#;

const PRESETS: &[Preset] = &[
    Preset {
        model: "infinity-parser2-flash",
        served_model: "infly/Infinity-Parser2-Flash",
        family: Family::Infinity,
        prompt: INFINITY_DOC2JSON,
        text_prefix: "",
        // `convert_pdf_to_images(dpi=300)`; sampling from `backends/vllm_server.py`.
        dpi: 300,
        temperature: 0.0,
        top_p: 1.0,
        max_tokens: ("max_tokens", 32_768),
    },
    Preset {
        model: "dots.mocr",
        // `vllm serve rednote-hilab/dots.mocr ... --served-model-name model` in the model card.
        served_model: "model",
        family: Family::Dots,
        prompt: DOTS_LAYOUT_ALL_EN,
        // `inference_with_vllm`: without the placeholder, vLLM v1 inserts a newline before the prompt.
        text_prefix: "<|img|><|imgpad|><|endofimg|>",
        // `DotsMOCRParser` defaults: dpi 200, temperature 0.1, top_p 1.0, 32 768 completion tokens.
        dpi: 200,
        temperature: 0.1,
        top_p: 1.0,
        max_tokens: ("max_completion_tokens", 32_768),
    },
];

fn preset(model: &str) -> Result<&'static Preset> {
    PRESETS.iter().find(|p| p.model == model).ok_or_else(|| {
        Error::unsupported_model(format!(
            "vllm: no preset for '{model}' (known: {})",
            PRESETS.iter().map(|p| p.model).collect::<Vec<_>>().join(", ")
        ))
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Vllm;

#[async_trait]
impl Provider for Vllm {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let preset = preset(model)?;
        let call = Call::new(request, preset)?;
        let images = page_images(request, &call).await?;
        let concurrency = call.concurrency;
        let pending = images.into_iter().map(|img| call.page(img));
        let results: Vec<Result<PageResult>> = stream::iter(pending).buffered(concurrency).collect().await;
        let results = results.into_iter().collect::<Result<Vec<_>>>()?;
        Ok(assemble(results, request, model, &call.served_model))
    }
}

// ---- call plumbing -------------------------------------------------------------------------------

struct Call {
    url: String,
    api_key: Option<String>,
    served_model: String,
    preset: &'static Preset,
    prompt: String,
    dpi: u32,
    concurrency: usize,
    extra: Option<Value>,
    deadline: Deadline,
    retry: Retry,
}

/// Never print the key, not even through `{:?}`.
impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("url", &self.url)
            .field("served_model", &self.served_model)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Call {
    fn new(request: &DocumentRequest, preset: &'static Preset) -> Result<Self> {
        if request.provider_options.as_ref().is_some_and(|o| !o.is_object()) {
            return Err(Error::input("vllm: provider_options must be a JSON object").with_provider(NAME));
        }
        let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
        let base = base.strip_suffix("/v1").unwrap_or(&base).to_string();
        let opt_u32 = |k: &str| -> Result<Option<u32>> {
            match request.option(k) {
                None | Some(Value::Null) => Ok(None),
                Some(v) => v
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|&n| n > 0)
                    .map(Some)
                    .ok_or_else(|| Error::input(format!("vllm: provider_options.{k} must be a positive integer"))),
            }
        };
        let extra = request.provider_options.clone().and_then(|mut o| {
            let map = o.as_object_mut()?;
            for k in OWN_OPTIONS {
                map.remove(*k);
            }
            (!map.is_empty()).then_some(o)
        });
        Ok(Self {
            url: format!("{base}/v1/chat/completions"),
            api_key: request.api_key.clone().filter(|k| !k.trim().is_empty()).or_else(|| env(ENV_KEY)),
            served_model: request
                .option("served_model")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| env(ENV_SERVED))
                .unwrap_or_else(|| preset.served_model.to_string()),
            preset,
            prompt: request.option("prompt").and_then(Value::as_str).unwrap_or(preset.prompt).to_string(),
            dpi: opt_u32("dpi")?.unwrap_or(preset.dpi),
            concurrency: opt_u32("concurrency")?.map(|n| n as usize).unwrap_or(DEFAULT_CONCURRENCY),
            extra,
            deadline: Deadline::new(request.timeout_secs),
            retry: Retry::new(request.max_retries),
        })
    }

    /// The chat-completions body for one page image.
    fn body(&self, image: &PageImage) -> Value {
        let p = self.preset;
        let mut body = json!({
            "model": self.served_model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "image_url", "image_url": {"url": data_url(&image.mime, &image.data)}},
                    {"type": "text", "text": format!("{}{}", p.text_prefix, self.prompt)},
                ],
            }],
            "temperature": p.temperature,
            "top_p": p.top_p,
        });
        body[p.max_tokens.0] = json!(p.max_tokens.1);
        if let Some(extra) = &self.extra {
            crate::util::deep_merge(&mut body, extra);
        }
        body
    }

    async fn page(&self, image: PageImage) -> Result<PageResult> {
        let body = self.body(&image);
        let client = http::client();
        let raw: Value = http::with_retry(NAME, self.retry, &self.deadline, || {
            let mut rb = client.post(&self.url).timeout(self.deadline.request_timeout()).json(&body);
            if let Some(key) = &self.api_key {
                rb = rb.bearer_auth(key);
            }
            let url = self.url.clone();
            async move {
                let resp = rb.send().await.map_err(|e| connect_hint(e, &url))?;
                http::read_json(NAME, resp).await
            }
        })
        .await?;
        let choice = raw.pointer("/choices/0").ok_or_else(|| {
            Error::provider(format!("response has no choices: {}", http::snippet(&raw.to_string()))).with_provider(NAME)
        })?;
        let content = choice.pointer("/message/content").and_then(Value::as_str).unwrap_or_default().to_string();
        let truncated = choice.get("finish_reason").and_then(Value::as_str) == Some("length");
        Ok(PageResult { page_number: image.page_number, size: image.size, content, truncated, raw })
    }
}

fn connect_hint(e: reqwest::Error, url: &str) -> Error {
    if e.is_connect() {
        Error::network(format!(
            "cannot reach the vLLM server at {url} ({e}); start it with `vllm serve <model>` (see \
             docs/providers/vllm.md) or set {ENV_BASE}"
        ))
        .with_provider(NAME)
    } else {
        e.into()
    }
}

/// One page as sent to the model.
struct PageImage {
    page_number: u32,
    data: Vec<u8>,
    mime: String,
    /// Pixel `(width, height)`, when the header could be read.
    size: Option<(u32, u32)>,
}

/// The document as page images: PDFs rasterised with `pdftoppm`, images sent as they are.
async fn page_images(request: &DocumentRequest, call: &Call) -> Result<Vec<PageImage>> {
    let filename = request.input.filename();
    let data = local::load_or_download(NAME, request, &call.deadline).await?;
    let ranges = request.pages.as_deref().map(crate::util::parse_page_ranges).transpose()?;
    match local::sniff(&data, &filename) {
        FileKind::Pdf => {
            let scratch = ScratchDir::new(NAME).await?;
            let pdftoppm = std::env::var(ENV_PDFTOPPM)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "pdftoppm".to_string());
            let (selection, deadline) = (ranges.as_deref(), &call.deadline);
            let pages = local::rasterize_pdf(NAME, &pdftoppm, &data, call.dpi, selection, &scratch, deadline).await?;
            if pages.is_empty() {
                let msg = format!("{filename}: no pages to parse (page selection {:?})", request.pages);
                return Err(Error::input(msg));
            }
            let mut out = Vec::with_capacity(pages.len());
            for (page_number, path) in pages {
                let png = tokio::fs::read(&path)
                    .await
                    .map_err(|e| Error::provider(format!("pdftoppm output {}: {e}", path.display())))?;
                let size = local::image_size(&png);
                out.push(PageImage { page_number, data: png, mime: "image/png".into(), size });
            }
            Ok(out)
        }
        FileKind::Image => {
            let mime = sniff_mime(&data, &request.input.mime_type());
            let size = local::image_size(&data);
            Ok(vec![PageImage { page_number: 1, data: data.to_vec(), mime, size }])
        }
        FileKind::Other => {
            let msg = format!("vllm reads images and PDFs only; '{filename}' is neither (convert it to PDF first)");
            Err(Error::input(msg).with_provider(NAME))
        }
    }
}

// ---- answer → unified types ------------------------------------------------------------------------

/// One page's answer.
#[derive(Debug)]
struct PageResult {
    page_number: u32,
    size: Option<(u32, u32)>,
    content: String,
    truncated: bool,
    raw: Value,
}

/// Pages, usage and metadata from every page's answer.
fn assemble(results: Vec<PageResult>, request: &DocumentRequest, model: &str, served_model: &str) -> ParseResponse {
    let family = preset(model).map(|p| p.family).unwrap_or(Family::Infinity);
    let mut blocks = Vec::new();
    let mut dims = BTreeMap::new();
    let (mut unstructured, mut recovered, mut truncated) = (Vec::new(), Vec::new(), Vec::new());
    let (mut input_tokens, mut output_tokens) = (0u64, 0u64);
    for r in &results {
        if let Some((w, h)) = r.size {
            dims.insert(r.page_number, (f64::from(w), f64::from(h)));
        }
        let tokens = |k: &str| r.raw.pointer(&format!("/usage/{k}")).and_then(Value::as_u64).unwrap_or(0);
        input_tokens += tokens("prompt_tokens");
        output_tokens += tokens("completion_tokens");
        if r.truncated {
            truncated.push(r.page_number);
        }
        match layout_cells(&r.content) {
            Some((cells, was_cut)) => {
                if was_cut {
                    recovered.push(r.page_number);
                }
                for cell in &cells {
                    if let Some(b) = cell_block(cell, family, r.page_number, r.size, request.output) {
                        blocks.push(b);
                    }
                }
            }
            None => {
                // Not a layout list: keep whatever the model said as one block rather than lose it.
                let text = strip_fences(&r.content).trim().to_string();
                if !text.is_empty() {
                    unstructured.push(r.page_number);
                    let plain = markdown_to_text(&text);
                    let content = if request.output == OutputFormat::Text { plain.clone() } else { text };
                    blocks.push(Block {
                        block_type: BlockType::Text,
                        content,
                        text: Some(plain),
                        bbox: None,
                        confidence: None,
                        page_number: r.page_number,
                    });
                }
            }
        }
    }
    let mut pages = crate::types::pages_from_blocks(blocks, &dims);
    for r in &results {
        if !pages.iter().any(|p| p.page_number == r.page_number) {
            let d = dims.get(&r.page_number);
            pages.push(Page {
                page_number: r.page_number,
                width: d.map(|d| d.0),
                height: d.map(|d| d.1),
                markdown: String::new(),
                text: String::new(),
                blocks: vec![],
            });
        }
    }
    pages.sort_by_key(|p| p.page_number);
    let usage = Usage { pages: pages.len() as u32, credits: None, provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    resp.metadata.insert("vllm_served_model".into(), json!(served_model));
    resp.metadata.insert("vllm_input_tokens".into(), json!(input_tokens));
    resp.metadata.insert("vllm_output_tokens".into(), json!(output_tokens));
    let page_lists = [
        ("vllm_unstructured_pages", unstructured),
        ("vllm_recovered_pages", recovered),
        ("vllm_truncated_pages", truncated),
    ];
    for (key, list) in page_lists {
        if !list.is_empty() {
            resp.metadata.insert(key.into(), json!(list));
        }
    }
    if request.include_raw {
        resp.raw = Some(Value::Array(results.into_iter().map(|r| r.raw).collect()));
    }
    resp
}

/// The JSON inside a ```` ```json ```` fence (closed or not), or the whole answer.
fn strip_fences(text: &str) -> &str {
    let t = text.trim();
    let Some(start) = t.find("```") else { return t };
    let after = &t[start + 3..];
    let after = after.strip_prefix("json").or_else(|| after.strip_prefix("markdown")).unwrap_or(after);
    let after = after.trim_start_matches(['\r', '\n']);
    match after.find("```") {
        Some(end) => after[..end].trim(),
        None => after.trim(),
    }
}

/// The layout cells of an answer, and whether a cut-off last cell had to be dropped to parse it
/// (Infinity-Parser2's `truncate_last_incomplete_element`). `None` when it is not a cell list.
fn layout_cells(content: &str) -> Option<(Vec<Value>, bool)> {
    let text = strip_fences(content);
    if let Some(cells) = cells_from(text) {
        return Some((cells, false));
    }
    if text.matches("{\"bbox\"").count() <= 1 {
        return None;
    }
    let cut = text[..text.rfind("{\"bbox\"")?].trim_end();
    let cut = format!("{}]", cut.strip_suffix(',').unwrap_or(cut));
    cells_from(&cut).map(|c| (c, true))
}

fn cells_from(text: &str) -> Option<Vec<Value>> {
    let v: Value = serde_json::from_str(text).ok()?;
    let list = match v {
        Value::Array(a) => a,
        Value::Object(o) if o.contains_key("bbox") || o.contains_key("category") => vec![Value::Object(o)],
        // `{"layout": [...]}`-style wrappers: take the first list of cells.
        Value::Object(o) => o.into_iter().find_map(|(_, x)| if let Value::Array(a) = x { Some(a) } else { None })?,
        _ => return None,
    };
    // Multi-page outputs nest a list per page; flatten one level as the reference code does.
    let list: Vec<Value> =
        list.into_iter().flat_map(|x| if let Value::Array(inner) = x { inner } else { vec![x] }).collect();
    list.iter().all(Value::is_object).then_some(list)
}

/// One layout cell → a block (or `None` for an empty non-figure cell).
fn cell_block(
    cell: &Value,
    family: Family,
    page_number: u32,
    size: Option<(u32, u32)>,
    fmt: OutputFormat,
) -> Option<Block> {
    let category = cell.get("category").and_then(Value::as_str).unwrap_or("Text");
    let raw = cell.get("text").and_then(Value::as_str).unwrap_or_default().trim();
    let block_type = block_type(category, family);
    let (markdown, plain) = match block_type {
        BlockType::Table => (raw.to_string(), if raw.contains('<') { table_html_text(raw) } else { raw.to_string() }),
        BlockType::Formula => (formula_markdown(raw), raw.to_string()),
        BlockType::Figure => (raw.to_string(), raw.to_string()),
        _ => (raw.to_string(), markdown_to_text(raw)),
    };
    if markdown.is_empty() && block_type != BlockType::Figure {
        return None;
    }
    let bbox = cell.get("bbox").and_then(|b| cell_bbox(b, family, size));
    let content = if fmt == OutputFormat::Text { plain.clone() } else { markdown };
    Some(Block { block_type, content, text: Some(plain), bbox, confidence: None, page_number })
}

fn block_type(category: &str, family: Family) -> BlockType {
    match family {
        Family::Infinity => match category {
            "header" => BlockType::Header,
            "title" => BlockType::Title,
            "figure" => BlockType::Figure,
            "table" => BlockType::Table,
            "formula" => BlockType::Formula,
            "figure_caption" | "table_caption" | "formula_caption" => BlockType::Caption,
            "figure_footnote" | "table_footnote" | "page_footnote" => BlockType::Footnote,
            "footer" => BlockType::Footer,
            _ => BlockType::Text,
        },
        Family::Dots => match category {
            "Caption" => BlockType::Caption,
            "Footnote" => BlockType::Footnote,
            "Formula" => BlockType::Formula,
            "List-item" => BlockType::List,
            "Page-footer" => BlockType::Footer,
            "Page-header" => BlockType::Header,
            "Picture" => BlockType::Figure,
            "Section-header" => BlockType::SectionHeader,
            "Table" => BlockType::Table,
            "Title" => BlockType::Title,
            _ => BlockType::Text,
        },
    }
}

/// `[x1, y1, x2, y2]` → a normalised box. Infinity-Parser2 answers on a 0–1000 grid; dots.mocr in
/// pixels of the image the server actually saw, i.e. after `smart_resize` of the page we sent.
fn cell_bbox(v: &Value, family: Family, size: Option<(u32, u32)>) -> Option<BBox> {
    let n: Vec<f64> = v.as_array()?.iter().filter_map(Value::as_f64).collect();
    if n.len() != 4 {
        return None;
    }
    let (frame_w, frame_h) = match family {
        Family::Infinity => (1000.0, 1000.0),
        Family::Dots => {
            let (w, h) = size?;
            let (rh, rw) = smart_resize(h, w, DOTS_FACTOR, DOTS_MIN_PIXELS, DOTS_MAX_PIXELS);
            (f64::from(rw), f64::from(rh))
        }
    };
    let (x0, y0, x1, y1) = (n[0].min(n[2]), n[1].min(n[3]), n[0].max(n[2]), n[1].max(n[3]));
    BBox::from_xywh(x0, y0, x1 - x0, y1 - y0, frame_w, frame_h)
}

/// `dots_mocr/utils/consts.py`.
const DOTS_FACTOR: u32 = 28;
const DOTS_MIN_PIXELS: u64 = 3136;
const DOTS_MAX_PIXELS: u64 = 11_289_600;

/// `smart_resize` from `dots_mocr/utils/image_utils.py` (the Qwen2-VL rule): both sides a multiple
/// of `factor`, total pixels within `[min_pixels, max_pixels]`, aspect ratio kept. Returns
/// `(height, width)`. Python's `round` rounds half to even, hence `round_ties_even`.
fn smart_resize(height: u32, width: u32, factor: u32, min_pixels: u64, max_pixels: u64) -> (u32, u32) {
    let (h, w, f) = (f64::from(height), f64::from(width), f64::from(factor));
    let (min, max) = (min_pixels as f64, max_pixels as f64);
    let round_by = |x: f64| (x / f).round_ties_even() * f;
    let floor_by = |x: f64| (x / f).floor() * f;
    let ceil_by = |x: f64| (x / f).ceil() * f;
    let mut h_bar = f.max(round_by(h));
    let mut w_bar = f.max(round_by(w));
    if h_bar * w_bar > max {
        let beta = (h * w / max).sqrt();
        h_bar = f.max(floor_by(h / beta));
        w_bar = f.max(floor_by(w / beta));
    } else if h_bar * w_bar < min {
        let beta = (min / (h * w)).sqrt();
        h_bar = ceil_by(h * beta);
        w_bar = ceil_by(w * beta);
        if h_bar * w_bar > max {
            let beta = (h_bar * w_bar / max).sqrt();
            h_bar = f.max(floor_by(h_bar / beta));
            w_bar = f.max(floor_by(w_bar / beta));
        }
    }
    (h_bar as u32, w_bar as u32)
}

/// A formula as a Markdown display block, following dots.mocr's `get_formula_in_markdown`:
/// `$$…$$` and `\[…\]` are normalised to a fenced `$$` block, inline `$…$` is kept, bare LaTeX is
/// wrapped.
fn formula_markdown(latex: &str) -> String {
    let t = latex.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.len() >= 4 && t.starts_with("$$") && t.ends_with("$$") {
        let inner = t[2..t.len() - 2].trim();
        return if inner.contains('$') { t.to_string() } else { format!("$$\n{inner}\n$$") };
    }
    if t.len() >= 4 && t.starts_with("\\[") && t.ends_with("\\]") {
        return format!("$$\n{}\n$$", t[2..t.len() - 2].trim());
    }
    if t.contains('$') || t.contains("\\[") {
        return t.to_string();
    }
    format!("$$\n{t}\n$$")
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFINITY_FIXTURE: &str = include_str!("../../tests/fixtures/vllm_infinity_parser2_chat.json");
    const DOTS_FIXTURE: &str = include_str!("../../tests/fixtures/vllm_dots_mocr_chat.json");

    fn result(raw: &str, page_number: u32, size: Option<(u32, u32)>) -> PageResult {
        let raw: Value = serde_json::from_str(raw).unwrap();
        let content = raw.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap().to_string();
        PageResult { page_number, size, content, truncated: false, raw }
    }

    /// A minimal PNG header of the given size (only IHDR is read).
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v
    }

    #[test]
    fn infinity_cells_become_typed_blocks_on_a_1000_grid() {
        let req = DocumentRequest::from_path("a.png");
        let resp = assemble(
            vec![result(INFINITY_FIXTURE, 1, Some((1700, 2200)))],
            &req,
            "infinity-parser2-flash",
            "infly/Infinity-Parser2-Flash",
        );
        assert_eq!(resp.model, "vllm/infinity-parser2-flash");
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.usage.provider_cost_usd, None);
        let p = &resp.pages[0];
        assert_eq!((p.width, p.height), (Some(1700.0), Some(2200.0)));
        let types: Vec<BlockType> = p.blocks.iter().map(|b| b.block_type).collect();
        assert_eq!(
            types,
            [
                BlockType::Header,
                BlockType::Title,
                BlockType::Text,
                BlockType::Table,
                BlockType::Caption,
                BlockType::Formula,
                BlockType::Figure,
                BlockType::Footnote,
                BlockType::Footer,
            ]
        );
        assert_eq!(p.blocks[1].content, "# Cedar Ridge Supply");
        let plain = p.blocks[2].text.as_deref().unwrap();
        assert!(plain.contains("Hillside Farms") && !plain.contains("**"), "{plain}");
        assert!(p.blocks[3].content.starts_with("<table>"), "tables stay HTML");
        assert_eq!(p.blocks[3].text.as_deref(), Some("Item Qty Price\nFence post 12 $8.50"));
        assert_eq!(p.blocks[5].content, "$$\n\\text{Total} = 12 \\times 8.50 = 102.00\n$$");
        assert_eq!(p.blocks[6].content, "", "figures keep their box but carry no text");
        let bb = p.blocks[1].bbox.unwrap();
        assert!((bb.x0 - 0.070).abs() < 1e-9 && (bb.y0 - 0.060).abs() < 1e-9, "{bb:?}");
        assert!((bb.x1 - 0.640).abs() < 1e-9 && (bb.y1 - 0.104).abs() < 1e-9, "{bb:?}");
        assert_eq!(resp.metadata["vllm_input_tokens"], 4105);
        assert_eq!(resp.metadata["vllm_output_tokens"], 532);
        assert_eq!(resp.metadata["vllm_served_model"], "infly/Infinity-Parser2-Flash");
        assert!(!resp.metadata.contains_key("vllm_unstructured_pages"));
        assert!(resp.markdown.contains("| Fence post") || resp.markdown.contains("<td>Fence post</td>"));
    }

    #[test]
    fn dots_boxes_map_back_through_smart_resize() {
        let req = DocumentRequest::from_path("a.pdf").include_raw(true);
        let resp = assemble(vec![result(DOTS_FIXTURE, 2, Some((1700, 2200)))], &req, "dots.mocr", "model");
        assert_eq!(resp.model, "vllm/dots.mocr");
        let p = &resp.pages[0];
        assert_eq!(p.page_number, 2);
        let types: Vec<BlockType> = p.blocks.iter().map(|b| b.block_type).collect();
        assert_eq!(
            types,
            [
                BlockType::Header,
                BlockType::Title,
                BlockType::SectionHeader,
                BlockType::Text,
                BlockType::List,
                BlockType::Table,
                BlockType::Formula,
                BlockType::Figure,
                BlockType::Footer,
            ]
        );
        assert_eq!(p.blocks[6].content, "$$\nfee = 0.015 \\times balance\n$$");
        // 1700x2200 is resized to 1708x2212 (multiples of 28); the picture covers the lower-right quarter.
        let bb = p.blocks[7].bbox.unwrap();
        assert!((bb.x0 - 0.5).abs() < 1e-9 && (bb.y0 - 0.5).abs() < 1e-9, "{bb:?}");
        assert!((bb.x1 - 1.0).abs() < 1e-9 && (bb.y1 - 1.0).abs() < 1e-9, "{bb:?}");
        assert!(resp.raw.as_ref().unwrap().is_array());

        // Without the image size there is no frame to normalise pixels against.
        let resp = assemble(vec![result(DOTS_FIXTURE, 1, None)], &req, "dots.mocr", "model");
        assert!(resp.pages[0].blocks.iter().all(|b| b.bbox.is_none()));
    }

    #[test]
    fn smart_resize_matches_the_reference() {
        assert_eq!(smart_resize(2200, 1700, 28, 3136, 11_289_600), (2212, 1708));
        // Too large: scaled down below max_pixels, floored to the factor.
        let (h, w) = smart_resize(6600, 5100, 28, 3136, 11_289_600);
        assert!(u64::from(h) * u64::from(w) <= 11_289_600 && h % 28 == 0 && w % 28 == 0, "{h}x{w}");
        // Too small: scaled up to min_pixels.
        assert_eq!(smart_resize(10, 10, 28, 3136, 11_289_600), (56, 56));
        // Python rounds half to even: 42/28 = 1.5 -> 2, 70/28 = 2.5 -> 2.
        assert_eq!(smart_resize(42, 70, 28, 0, 11_289_600), (56, 56));
    }

    #[test]
    fn broken_answers_are_recovered_or_kept_as_text() {
        let cut = r#"```json
[{"bbox": [1, 2, 3, 4], "category": "text", "text": "kept"}, {"bbox": [5, 6, 7, 8], "category": "text", "text": "cut of"#;
        let (cells, was_cut) = layout_cells(cut).unwrap();
        assert!(was_cut);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0]["text"], "kept");
        assert!(layout_cells("Just some prose.").is_none());
        let wrapped = r#"{"layout": [{"bbox": [1,2,3,4], "category": "Text", "text": "x"}]}"#;
        assert_eq!(layout_cells(wrapped).unwrap().0.len(), 1);
        assert_eq!(layout_cells(r#"[[{"bbox": [1,2,3,4], "category": "Text", "text": "x"}]]"#).unwrap().0.len(), 1);

        let req = DocumentRequest::from_path("a.png");
        let mut r = result(INFINITY_FIXTURE, 1, None);
        r.content = "# Heading\n\nNot JSON at all.".into();
        r.truncated = true;
        let resp = assemble(vec![r], &req, "infinity-parser2-flash", "m");
        assert_eq!(resp.pages[0].blocks.len(), 1);
        assert_eq!(resp.pages[0].markdown, "# Heading\n\nNot JSON at all.");
        assert_eq!(resp.metadata["vllm_unstructured_pages"], json!([1]));
        assert_eq!(resp.metadata["vllm_truncated_pages"], json!([1]));
    }

    #[test]
    fn formulas_become_display_blocks() {
        assert_eq!(formula_markdown("x^2"), "$$\nx^2\n$$");
        assert_eq!(formula_markdown("$$ x^2 $$"), "$$\nx^2\n$$");
        assert_eq!(formula_markdown("\\[ x^2 \\]"), "$$\nx^2\n$$");
        assert_eq!(formula_markdown("where $x$ is"), "where $x$ is");
        assert_eq!(formula_markdown("  "), "");
    }

    #[test]
    fn body_follows_each_reference_client() {
        let req = DocumentRequest::from_path("a.png");
        let img = PageImage { page_number: 1, data: png(10, 10), mime: "image/png".into(), size: Some((10, 10)) };

        let call = Call::new(&req, preset("infinity-parser2-flash").unwrap()).unwrap();
        let body = call.body(&img);
        assert_eq!(body["model"], "infly/Infinity-Parser2-Flash");
        assert_eq!(body["messages"][0]["content"][0]["type"], "image_url");
        let url = body["messages"][0]["content"][0]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert_eq!(body["messages"][0]["content"][1]["text"], INFINITY_DOC2JSON);
        assert_eq!((body["temperature"].as_f64(), body["top_p"].as_f64()), (Some(0.0), Some(1.0)));
        assert_eq!(body["max_tokens"], 32_768);
        assert_eq!(call.dpi, 300);

        let req = DocumentRequest::from_path("a.png").provider_options(json!({
            "served_model": "dots", "dpi": 150, "concurrency": 2, "repetition_penalty": 1.05,
        }));
        let call = Call::new(&req, preset("dots.mocr").unwrap()).unwrap();
        let body = call.body(&img);
        assert_eq!(body["model"], "dots");
        let text = body["messages"][0]["content"][1]["text"].as_str().unwrap();
        assert!(text.starts_with("<|img|><|imgpad|><|endofimg|>Please output the layout information"));
        assert_eq!(body["temperature"].as_f64(), Some(0.1));
        assert_eq!(body["max_completion_tokens"], 32_768);
        assert_eq!(body["repetition_penalty"], 1.05, "unknown options pass through");
        assert!(body.get("served_model").is_none() && body.get("dpi").is_none());
        assert_eq!((call.dpi, call.concurrency), (150, 2));

        assert!(Call::new(&DocumentRequest::from_path("a").provider_options(json!({"dpi": 0})), &PRESETS[0]).is_err());
        assert!(preset("nope").is_err());
        assert!(!format!("{call:?}").contains("Bearer"));
    }

    #[tokio::test]
    async fn sends_one_request_per_image_and_parses_the_answer() {
        let (base, captured) = crate::testutil::serve(vec![(200, DOTS_FIXTURE.to_string())]).await;
        let req = DocumentRequest::from_bytes(png(1700, 2200), "page.png")
            .base_url(format!("{base}/v1"))
            .api_key("secret-test-key");
        let resp = Vllm.parse(&req, "dots.mocr").await.unwrap();
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.pages[0].blocks[7].block_type, BlockType::Figure);
        assert!(resp.pages[0].blocks[7].bbox.is_some());
        let seen = captured.lock().unwrap();
        assert_eq!(seen[0].route(), "POST /v1/chat/completions");
        assert_eq!(seen[0].header("authorization").as_deref(), Some("bearer secret-test-key"));
        let sent: Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(sent["model"], "model");
    }

    #[tokio::test]
    async fn server_errors_keep_the_message() {
        let err = r#"{"object":"error","message":"The model `x` does not exist.","type":"NotFoundError","code":404}"#;
        let (base, _) = crate::testutil::serve(vec![(404, err.into())]).await;
        let req = DocumentRequest::from_bytes(png(10, 10), "p.png").base_url(base);
        let e = Vllm.parse(&req, "infinity-parser2-flash").await.unwrap_err();
        assert!(e.message.contains("does not exist"), "{e}");
    }

    #[tokio::test]
    async fn rejects_other_file_types() {
        let req = DocumentRequest::from_bytes(&b"PK\x03\x04"[..], "a.docx");
        let e = Vllm.parse(&req, "dots.mocr").await.unwrap_err();
        assert_eq!(e.kind, crate::error::ErrorKind::Input);
    }

    /// Needs a running `vllm serve` for the model (see docs/providers/vllm.md). Run with:
    /// `VLLM_BASE_URL=http://localhost:8000 cargo test -p puffinparse-core vllm_live -- --ignored`
    #[tokio::test]
    #[ignore = "needs a vLLM server"]
    async fn vllm_live_parse() {
        let model = std::env::var("VLLM_LIVE_MODEL").unwrap_or_else(|_| "infinity-parser2-flash".into());
        let sample =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");
        let req = DocumentRequest::from_path(sample).timeout_secs(600.0);
        let resp = Vllm.parse(&req, &model).await.unwrap();
        assert!(!resp.pages.is_empty() && !resp.markdown.is_empty());
    }
}
