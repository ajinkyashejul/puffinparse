//! OpenAI vision-LLM document parsing through the Responses API (`POST /v1/responses`).
//!
//! There is no job queue: one HTTP call carries the document and returns the whole result.
//! The document is inlined as a `data:` URL — `input_file` (with `filename`) for PDFs,
//! `input_image` for images — followed by an `input_text` prompt. The answer is constrained with
//! structured outputs (`text.format = {type: "json_schema", strict: true}`): a fixed
//! `{"pages":[{"page_number":1,"markdown":"…"}]}` schema for `parse`, the caller's schema for
//! `extract`. `ocr` is the default trait derivation from `parse`.
//!
//! Document loading, prompts, the strict-schema rewrite, page building and token pricing are
//! shared with the other vision-LLM providers in [`super::vlm`].

use super::vlm::{self, Answer, Completion};
use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
#[cfg(test)]
use crate::types::OutputFormat;
#[cfg(test)]
use crate::types::{BlockType, Page};
use crate::types::{DocumentRequest, ExtractRequest, ExtractResponse, Mode, ParseResponse};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const NAME: &str = "openai";
const ENV_KEY: &str = "OPENAI_API_KEY";
const ENV_BASE: &str = "OPENAI_BASE_URL";
const DEFAULT_BASE: &str = "https://api.openai.com";

/// Ceiling for one response. A dense page transcribes to ~1–2k output tokens, so this covers a
/// ~20-page document; override with `provider_options.max_output_tokens` for longer ones.
const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 32_000;
/// Transcription is a perception task, not a reasoning one: keep reasoning tokens (billed as
/// output) low unless the caller asks for more via `provider_options.reasoning`.
const DEFAULT_REASONING_EFFORT: &str = "low";
/// Name given to the structured-output schema in `text.format.name`.
const PARSE_SCHEMA_NAME: &str = "puffinparse_pages";
const EXTRACT_SCHEMA_NAME: &str = "puffinparse_extraction";

/// List price in USD per **1M** tokens, `(model, input, output)`.
/// Source: <https://developers.openai.com/api/docs/pricing> (standard tier, short context).
pub const PRICES_UPDATED: &str = "2026-09-11";
const PRICES: &[(&str, f64, f64)] = &[
    ("gpt-6-astra", 10.00, 50.00),
    ("gpt-5.6-astra", 10.00, 50.00),
    ("gpt-5.6-sol", 4.00, 20.00),
    ("gpt-5.6", 4.00, 20.00),
    ("gpt-5.6-terra", 2.00, 12.00),
    ("gpt-5.6-luna", 0.20, 1.20),
    ("gpt-5.5", 5.00, 30.00),
    ("gpt-5.4", 2.50, 15.00),
    ("gpt-5.4-mini", 0.75, 4.50),
    ("gpt-5.4-nano", 0.20, 1.25),
    ("gpt-5.2", 1.75, 14.00),
    ("gpt-5.1", 1.25, 10.00),
    ("gpt-5", 1.25, 10.00),
    ("gpt-5-mini", 0.25, 2.00),
    ("gpt-5-nano", 0.05, 0.40),
    ("gpt-4.1", 2.00, 8.00),
    ("gpt-4.1-mini", 0.40, 1.60),
    ("gpt-4.1-nano", 0.10, 0.40),
    ("gpt-4o", 2.50, 10.00),
    ("gpt-4o-mini", 0.15, 0.60),
];

#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAi;

#[async_trait]
impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let prompt = parse_prompt(request);
        let call = respond(request, model, &prompt, PARSE_SCHEMA_NAME, pages_schema()).await?;
        Ok(vlm::parse_response(NAME, PRICES, request, model, call))
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let prompt = extract_prompt(request);
        // `build_body` sanitises the schema when strict structured outputs are on (the default).
        let call = respond(&request.document, model, &prompt, EXTRACT_SCHEMA_NAME, request.schema.clone()).await?;
        // The Responses API has no per-field grounding, so `fields` stays empty.
        Ok(vlm::extract_response(NAME, PRICES, request, model, call))
    }
}

/// Send the document + prompt and decode the structured JSON answer.
async fn respond(
    request: &DocumentRequest,
    model: &str,
    prompt: &str,
    schema_name: &str,
    schema: Value,
) -> Result<Completion> {
    let api_key = provider::resolve_api_key(request, ENV_KEY, NAME)?;
    let base = provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE);
    let deadline = Deadline::new(request.timeout_secs);
    let retry = Retry::new(request.max_retries);
    let client = http::client();

    let document = document_part(request, &deadline, retry).await?;
    let body = build_body(request, model, document, prompt, schema_name, schema)?;

    let raw: Value = http::with_retry(NAME, retry, &deadline, || {
        let rb = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth(&api_key)
            .timeout(deadline.request_timeout())
            .json(&body);
        async move { http::read_json(NAME, rb.send().await?).await }
    })
    .await?;

    let id = raw.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let text = output_text(&raw).map_err(|e| if id.is_empty() { e } else { e.with_job_id(id.clone()) })?;
    let data: Value = serde_json::from_str(&text).map_err(|e| {
        Error::provider(format!("model did not return valid JSON ({e}); output starts: {}", http::snippet(&text)))
            .with_provider(NAME)
    })?;
    Ok(Completion::new(raw, data))
}

/// The request body for `POST /v1/responses`.
fn build_body(
    request: &DocumentRequest,
    model: &str,
    document: Value,
    prompt: &str,
    schema_name: &str,
    schema: Value,
) -> Result<Value> {
    let strict = strict_enabled(request);
    let mut schema = schema;
    if strict {
        sanitize_strict_schema(&mut schema);
    }
    let mut body = json!({
        "model": model,
        "input": [{
            "role": "user",
            "content": [document, {"type": "input_text", "text": prompt}],
        }],
        "text": {"format": {"type": "json_schema", "name": schema_name, "schema": schema, "strict": strict}},
        "max_output_tokens": DEFAULT_MAX_OUTPUT_TOKENS,
        // Do not let the provider retain the document by default; override with provider_options.
        "store": false,
    });
    if supports_temperature(model) {
        body["temperature"] = json!(0);
    } else {
        // Reasoning models (gpt-5.x / gpt-6) reject `temperature`; steer them with effort instead.
        body["reasoning"] = json!({"effort": DEFAULT_REASONING_EFFORT});
    }
    if let Some(opts) = passthrough_options(request) {
        if !opts.is_object() {
            return Err(Error::input("openai: provider_options must be a JSON object").with_provider(NAME));
        }
        crate::util::deep_merge(&mut body, &opts);
    }
    Ok(body)
}

/// Concatenate the assistant's `output_text` parts, or explain why there are none.
fn output_text(resp: &Value) -> Result<String> {
    match resp.get("status").and_then(Value::as_str) {
        Some("incomplete") => {
            let reason = resp.pointer("/incomplete_details/reason").and_then(Value::as_str).unwrap_or("unknown reason");
            return Err(Error::provider(format!(
                "response incomplete ({reason}); raise provider_options.max_output_tokens or split the document"
            ))
            .with_provider(NAME));
        }
        Some("failed") => {
            let msg = resp.pointer("/error/message").and_then(Value::as_str).unwrap_or("response failed");
            return Err(Error::provider(msg.to_string()).with_provider(NAME));
        }
        _ => {}
    }
    let mut out = String::new();
    if let Some(items) = resp.get("output").and_then(Value::as_array) {
        for item in items {
            let Some(content) = item.get("content").and_then(Value::as_array) else { continue };
            for part in content {
                match part.get("type").and_then(Value::as_str) {
                    Some("output_text") => out.push_str(part.get("text").and_then(Value::as_str).unwrap_or_default()),
                    Some("refusal") => {
                        let msg = part.get("refusal").and_then(Value::as_str).unwrap_or("model refused the request");
                        return Err(Error::provider(format!("model refused: {msg}")).with_provider(NAME));
                    }
                    _ => {}
                }
            }
        }
    }
    if out.trim().is_empty() {
        // `output_text` is the SDK convenience aggregate; accept it if a gateway supplies it.
        if let Some(s) = resp.get("output_text").and_then(Value::as_str) {
            out = s.to_string();
        }
    }
    if out.trim().is_empty() {
        return Err(Error::provider("response contained no output text").with_provider(NAME));
    }
    Ok(out)
}

/// The document content block: `input_file` for PDFs, `input_image` for images.
async fn document_part(request: &DocumentRequest, deadline: &Deadline, retry: Retry) -> Result<Value> {
    let (data, mime) = vlm::load_document(request, deadline, retry, NAME).await?;
    let url = data_url(&mime, &data);
    if mime == "application/pdf" {
        Ok(json!({"type": "input_file", "filename": request.input.filename(), "file_data": url}))
    } else if mime.starts_with("image/") {
        Ok(json!({"type": "input_image", "image_url": url, "detail": "high"}))
    } else {
        Err(Error::input(format!(
            "openai: unsupported document type '{mime}' — the Responses API takes PDFs (input_file) \
             and images (input_image); convert other formats to PDF first"
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

#[cfg(test)]
fn base64_encode(input: &[u8]) -> String {
    vlm::base64_encode(input)
}

fn data_url(mime: &str, data: &[u8]) -> String {
    vlm::data_url(mime, data)
}

fn pages_schema() -> Value {
    vlm::pages_schema()
}

fn parse_prompt(request: &DocumentRequest) -> String {
    vlm::parse_prompt(request, Answer::Json)
}

fn extract_prompt(request: &ExtractRequest) -> String {
    vlm::extract_prompt(request, Answer::Json)
}

/// Strict structured outputs are on by default; `provider_options.strict = false` turns them off.
fn strict_enabled(request: &DocumentRequest) -> bool {
    vlm::strict_enabled(request, true)
}

fn passthrough_options(request: &DocumentRequest) -> Option<Value> {
    vlm::passthrough_options(request)
}

/// gpt-5.x / gpt-6 are reasoning models and reject `temperature`; gpt-4.x still accepts it.
fn supports_temperature(model: &str) -> bool {
    model.starts_with("gpt-4") || model.starts_with("chatgpt-4")
}

/// Make a schema satisfy OpenAI strict structured outputs (see [`vlm::sanitize_strict_schema`]).
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

    const PARSE_FIXTURE: &str = include_str!("../../tests/fixtures/openai_responses_parse.json");
    const EXTRACT_FIXTURE: &str = include_str!("../../tests/fixtures/openai_responses_extract.json");

    fn pdf_request() -> DocumentRequest {
        DocumentRequest::from_bytes(bytes::Bytes::from_static(b"%PDF-1.7 fake"), "invoice.pdf")
            .model("openai/gpt-5.6-luna")
    }

    /// `required` order follows serde_json's map order, which depends on the `preserve_order`
    /// feature of whatever else is in the build — compare sorted.
    fn sorted_strings(value: &Value) -> Vec<String> {
        let mut v: Vec<String> = value.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn normalizes_parse_fixture() {
        let raw: Value = serde_json::from_str(PARSE_FIXTURE).unwrap();
        let text = output_text(&raw).unwrap();
        let data: Value = serde_json::from_str(&text).unwrap();
        let mut resp = normalize_parse(&data, OutputFormat::Markdown, "gpt-5.6-luna");
        resp.usage.provider_cost_usd = provider_cost("gpt-5.6-luna", 3120, 412);

        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages[0].page_number, 1);
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"));
        assert_eq!(resp.pages[0].blocks.len(), 1, "one text block per page");
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Text);
        assert!(resp.pages[0].blocks[0].bbox.is_none(), "vision LLMs report no geometry");
        assert!(resp.pages[1].markdown.contains("| Widget | 2 | $10.00 |"));
        assert!(resp.text.contains("Widget"));
        assert!(resp.pages[0].width.is_none());
        // 3120 * $0.20/1M + 412 * $1.20/1M
        let cost = resp.usage.provider_cost_usd.unwrap();
        assert!((cost - 0.0011184).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn reads_usage_and_id_from_fixture() {
        let raw: Value = serde_json::from_str(PARSE_FIXTURE).unwrap();
        assert_eq!(raw["id"], "resp_68f0a1b2c3d4e5f60123456789abcdef");
        assert_eq!(raw["usage"]["input_tokens"], 3120);
        assert_eq!(raw["usage"]["output_tokens"], 412);
    }

    #[test]
    fn extract_fixture_yields_data() {
        let raw: Value = serde_json::from_str(EXTRACT_FIXTURE).unwrap();
        let data: Value = serde_json::from_str(&output_text(&raw).unwrap()).unwrap();
        assert_eq!(data["invoice_number"], "INV-1234");
        assert_eq!(data["total"], 56.78);
        assert_eq!(data["line_items"][0]["description"], "Widget");
        assert!(data["due_date"].is_null(), "absent values come back null");
    }

    #[test]
    fn incomplete_and_refusal_become_provider_errors() {
        let incomplete = json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}});
        let e = output_text(&incomplete).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Provider);
        assert!(e.message.contains("max_output_tokens"), "{e}");

        let refused = json!({
            "status": "completed",
            "output": [{"type": "message", "content": [{"type": "refusal", "refusal": "I can't help"}]}],
        });
        let e = output_text(&refused).unwrap_err();
        assert!(e.message.contains("I can't help"), "{e}");

        let empty = json!({"status": "completed", "output": []});
        assert!(output_text(&empty).is_err());
    }

    #[test]
    fn output_text_concatenates_parts_and_skips_reasoning() {
        let resp = json!({
            "status": "completed",
            "output": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "content": [
                    {"type": "output_text", "text": "{\"pages\":"},
                    {"type": "output_text", "text": "[]}"},
                ]},
            ],
        });
        assert_eq!(output_text(&resp).unwrap(), "{\"pages\":[]}");
        // `output_text` aggregate as a fallback.
        assert_eq!(output_text(&json!({"output": [], "output_text": "{}"})).unwrap(), "{}");
    }

    #[test]
    fn builds_responses_body() {
        let req = pdf_request();
        let doc =
            json!({"type": "input_file", "filename": "invoice.pdf", "file_data": "data:application/pdf;base64,x"});
        let body = build_body(&req, "gpt-5.6-luna", doc, "prompt", PARSE_SCHEMA_NAME, pages_schema()).unwrap();

        assert_eq!(body["model"], "gpt-5.6-luna");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_file");
        assert_eq!(body["input"][0]["content"][1], json!({"type": "input_text", "text": "prompt"}));
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["name"], "puffinparse_pages");
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(body["text"]["format"]["schema"]["additionalProperties"], false);
        assert_eq!(body["store"], false);
        assert_eq!(body["max_output_tokens"], 32_000);
        // Reasoning models reject `temperature`.
        assert!(body.get("temperature").is_none(), "{body}");
        assert_eq!(body["reasoning"]["effort"], "low");
    }

    #[test]
    fn sends_temperature_only_for_non_reasoning_models() {
        let req = pdf_request();
        let body = build_body(&req, "gpt-4.1-mini", json!({}), "p", "s", json!({"type": "object"})).unwrap();
        assert_eq!(body["temperature"], 0);
        assert!(body.get("reasoning").is_none());
        assert!(supports_temperature("gpt-4o-mini") && !supports_temperature("gpt-5.6-luna"));
    }

    #[test]
    fn provider_options_merge_and_private_keys_are_not_forwarded() {
        let req = pdf_request().provider_options(json!({
            "strict": false,
            "max_output_tokens": 4096,
            "reasoning": {"effort": "medium"},
        }));
        let body = build_body(&req, "gpt-5.6-luna", json!({}), "p", "s", json!({"type": "object"})).unwrap();
        assert_eq!(body["max_output_tokens"], 4096);
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["text"]["format"]["strict"], false, "strict is a PuffinParse option");
        assert!(body.get("strict").is_none(), "private options are stripped from the body");
        // strict: false leaves the schema untouched.
        assert!(body["text"]["format"]["schema"].get("additionalProperties").is_none());
    }

    #[test]
    fn rejects_non_object_provider_options() {
        let req = pdf_request().provider_options(json!([1, 2]));
        let e = build_body(&req, "gpt-5.6-luna", json!({}), "p", "s", json!({})).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Input);
    }

    #[test]
    fn sanitises_nested_schemas_for_strict_mode() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "invoice_number": {"type": "string"},
                "total": {"type": ["number", "null"]},
                "line_items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {"description": {"type": "string"}, "qty": {"type": "integer"}},
                        "required": ["description"],
                    },
                },
                "vendor": {"$ref": "#/$defs/party"},
            },
            "required": ["invoice_number"],
            "$defs": {
                "party": {"type": "object", "properties": {"name": {"type": "string"}, "vat": {"type": "string"}}},
            },
        });
        sanitize_strict_schema(&mut schema);

        assert_eq!(schema["additionalProperties"], false);
        // Key order depends on serde_json's `preserve_order` feature, so compare as a set.
        assert_eq!(sorted_strings(&schema["required"]), ["invoice_number", "line_items", "total", "vendor"]);
        let item = &schema["properties"]["line_items"]["items"];
        assert_eq!(item["additionalProperties"], false);
        assert_eq!(sorted_strings(&item["required"]), ["description", "qty"]);
        assert_eq!(sorted_strings(&schema["$defs"]["party"]["required"]), ["name", "vat"]);
        assert_eq!(schema["$defs"]["party"]["additionalProperties"], false);
        // Non-object leaves are untouched.
        assert!(schema["properties"]["invoice_number"].get("additionalProperties").is_none());
    }

    #[test]
    fn sanitises_union_branches() {
        let mut schema = json!({
            "anyOf": [
                {"type": "object", "properties": {"a": {"type": "string"}}},
                {"type": "null"},
            ],
        });
        sanitize_strict_schema(&mut schema);
        assert_eq!(schema["anyOf"][0]["required"], json!(["a"]));
        assert_eq!(schema["anyOf"][0]["additionalProperties"], false);
        assert!(schema["anyOf"][1].get("required").is_none());
    }

    #[test]
    fn prompt_carries_pages_language_and_format() {
        let req = DocumentRequest::from_path("a.pdf").pages("1-3,7").language("de").output(OutputFormat::Text);
        let p = parse_prompt(&req);
        assert!(p.contains("only pages 1-3,7"), "{p}");
        assert!(p.contains("mainly in de"), "{p}");
        assert!(p.contains("plain text only"), "{p}");
        assert!(parse_prompt(&DocumentRequest::from_path("a.pdf")).contains("GitHub-Flavored Markdown"));
    }

    #[test]
    fn extract_prompt_includes_instructions() {
        let req = ExtractRequest::new(DocumentRequest::from_path("a.pdf"), json!({"type": "object"}))
            .instructions("Totals are in EUR.");
        let p = extract_prompt(&req);
        assert!(p.contains("Totals are in EUR."), "{p}");
        assert!(p.contains("never guess"), "{p}");
    }

    #[test]
    fn pages_fall_back_to_ordinal_numbers_and_text_output() {
        let data = json!({"pages": [{"markdown": "# A"}, {"page_number": 0, "markdown": "**B**"}]});
        let pages = pages_from_value(&data, OutputFormat::Markdown);
        assert_eq!(pages[0].page_number, 1);
        assert_eq!(pages[1].page_number, 2);
        assert_eq!(pages[1].text, "B", "text is derived from markdown");
        let pages = pages_from_value(&data, OutputFormat::Text);
        assert_eq!(pages[1].markdown, pages[1].text, "text output keeps the model's own string");
    }

    #[test]
    fn errors_use_the_openai_envelope() {
        let body =
            r#"{"error":{"message":"Invalid schema for response_format","type":"invalid_request_error","code":null}}"#;
        let e = Error::from_http(NAME, 400, body);
        assert_eq!(e.kind, ErrorKind::BadRequest);
        assert_eq!(e.message, "Invalid schema for response_format");
        assert_eq!(Error::from_http(NAME, 401, body).kind, ErrorKind::Authentication);
        assert_eq!(Error::from_http(NAME, 429, body).kind, ErrorKind::RateLimit);
        assert_eq!(Error::from_http(NAME, 503, body).kind, ErrorKind::Provider);
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(data_url("application/pdf", b"foobar"), "data:application/pdf;base64,Zm9vYmFy");
    }

    #[test]
    fn document_part_picks_the_right_block() {
        let dl = Deadline::new(5.0);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

        let pdf = pdf_request();
        let part = rt.block_on(document_part(&pdf, &dl, Retry::new(0))).unwrap();
        assert_eq!(part["type"], "input_file");
        assert_eq!(part["filename"], "invoice.pdf");
        assert!(part["file_data"].as_str().unwrap().starts_with("data:application/pdf;base64,"));

        let png = DocumentRequest::from_bytes(bytes::Bytes::from_static(b"\x89PNG"), "scan.png");
        let part = rt.block_on(document_part(&png, &dl, Retry::new(0))).unwrap();
        assert_eq!(part["type"], "input_image");
        assert!(part["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));

        let other = DocumentRequest::from_bytes(bytes::Bytes::from_static(b"x"), "notes.docx");
        let e = rt.block_on(document_part(&other, &dl, Retry::new(0))).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Input);
    }

    #[test]
    fn per_page_estimates_match_pricing_json() {
        // 1,500 input + 700 output tokens per page — the method noted in docs/providers/openai.md.
        let round = |x: f64| (x * 1e6).round() / 1e6;
        assert_eq!(round(per_page_estimate("gpt-5.6-luna").unwrap()), 0.00114);
        assert_eq!(round(per_page_estimate("gpt-5.6-terra").unwrap()), 0.0114);
        assert_eq!(round(per_page_estimate("gpt-5.6-sol").unwrap()), 0.02);
        assert_eq!(round(per_page_estimate("gpt-6-astra").unwrap()), 0.05);
        assert!(per_page_estimate("gpt-unknown").is_none());
        assert_eq!(PRICES_UPDATED, "2026-09-11");
    }

    const SAMPLE: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");

    #[tokio::test]
    #[ignore = "needs OPENAI_API_KEY and network"]
    async fn openai_parse_live() {
        if std::env::var("OPENAI_API_KEY").map(|v| v.is_empty()).unwrap_or(true) {
            eprintln!("skipping: OPENAI_API_KEY not set");
            return;
        }
        let resp = crate::parse(DocumentRequest::from_path(SAMPLE).model("openai/gpt-5.6-luna").timeout_secs(240.0))
            .await
            .expect("parse succeeds");
        assert_eq!(resp.usage.pages, 2);
        assert!(!resp.markdown.trim().is_empty());
        assert!(resp.cost_usd.unwrap_or(0.0) > 0.0);
    }

    #[tokio::test]
    #[ignore = "needs OPENAI_API_KEY and network"]
    async fn openai_extract_live() {
        if std::env::var("OPENAI_API_KEY").map(|v| v.is_empty()).unwrap_or(true) {
            eprintln!("skipping: OPENAI_API_KEY not set");
            return;
        }
        let schema = json!({
            "type": "object",
            "properties": {"title": {"type": ["string", "null"]}},
        });
        let req = ExtractRequest::new(
            DocumentRequest::from_path(SAMPLE).model("openai/gpt-5.6-luna").timeout_secs(240.0),
            schema,
        );
        let resp = crate::extract(req).await.expect("extract succeeds");
        assert!(resp.data.is_object());
        assert!(resp.cost_usd.unwrap_or(0.0) > 0.0);
    }
}
