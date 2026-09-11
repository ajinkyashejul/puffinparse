//! Anthropic (Claude) vision-LLM document parsing through the Messages API (`POST /v1/messages`).
//!
//! One HTTP call per document, no job queue. The document is inlined as base64 — a `document`
//! block for PDFs, an `image` block for images — followed by a `text` prompt. Structured output
//! comes from a **forced tool call**: a single tool whose `input_schema` is the wanted shape, with
//! `tool_choice: {"type": "tool", "name": …}`, and the answer read from the `tool_use` block's
//! `input`. `parse` uses a fixed `{"pages":[{"page_number":1,"markdown":"…"}]}` schema; `extract`
//! uses the caller's schema; `ocr` is the default trait derivation from `parse`.
//!
//! NOTE: the private helper section at the bottom (base64, document loading, prompts, schema
//! sanitising, token pricing) is duplicated in `providers/openai.rs` on purpose while the
//! vision-LLM providers land in parallel; it is meant to be factored into a shared `vlm` module.

use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::types::{
    Block, BlockType, DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, Mode, OutputFormat, Page,
    ParseResponse, Usage,
};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const NAME: &str = "anthropic";
const ENV_KEY: &str = "ANTHROPIC_API_KEY";
const ENV_BASE: &str = "ANTHROPIC_BASE_URL";
const DEFAULT_BASE: &str = "https://api.anthropic.com";
/// The only API version the Messages API has ever had; it is mandatory on every request.
const API_VERSION: &str = "2023-06-01";

/// Ceiling for one response. A dense page transcribes to ~1–2k output tokens, so this covers a
/// ~20-page document; override with `provider_options.max_tokens` for longer ones.
const DEFAULT_MAX_TOKENS: u64 = 32_000;
/// Transcription is a perception task, not a reasoning one: keep thinking (billed as output
/// tokens) shallow unless the caller raises `provider_options.output_config.effort`.
const DEFAULT_EFFORT: &str = "low";
const PARSE_TOOL: &str = "emit_pages";
const EXTRACT_TOOL: &str = "record_extraction";
const SYSTEM_PROMPT: &str =
    "You are a precise document transcription and extraction engine. You answer only by calling the \
     provided tool, and you never invent content that is not in the document.";

/// Models that accept `output_config.effort` (Claude 4.6 and later). Older ones reject it.
const EFFORT_MODELS: &[&str] =
    &["claude-opus-5", "claude-opus-4-8", "claude-opus-4-7", "claude-opus-4-6", "claude-sonnet-5", "claude-sonnet-4-6"];
/// Models that still accept sampling parameters. Claude 4.6+ removed `temperature` (400 if sent).
const TEMPERATURE_MODELS: &[&str] = &["claude-haiku-4-5", "claude-sonnet-4-5", "claude-opus-4-5"];

/// List price in USD per **1M** tokens, `(model, input, output)`.
/// Source: <https://platform.claude.com/docs/en/about-claude/pricing> (standard tier, no caching).
pub const PRICES_UPDATED: &str = "2026-09-11";
const PRICES: &[(&str, f64, f64)] = &[
    ("claude-fable-5-1", 10.00, 50.00),
    ("claude-fable-5", 10.00, 50.00),
    ("claude-opus-5", 5.00, 25.00),
    ("claude-opus-4-8", 5.00, 25.00),
    ("claude-opus-4-7", 5.00, 25.00),
    ("claude-opus-4-6", 5.00, 25.00),
    ("claude-sonnet-5", 2.00, 10.00),
    ("claude-sonnet-4-6", 3.00, 15.00),
    ("claude-haiku-4-5", 1.00, 5.00),
];

#[derive(Debug, Default, Clone, Copy)]
pub struct Anthropic;

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let prompt = parse_prompt(request);
        let tool =
            tool_definition(PARSE_TOOL, "Return the transcription of every page of the document.", pages_schema());
        let call = message(request, model, &prompt, PARSE_TOOL, tool).await?;
        let mut resp = normalize_parse(&call.data, request.output, model);
        resp.usage.provider_cost_usd = provider_cost(model, call.input_tokens, call.output_tokens);
        resp.provider_job_id = Some(call.id);
        resp.metadata.insert("anthropic_input_tokens".into(), json!(call.input_tokens));
        resp.metadata.insert("anthropic_output_tokens".into(), json!(call.output_tokens));
        if request.include_raw {
            resp.raw = Some(call.raw);
        }
        Ok(resp)
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let doc = &request.document;
        let prompt = extract_prompt(request);
        let mut schema = request.schema.clone();
        if strict_enabled(doc) {
            sanitize_strict_schema(&mut schema);
        }
        let mut tool = tool_definition(EXTRACT_TOOL, "Record the values extracted from the document.", schema);
        if strict_enabled(doc) {
            tool["strict"] = json!(true);
        }
        let call = message(doc, model, &prompt, EXTRACT_TOOL, tool).await?;
        // The provider reports tokens, never pages: `usage.pages` stays 0 in extract mode and the
        // cost comes from the token counts.
        let usage = Usage {
            pages: 0,
            credits: None,
            provider_cost_usd: provider_cost(model, call.input_tokens, call.output_tokens),
        };
        let mut resp = ExtractResponse::new(NAME, &format!("{NAME}/{model}"), call.data, usage);
        resp.provider_job_id = Some(call.id);
        resp.metadata.insert("anthropic_input_tokens".into(), json!(call.input_tokens));
        resp.metadata.insert("anthropic_output_tokens".into(), json!(call.output_tokens));
        if request.citations {
            // Claude's citations feature grounds *text* answers in a document; it cannot be combined
            // with the forced tool call LiteOCR uses for schema output, so `fields` stays empty.
            resp.metadata.insert("anthropic_citations_unsupported".into(), json!(true));
        }
        if doc.include_raw {
            resp.raw = Some(call.raw);
        }
        Ok(resp)
    }
}

/// What one `POST /v1/messages` round-trip yields.
#[derive(Debug)]
struct Call {
    id: String,
    data: Value,
    input_tokens: u64,
    output_tokens: u64,
    raw: Value,
}

/// Send the document + prompt and decode the forced tool call's input.
async fn message(request: &DocumentRequest, model: &str, prompt: &str, tool_name: &str, tool: Value) -> Result<Call> {
    let api_key = provider::resolve_api_key(request, ENV_KEY, NAME)?;
    let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
    let deadline = Deadline::new(request.timeout_secs);
    let retry = Retry::new(request.max_retries);
    let client = http::client();

    let document = document_block(request, &deadline, retry).await?;
    let body = build_body(request, model, document, prompt, tool_name, tool)?;

    let raw: Value = http::with_retry(NAME, retry, &deadline, || {
        let rb = client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", &api_key)
            .header("anthropic-version", API_VERSION)
            .timeout(deadline.request_timeout())
            .json(&body);
        async move { send_json(rb).await }
    })
    .await?;

    let id = raw.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let data = tool_input(&raw, tool_name).map_err(|e| if id.is_empty() { e } else { e.with_job_id(id.clone()) })?;
    let usage = raw.get("usage");
    Ok(Call {
        id,
        data,
        input_tokens: usage.and_then(|u| u.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0),
        output_tokens: usage.and_then(|u| u.get("output_tokens")).and_then(Value::as_u64).unwrap_or(0),
        raw,
    })
}

/// Send one request, decoding JSON on success and mapping the Anthropic error envelope otherwise.
async fn send_json(rb: reqwest::RequestBuilder) -> Result<Value> {
    let resp = rb.send().await?;
    let status = resp.status().as_u16();
    let body = resp.text().await?;
    if (200..300).contains(&status) {
        serde_json::from_str(&body).map_err(|e| {
            Error::provider(format!("unexpected response shape: {e}; body starts: {}", http::snippet(&body)))
                .with_provider(NAME)
        })
    } else {
        Err(map_http_error(status, &body))
    }
}

/// `{"type":"error","error":{"type":…,"message":…}}` → [`Error`]. `Error::from_http` already digs
/// the nested `error.message` out; the only special case is 529.
fn map_http_error(status: u16, body: &str) -> Error {
    // 529 `overloaded_error` is a transient capacity signal, but the shared retry policy only knows
    // 500/502/503/504 — report it as 503 so it is retried, and keep the real status in the message.
    let effective = if status == 529 { 503 } else { status };
    let mut error = Error::from_http(NAME, effective, body);
    if status == 529 {
        error.message = format!("overloaded (HTTP 529): {}", error.message);
    }
    error
}

/// The request body for `POST /v1/messages`.
fn build_body(
    request: &DocumentRequest,
    model: &str,
    document: Value,
    prompt: &str,
    tool_name: &str,
    tool: Value,
) -> Result<Value> {
    let mut body = json!({
        "model": model,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        // Documents before text: Claude does better when the page comes first.
        "messages": [{"role": "user", "content": [document, {"type": "text", "text": prompt}]}],
        "tools": [tool],
        "tool_choice": {"type": "tool", "name": tool_name},
    });
    if supports_temperature(model) {
        body["temperature"] = json!(0);
    }
    if supports_effort(model) {
        body["output_config"] = json!({"effort": DEFAULT_EFFORT});
    }
    if let Some(opts) = passthrough_options(request) {
        if !opts.is_object() {
            return Err(Error::input("anthropic: provider_options must be a JSON object").with_provider(NAME));
        }
        crate::util::deep_merge(&mut body, &opts);
    }
    Ok(body)
}

/// One tool whose `input_schema` is the structured output LiteOCR wants back.
fn tool_definition(name: &str, description: &str, schema: Value) -> Value {
    json!({"name": name, "description": description, "input_schema": schema})
}

/// Pull the forced tool call's `input` out of `content[]`, or explain why there is none.
fn tool_input(resp: &Value, tool_name: &str) -> Result<Value> {
    if let Some(blocks) = resp.get("content").and_then(Value::as_array) {
        for block in blocks {
            if block.get("type").and_then(Value::as_str) == Some("tool_use")
                && block.get("name").and_then(Value::as_str) == Some(tool_name)
            {
                return Ok(block.get("input").cloned().unwrap_or(Value::Null));
            }
        }
    }
    match resp.get("stop_reason").and_then(Value::as_str) {
        Some("max_tokens") => Err(Error::provider(
            "response truncated (stop_reason=max_tokens); raise provider_options.max_tokens or split the document",
        )
        .with_provider(NAME)),
        Some("refusal") => {
            let category = resp.pointer("/stop_details/category").and_then(Value::as_str).unwrap_or("unspecified");
            Err(Error::provider(format!("model refused the request (category: {category})")).with_provider(NAME))
        }
        _ => {
            // Without a tool call the model answered in prose; surface the beginning of it.
            let text = resp
                .get("content")
                .and_then(Value::as_array)
                .and_then(|b| b.iter().find(|x| x.get("type").and_then(Value::as_str) == Some("text")))
                .and_then(|b| b.get("text").and_then(Value::as_str))
                .unwrap_or_default();
            Err(Error::provider(format!(
                "no '{tool_name}' tool_use block in the response{}",
                if text.is_empty() { String::new() } else { format!("; model said: {}", http::snippet(text)) }
            ))
            .with_provider(NAME))
        }
    }
}

/// The document content block: `document` for PDFs, `image` for images.
async fn document_block(request: &DocumentRequest, deadline: &Deadline, retry: Retry) -> Result<Value> {
    let (data, mime) = load_document(request, deadline, retry).await?;
    let encoded = base64_encode(&data);
    if mime == "application/pdf" {
        Ok(json!({
            "type": "document",
            "source": {"type": "base64", "media_type": "application/pdf", "data": encoded},
        }))
    } else if matches!(mime.as_str(), "image/jpeg" | "image/png" | "image/gif" | "image/webp") {
        Ok(json!({"type": "image", "source": {"type": "base64", "media_type": mime, "data": encoded}}))
    } else {
        Err(Error::input(format!(
            "anthropic: unsupported document type '{mime}' — the Messages API takes PDFs and \
             JPEG/PNG/GIF/WebP images; convert other formats to PDF first"
        ))
        .with_provider(NAME))
    }
}

// ---- normalisation -----------------------------------------------------------------------------

/// Turn `{"pages":[{"page_number":1,"markdown":"…"}]}` into a [`ParseResponse`].
///
/// Vision LLMs return text, not geometry: each page gets exactly one `text` block and no bboxes.
pub(crate) fn normalize_parse(data: &Value, fmt: OutputFormat, model: &str) -> ParseResponse {
    let pages = pages_from_value(data, fmt);
    let usage = Usage { pages: pages.len() as u32, credits: None, provider_cost_usd: None };
    ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage)
}

// ---- private helpers (duplicated with providers/openai.rs; see module docs) ---------------------

/// Base64 (standard alphabet, padded). Small and dependency-free — the crate vendors no base64.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Read the document bytes and decide its MIME type. URLs are downloaded and inlined — neither
/// vision API fetches a URL that needs the caller's credentials.
async fn load_document(request: &DocumentRequest, deadline: &Deadline, retry: Retry) -> Result<(bytes::Bytes, String)> {
    if let Some(data) = provider::load_bytes(&request.input).await? {
        return Ok((data, request.input.mime_type()));
    }
    let DocumentInput::Url { url } = &request.input else {
        unreachable!("load_bytes only returns None for URL inputs");
    };
    let url = url.clone();
    let client = http::client();
    let (data, content_type) = http::with_retry(NAME, retry, deadline, || {
        let rb = client.get(&url).timeout(deadline.request_timeout());
        let url = url.clone();
        async move {
            let resp = rb.send().await?;
            let status = resp.status();
            if !status.is_success() {
                return Err(Error::input(format!("cannot download {url}: HTTP {}", status.as_u16())));
            }
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase());
            Ok((resp.bytes().await?, content_type))
        }
    })
    .await?;
    if data.is_empty() {
        return Err(Error::input(format!("{url} returned an empty body")).with_provider(NAME));
    }
    // The URL's own extension is more reliable than a generic `application/octet-stream`.
    let mime = match content_type {
        Some(ct) if ct != "application/octet-stream" && ct != "binary/octet-stream" => ct,
        _ => request.input.mime_type(),
    };
    Ok((data, mime))
}

/// The tool input schema for `parse` (already strict-compatible).
fn pages_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pages": {
                "type": "array",
                "description": "Every page of the document, in order.",
                "items": {
                    "type": "object",
                    "properties": {
                        "page_number": {"type": "integer", "description": "1-based page number in the document."},
                        "markdown": {"type": "string", "description": "The full content of the page."},
                    },
                    "required": ["page_number", "markdown"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["pages"],
        "additionalProperties": false,
    })
}

fn parse_prompt(request: &DocumentRequest) -> String {
    let mut p = String::from(
        "Transcribe this document. Return one entry in `pages` for every page, in order.\n\n\
         Rules:\n\
         - Use the document's own 1-based page numbers.\n\
         - `markdown` holds the complete content of that page as GitHub-Flavored Markdown: `#` headings, \
         `-` lists, GFM pipe tables, fenced code, LaTeX between `$` for formulas.\n\
         - Transcribe verbatim: same wording, numbers, casing and reading order (top to bottom; \
         column by column on multi-column pages). Never summarise, translate, explain or invent content.\n\
         - Describe a figure, photo or chart as one short italic line, e.g. \
         `*Figure: bar chart of quarterly revenue.*`\n\
         - Keep running headers, footers and printed page numbers where they appear.\n\
         - A blank page gets an empty `markdown` string.",
    );
    if request.output == OutputFormat::Text {
        p.push_str("\n- Exception: emit plain text only — no Markdown syntax, no table pipes, no `#`.");
    }
    if let Some(pages) = &request.pages {
        p.push_str(&format!(
            "\n- Transcribe only pages {pages} (1-based, inclusive ranges). Skip every other page and keep \
             the original page numbers for the ones you return."
        ));
    }
    if let Some(language) = &request.language {
        p.push_str(&format!("\n- The document is mainly in {language}; transcribe it in that language."));
    }
    p
}

fn extract_prompt(request: &ExtractRequest) -> String {
    let mut p = String::from(
        "Extract the requested fields from this document and record them with the tool.\n\n\
         Rules:\n\
         - Use only values that appear in the document; never guess, infer or fabricate.\n\
         - Use `null` for a value the document does not contain.\n\
         - Numbers must be plain numbers (no thousands separators, no currency symbols); keep dates as \
         written unless the schema says otherwise.",
    );
    if let Some(pages) = &request.document.pages {
        p.push_str(&format!("\n- Only look at pages {pages} (1-based, inclusive ranges)."));
    }
    if let Some(language) = &request.document.language {
        p.push_str(&format!("\n- The document is mainly in {language}."));
    }
    if let Some(instructions) = &request.instructions {
        p.push_str("\n\nAdditional instructions from the caller:\n");
        p.push_str(instructions);
    }
    p
}

/// Build pages (one text block each, no geometry) from the model's JSON answer.
fn pages_from_value(data: &Value, fmt: OutputFormat) -> Vec<Page> {
    let entries = data.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let page_number = entry
                .get("page_number")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n > 0)
                .unwrap_or(i as u32 + 1);
            let content = entry.get("markdown").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let text = match fmt {
                OutputFormat::Markdown => crate::types::markdown_to_text(&content),
                OutputFormat::Text => content.clone(),
            };
            let block = Block {
                block_type: BlockType::Text,
                content: content.clone(),
                text: Some(text.clone()),
                bbox: None,
                confidence: None,
                page_number,
            };
            Page { page_number, width: None, height: None, markdown: content, text, blocks: vec![block] }
        })
        .collect()
}

/// Keys of `provider_options` LiteOCR consumes instead of forwarding to the provider.
const PRIVATE_OPTIONS: &[&str] = &["strict"];

/// Strict tool use (`strict: true` + a sanitised schema) is opt-in here: Claude accepts JSON Schema
/// keywords that strict mode rejects, so plain tool use is the safer default for caller schemas.
fn strict_enabled(request: &DocumentRequest) -> bool {
    request.option("strict").and_then(Value::as_bool).unwrap_or(false)
}

fn passthrough_options(request: &DocumentRequest) -> Option<Value> {
    let mut opts = request.provider_options.clone()?;
    if let Value::Object(map) = &mut opts {
        for key in PRIVATE_OPTIONS {
            map.remove(*key);
        }
        if map.is_empty() {
            return None;
        }
    }
    Some(opts)
}

fn supports_temperature(model: &str) -> bool {
    TEMPERATURE_MODELS.contains(&model) || model.starts_with("claude-3")
}

fn supports_effort(model: &str) -> bool {
    EFFORT_MODELS.contains(&model)
}

/// Make a schema satisfy strict tool use: every object must set `additionalProperties: false` and
/// list **all** of its properties in `required`.
pub(crate) fn sanitize_strict_schema(schema: &mut Value) {
    const SCHEMA_MAPS: &[&str] = &["properties", "$defs", "definitions", "patternProperties"];
    const SCHEMA_NODES: &[&str] =
        &["items", "prefixItems", "anyOf", "oneOf", "allOf", "not", "if", "then", "else", "contains"];

    let Value::Object(map) = schema else { return };
    for key in SCHEMA_MAPS {
        if let Some(Value::Object(children)) = map.get_mut(*key) {
            for child in children.values_mut() {
                sanitize_strict_schema(child);
            }
        }
    }
    for key in SCHEMA_NODES {
        match map.get_mut(*key) {
            Some(Value::Array(items)) => items.iter_mut().for_each(sanitize_strict_schema),
            Some(child) if child.is_object() => sanitize_strict_schema(child),
            _ => {}
        }
    }
    let is_object = match map.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
        _ => map.contains_key("properties"),
    };
    if is_object {
        map.insert("additionalProperties".to_string(), json!(false));
        let required: Vec<Value> = map
            .get("properties")
            .and_then(Value::as_object)
            .map(|p| p.keys().map(|k| Value::String(k.clone())).collect())
            .unwrap_or_default();
        map.insert("required".to_string(), Value::Array(required));
    }
}

/// Dollar cost of one call from the embedded per-token price table (see [`PRICES`]).
fn provider_cost(model: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
    let (_, input, output) = PRICES.iter().find(|(m, _, _)| *m == model)?;
    Some((input_tokens as f64) * input / 1e6 + (output_tokens as f64) * output / 1e6)
}

/// Per-page price estimate used to seed `pricing.json` (~1,500 input + ~700 output tokens/page).
#[cfg(test)]
fn per_page_estimate(model: &str) -> Option<f64> {
    provider_cost(model, 1_500, 700)
}

/// Modes this provider serves; mirrors the registry entry in `model.rs`.
pub const MODES: &[Mode] = &[Mode::Parse, Mode::Ocr, Mode::Extract];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    const PARSE_FIXTURE: &str = include_str!("../../tests/fixtures/anthropic_messages_parse.json");
    const EXTRACT_FIXTURE: &str = include_str!("../../tests/fixtures/anthropic_messages_extract.json");

    fn pdf_request() -> DocumentRequest {
        DocumentRequest::from_bytes(bytes::Bytes::from_static(b"%PDF-1.7 fake"), "invoice.pdf")
            .model("anthropic/claude-sonnet-5")
    }

    #[test]
    fn normalizes_parse_fixture() {
        let raw: Value = serde_json::from_str(PARSE_FIXTURE).unwrap();
        let data = tool_input(&raw, PARSE_TOOL).unwrap();
        let mut resp = normalize_parse(&data, OutputFormat::Markdown, "claude-sonnet-5");
        resp.usage.provider_cost_usd = provider_cost("claude-sonnet-5", 3210, 389);

        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages[0].page_number, 1);
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"));
        assert_eq!(resp.pages[0].blocks.len(), 1, "one text block per page");
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Text);
        assert!(resp.pages[0].blocks[0].bbox.is_none(), "vision LLMs report no geometry");
        assert!(resp.pages[1].markdown.contains("| Widget | 2 | $10.00 |"));
        assert!(resp.text.contains("Widget"));
        // 3210 * $2/1M + 389 * $10/1M
        let cost = resp.usage.provider_cost_usd.unwrap();
        assert!((cost - 0.010310).abs() < 1e-9, "{cost}");
        assert_eq!(raw["id"], "msg_01XhT9Eq8bQ7vPz2kKcM4dLp");
        assert_eq!(raw["usage"]["input_tokens"], 3210);
    }

    #[test]
    fn extract_fixture_yields_tool_input() {
        let raw: Value = serde_json::from_str(EXTRACT_FIXTURE).unwrap();
        let data = tool_input(&raw, EXTRACT_TOOL).unwrap();
        assert_eq!(data["invoice_number"], "INV-1234");
        assert_eq!(data["total"], 56.78);
        assert_eq!(data["line_items"][0]["description"], "Widget");
        assert!(data["due_date"].is_null(), "absent values come back null");
    }

    #[test]
    fn missing_tool_call_becomes_a_provider_error() {
        let truncated = json!({"content": [], "stop_reason": "max_tokens"});
        let e = tool_input(&truncated, PARSE_TOOL).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Provider);
        assert!(e.message.contains("max_tokens"), "{e}");

        let refused = json!({"content": [], "stop_reason": "refusal", "stop_details": {"category": "cyber"}});
        assert!(tool_input(&refused, PARSE_TOOL).unwrap_err().message.contains("cyber"));

        let prose = json!({
            "content": [{"type": "text", "text": "I cannot read this scan."}],
            "stop_reason": "end_turn",
        });
        let e = tool_input(&prose, PARSE_TOOL).unwrap_err();
        assert!(e.message.contains("I cannot read this scan."), "{e}");

        let wrong_tool = json!({
            "content": [{"type": "tool_use", "name": "other", "input": {}}],
            "stop_reason": "tool_use",
        });
        assert!(tool_input(&wrong_tool, PARSE_TOOL).is_err());
    }

    #[test]
    fn skips_thinking_blocks_before_the_tool_call() {
        let resp = json!({
            "content": [
                {"type": "thinking", "thinking": "", "signature": "abc"},
                {"type": "tool_use", "id": "toolu_1", "name": PARSE_TOOL, "input": {"pages": []}},
            ],
            "stop_reason": "tool_use",
        });
        assert_eq!(tool_input(&resp, PARSE_TOOL).unwrap(), json!({"pages": []}));
    }

    #[test]
    fn builds_messages_body() {
        let req = pdf_request();
        let doc =
            json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "x"}});
        let tool = tool_definition(PARSE_TOOL, "desc", pages_schema());
        let body = build_body(&req, "claude-sonnet-5", doc, "prompt", PARSE_TOOL, tool).unwrap();

        assert_eq!(body["model"], "claude-sonnet-5");
        assert_eq!(body["max_tokens"], 32_000);
        assert_eq!(body["messages"][0]["content"][0]["type"], "document", "document goes before the text");
        assert_eq!(body["messages"][0]["content"][1], json!({"type": "text", "text": "prompt"}));
        assert_eq!(body["tools"][0]["name"], PARSE_TOOL);
        assert_eq!(body["tools"][0]["input_schema"]["required"], json!(["pages"]));
        assert_eq!(body["tool_choice"], json!({"type": "tool", "name": PARSE_TOOL}));
        assert_eq!(body["output_config"]["effort"], "low");
        // Claude 4.6+ rejects sampling parameters.
        assert!(body.get("temperature").is_none(), "{body}");
        assert!(body.get("thinking").is_none(), "adaptive thinking is compatible with forced tool use");
    }

    #[test]
    fn model_capability_flags() {
        let req = pdf_request();
        let body = build_body(&req, "claude-haiku-4-5", json!({}), "p", "t", json!({})).unwrap();
        assert_eq!(body["temperature"], 0);
        assert!(body.get("output_config").is_none(), "haiku 4.5 rejects output_config.effort");
        assert!(supports_effort("claude-opus-5") && !supports_effort("claude-haiku-4-5"));
        assert!(supports_temperature("claude-haiku-4-5") && !supports_temperature("claude-sonnet-5"));
    }

    #[test]
    fn provider_options_merge_and_private_keys_are_not_forwarded() {
        let req = pdf_request().provider_options(json!({
            "strict": true,
            "max_tokens": 8000,
            "output_config": {"effort": "high"},
        }));
        let body = build_body(&req, "claude-sonnet-5", json!({}), "p", "t", json!({})).unwrap();
        assert_eq!(body["max_tokens"], 8000);
        assert_eq!(body["output_config"]["effort"], "high");
        assert!(body.get("strict").is_none(), "private options are stripped from the body");
        assert!(strict_enabled(&req), "strict is a LiteOCR option");
        assert!(!strict_enabled(&pdf_request()), "strict tool use is opt-in for anthropic");
    }

    #[test]
    fn rejects_non_object_provider_options() {
        let req = pdf_request().provider_options(json!("nope"));
        let e = build_body(&req, "claude-sonnet-5", json!({}), "p", "t", json!({})).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Input);
    }

    #[test]
    fn maps_the_anthropic_error_envelope() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: 99999999 > 64000"},"request_id":"req_011CS"}"#;
        let e = map_http_error(400, body);
        assert_eq!(e.kind, ErrorKind::BadRequest);
        assert_eq!(e.message, "max_tokens: 99999999 > 64000");
        assert_eq!(map_http_error(401, body).kind, ErrorKind::Authentication);
        assert_eq!(map_http_error(429, body).kind, ErrorKind::RateLimit);
        assert_eq!(map_http_error(413, body).kind, ErrorKind::BadRequest);
    }

    #[test]
    fn overloaded_529_is_retryable_like_503() {
        let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let e = map_http_error(529, body);
        assert_eq!(e.kind, ErrorKind::Provider);
        assert_eq!(e.status_code, Some(503), "reported as 503 so the shared retry policy retries it");
        assert!(e.message.contains("HTTP 529"), "{e}");
        assert!(e.retryable);
        assert_eq!(map_http_error(500, body).status_code, Some(500));
    }

    #[test]
    fn sanitises_nested_schemas_for_strict_tool_use() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "invoice_number": {"type": "string"},
                "line_items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {"description": {"type": "string"}, "qty": {"type": "integer"}},
                        "required": ["description"],
                    },
                },
            },
            "required": ["invoice_number"],
            "$defs": {"party": {"type": "object", "properties": {"name": {"type": "string"}}}},
        });
        sanitize_strict_schema(&mut schema);
        assert_eq!(schema["additionalProperties"], false);
        // Key order depends on serde_json's `preserve_order` feature, so compare as a set.
        let mut required: Vec<&str> =
            schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        required.sort();
        assert_eq!(required, ["invoice_number", "line_items"]);
        let items = &schema["properties"]["line_items"]["items"]["required"];
        let mut item_required: Vec<&str> = items.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        item_required.sort();
        assert_eq!(item_required, ["description", "qty"]);
        assert_eq!(schema["$defs"]["party"]["additionalProperties"], false);
    }

    #[test]
    fn prompt_carries_pages_language_and_format() {
        let req = DocumentRequest::from_path("a.pdf").pages("2-4").language("fr").output(OutputFormat::Text);
        let p = parse_prompt(&req);
        assert!(p.contains("only pages 2-4"), "{p}");
        assert!(p.contains("mainly in fr"), "{p}");
        assert!(p.contains("plain text only"), "{p}");
        let e = ExtractRequest::new(DocumentRequest::from_path("a.pdf"), json!({})).instructions("VAT is 19%.");
        assert!(extract_prompt(&e).contains("VAT is 19%."));
    }

    #[test]
    fn pages_fall_back_to_ordinal_numbers() {
        let data = json!({"pages": [{"markdown": "# A"}, {"page_number": 2, "markdown": "**B**"}]});
        let pages = pages_from_value(&data, OutputFormat::Markdown);
        assert_eq!(pages[0].page_number, 1);
        assert_eq!(pages[1].text, "B");
        assert!(pages_from_value(&json!({}), OutputFormat::Markdown).is_empty());
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn document_block_picks_the_right_block() {
        let dl = Deadline::new(5.0);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

        let block = rt.block_on(document_block(&pdf_request(), &dl, Retry::new(0))).unwrap();
        assert_eq!(block["type"], "document");
        assert_eq!(block["source"]["media_type"], "application/pdf");
        assert_eq!(block["source"]["type"], "base64");
        assert!(!block["source"]["data"].as_str().unwrap().starts_with("data:"), "raw base64, no data: URL");

        let png = DocumentRequest::from_bytes(bytes::Bytes::from_static(b"\x89PNG"), "scan.png");
        let block = rt.block_on(document_block(&png, &dl, Retry::new(0))).unwrap();
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["media_type"], "image/png");

        let tiff = DocumentRequest::from_bytes(bytes::Bytes::from_static(b"II*"), "scan.tiff");
        let e = rt.block_on(document_block(&tiff, &dl, Retry::new(0))).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Input);
    }

    #[test]
    fn per_page_estimates_match_pricing_json() {
        // 1,500 input + 700 output tokens per page — the method noted in docs/providers/anthropic.md.
        let round = |x: f64| (x * 1e6).round() / 1e6;
        assert_eq!(round(per_page_estimate("claude-haiku-4-5").unwrap()), 0.005);
        assert_eq!(round(per_page_estimate("claude-sonnet-5").unwrap()), 0.01);
        assert_eq!(round(per_page_estimate("claude-opus-5").unwrap()), 0.025);
        assert!(per_page_estimate("claude-unknown").is_none());
        assert_eq!(PRICES_UPDATED, "2026-09-11");
    }

    const SAMPLE: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");

    #[tokio::test]
    #[ignore = "needs ANTHROPIC_API_KEY and network"]
    async fn anthropic_parse_live() {
        if std::env::var("ANTHROPIC_API_KEY").map(|v| v.is_empty()).unwrap_or(true) {
            eprintln!("skipping: ANTHROPIC_API_KEY not set");
            return;
        }
        let resp =
            crate::parse(DocumentRequest::from_path(SAMPLE).model("anthropic/claude-haiku-4-5").timeout_secs(240.0))
                .await
                .expect("parse succeeds");
        assert_eq!(resp.usage.pages, 2);
        assert!(!resp.markdown.trim().is_empty());
        assert!(resp.cost_usd.unwrap_or(0.0) > 0.0);
    }

    #[tokio::test]
    #[ignore = "needs ANTHROPIC_API_KEY and network"]
    async fn anthropic_extract_live() {
        if std::env::var("ANTHROPIC_API_KEY").map(|v| v.is_empty()).unwrap_or(true) {
            eprintln!("skipping: ANTHROPIC_API_KEY not set");
            return;
        }
        let schema = json!({"type": "object", "properties": {"title": {"type": ["string", "null"]}}});
        let req = ExtractRequest::new(
            DocumentRequest::from_path(SAMPLE).model("anthropic/claude-haiku-4-5").timeout_secs(240.0),
            schema,
        );
        let resp = crate::extract(req).await.expect("extract succeeds");
        assert!(resp.data.is_object());
        assert!(resp.cost_usd.unwrap_or(0.0) > 0.0);
    }
}
