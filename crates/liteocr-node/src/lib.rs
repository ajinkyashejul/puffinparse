//! N-API bindings: the native addon behind the `liteocr` npm package (`js/`).
//!
//! This layer mirrors `crates/liteocr-python`: it only converts values and bridges runtimes, and
//! every behaviour (model resolution, providers, pricing, scoring, native-format rendering) stays
//! in `liteocr-core`. Requests arrive as the core's snake_case serde JSON (the TypeScript wrapper
//! in `js/index.js` builds them from camelCase options) and responses leave the same way; the
//! wrapper converts them to camelCase objects typed by `js/index.d.ts`.
//!
//! Errors: every core error is thrown as a JS `Error` whose message is [`CORE_ERROR_PREFIX`]
//! followed by the JSON-encoded [`liteocr_core::Error`], so the wrapper can rebuild the typed
//! `LiteOCRError` subclass (`kind`, `provider`, `statusCode`, `jobId`, `retryable`) losslessly.

// `deny`, not `forbid`: the `#[napi]` macro registers exports through `ctor`, whose expansion
// carries `#[allow(unsafe_code)]`, which `forbid` rejects. Hand-written unsafe is still an error.
#![deny(unsafe_code)]

use liteocr_core::compat::Format;
use liteocr_core::router::{Router as CoreRouter, RouterConfig, Strategy};
use liteocr_core::{
    DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, JobHandle, JobStatus, Mode, ParseResponse,
    RetrieveOptions,
};
use napi::bindgen_prelude::Buffer;
use napi::{Error as NapiError, Result, Status};
use napi_derive::napi;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Marker that starts the message of every error that came from the core.
pub const CORE_ERROR_PREFIX: &str = "LITEOCR_CORE_ERROR:";

fn core_err(e: liteocr_core::Error) -> NapiError {
    let json = serde_json::to_string(&e).unwrap_or_else(|_| e.to_string());
    NapiError::new(Status::GenericFailure, format!("{CORE_ERROR_PREFIX}{json}"))
}

fn invalid_arg(message: String) -> NapiError {
    NapiError::new(Status::InvalidArg, message)
}

fn parse_mode(mode: &str) -> Result<Mode> {
    mode.parse::<Mode>().map_err(core_err)
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| NapiError::from_reason(format!("failed to serialise response: {e}")))
}

/// Replace the document input with separately-passed bytes, keeping the declared filename.
///
/// Bytes travel as a `Buffer` rather than base64 inside the request JSON, like the Python bridge.
fn attach_bytes(input: &mut DocumentInput, data: &[u8]) {
    let filename = match &input {
        DocumentInput::Bytes { filename, .. } => filename.clone(),
        other => other.filename(),
    };
    *input = DocumentInput::Bytes { data: bytes::Bytes::copy_from_slice(data), filename };
}

fn build_request(request: Value, data: Option<&[u8]>) -> Result<DocumentRequest> {
    let mut req: DocumentRequest =
        serde_json::from_value(request).map_err(|e| invalid_arg(format!("invalid request: {e}")))?;
    if let Some(bytes) = data {
        attach_bytes(&mut req.input, bytes);
    }
    Ok(req)
}

fn build_extract_request(request: Value, data: Option<&[u8]>) -> Result<ExtractRequest> {
    let mut req: ExtractRequest =
        serde_json::from_value(request).map_err(|e| invalid_arg(format!("invalid extract request: {e}")))?;
    if let Some(bytes) = data {
        attach_bytes(&mut req.document.input, bytes);
    }
    Ok(req)
}

// ---- the three modes -----------------------------------------------------------------------------

/// Run a `parse`-mode request. `request` matches the core's `DocumentRequest` JSON.
#[napi]
pub async fn parse(request: Value, data: Option<Buffer>) -> Result<Value> {
    let req = build_request(request, data.as_deref())?;
    let resp = liteocr_core::parse(req).await.map_err(core_err)?;
    to_value(&resp)
}

/// Run an `ocr`-mode request; resolves to a `TextResponse` JSON object.
#[napi]
pub async fn ocr(request: Value, data: Option<Buffer>) -> Result<Value> {
    let req = build_request(request, data.as_deref())?;
    let resp = liteocr_core::ocr(req).await.map_err(core_err)?;
    to_value(&resp)
}

/// Run an `extract`-mode request (`DocumentRequest` fields flattened with `schema`,
/// `instructions`, `citations`); resolves to an `ExtractResponse` JSON object.
#[napi]
pub async fn extract(request: Value, data: Option<Buffer>) -> Result<Value> {
    let req = build_extract_request(request, data.as_deref())?;
    let resp = liteocr_core::extract(req).await.map_err(core_err)?;
    to_value(&resp)
}

// ---- asynchronous jobs (SPEC §15) ----------------------------------------------------------------

fn build_job(job: Value) -> Result<JobHandle> {
    serde_json::from_value(job).map_err(|e| invalid_arg(format!("invalid job: {e}")))
}

fn retrieve_options(options: Option<Value>) -> Result<RetrieveOptions> {
    match options {
        None | Some(Value::Null) => Ok(RetrieveOptions::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| invalid_arg(format!("invalid retrieve options: {e}"))),
    }
}

/// `{"status": "pending"}` or `{"status": "succeeded", "result": <ParseResponse>}`; a failed job
/// rejects with the core error (job id set), like a failed `parse`.
fn job_status_value(status: JobStatus) -> Result<Value> {
    match status {
        JobStatus::Failed(e) => Err(core_err(e)),
        other => to_value(&other),
    }
}

/// Start a `parse` job (`request` may carry `webhook_url`); resolves to the `JobHandle` JSON.
#[napi]
pub async fn submit(request: Value, data: Option<Buffer>) -> Result<Value> {
    let req = build_request(request, data.as_deref())?;
    let job = liteocr_core::submit_parse(req).await.map_err(core_err)?;
    to_value(&job)
}

/// Check a submitted job once. `options` matches the core's `RetrieveOptions` JSON.
#[napi]
pub async fn retrieve(job: Value, options: Option<Value>) -> Result<Value> {
    let job = build_job(job)?;
    let opts = retrieve_options(options)?;
    let status = liteocr_core::retrieve_parse_with(&job, &opts).await.map_err(core_err)?;
    job_status_value(status)
}

/// Interpret a provider webhook body without any network call: `{"job": <JobHandle> | null,
/// "status": {"status": "pending" | "finished" | "succeeded" | "failed", "result"?}}`.
#[napi]
pub fn parse_webhook(model: String, payload: Value) -> Result<Value> {
    let event = liteocr_core::parse_webhook(&model, &payload).map_err(core_err)?;
    to_value(&event)
}

// ---- router --------------------------------------------------------------------------------------

/// Multi-model router with fallbacks, bound to one mode.
#[napi(js_name = "NativeRouter")]
pub struct NativeRouter {
    inner: Arc<CoreRouter>,
}

#[napi]
impl NativeRouter {
    /// `fallback_on` holds `ErrorKind` values (`"provider"`, `"rate_limit"`, ...).
    #[napi(constructor)]
    pub fn new(models: Vec<String>, mode: String, strategy: String, fallback_on: Option<Vec<String>>) -> Result<Self> {
        let strategy: Strategy = strategy.parse().map_err(core_err)?;
        let mut config = RouterConfig::new(models).mode(parse_mode(&mode)?);
        config.strategy = strategy;
        if let Some(kinds) = fallback_on {
            config.fallback_on = kinds
                .into_iter()
                .map(|k| serde_json::from_value(Value::String(k)))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| invalid_arg(format!("invalid fallbackOn: {e}")))?;
        }
        Ok(Self { inner: Arc::new(CoreRouter::new(config).map_err(core_err)?) })
    }

    #[napi]
    pub fn models(&self) -> Vec<String> {
        self.inner.models()
    }

    #[napi]
    pub fn mode(&self) -> String {
        self.inner.mode().as_str().to_string()
    }

    /// The order models would be tried for the next call (advances round-robin).
    #[napi]
    pub fn plan(&self) -> Vec<String> {
        self.inner.plan()
    }

    /// Per-model counters keyed by qualified model name.
    #[napi]
    pub fn stats(&self) -> Result<Value> {
        to_value(&self.inner.stats())
    }

    #[napi]
    pub async fn parse(&self, request: Value, data: Option<Buffer>) -> Result<Value> {
        let req = build_request(request, data.as_deref())?;
        let router = self.inner.clone();
        let resp = router.parse(&req).await.map_err(core_err)?;
        to_value(&resp)
    }

    #[napi]
    pub async fn ocr(&self, request: Value, data: Option<Buffer>) -> Result<Value> {
        let req = build_request(request, data.as_deref())?;
        let router = self.inner.clone();
        let resp = router.ocr(&req).await.map_err(core_err)?;
        to_value(&resp)
    }

    #[napi]
    pub async fn extract(&self, request: Value, data: Option<Buffer>) -> Result<Value> {
        let req = build_extract_request(request, data.as_deref())?;
        let router = self.inner.clone();
        let resp = router.extract(&req).await.map_err(core_err)?;
        to_value(&resp)
    }
}

// ---- native-format compatibility -----------------------------------------------------------------

/// Every value `outputFormat` accepts, in documentation order.
#[napi]
pub fn output_formats() -> Vec<&'static str> {
    Format::ALL.iter().map(Format::as_str).collect()
}

/// Canonicalise an `outputFormat` before any provider call.
#[napi]
pub fn validate_output_format(format: String) -> Result<String> {
    format.parse::<Format>().map(|f| f.as_str().to_string()).map_err(core_err)
}

/// Render a unified `ParseResponse` JSON object in a provider's native shape.
#[napi]
pub fn render_parse(response: Value, format: String) -> Result<Value> {
    let resp: ParseResponse =
        serde_json::from_value(response).map_err(|e| invalid_arg(format!("invalid parse response: {e}")))?;
    resp.to_format(&format).map_err(core_err)
}

/// Render a unified `ExtractResponse` JSON object in a provider's native shape.
#[napi]
pub fn render_extract(response: Value, format: String) -> Result<Value> {
    let resp: ExtractResponse =
        serde_json::from_value(response).map_err(|e| invalid_arg(format!("invalid extract response: {e}")))?;
    resp.to_format(&format).map_err(core_err)
}

// ---- registry, pricing, helpers ------------------------------------------------------------------

/// Version of the compiled core.
#[napi]
pub fn version() -> &'static str {
    liteocr_core::VERSION
}

/// All fully-qualified model names, optionally only those serving `mode`.
#[napi]
pub fn list_models(mode: Option<String>) -> Result<Vec<String>> {
    match mode {
        Some(m) => Ok(liteocr_core::list_models_for(parse_mode(&m)?)),
        None => Ok(liteocr_core::list_models()),
    }
}

/// The known modes, in order.
#[napi]
pub fn modes() -> Vec<&'static str> {
    Mode::ALL.iter().map(Mode::as_str).collect()
}

#[napi]
pub fn providers() -> Result<Value> {
    to_value(&liteocr_core::PROVIDERS)
}

#[napi]
pub fn pricing() -> Result<Value> {
    to_value(&liteocr_core::pricing::all_prices())
}

#[napi]
pub fn set_pricing(prices: HashMap<String, f64>, mode: String) -> Result<()> {
    let mode = parse_mode(&mode)?;
    liteocr_core::pricing::set_prices(prices.into_iter().collect(), mode);
    Ok(())
}

#[napi]
pub fn reset_pricing() {
    liteocr_core::pricing::reset_prices();
}

#[napi]
pub fn estimate_cost(model: String, mode: String, pages: u32) -> Result<Option<f64>> {
    Ok(liteocr_core::pricing::estimate_cost(&model, parse_mode(&mode)?, pages))
}

/// Validate and canonicalise a model string; with `mode`, the model must serve it.
#[napi]
pub fn resolve_model(model: String, mode: Option<String>) -> Result<String> {
    let parsed = match mode {
        Some(m) => liteocr_core::ModelRef::parse_for(&model, parse_mode(&m)?),
        None => liteocr_core::ModelRef::parse(&model),
    };
    parsed.map(|m| m.qualified()).map_err(core_err)
}

fn normalize_options(
    case_insensitive: Option<bool>,
    strip_markdown: Option<bool>,
    strip_punctuation: Option<bool>,
) -> liteocr_core::bench::NormalizeOptions {
    liteocr_core::bench::NormalizeOptions {
        case_insensitive: case_insensitive.unwrap_or(true),
        strip_markdown: strip_markdown.unwrap_or(true),
        strip_punctuation: strip_punctuation.unwrap_or(false),
    }
}

/// Benchmark metrics between a prediction and ground truth.
#[napi]
pub fn score(
    prediction: String,
    truth: String,
    case_insensitive: Option<bool>,
    strip_markdown: Option<bool>,
    strip_punctuation: Option<bool>,
) -> Result<Value> {
    let opts = normalize_options(case_insensitive, strip_markdown, strip_punctuation);
    to_value(&liteocr_core::bench::score(&prediction, &truth, opts))
}

/// The normalisation applied before scoring.
#[napi]
pub fn normalize_text(
    text: String,
    case_insensitive: Option<bool>,
    strip_markdown: Option<bool>,
    strip_punctuation: Option<bool>,
) -> String {
    liteocr_core::bench::normalize(&text, normalize_options(case_insensitive, strip_markdown, strip_punctuation))
}

#[napi]
pub fn markdown_to_text(markdown: String) -> String {
    liteocr_core::types::markdown_to_text(&markdown)
}

/// Enable core tracing output on stderr at the given level (e.g. "info", "debug").
#[napi]
pub fn init_logging(level: String) -> Result<()> {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_new(level).map_err(|e| invalid_arg(e.to_string()))?;
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_errors_round_trip_through_the_message() {
        let err = core_err(liteocr_core::Error::unsupported_model("nope").with_provider("x"));
        let msg = err.reason.clone();
        let json = msg.strip_prefix(CORE_ERROR_PREFIX).expect("prefixed");
        let back: liteocr_core::Error = serde_json::from_str(json).expect("json");
        assert_eq!(back.kind, liteocr_core::ErrorKind::UnsupportedModel);
        assert_eq!(back.provider.as_deref(), Some("x"));
    }

    #[test]
    fn bytes_replace_the_input_and_keep_the_filename() {
        let req = serde_json::json!({
            "input": {"kind": "bytes", "data": "", "filename": "a.pdf"},
            "model": "reducto",
        });
        let req = build_request(req, Some(b"%PDF")).expect("request");
        match req.input {
            DocumentInput::Bytes { data, filename } => {
                assert_eq!(&data[..], b"%PDF");
                assert_eq!(filename, "a.pdf");
            }
            other => panic!("unexpected input {other:?}"),
        }
    }

    #[test]
    fn jobs_round_trip_and_failed_status_is_an_error() {
        let job = build_job(serde_json::json!({
            "provider": "reducto", "model": "reducto/standard", "job_id": "j1", "submitted_at": "",
        }))
        .expect("job");
        assert_eq!(job.job_id, "j1");
        assert!(build_job(serde_json::json!({ "job_id": "j1" })).is_err());
        let opts = retrieve_options(Some(serde_json::json!({ "api_key": "k", "timeout_secs": 5.0 }))).expect("opts");
        assert_eq!((opts.api_key.as_deref(), opts.timeout_secs, opts.max_retries), (Some("k"), 5.0, 2));
        assert_eq!(job_status_value(JobStatus::Pending).unwrap(), serde_json::json!({ "status": "pending" }));
        let err = job_status_value(JobStatus::Failed(liteocr_core::Error::provider("boom").with_job_id("j1")))
            .expect_err("failed jobs reject");
        assert!(err.reason.starts_with(CORE_ERROR_PREFIX) && err.reason.contains("\"job_id\":\"j1\""));
    }

    #[test]
    fn extract_request_flattens_document_fields() {
        let req = serde_json::json!({
            "input": {"kind": "url", "url": "https://example.com/a.pdf"},
            "model": "reducto/extract",
            "schema": {"type": "object"},
            "citations": true,
        });
        let req = build_extract_request(req, None).expect("request");
        assert!(req.citations);
        assert_eq!(req.document.model, "reducto/extract");
    }
}
