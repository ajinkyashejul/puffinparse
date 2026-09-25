//! Azure AI Document Intelligence (formerly Form Recognizer), REST API `2024-11-30` (v4.0 GA).
//!
//! Flow: `POST {endpoint}/documentintelligence/documentModels/{modelId}:analyze` with a JSON body
//! holding `urlSource` or `base64Source` → `202 Accepted` + an `Operation-Location` header → poll
//! that URL until `status` is `succeeded` or `failed`.
//!
//! Azure endpoints are **per resource**, so there is no built-in base URL: the endpoint comes from
//! `AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT` (or `base_url` on the request) and the key from
//! `AZURE_DOCUMENT_INTELLIGENCE_KEY` (or `api_key`), sent as `Ocp-Apim-Subscription-Key`.
//!
//! Modes: `parse` (`prebuilt-layout`, markdown + paragraphs/tables), `ocr` (`prebuilt-read` or
//! `prebuilt-layout`, native words/lines with polygons), `extract` (`prebuilt-invoice`,
//! `prebuilt-receipt`, … — a **fixed** per-model schema, plus custom models via
//! `provider_options.model_id`).

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::provider::{self, Provider};
use crate::types::{
    BBox, Block, BlockType, Citation, DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, FieldInfo, Line,
    Mode, OutputFormat, Page, ParseResponse, TextPage, TextResponse, Usage, Word,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME: &str = "azure";
/// REST API version PuffinParse pins (`2024-11-30` is the v4.0 GA release).
pub const API_VERSION: &str = "2024-11-30";
const ENV_KEY: &str = "AZURE_DOCUMENT_INTELLIGENCE_KEY";
/// Per-resource endpoint, e.g. `https://my-di.cognitiveservices.azure.com`.
const ENV_ENDPOINT: &str = "AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT";
const KEY_HEADER: &str = "Ocp-Apim-Subscription-Key";
/// Azure's `pages` grammar has no open-ended range, so `"10-"` becomes `"10-{MAX_PAGE}"`.
const MAX_PAGE: u32 = 2000;

#[derive(Debug, Default, Clone, Copy)]
pub struct Azure;

#[async_trait]
impl Provider for Azure {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let Analyzed { result, result_id, model_id, raw_json } = analyze(request, model, Mode::Parse).await?;
        let mut resp = normalize_parse(&result, request.output, model);
        resp.provider_job_id = Some(result_id);
        resp.metadata.insert("azure_model_id".into(), json!(model_id));
        resp.raw = raw_json;
        Ok(resp)
    }

    async fn ocr(&self, request: &DocumentRequest, model: &str) -> Result<TextResponse> {
        let Analyzed { result, result_id, model_id, raw_json } = analyze(request, model, Mode::Ocr).await?;
        let mut resp = normalize_ocr(&result, model);
        resp.provider_job_id = Some(result_id);
        resp.metadata.insert("azure_model_id".into(), json!(model_id));
        resp.raw = raw_json;
        Ok(resp)
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let Analyzed { result, result_id, model_id, raw_json } =
            analyze(&request.document, model, Mode::Extract).await?;
        let mut resp = normalize_extract(&result, &request.schema, model)?;
        resp.provider_job_id = Some(result_id);
        resp.metadata.insert("azure_model_id".into(), json!(model_id));
        resp.raw = raw_json;
        Ok(resp)
    }
}

/// A finished (succeeded) analyze operation plus the bits of context callers need.
struct Analyzed {
    result: AnalyzeResult,
    /// `resultId` from the `Operation-Location` URL — surfaced as `provider_job_id`.
    result_id: String,
    model_id: String,
    /// Original payload, kept only when `include_raw` was set.
    raw_json: Option<Value>,
}

// ---- request ------------------------------------------------------------------------------------

/// Map a PuffinParse model name to an Azure `modelId`. `provider_options.model_id` always wins, which is
/// how custom models (`azure/custom`) and prebuilt models PuffinParse does not list are reached.
fn model_id(request: &DocumentRequest, model: &str) -> Result<String> {
    if let Some(id) = request.option("model_id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(id.to_string());
    }
    let id = match model {
        "layout" => "prebuilt-layout",
        "read" => "prebuilt-read",
        "invoice" => "prebuilt-invoice",
        "receipt" => "prebuilt-receipt",
        "id_document" => "prebuilt-idDocument",
        "tax_us_w2" => "prebuilt-tax.us.w2",
        "custom" => {
            return Err(Error::unsupported_model(
                "azure/custom needs the model id: provider_options={\"model_id\": \"<your custom model>\"}",
            )
            .with_provider(NAME))
        }
        other => return Err(Error::unsupported_model(format!("azure: unknown model '{other}'")).with_provider(NAME)),
    };
    Ok(id.to_string())
}

/// Query parameters for `:analyze`. Azure takes every knob as a query parameter — the request body
/// only carries the document — so `provider_options` are merged here.
fn query_params(request: &DocumentRequest, mode: Mode) -> Result<Vec<(String, String)>> {
    let opt_str = |k: &str| request.option(k).and_then(Value::as_str).map(str::to_string);
    let list = |k: &str| -> Option<Vec<String>> {
        match request.option(k) {
            Some(Value::Array(a)) => Some(a.iter().map(scalar_to_string).collect()),
            Some(Value::String(s)) => Some(vec![s.clone()]),
            _ => None,
        }
    };

    let mut params: Vec<(String, String)> = vec![
        ("api-version".into(), opt_str("api_version").unwrap_or_else(|| API_VERSION.to_string())),
        // Offsets are sliced with Rust `char` indices, which is exactly `unicodeCodePoint`.
        ("stringIndexType".into(), opt_str("string_index_type").unwrap_or_else(|| "unicodeCodePoint".into())),
    ];
    // Markdown is only worth asking for in `parse`; `ocr` reads words/lines and `extract` reads fields.
    let format = opt_str("output_content_format").unwrap_or_else(|| {
        if mode == Mode::Parse {
            "markdown".into()
        } else {
            "text".into()
        }
    });
    params.push(("outputContentFormat".into(), format));

    if let Some(pages) = &request.pages {
        params.push(("pages".into(), azure_pages(pages)?));
    }
    if let Some(locale) = opt_str("locale").or_else(|| request.language.clone()) {
        params.push(("locale".into(), locale));
    }
    let query_fields = list("query_fields").unwrap_or_default();
    let mut features = list("features").unwrap_or_default();
    if !query_fields.is_empty() && !features.iter().any(|f| f == "queryFields") {
        features.push("queryFields".into());
    }
    if !features.is_empty() {
        params.push(("features".into(), features.join(",")));
    }
    if !query_fields.is_empty() {
        params.push(("queryFields".into(), query_fields.join(",")));
    }
    if let Some(output) = list("output") {
        if !output.is_empty() {
            params.push(("output".into(), output.join(",")));
        }
    }
    Ok(params)
}

/// Percent-encode the query parameters (`reqwest`'s `query` feature is not enabled here).
fn query_string(params: &[(String, String)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v))).collect::<Vec<_>>().join("&")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn scalar_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `"1-3,7,10-"` → `"1-3,7,10-2000"` (Azure's grammar is `^(\d+(-\d+)?)(,\s*(\d+(-\d+)?))*$`).
fn azure_pages(spec: &str) -> Result<String> {
    let parts: Vec<String> = crate::util::parse_page_ranges(spec)?
        .into_iter()
        .map(|(start, end)| match end {
            Some(e) if e == start => start.to_string(),
            Some(e) => format!("{start}-{e}"),
            None => format!("{start}-{MAX_PAGE}"),
        })
        .collect();
    Ok(parts.join(","))
}

/// Endpoint: request `base_url`, then `AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT`. There is no default.
fn resolve_endpoint(request: &DocumentRequest) -> Result<String> {
    let endpoint = provider::resolve_base_url(request, ENV_ENDPOINT, "");
    if endpoint.is_empty() {
        return Err(Error::authentication(format!(
            "no endpoint for azure: set {ENV_ENDPOINT} (e.g. https://<resource>.cognitiveservices.azure.com) or pass base_url"
        ))
        .with_provider(NAME));
    }
    Ok(endpoint)
}

/// Submit the document, poll the operation, and return the succeeded result.
async fn analyze(request: &DocumentRequest, model: &str, mode: Mode) -> Result<Analyzed> {
    let api_key = provider::resolve_api_key(request, ENV_KEY, NAME)?;
    let endpoint = resolve_endpoint(request)?;
    let model_id = model_id(request, model)?;
    let params = query_params(request, mode)?;
    let deadline = Deadline::new(request.timeout_secs);
    let retry = Retry::new(request.max_retries);
    let client = http::client();

    // 1. Body: a public URL is passed through, anything else is uploaded inline as base64.
    let body = match provider::load_bytes(&request.input).await? {
        None => {
            let DocumentInput::Url { url } = &request.input else { unreachable!() };
            json!({ "urlSource": url })
        }
        Some(data) => json!({ "base64Source": base64_encode(&data) }),
    };

    // 2. Submit. Success is `202` + `Operation-Location`; the body is empty.
    let url = format!("{endpoint}/documentintelligence/documentModels/{model_id}:analyze?{}", query_string(&params));
    let (operation_location, retry_after): (String, Option<f64>) = http::with_retry(NAME, retry, &deadline, || {
        let rb = client.post(&url).header(KEY_HEADER, &api_key).json(&body).timeout(deadline.request_timeout());
        async move {
            let resp = rb.send().await?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await?;
                return Err(Error::from_http(NAME, status.as_u16(), &text));
            }
            let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            let location = header("operation-location").ok_or_else(|| {
                Error::provider("analyze was accepted without an Operation-Location header").with_provider(NAME)
            })?;
            Ok((location, header("retry-after").and_then(|v| v.parse::<f64>().ok())))
        }
    })
    .await?;

    let result_id = result_id(&operation_location);
    tracing::debug!(result_id = %result_id, model_id = %model_id, "azure: analysis started");

    // 3. Poll. Azure suggests a first delay through `Retry-After` (seconds); honour it, then back off
    //    from 2 s (Microsoft asks for at most one GET every 2 s per analyze request) to 10 s.
    if let Some(secs) = retry_after.filter(|s| *s > 0.0) {
        tokio::time::sleep(Duration::from_secs_f64(secs).min(deadline.remaining())).await;
        deadline.check(NAME, "waiting for job to finish")?;
    }
    let (operation, raw) = http::poll_until(NAME, &deadline, Duration::from_secs(2), Duration::from_secs(10), || {
        let location = operation_location.clone();
        let api_key = api_key.clone();
        async move {
            let (operation, raw) = get_operation(&location, &api_key, &deadline, retry).await?;
            Ok(match operation.status.as_str() {
                "notStarted" | "running" => None,
                _ => Some((operation, raw)),
            })
        }
    })
    .await
    .map_err(|e| e.with_job_id(result_id.clone()))?;

    // 4. Terminal state.
    if operation.status != "succeeded" {
        return Err(operation_error(&operation).with_job_id(result_id));
    }
    let result = operation.analyze_result.ok_or_else(|| {
        Error::provider("operation succeeded without an analyzeResult")
            .with_provider(NAME)
            .with_job_id(result_id.clone())
    })?;
    Ok(Analyzed { result, result_id, model_id, raw_json: request.include_raw.then_some(raw) })
}

/// One poll of the operation URL: retried on rate-limit / 5xx / network errors.
async fn get_operation(
    location: &str,
    api_key: &str,
    deadline: &Deadline,
    retry: Retry,
) -> Result<(AnalyzeOperation, Value)> {
    http::with_retry(NAME, retry, deadline, || {
        let rb = http::client().get(location).header(KEY_HEADER, api_key).timeout(deadline.request_timeout());
        async move {
            let body = http::read_response(NAME, rb.send().await?).await?;
            let raw: Value = serde_json::from_str(&body).map_err(|e| {
                Error::provider(format!("unexpected response shape: {e}; body starts: {}", http::snippet(&body)))
                    .with_provider(NAME)
            })?;
            let operation: AnalyzeOperation = serde_json::from_value(raw.clone())
                .map_err(|e| Error::provider(format!("unexpected analyze operation shape: {e}")).with_provider(NAME))?;
            Ok((operation, raw))
        }
    })
    .await
}

/// `…/analyzeResults/{resultId}?api-version=…` → `{resultId}`.
fn result_id(operation_location: &str) -> String {
    operation_location
        .split('?')
        .next()
        .unwrap_or(operation_location)
        .rsplit('/')
        .next()
        .unwrap_or(operation_location)
        .to_string()
}

/// Map a failed operation onto an `Error`. Azure reports the reason in `error.code` / `error.message`.
fn operation_error(operation: &AnalyzeOperation) -> Error {
    let Some(err) = &operation.error else {
        return Error::provider(format!("analysis ended with status '{}'", operation.status)).with_provider(NAME);
    };
    let inner = err.innererror.as_ref().map(|i| format!(" ({}: {})", i.code, i.message)).unwrap_or_default();
    let kind = match err.code.as_str() {
        c if c.starts_with("Invalid") || c.starts_with("Unsupported") || c.starts_with("NotSupported") => {
            ErrorKind::BadRequest
        }
        "ContentSourceNotAccessible" | "ContentSourceTimeout" | "ContentSourceSizeExceeded" => ErrorKind::BadRequest,
        _ => ErrorKind::Provider,
    };
    Error::new(kind, format!("analysis failed: {}: {}{inner}", err.code, err.message)).with_provider(NAME)
}

/// Minimal base64 encoder (no extra dependency; the payload is the whole document).
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

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalyzeOperation {
    pub status: String,
    #[serde(default)]
    pub created_date_time: Option<String>,
    #[serde(default)]
    pub last_updated_date_time: Option<String>,
    #[serde(default)]
    pub error: Option<AzureError>,
    #[serde(default)]
    pub analyze_result: Option<AnalyzeResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct AzureError {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub innererror: Option<InnerError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct InnerError {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnalyzeResult {
    #[serde(default)]
    pub api_version: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub string_index_type: Option<String>,
    #[serde(default)]
    pub content_format: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub pages: Vec<WirePage>,
    #[serde(default)]
    pub paragraphs: Vec<WireParagraph>,
    #[serde(default)]
    pub tables: Vec<WireTable>,
    #[serde(default)]
    pub figures: Vec<WireFigure>,
    #[serde(default)]
    pub key_value_pairs: Vec<WireKeyValuePair>,
    #[serde(default)]
    pub documents: Vec<WireDocument>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub(crate) struct Span {
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub length: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BoundingRegion {
    #[serde(default = "one")]
    pub page_number: u32,
    #[serde(default)]
    pub polygon: Vec<f64>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WirePage {
    #[serde(default = "one")]
    pub page_number: u32,
    #[serde(default)]
    pub angle: Option<f64>,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
    /// `pixel` for images, `inch` for PDFs — irrelevant once boxes are normalised by width/height.
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub spans: Vec<Span>,
    #[serde(default)]
    pub words: Vec<WireWord>,
    #[serde(default)]
    pub lines: Vec<WireLine>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireWord {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub polygon: Vec<f64>,
    #[serde(default)]
    pub span: Option<Span>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireLine {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub polygon: Vec<f64>,
    #[serde(default)]
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireParagraph {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub bounding_regions: Vec<BoundingRegion>,
    #[serde(default)]
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireTable {
    #[serde(default)]
    pub row_count: usize,
    #[serde(default)]
    pub column_count: usize,
    #[serde(default)]
    pub cells: Vec<WireTableCell>,
    #[serde(default)]
    pub bounding_regions: Vec<BoundingRegion>,
    #[serde(default)]
    pub spans: Vec<Span>,
    #[serde(default)]
    pub caption: Option<WireCaption>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireTableCell {
    /// `content` (default), `columnHeader`, `rowHeader`, `stubHead`, `description`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub row_index: usize,
    #[serde(default)]
    pub column_index: usize,
    #[serde(default)]
    pub row_span: Option<usize>,
    #[serde(default)]
    pub column_span: Option<usize>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub bounding_regions: Vec<BoundingRegion>,
    #[serde(default)]
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireCaption {
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireFigure {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub bounding_regions: Vec<BoundingRegion>,
    #[serde(default)]
    pub spans: Vec<Span>,
    #[serde(default)]
    pub caption: Option<WireCaption>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireKeyValuePair {
    #[serde(default)]
    pub key: Option<WireKeyValueElement>,
    #[serde(default)]
    pub value: Option<WireKeyValueElement>,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireKeyValueElement {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub bounding_regions: Vec<BoundingRegion>,
    #[serde(default)]
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireDocument {
    #[serde(default)]
    pub doc_type: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub bounding_regions: Vec<BoundingRegion>,
    #[serde(default)]
    pub spans: Vec<Span>,
    #[serde(default)]
    pub fields: BTreeMap<String, WireField>,
}

/// One extracted field. The typed value lives under `valueString` / `valueNumber` / … depending on
/// `type`, so the untyped remainder is captured and read by name.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireField {
    #[serde(rename = "type", default)]
    pub field_type: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub bounding_regions: Vec<BoundingRegion>,
    #[serde(default)]
    pub spans: Vec<Span>,
    #[serde(flatten)]
    pub values: Map<String, Value>,
}

// ---- normalisation: shared -----------------------------------------------------------------------

/// `polygon` is a flat `[x1,y1,x2,y2,…]` quad in page units; PuffinParse stores the axis-aligned hull.
fn polygon_bbox(polygon: &[f64], dims: Option<(f64, f64)>) -> Option<BBox> {
    if polygon.len() < 4 {
        return None;
    }
    let (w, h) = dims?;
    let xs: Vec<f64> = polygon.iter().copied().step_by(2).collect();
    let ys: Vec<f64> = polygon.iter().copied().skip(1).step_by(2).collect();
    let (min_x, max_x) = min_max(&xs)?;
    let (min_y, max_y) = min_max(&ys)?;
    BBox::from_xywh(min_x, min_y, max_x - min_x, max_y - min_y, w, h)
}

fn min_max(values: &[f64]) -> Option<(f64, f64)> {
    let mut it = values.iter().copied().filter(|v| v.is_finite());
    let first = it.next()?;
    Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
}

fn region_bbox(regions: &[BoundingRegion], dims: &BTreeMap<u32, (f64, f64)>) -> Option<BBox> {
    let region = regions.first()?;
    polygon_bbox(&region.polygon, dims.get(&region.page_number).copied())
}

fn page_of(regions: &[BoundingRegion]) -> Option<u32> {
    regions.first().map(|r| r.page_number)
}

/// Slice the top-level `content` for a span. Offsets are `unicodeCodePoint` (what PuffinParse asks for),
/// i.e. Rust `char` indices.
fn slice_span(chars: &[char], span: &Span) -> String {
    let start = span.offset.min(chars.len());
    let end = span.offset.saturating_add(span.length).min(chars.len());
    chars[start..end].iter().collect()
}

/// Word confidences flattened across pages and sorted by offset, for per-block averaging.
fn word_confidences(result: &AnalyzeResult) -> Vec<(usize, f64)> {
    let mut words: Vec<(usize, f64)> = result
        .pages
        .iter()
        .flat_map(|p| p.words.iter())
        .filter_map(|w| Some((w.span?.offset, w.confidence?)))
        .collect();
    words.sort_by_key(|(offset, _)| *offset);
    words
}

/// Mean confidence of the words covered by `spans`.
fn avg_confidence(words: &[(usize, f64)], spans: &[Span]) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0u32;
    for span in spans {
        let end = span.offset.saturating_add(span.length);
        let start = words.partition_point(|(offset, _)| *offset < span.offset);
        for (offset, confidence) in &words[start..] {
            if *offset >= end {
                break;
            }
            sum += *confidence;
            count += 1;
        }
    }
    (count > 0).then(|| sum / f64::from(count))
}

fn page_dims(result: &AnalyzeResult) -> BTreeMap<u32, (f64, f64)> {
    result
        .pages
        .iter()
        .filter_map(|p| Some((p.page_number, (p.width?, p.height?))))
        .filter(|(_, (w, h))| *w > 0.0 && *h > 0.0)
        .collect()
}

fn first_offset(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.offset).min().unwrap_or(usize::MAX)
}

// ---- normalisation: parse ------------------------------------------------------------------------

/// `paragraphs[].role` → [`BlockType`]. Roleless paragraphs are plain text.
fn map_role(role: Option<&str>) -> BlockType {
    match role {
        Some("title") => BlockType::Title,
        Some("sectionHeading") => BlockType::SectionHeader,
        Some("pageHeader") => BlockType::Header,
        Some("pageFooter") => BlockType::Footer,
        Some("footnote") => BlockType::Footnote,
        Some("formulaBlock") => BlockType::Formula,
        Some("pageNumber") => BlockType::Other,
        _ => BlockType::Text,
    }
}

/// Render table cells as a markdown table (Azure's own markdown uses HTML tables).
fn render_table(table: &WireTable) -> String {
    let rows = table.row_count.max(table.cells.iter().map(|c| c.row_index + 1).max().unwrap_or(0));
    let cols = table.column_count.max(table.cells.iter().map(|c| c.column_index + 1).max().unwrap_or(0));
    if rows == 0 || cols == 0 {
        return String::new();
    }
    let mut grid = vec![vec![String::new(); cols]; rows];
    for cell in &table.cells {
        if cell.row_index < rows && cell.column_index < cols {
            grid[cell.row_index][cell.column_index] = cell.content.replace('|', "\\|").replace('\n', " ").trim().into();
        }
    }
    let header_row = table
        .cells
        .iter()
        .any(|c| c.row_index == 0 && matches!(c.kind.as_deref(), Some("columnHeader") | Some("stubHead")));
    let render_row = |row: &Vec<String>| format!("| {} |", row.join(" | "));
    let mut out: Vec<String> = Vec::with_capacity(rows + 2);
    if let Some(caption) = table.caption.as_ref().map(|c| c.content.trim()).filter(|c| !c.is_empty()) {
        out.push(caption.to_string());
        out.push(String::new());
    }
    let mut iter = grid.iter();
    if header_row {
        if let Some(first) = iter.next() {
            out.push(render_row(first));
        }
    } else {
        // A markdown table needs a header row; use an empty one so the body still renders.
        out.push(render_row(&vec![String::new(); cols]));
    }
    out.push(format!("| {} |", vec!["---"; cols].join(" | ")));
    for row in iter {
        out.push(render_row(row));
    }
    out.join("\n")
}

pub(crate) fn normalize_parse(result: &AnalyzeResult, fmt: OutputFormat, model: &str) -> ParseResponse {
    let chars: Vec<char> = result.content.chars().collect();
    let dims = page_dims(result);
    let words = word_confidences(result);
    // Table cell text is repeated in `paragraphs`; keep the table block and drop those paragraphs.
    let table_ranges: Vec<(usize, usize)> =
        result.tables.iter().flat_map(|t| t.spans.iter()).map(|s| (s.offset, s.offset + s.length)).collect();
    let in_table = |spans: &[Span]| {
        spans.iter().any(|s| table_ranges.iter().any(|(start, end)| s.offset >= *start && s.offset < *end))
    };

    let mut ordered: Vec<(u32, usize, Block)> = Vec::new();
    for paragraph in &result.paragraphs {
        if in_table(&paragraph.spans) {
            continue;
        }
        let page_number = page_of(&paragraph.bounding_regions).unwrap_or(1);
        let content = match fmt {
            OutputFormat::Markdown => paragraph.content.clone(),
            OutputFormat::Text => crate::types::markdown_to_text(&paragraph.content),
        };
        ordered.push((
            page_number,
            first_offset(&paragraph.spans),
            Block {
                block_type: map_role(paragraph.role.as_deref()),
                content,
                text: None,
                bbox: region_bbox(&paragraph.bounding_regions, &dims),
                confidence: avg_confidence(&words, &paragraph.spans),
                page_number,
            },
        ));
    }
    for table in &result.tables {
        let page_number = page_of(&table.bounding_regions).unwrap_or(1);
        let markdown = render_table(table);
        let content = match fmt {
            OutputFormat::Markdown => markdown.clone(),
            OutputFormat::Text => crate::types::markdown_to_text(&markdown),
        };
        ordered.push((
            page_number,
            first_offset(&table.spans),
            Block {
                block_type: BlockType::Table,
                content,
                text: None,
                bbox: region_bbox(&table.bounding_regions, &dims),
                confidence: avg_confidence(&words, &table.spans),
                page_number,
            },
        ));
    }
    ordered.sort_by_key(|(page_number, offset, _)| (*page_number, *offset));
    let blocks: Vec<Block> = ordered.into_iter().map(|(_, _, b)| b).collect();

    let mut pages = crate::types::pages_from_blocks(blocks, &dims);
    // Prefer Azure's own per-page slice of the document content (markdown when we asked for it).
    for wire in &result.pages {
        let markdown = page_markdown(&chars, &wire.spans);
        let index = match pages.iter().position(|p| p.page_number == wire.page_number) {
            Some(i) => i,
            None => {
                pages.push(Page {
                    page_number: wire.page_number,
                    width: wire.width,
                    height: wire.height,
                    markdown: String::new(),
                    text: String::new(),
                    blocks: vec![],
                });
                pages.len() - 1
            }
        };
        let page = &mut pages[index];
        page.width = wire.width;
        page.height = wire.height;
        if !markdown.is_empty() {
            page.text = page_text(&markdown);
            page.markdown = match fmt {
                OutputFormat::Markdown => markdown,
                OutputFormat::Text => page.text.clone(),
            };
        } else if fmt == OutputFormat::Text {
            page.markdown = page.text.clone();
        }
    }

    let usage = Usage { pages: result.pages.len() as u32, credits: None, provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    if let Some(format) = &result.content_format {
        resp.metadata.insert("azure_content_format".into(), json!(format));
    }
    resp
}

/// Plain text for a page: Azure's markdown wraps page headers, footers and page numbers in HTML
/// comments (`<!-- PageHeader="…" -->`), so unwrap those to their text and drop other comments.
fn page_text(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        let Some(end) = after.find("-->") else {
            out.push_str(&rest[start..]);
            return crate::types::markdown_to_text(&out);
        };
        let inner = after[..end].trim();
        if let Some((_, value)) = inner.split_once('=') {
            out.push_str(value.trim().trim_matches('"'));
        }
        rest = &after[end + 3..];
    }
    out.push_str(rest);
    crate::types::markdown_to_text(&out)
}

/// Join a page's spans of the document content and drop the page-break marker.
fn page_markdown(chars: &[char], spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| slice_span(chars, s))
        .collect::<Vec<_>>()
        .join("")
        .replace("<!-- PageBreak -->", "")
        .trim()
        .to_string()
}

// ---- normalisation: ocr --------------------------------------------------------------------------

pub(crate) fn normalize_ocr(result: &AnalyzeResult, model: &str) -> TextResponse {
    let chars: Vec<char> = result.content.chars().collect();
    let words_index = word_confidences(result);
    let mut pages: Vec<TextPage> = Vec::with_capacity(result.pages.len());
    for wire in &result.pages {
        let dims = wire.width.zip(wire.height).filter(|(w, h)| *w > 0.0 && *h > 0.0);
        let lines: Vec<Line> = wire
            .lines
            .iter()
            .map(|l| Line {
                text: l.content.clone(),
                bbox: polygon_bbox(&l.polygon, dims),
                confidence: avg_confidence(&words_index, &l.spans),
            })
            .collect();
        let words: Vec<Word> = wire
            .words
            .iter()
            .map(|w| Word { text: w.content.clone(), bbox: polygon_bbox(&w.polygon, dims), confidence: w.confidence })
            .collect();
        let text = if lines.is_empty() {
            page_text(&page_markdown(&chars, &wire.spans))
        } else {
            lines.iter().map(|l| l.text.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
        };
        pages.push(TextPage {
            page_number: wire.page_number,
            width: wire.width,
            height: wire.height,
            text,
            lines,
            words,
        });
    }
    let usage = Usage { pages: result.pages.len() as u32, credits: None, provider_cost_usd: None };
    TextResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage)
}

// ---- normalisation: extract ----------------------------------------------------------------------

/// Typed value of a field, flattened to plain JSON. Nested arrays/objects recurse and register their
/// own entries in `fields` (keyed by JSON pointer).
fn field_value(
    pointer: &str,
    field: &WireField,
    dims: &BTreeMap<u32, (f64, f64)>,
    out: &mut BTreeMap<String, FieldInfo>,
) -> Value {
    let info = FieldInfo {
        confidence: field.confidence,
        citations: field
            .bounding_regions
            .iter()
            .map(|r| Citation {
                page_number: r.page_number,
                bbox: polygon_bbox(&r.polygon, dims.get(&r.page_number).copied()),
                text: field.content.clone(),
            })
            .collect(),
    };
    if info.confidence.is_some() || !info.citations.is_empty() {
        out.insert(pointer.to_string(), info);
    }

    if let Some(Value::Array(items)) = field.values.get("valueArray") {
        let mut values = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            match serde_json::from_value::<WireField>(item.clone()) {
                Ok(item) => values.push(field_value(&format!("{pointer}/{i}"), &item, dims, out)),
                Err(_) => values.push(item.clone()),
            }
        }
        return Value::Array(values);
    }
    if let Some(Value::Object(props)) = field.values.get("valueObject") {
        let mut object = Map::new();
        for (name, raw) in props {
            let child = match serde_json::from_value::<WireField>(raw.clone()) {
                Ok(child) => field_value(&format!("{pointer}/{name}"), &child, dims, out),
                Err(_) => raw.clone(),
            };
            object.insert(name.clone(), child);
        }
        return Value::Object(object);
    }
    // Scalars and the composite value types (currency, address, …) are taken as-is.
    for key in [
        "valueString",
        "valueNumber",
        "valueInteger",
        "valueBoolean",
        "valueDate",
        "valueTime",
        "valuePhoneNumber",
        "valueCountryRegion",
        "valueSelectionMark",
        "valueSignature",
        "valueCurrency",
        "valueAddress",
    ] {
        if let Some(v) = field.values.get(key) {
            return v.clone();
        }
    }
    field.content.clone().map(Value::String).unwrap_or(Value::Null)
}

/// `"invoice_total"` and `"InvoiceTotal"` compare equal.
fn normalise_name(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub(crate) fn normalize_extract(result: &AnalyzeResult, schema: &Value, model: &str) -> Result<ExtractResponse> {
    let dims = page_dims(result);
    let mut fields: BTreeMap<String, FieldInfo> = BTreeMap::new();
    let mut data = Map::new();
    let mut doc_type = None;

    if let Some(document) = result.documents.first() {
        doc_type = document.doc_type.clone();
        for (name, field) in &document.fields {
            data.insert(name.clone(), field_value(&format!("/{name}"), field, &dims, &mut fields));
        }
    } else if !result.key_value_pairs.is_empty() {
        // Custom / layout models with the `keyValuePairs` feature return form fields instead.
        for pair in &result.key_value_pairs {
            let (Some(key), Some(value)) = (pair.key.as_ref(), pair.value.as_ref()) else { continue };
            let name = key.content.trim();
            if name.is_empty() {
                continue;
            }
            data.insert(name.to_string(), Value::String(value.content.clone()));
            fields.insert(
                format!("/{name}"),
                FieldInfo {
                    confidence: pair.confidence,
                    citations: value
                        .bounding_regions
                        .iter()
                        .map(|r| Citation {
                            page_number: r.page_number,
                            bbox: polygon_bbox(&r.polygon, dims.get(&r.page_number).copied()),
                            text: Some(value.content.clone()),
                        })
                        .collect(),
                },
            );
        }
    } else {
        return Err(Error::provider(
            "analysis returned no documents; azure extract needs a prebuilt extraction model or a custom model",
        )
        .with_provider(NAME));
    }

    // Azure's prebuilt schemas are fixed. The request schema can only *select* and *rename* fields:
    // each schema property is matched case/underscore-insensitively against the returned field names.
    let wanted: Vec<String> =
        schema.get("properties").and_then(Value::as_object).map(|p| p.keys().cloned().collect()).unwrap_or_default();
    let mut selected = false;
    if !wanted.is_empty() {
        let mut filtered = Map::new();
        let mut renamed: BTreeMap<String, FieldInfo> = BTreeMap::new();
        for name in &wanted {
            let normalised = normalise_name(name);
            let Some(found) = data.keys().find(|k| normalise_name(k) == normalised).cloned() else { continue };
            if let Some(value) = data.get(&found) {
                filtered.insert(name.clone(), value.clone());
            }
            let from = format!("/{found}");
            let to = format!("/{name}");
            for (pointer, info) in &fields {
                if let Some(rest) = pointer.strip_prefix(&from) {
                    if rest.is_empty() || rest.starts_with('/') {
                        renamed.insert(format!("{to}{rest}"), info.clone());
                    }
                }
            }
        }
        if !filtered.is_empty() {
            data = filtered;
            fields = renamed;
            selected = true;
        }
    }

    let usage = Usage { pages: result.pages.len() as u32, credits: None, provider_cost_usd: None };
    let mut resp = ExtractResponse::new(NAME, &format!("{NAME}/{model}"), Value::Object(data), usage);
    resp.fields = fields;
    if let Some(doc_type) = doc_type {
        resp.metadata.insert("azure_doc_type".into(), json!(doc_type));
    }
    resp.metadata.insert("azure_schema_selected_fields".into(), json!(selected));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> AnalyzeResult {
        let raw = include_str!("../../tests/fixtures/azure_layout.json");
        let op: AnalyzeOperation = serde_json::from_str(raw).unwrap();
        assert_eq!(op.status, "succeeded");
        op.analyze_result.unwrap()
    }

    fn read() -> AnalyzeResult {
        let raw = include_str!("../../tests/fixtures/azure_read.json");
        serde_json::from_str::<AnalyzeOperation>(raw).unwrap().analyze_result.unwrap()
    }

    fn invoice() -> AnalyzeResult {
        let raw = include_str!("../../tests/fixtures/azure_invoice.json");
        serde_json::from_str::<AnalyzeOperation>(raw).unwrap().analyze_result.unwrap()
    }

    #[test]
    fn normalizes_layout_fixture() {
        let resp = normalize_parse(&layout(), OutputFormat::Markdown, "layout");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        // Page markdown comes from the page's spans into `content`, minus the page-break marker.
        assert!(resp.pages[0].markdown.starts_with("<!-- PageHeader=\"LiteOCR sample\" -->"));
        assert!(resp.pages[0].markdown.contains("# Hello LiteOCR"));
        assert!(!resp.pages[0].markdown.contains("PageBreak"));
        assert!(resp.pages[1].markdown.starts_with("## Page Two"));
        // Page header / footer comments are unwrapped to their text in `Page.text`.
        assert!(resp.pages[0].text.starts_with("LiteOCR sample"), "{}", resp.pages[0].text);
        assert!(!resp.pages[0].text.contains("<!--"));
        assert!(resp.pages[0].text.trim_end().ends_with("Page 1"));
        assert_eq!(resp.pages[0].width, Some(8.5));
        assert_eq!(resp.pages[0].height, Some(11.0));
        assert!(resp.markdown.contains("Reference: ABC-9876"));

        let types: Vec<BlockType> = resp.pages[0].blocks.iter().map(|b| b.block_type).collect();
        assert_eq!(
            types,
            vec![
                BlockType::Header,
                BlockType::Title,
                BlockType::Text,
                BlockType::Text,
                BlockType::Text,
                BlockType::Table,
                BlockType::Footer,
            ]
        );
        assert_eq!(resp.pages[1].blocks[0].block_type, BlockType::SectionHeader);

        // Table cells are rendered as markdown, and their paragraphs are not repeated as blocks.
        let table = resp.pages[0].blocks.iter().find(|b| b.block_type == BlockType::Table).unwrap();
        assert_eq!(table.content, "| Item | Amount |\n| --- | --- |\n| Widget | $56.78 |");
        assert!(!resp.pages[0].blocks.iter().any(|b| b.block_type != BlockType::Table && b.content == "Widget"));

        // Polygons are in inches here; boxes are normalised by the page size.
        let title = &resp.pages[0].blocks[1];
        let bb = title.bbox.unwrap();
        assert!((bb.x0 - 1.0 / 8.5).abs() < 1e-6, "{bb:?}");
        assert!((bb.y1 - 1.5 / 11.0).abs() < 1e-6, "{bb:?}");
        // Confidence is the mean of the words inside the paragraph's span.
        assert!((title.confidence.unwrap() - 0.9895).abs() < 1e-6, "{:?}", title.confidence);
        assert!(resp.pages[1].blocks.iter().all(|b| b.confidence.is_some()));
    }

    #[test]
    fn text_output_strips_markdown() {
        let resp = normalize_parse(&layout(), OutputFormat::Text, "layout");
        assert!(!resp.pages[0].markdown.contains("# Hello"), "{}", resp.pages[0].markdown);
        assert!(resp.pages[0].markdown.contains("Hello LiteOCR"));
        assert!(resp.pages[0].markdown.contains("Invoice #1234"), "plain '#' in text is kept");
        assert_eq!(resp.pages[0].markdown, resp.pages[0].text);
        let table = resp.pages[0].blocks.iter().find(|b| b.block_type == BlockType::Table).unwrap();
        assert_eq!(table.content, "Item Amount\nWidget $56.78");
    }

    #[test]
    fn normalizes_read_fixture_natively() {
        let resp = normalize_ocr(&read(), "read");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages[0].text, "Hello LiteOCR\nInvoice #1234");
        assert_eq!(resp.pages[0].lines.len(), 2);
        assert_eq!(resp.pages[0].words.len(), 4);
        assert_eq!(resp.pages[0].words[0].text, "Hello");
        // Image pages are in pixels: 1700x2200.
        let bb = resp.pages[0].words[0].bbox.unwrap();
        assert!((bb.x0 - 100.0 / 1700.0).abs() < 1e-6, "{bb:?}");
        assert_eq!(resp.pages[0].words[0].confidence, Some(0.995));
        // Line confidence is averaged from its words.
        assert!((resp.pages[0].lines[0].confidence.unwrap() - 0.9915).abs() < 1e-6);
        assert_eq!(resp.text, "Hello LiteOCR\nInvoice #1234\n\nPage Two\nReference: ABC-9876");
    }

    #[test]
    fn ocr_from_layout_uses_lines_not_blocks() {
        let resp = normalize_ocr(&layout(), "layout");
        assert_eq!(resp.pages.len(), 2);
        assert!(resp.pages[0].lines.len() >= 3);
        assert!(resp.pages[0].words.iter().all(|w| w.bbox.is_some()));
        assert!(resp.text.contains("Hello LiteOCR"));
    }

    #[test]
    fn normalizes_invoice_fields_with_citations() {
        let resp = normalize_extract(&invoice(), &json!({}), "invoice").unwrap();
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.metadata["azure_doc_type"], "invoice");
        assert_eq!(resp.data["VendorName"], "LiteOCR Supplies Ltd");
        assert_eq!(resp.data["InvoiceId"], "1234");
        assert_eq!(resp.data["InvoiceDate"], "2026-09-11");
        assert_eq!(resp.data["InvoiceTotal"]["amount"], 56.78);
        assert_eq!(resp.data["InvoiceTotal"]["currencyCode"], "USD");
        assert_eq!(resp.data["Items"][0]["Description"], "Widget");
        assert_eq!(resp.data["Items"][0]["Amount"]["amount"], 56.78);

        let total = &resp.fields["/InvoiceTotal"];
        assert_eq!(total.confidence, Some(0.962));
        assert_eq!(total.citations[0].page_number, 1);
        let bb = total.citations[0].bbox.unwrap();
        assert!((bb.x0 - 5.0 / 8.5).abs() < 1e-6, "{bb:?}");
        assert_eq!(total.citations[0].text.as_deref(), Some("$56.78"));
        assert!(resp.fields.contains_key("/Items/0/Description"));
        assert_eq!(resp.metadata["azure_schema_selected_fields"], false);
    }

    #[test]
    fn schema_selects_and_renames_fields() {
        let schema = json!({
            "type": "object",
            "properties": { "invoice_id": { "type": "string" }, "invoice_total": { "type": "object" } }
        });
        let resp = normalize_extract(&invoice(), &schema, "invoice").unwrap();
        assert_eq!(resp.data.as_object().unwrap().len(), 2);
        assert_eq!(resp.data["invoice_id"], "1234");
        assert_eq!(resp.data["invoice_total"]["amount"], 56.78);
        assert_eq!(resp.fields["/invoice_total"].confidence, Some(0.962));
        assert!(!resp.fields.contains_key("/InvoiceTotal"));
        assert_eq!(resp.metadata["azure_schema_selected_fields"], true);

        // A schema that matches nothing falls back to everything Azure returned.
        let none = normalize_extract(&invoice(), &json!({"properties": {"nope": {}}}), "invoice").unwrap();
        assert!(none.data["VendorName"].is_string());
        assert_eq!(none.metadata["azure_schema_selected_fields"], false);
    }

    #[test]
    fn extract_falls_back_to_key_value_pairs() {
        let result: AnalyzeResult = serde_json::from_value(json!({
            "pages": [{"pageNumber": 1, "width": 8.5, "height": 11, "unit": "inch"}],
            "keyValuePairs": [{
                "key": {"content": "Invoice Number", "boundingRegions": [], "spans": []},
                "value": {"content": "1234", "boundingRegions": [{"pageNumber": 1, "polygon": [1,1,2,1,2,2,1,2]}],
                          "spans": []},
                "confidence": 0.9
            }]
        }))
        .unwrap();
        let resp = normalize_extract(&result, &json!({}), "custom").unwrap();
        assert_eq!(resp.data["Invoice Number"], "1234");
        assert_eq!(resp.fields["/Invoice Number"].confidence, Some(0.9));
        assert_eq!(resp.fields["/Invoice Number"].citations[0].page_number, 1);
    }

    #[test]
    fn extract_without_documents_errors() {
        let result = AnalyzeResult::default();
        let err = normalize_extract(&result, &json!({}), "invoice").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Provider);
        assert!(err.to_string().contains("no documents"), "{err}");
    }

    #[test]
    fn model_ids_and_overrides() {
        let req = DocumentRequest::from_url("https://x/y.pdf");
        assert_eq!(model_id(&req, "layout").unwrap(), "prebuilt-layout");
        assert_eq!(model_id(&req, "read").unwrap(), "prebuilt-read");
        assert_eq!(model_id(&req, "id_document").unwrap(), "prebuilt-idDocument");
        assert_eq!(model_id(&req, "tax_us_w2").unwrap(), "prebuilt-tax.us.w2");
        assert!(model_id(&req, "custom").is_err());
        assert!(model_id(&req, "nope").is_err());
        let custom = req.clone().provider_options(json!({"model_id": "my-contracts-v3"}));
        assert_eq!(model_id(&custom, "custom").unwrap(), "my-contracts-v3");
        assert_eq!(model_id(&custom, "layout").unwrap(), "my-contracts-v3");
    }

    #[test]
    fn query_params_cover_pages_language_and_options() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .pages("1-3,7,10-")
            .language("en-US")
            .provider_options(json!({"features": ["ocrHighResolution"], "query_fields": ["StoreNumber"],
                                     "output": ["figures"]}));
        let params = query_params(&req, Mode::Parse).unwrap();
        let get = |k: &str| params.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("api-version"), Some(API_VERSION));
        assert_eq!(get("outputContentFormat"), Some("markdown"));
        assert_eq!(get("stringIndexType"), Some("unicodeCodePoint"));
        assert_eq!(get("pages"), Some("1-3,7,10-2000"));
        assert_eq!(get("locale"), Some("en-US"));
        assert_eq!(get("features"), Some("ocrHighResolution,queryFields"));
        assert_eq!(get("queryFields"), Some("StoreNumber"));
        assert_eq!(get("output"), Some("figures"));

        // `ocr` and `extract` do not ask for markdown.
        let plain = query_params(&DocumentRequest::from_url("https://x/y.pdf"), Mode::Ocr).unwrap();
        assert_eq!(plain.iter().find(|(k, _)| k == "outputContentFormat").map(|(_, v)| v.as_str()), Some("text"));
        let bad = DocumentRequest::from_url("https://x/y.pdf").pages("oops");
        assert_eq!(query_params(&bad, Mode::Parse).unwrap_err().kind, ErrorKind::Input);
    }

    #[test]
    fn query_string_is_percent_encoded() {
        let params =
            vec![("api-version".to_string(), API_VERSION.to_string()), ("pages".to_string(), "1-3,7".to_string())];
        assert_eq!(query_string(&params), "api-version=2024-11-30&pages=1-3%2C7");
    }

    #[test]
    fn failed_operations_map_to_error_kinds() {
        let failed = |code: &str| {
            let op: AnalyzeOperation = serde_json::from_value(json!({
                "status": "failed",
                "error": {"code": code, "message": "nope", "innererror": {"code": "X", "message": "why"}}
            }))
            .unwrap();
            operation_error(&op)
        };
        assert_eq!(failed("InvalidContent").kind, ErrorKind::BadRequest);
        assert_eq!(failed("UnsupportedMediaType").kind, ErrorKind::BadRequest);
        assert_eq!(failed("ContentSourceTimeout").kind, ErrorKind::BadRequest);
        let internal = failed("InternalServerError");
        assert_eq!(internal.kind, ErrorKind::Provider);
        assert!(internal.to_string().contains("InternalServerError: nope (X: why)"), "{internal}");
        let bare: AnalyzeOperation = serde_json::from_value(json!({"status": "failed"})).unwrap();
        assert_eq!(operation_error(&bare).kind, ErrorKind::Provider);
    }

    #[test]
    fn http_errors_use_the_azure_error_envelope() {
        let body = r#"{"error":{"code":"InvalidRequest","message":"Invalid request.",
                       "innererror":{"code":"InvalidContent","message":"The file is corrupted."}}}"#;
        let err = Error::from_http(NAME, 400, body);
        assert_eq!(err.kind, ErrorKind::BadRequest);
        assert_eq!(err.message, "Invalid request.");
        assert_eq!(
            Error::from_http(NAME, 401, r#"{"error":{"code":"401","message":"Access denied"}}"#).kind,
            ErrorKind::Authentication
        );
        assert_eq!(Error::from_http(NAME, 429, "").kind, ErrorKind::RateLimit);
    }

    #[test]
    fn result_id_comes_from_the_operation_location() {
        let url = "https://x.cognitiveservices.azure.com/documentintelligence/documentModels/prebuilt-layout/\
                   analyzeResults/3b31320d-8bab-4f88-b19c-2322a7f11034?api-version=2024-11-30";
        assert_eq!(result_id(url), "3b31320d-8bab-4f88-b19c-2322a7f11034");
        assert_eq!(result_id("no-slashes"), "no-slashes");
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0u8, 255, 17]), "AP8R");
    }

    #[test]
    fn endpoint_override_wins_and_is_trimmed() {
        // `base_url` wins over the environment, so this case is deterministic.
        let req = DocumentRequest::from_url("https://x/y.pdf").base_url("https://my-di.cognitiveservices.azure.com/");
        assert_eq!(resolve_endpoint(&req).unwrap(), "https://my-di.cognitiveservices.azure.com");
    }

    // ---- live tests (opt-in) -------------------------------------------------------------------
    //
    // `cargo test -p puffinparse-core azure -- --ignored` with both env vars set.

    const SAMPLE_PDF: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");
    const SAMPLE_INVOICE: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/invoice_001.png");

    fn live_keys_present() -> bool {
        for var in [ENV_KEY, ENV_ENDPOINT] {
            if std::env::var(var).map(|v| v.trim().is_empty()).unwrap_or(true) {
                eprintln!("skipping azure live test: {var} not set");
                return false;
            }
        }
        true
    }

    #[tokio::test]
    #[ignore = "needs AZURE_DOCUMENT_INTELLIGENCE_KEY + _ENDPOINT and network"]
    async fn azure_layout_live() {
        if !live_keys_present() {
            return;
        }
        let req = DocumentRequest::from_path(SAMPLE_PDF).timeout_secs(240.0);
        let resp = Azure.parse(&req, "layout").await.expect("layout parse succeeds");
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.pages.len(), 2);
        assert!(!resp.markdown.trim().is_empty());
        assert!(resp.pages.iter().any(|p| !p.blocks.is_empty()));
        assert!(resp.provider_job_id.is_some());
    }

    #[tokio::test]
    #[ignore = "needs AZURE_DOCUMENT_INTELLIGENCE_KEY + _ENDPOINT and network"]
    async fn azure_read_live() {
        if !live_keys_present() {
            return;
        }
        let req = DocumentRequest::from_path(SAMPLE_PDF).timeout_secs(240.0);
        let resp = Azure.ocr(&req, "read").await.expect("read ocr succeeds");
        assert_eq!(resp.usage.pages, 2);
        assert!(resp.pages[0].words.iter().any(|w| w.bbox.is_some() && w.confidence.is_some()));
        assert!(!resp.text.trim().is_empty());
    }

    #[tokio::test]
    #[ignore = "needs AZURE_DOCUMENT_INTELLIGENCE_KEY + _ENDPOINT and network"]
    async fn azure_invoice_live() {
        if !live_keys_present() {
            return;
        }
        let doc = DocumentRequest::from_path(SAMPLE_INVOICE).timeout_secs(240.0);
        let schema = json!({"type": "object", "properties": {"InvoiceId": {"type": "string"},
                                                             "InvoiceTotal": {"type": "object"}}});
        let resp = Azure.extract(&ExtractRequest::new(doc, schema), "invoice").await.expect("invoice extract succeeds");
        assert!(resp.data.is_object());
        assert!(resp.usage.pages >= 1);
    }
}
