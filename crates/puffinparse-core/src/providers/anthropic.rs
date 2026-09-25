//! Anthropic (Claude) vision-LLM document parsing through the Messages API (`POST /v1/messages`).
//!
//! One HTTP call per document, no job queue. The document is inlined as base64 — a `document`
//! block for PDFs, an `image` block for images — followed by a `text` prompt. Structured output
//! comes from a **forced tool call**: a single tool whose `input_schema` is the wanted shape, with
//! `tool_choice: {"type": "tool", "name": …}`, and the answer read from the `tool_use` block's
//! `input`. `parse` uses a fixed `{"pages":[{"page_number":1,"markdown":"…"}]}` schema; `extract`
//! uses the caller's schema; `ocr` is the default trait derivation from `parse`.
//!
//! Document loading, prompts, the strict-schema rewrite, page building and token pricing are
//! shared with the other vision-LLM providers in [`super::vlm`].

use super::vlm::{self, Answer, Completion};
use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
#[cfg(test)]
use crate::types::{BlockType, OutputFormat, Page};
use crate::types::{DocumentRequest, ExtractRequest, ExtractResponse, Mode, ParseResponse};
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
        Ok(vlm::parse_response(NAME, PRICES, request, model, call))
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
        // Claude's citations feature grounds *text* answers in a document; it cannot be combined
        // with the forced tool call PuffinParse uses for schema output, so `fields` stays empty.
        Ok(vlm::extract_response(NAME, PRICES, request, model, call))
    }
}

/// Send the document + prompt and decode the forced tool call's input.
async fn message(
    request: &DocumentRequest,
    model: &str,
    prompt: &str,
    tool_name: &str,
    tool: Value,
) -> Result<Completion> {
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
    Ok(Completion::new(raw, data))
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

/// One tool whose `input_schema` is the structured output PuffinParse wants back.
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
    let (data, mime) = vlm::load_document(request, deadline, retry, NAME).await?;
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

// ---- thin wrappers over the shared vision-LLM helpers -------------------------------------------

/// Turn `{"pages":[{"page_number":1,"markdown":"…"}]}` into a [`ParseResponse`].
///
/// Vision LLMs return text, not geometry: each page gets exactly one `text` block and no bboxes.
#[cfg(test)]
pub(crate) fn normalize_parse(data: &Value, fmt: OutputFormat, model: &str) -> ParseResponse {
    vlm::normalize_parse(NAME, data, fmt, model)
}

#[cfg(test)]
fn pages_from_value(data: &Value, fmt: OutputFormat) -> Vec<Page> {
    vlm::pages_from_value(data, fmt)
}

fn base64_encode(input: &[u8]) -> String {
    vlm::base64_encode(input)
}

/// The tool input schema for `parse` (already strict-compatible).
fn pages_schema() -> Value {
    vlm::pages_schema()
}

fn parse_prompt(request: &DocumentRequest) -> String {
    vlm::parse_prompt(request, Answer::Tool)
}

fn extract_prompt(request: &ExtractRequest) -> String {
    vlm::extract_prompt(request, Answer::Tool)
}

/// Strict tool use (`strict: true` + a sanitised schema) is opt-in here: Claude accepts JSON Schema
/// keywords that strict mode rejects, so plain tool use is the safer default for caller schemas.
fn strict_enabled(request: &DocumentRequest) -> bool {
    vlm::strict_enabled(request, false)
}

fn passthrough_options(request: &DocumentRequest) -> Option<Value> {
    vlm::passthrough_options(request)
}

fn supports_temperature(model: &str) -> bool {
    TEMPERATURE_MODELS.contains(&model) || model.starts_with("claude-3")
}

fn supports_effort(model: &str) -> bool {
    EFFORT_MODELS.contains(&model)
}

/// Make a schema satisfy strict tool use (see [`vlm::sanitize_strict_schema`]).
pub(crate) fn sanitize_strict_schema(schema: &mut Value) {
    vlm::sanitize_strict_schema(schema)
}

/// Dollar cost of one call from the embedded per-token price table (see [`PRICES`]).
#[cfg(test)]
fn provider_cost(model: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
    vlm::token_cost(PRICES, model, input_tokens, output_tokens)
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
        assert!(strict_enabled(&req), "strict is a PuffinParse option");
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
