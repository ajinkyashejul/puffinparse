//! OpenDocRouter (<https://www.opendocrouter.ai>): LlamaIndex's hosted router that runs one
//! document-parsing model per call (frontier VLMs and open OCR models) behind a single API.
//!
//! Model strings carry the router's own vendor-qualified id after the provider name:
//! `opendocrouter/google/gemini-3-flash` → `{"model": "google/gemini-3-flash"}`.
//!
//! Sync flow (≤ 50 pages): `POST /v1/parse` with `{model, document, layout: true, pages?}` → 200 with
//! every page. `document` is `{"data", "mime_type"}` inline for files up to ~3 MB, or
//! `{"upload_id"}` after `POST /v1/uploads` + `PUT upload_url` for larger files, or `{"url"}`.
//!
//! Async flow (> 50 pages, `provider_options.mode = "async"`, or a sync request refused with 413):
//! the same body with `mode: "async", cache: true` → 202 `{id, status: "processing"}` → poll
//! `GET /v1/parse/{id}` until the status changes → `GET /v1/parse/{id}?expand=markdown[,layout]`,
//! following `next_cursor` while `has_more`.
//!
//! Jobs API (`crate::submit_parse` / `retrieve_parse`): the async submission, then one status check
//! (plus the expanded result once finished) per retrieve. OpenDocRouter sends no webhooks.
//!
//! Every page carries its own status and charge. `status: "partial"` returns the pages that worked;
//! the failed ones are listed in `metadata.opendocrouter_failed_pages`. When no page worked the call
//! fails with an error built from the page errors.

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::jobs::{JobHandle, JobStatus, WebhookEvent};
use crate::provider::{self, Provider};
use crate::providers::vlm;
use crate::types::{
    markdown_to_text, BBox, Block, BlockType, DocumentInput, DocumentRequest, OutputFormat, Page, ParseResponse, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub const NAME: &str = "opendocrouter";
const ENV_KEY: &str = "OPEN_DOC_ROUTER_API_KEY";
const ENV_BASE: &str = "OPEN_DOC_ROUTER_BASE_URL";
const DEFAULT_BASE: &str = "https://www.opendocrouter.ai";
/// Inline documents are base64 inside a JSON body capped at 4 MB, so raw files above this go
/// through `POST /v1/uploads` instead (the docs say "about 3 MB").
pub(crate) const INLINE_MAX_BYTES: usize = 2_900_000;
/// `max_sync_pages` of every listed model (`GET /v1/models`, price version 2026-10-06).
const MAX_SYNC_PAGES: u32 = 50;
/// Upper bound on one `Retry-After` wait (`model_starting` asks for "a few minutes").
const MAX_RETRY_AFTER: Duration = Duration::from_secs(120);
/// Most result parts followed through `next_cursor` (each part is up to 4 MB).
const MAX_RESULT_PARTS: usize = 1000;
const ACCEPTED_TYPES: &[&str] = &["application/pdf", "image/png", "image/jpeg"];
/// `provider_options` keys PuffinParse consumes itself; everything else is merged into the body.
const LOCAL_OPTIONS: &[&str] = &["upload"];

#[derive(Debug, Default, Clone, Copy)]
pub struct OpenDocRouter;

#[async_trait]
impl Provider for OpenDocRouter {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let ctx = Ctx::new(request)?;
        let mut body = request_body(request, model)?;
        let data = provider::load_bytes(&request.input).await?;
        let explicit = explicit_mode(&body)?;
        let estimated = estimated_pages(body.get("pages").and_then(Value::as_str), data.as_deref());
        let run_async = match explicit {
            Some(mode) => mode == RunMode::Async,
            None => estimated.is_some_and(|n| n > MAX_SYNC_PAGES),
        };

        body["document"] = ctx.document(request, data.as_ref()).await?;
        if !run_async {
            match ctx.post_parse(&body).await {
                Ok(record) => return finish(record, request, model),
                // A sync request over the page limit is refused (and not charged) with 413
                // `too_large`. Page counts of URLs and compressed PDFs are unknown up front, so
                // the first refusal moves the call to the async flow.
                Err(e) if explicit.is_none() && e.status_code == Some(413) => {
                    tracing::info!(error = %e, "opendocrouter: sync request refused as too large; retrying async");
                    if body["document"].get("upload_id").is_some() {
                        // Uploads are single-use: the refused request may have consumed it.
                        body["document"] = ctx.document(request, data.as_ref()).await?;
                    }
                }
                Err(e) => return Err(e),
            }
        }

        make_async(&mut body)?;
        let accepted = ctx.post_parse(&body).await?;
        let id = record_id(&accepted)?;
        tracing::debug!(id = %id, "opendocrouter: async request accepted");
        if is_rejected(&accepted) {
            return finish(accepted, request, model);
        }
        http::poll_until(NAME, &ctx.deadline, Duration::from_secs(1), Duration::from_secs(10), || async {
            let record = ctx.get_record(&id, None, None).await?;
            Ok((status_of(&record) != "processing").then_some(record))
        })
        .await
        .map_err(|e| e.with_job_id(id.clone()))?;
        let full = ctx.results(&id, expand_for(layout_on(&body))).await.map_err(|e| e.with_job_id(id.clone()))?;
        finish(full, request, model)
    }

    /// `POST /v1/parse` with `mode: "async", cache: true`, returning the request id.
    async fn submit_parse(&self, request: &DocumentRequest, model: &str) -> Result<JobHandle> {
        if provider::webhook_url(request)?.is_some() {
            return Err(Error::input(
                "opendocrouter sends no webhooks; leave webhook_url unset and poll the job with retrieve_parse",
            )
            .with_provider(NAME));
        }
        let ctx = Ctx::new(request)?;
        let mut body = request_body(request, model)?;
        if explicit_mode(&body)? == Some(RunMode::Sync) {
            return Err(Error::input("opendocrouter: jobs always use mode \"async\"; drop provider_options.mode")
                .with_provider(NAME));
        }
        make_async(&mut body)?;
        let data = provider::load_bytes(&request.input).await?;
        body["document"] = ctx.document(request, data.as_ref()).await?;
        let accepted = ctx.post_parse(&body).await?;
        let id = record_id(&accepted)?;
        tracing::debug!(id = %id, "opendocrouter: job submitted");
        Ok(JobHandle::new(NAME, model, id).with_state("layout", json!(layout_on(&body))))
    }

    /// One `GET /v1/parse/{id}`; once it is no longer processing, the expanded results.
    async fn retrieve_parse(&self, job: &JobHandle, request: &DocumentRequest, model: &str) -> Result<JobStatus> {
        let ctx = Ctx::new(request)?;
        let id = job.job_id.as_str();
        let record = ctx.get_record(id, None, None).await.map_err(|e| e.with_job_id(id))?;
        if status_of(&record) == "processing" {
            return Ok(JobStatus::Pending);
        }
        let record = if is_rejected(&record) {
            record
        } else {
            // The handle records whether layout was requested; default to the provider default.
            let layout = request.option("layout").and_then(Value::as_bool).unwrap_or(true);
            ctx.results(id, expand_for(layout)).await.map_err(|e| e.with_job_id(id))?
        };
        Ok(match finish(record, request, model) {
            Ok(resp) => JobStatus::Succeeded(Box::new(resp)),
            Err(e) => JobStatus::Failed(e),
        })
    }

    fn parse_webhook(&self, _model: &str, _payload: &Value) -> Result<WebhookEvent> {
        Err(Error::input("opendocrouter sends no webhooks; poll the job with retrieve_parse").with_provider(NAME))
    }
}

// ---- request building ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Sync,
    Async,
}

/// The `POST /v1/parse` body without `document`: model, `layout: true`, pages, then
/// `provider_options` merged on top (a `null` value removes a default).
fn request_body(request: &DocumentRequest, model: &str) -> Result<Value> {
    let mut body = json!({ "model": model, "layout": true });
    if let Some(pages) = request.pages.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        crate::util::parse_page_ranges(pages)?;
        body["pages"] = json!(pages.replace(' ', ""));
    }
    if let Some(Value::Object(opts)) = &request.provider_options {
        let map = body.as_object_mut().expect("body is an object");
        for (k, v) in opts {
            if LOCAL_OPTIONS.contains(&k.as_str()) || k == "document" {
                continue;
            }
            if v.is_null() {
                map.remove(k);
            } else {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    for key in ["layout", "cache"] {
        if body.get(key).is_some_and(|v| !v.is_boolean()) {
            return Err(Error::input(format!("opendocrouter: provider_options.{key} must be true or false"))
                .with_provider(NAME));
        }
    }
    Ok(body)
}

fn explicit_mode(body: &Value) -> Result<Option<RunMode>> {
    match body.get("mode") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s == "sync" => Ok(Some(RunMode::Sync)),
        Some(Value::String(s)) if s == "async" => Ok(Some(RunMode::Async)),
        Some(other) => Err(Error::input(format!(
            "opendocrouter: provider_options.mode must be \"sync\" or \"async\", got {other}"
        ))
        .with_provider(NAME)),
    }
}

/// Async requests must store their results (`cache: true`): that is how they are read back.
fn make_async(body: &mut Value) -> Result<()> {
    if body.get("cache") == Some(&Value::Bool(false)) {
        return Err(Error::input(
            "opendocrouter: async requests need cache: true (results are stored encrypted for 24 hours); \
             remove provider_options.cache = false, or select at most 50 pages with `pages`",
        )
        .with_provider(NAME));
    }
    body["mode"] = json!("async");
    body["cache"] = json!(true);
    Ok(())
}

fn layout_on(body: &Value) -> bool {
    body.get("layout").and_then(Value::as_bool).unwrap_or(false)
}

fn expand_for(layout: bool) -> &'static str {
    if layout {
        "markdown,layout"
    } else {
        "markdown"
    }
}

/// Pages the request will cover, when it can be known without the provider: a closed `pages`
/// selection, else a best-effort count of a local PDF (`None` for URLs and unreadable PDFs).
fn estimated_pages(pages: Option<&str>, data: Option<&[u8]>) -> Option<u32> {
    if let Some(ranges) = pages.and_then(|p| crate::util::parse_page_ranges(p).ok()) {
        if ranges.iter().all(|(_, end)| end.is_some()) {
            return Some(ranges.iter().map(|(s, e)| e.unwrap_or(*s) - s + 1).sum());
        }
    }
    let data = data?;
    if data.starts_with(b"%PDF-") {
        vlm::pdf_page_count(data)
    } else {
        Some(1)
    }
}

/// The document's MIME type from its magic bytes; OpenDocRouter takes PDF, PNG and JPEG only.
fn document_mime(data: &[u8], input: &DocumentInput) -> Result<String> {
    let mime = vlm::sniff_mime(data, &input.mime_type());
    if ACCEPTED_TYPES.contains(&mime.as_str()) {
        Ok(mime)
    } else {
        Err(Error::input(format!(
            "opendocrouter accepts PDF, PNG and JPEG only; {} looks like {mime}",
            input.filename()
        ))
        .with_provider(NAME))
    }
}

/// Request ids are UUIDs; anything else is refused before it is spliced into a URL path.
fn check_id(id: &str) -> Result<()> {
    if !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        Ok(())
    } else {
        Err(Error::input(format!("opendocrouter: invalid request id '{id}'")).with_provider(NAME))
    }
}

fn record_id(record: &Value) -> Result<String> {
    let id = record.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    if id.is_empty() {
        return Err(Error::provider(format!(
            "parse response carried no id; body starts: {}",
            http::snippet(&record.to_string())
        ))
        .with_provider(NAME));
    }
    check_id(&id)?;
    Ok(id)
}

fn status_of(record: &Value) -> &str {
    record.get("status").and_then(Value::as_str).unwrap_or_default()
}

fn is_rejected(record: &Value) -> bool {
    matches!(status_of(record), "rejected" | "expired")
}

// ---- HTTP ------------------------------------------------------------------------------------------

/// Credentials, endpoint and limits resolved once per call.
struct Ctx {
    api_key: String,
    base: String,
    deadline: Deadline,
    retry: Retry,
    client: &'static reqwest::Client,
}

impl Ctx {
    fn new(request: &DocumentRequest) -> Result<Self> {
        Ok(Self {
            api_key: provider::resolve_api_key(request, ENV_KEY, NAME)?,
            base: provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE),
            deadline: Deadline::new(request.timeout_secs),
            retry: Retry::new(request.max_retries),
            client: http::client(),
        })
    }

    /// One authenticated JSON call with retries on 429 / 5xx / network errors. A `Retry-After`
    /// header (429, 503) is honoured before the next attempt, within the deadline.
    async fn call(&self, method: reqwest::Method, url: &str, body: Option<&Value>) -> Result<Value> {
        let attempts_left = AtomicU32::new(self.retry.max_retries);
        http::with_retry(NAME, self.retry, &self.deadline, || {
            let mut rb = self
                .client
                .request(method.clone(), url)
                .bearer_auth(&self.api_key)
                .header(reqwest::header::ACCEPT, "application/json")
                .timeout(self.deadline.request_timeout());
            if let Some(b) = body {
                rb = rb.json(b);
            }
            let attempts_left = &attempts_left;
            let deadline = &self.deadline;
            async move {
                match send_once(rb).await {
                    Ok(v) => Ok(v),
                    Err((err, retry_after)) => {
                        if http::is_transient(&err) {
                            let left = attempts_left.load(Ordering::Relaxed);
                            if left > 0 {
                                attempts_left.store(left - 1, Ordering::Relaxed);
                                if let Some(wait) = retry_after {
                                    let wait = wait.min(MAX_RETRY_AFTER).min(deadline.remaining());
                                    tracing::info!(?wait, "opendocrouter: honouring Retry-After");
                                    tokio::time::sleep(wait).await;
                                }
                            }
                        }
                        Err(err)
                    }
                }
            }
        })
        .await
    }

    async fn post_parse(&self, body: &Value) -> Result<Value> {
        self.call(reqwest::Method::POST, &format!("{}/v1/parse", self.base), Some(body)).await
    }

    async fn get_record(&self, id: &str, expand: Option<&str>, cursor: Option<i64>) -> Result<Value> {
        check_id(id)?;
        let mut url = format!("{}/v1/parse/{id}", self.base);
        let mut sep = '?';
        if let Some(expand) = expand {
            url.push_str(&format!("{sep}expand={expand}"));
            sep = '&';
        }
        if let Some(cursor) = cursor {
            url.push_str(&format!("{sep}cursor={cursor}"));
        }
        self.call(reqwest::Method::GET, &url, None).await
    }

    /// The finished request with its markdown (and layout), every part joined into one record.
    async fn results(&self, id: &str, expand: &str) -> Result<Value> {
        let mut merged = self.get_record(id, Some(expand), None).await?;
        for _ in 0..MAX_RESULT_PARTS {
            let more = merged.get("has_more").and_then(Value::as_bool).unwrap_or(false);
            let Some(cursor) = merged.get("next_cursor").and_then(Value::as_i64).filter(|_| more) else {
                return Ok(merged);
            };
            let next = self.get_record(id, Some(expand), Some(cursor)).await?;
            merge_part(&mut merged, next);
        }
        Err(Error::provider(format!("result of request {id} did not end after {MAX_RESULT_PARTS} parts"))
            .with_provider(NAME))
    }

    /// The `document` object: a URL as-is, small files inline (base64), larger ones uploaded.
    /// `provider_options.upload` (bool) forces either way.
    async fn document(&self, request: &DocumentRequest, data: Option<&bytes::Bytes>) -> Result<Value> {
        let Some(data) = data else {
            let DocumentInput::Url { url } = &request.input else {
                unreachable!("load_bytes returns bytes for non-URL inputs")
            };
            return Ok(json!({ "url": url }));
        };
        let mime = document_mime(data, &request.input)?;
        let upload = request.option("upload").and_then(Value::as_bool).unwrap_or(data.len() > INLINE_MAX_BYTES);
        if upload {
            Ok(json!({ "upload_id": self.upload(data).await? }))
        } else {
            Ok(json!({ "data": vlm::base64_encode(data), "mime_type": mime }))
        }
    }

    /// `POST /v1/uploads`, then `PUT` the bytes to the returned one-time URL (no API key: the URL
    /// authorises by itself).
    async fn upload(&self, data: &bytes::Bytes) -> Result<String> {
        let created = self.call(reqwest::Method::POST, &format!("{}/v1/uploads", self.base), None).await?;
        let up: UploadWire = serde_json::from_value(created.clone()).map_err(|e| {
            Error::provider(format!(
                "unexpected upload response: {e}; body starts: {}",
                http::snippet(&created.to_string())
            ))
            .with_provider(NAME)
        })?;
        if let Some(max) = up.max_bytes.filter(|&m| m > 0) {
            if data.len() as u64 > max {
                return Err(Error::input(format!(
                    "opendocrouter: the document is {} bytes; uploads take at most {max}",
                    data.len()
                ))
                .with_provider(NAME));
            }
        }
        let target = url::Url::parse(&up.upload_url)
            .ok()
            .filter(|u| matches!(u.scheme(), "https" | "http"))
            .ok_or_else(|| Error::provider("upload response carried no usable upload_url").with_provider(NAME))?;
        http::with_retry(NAME, self.retry, &self.deadline, || {
            let rb = self.client.put(target.clone()).timeout(self.deadline.request_timeout()).body(data.clone());
            async move {
                http::read_response(NAME, rb.send().await?).await.map_err(|mut e| {
                    e.message = format!("upload PUT failed: {}", e.message);
                    e
                })
            }
        })
        .await?;
        tracing::debug!(upload_id = %up.upload_id, bytes = data.len(), "opendocrouter: uploaded");
        Ok(up.upload_id)
    }
}

/// Send one request; on failure also return the `Retry-After` the provider asked for.
async fn send_once(rb: reqwest::RequestBuilder) -> std::result::Result<Value, (Error, Option<Duration>)> {
    let resp = rb.send().await.map_err(|e| (Error::from(e).with_provider(NAME), None))?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64);
    let request_id = resp.headers().get("x-request-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let text = resp.text().await.map_err(|e| (Error::from(e).with_provider(NAME), None))?;
    if (200..300).contains(&status) {
        serde_json::from_str(&text).map_err(|e| {
            let err = Error::provider(format!("unexpected response shape: {e}; body starts: {}", http::snippet(&text)))
                .with_provider(NAME);
            (err, None)
        })
    } else {
        Err((map_http_error(status, &text, retry_after, request_id.as_deref()), retry_after))
    }
}

/// Append a later result part's pages to the first, keeping the freshest totals.
fn merge_part(merged: &mut Value, next: Value) {
    let pages = next.get("pages").and_then(Value::as_array).cloned().unwrap_or_default();
    match merged.get_mut("pages").and_then(Value::as_array_mut) {
        Some(existing) => existing.extend(pages),
        None => merged["pages"] = Value::Array(pages),
    }
    for key in ["has_more", "next_cursor", "status", "usage", "charge_usd", "results_expire_at"] {
        if let Some(v) = next.get(key) {
            if !v.is_null() || matches!(key, "next_cursor" | "has_more") {
                merged[key] = v.clone();
            }
        }
    }
}

/// `{ "error": { "code", "message", … } }` → an [`Error`] whose message keeps the provider's code
/// and message, plus `Retry-After` and the `X-Request-Id` when present.
///
/// 401 / 403 (`unauthorized`, `account_paused`) → authentication; 429 → rate limit (retried);
/// 402 `insufficient_credits` and the other 4xx → bad request (not retried, no router fallback);
/// 5xx (`at_capacity`, `model_starting`, `internal_error`) → provider (retried).
fn map_http_error(status: u16, body: &str, retry_after: Option<Duration>, request_id: Option<&str>) -> Error {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let err = parsed.as_ref().and_then(|v| v.get("error")).filter(|e| e.is_object());
    let code = err.and_then(|e| e.get("code")).and_then(Value::as_str);
    let message = err.and_then(|e| e.get("message")).and_then(Value::as_str);
    let mut msg = match (code, message) {
        (Some(c), Some(m)) => format!("{c}: {m}"),
        (Some(c), None) => c.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => Error::from_http(NAME, status, body).message,
    };
    if code == Some("insufficient_credits") {
        let field = |k: &str| err.and_then(|e| e.get(k)).and_then(Value::as_f64);
        if let (Some(req), Some(avail)) = (field("required_usd"), field("available_usd")) {
            msg.push_str(&format!(" (required_usd: {req}, available_usd: {avail})"));
        }
    }
    if let Some(wait) = retry_after {
        msg.push_str(&format!(" (Retry-After: {}s)", wait.as_secs_f64()));
    }
    if let Some(id) = request_id {
        msg.push_str(&format!(" [X-Request-Id: {id}]"));
    }
    let kind = match status {
        401 | 403 => ErrorKind::Authentication,
        429 => ErrorKind::RateLimit,
        400..=499 => ErrorKind::BadRequest,
        _ => ErrorKind::Provider,
    };
    Error::new(kind, msg).with_provider(NAME).with_status(status)
}

/// Kind for a request-level `error_code` on a rejected / failed record (same vocabulary as HTTP).
fn code_kind(code: &str) -> ErrorKind {
    match code {
        "unauthorized" | "account_paused" => ErrorKind::Authentication,
        "rate_limited" => ErrorKind::RateLimit,
        "invalid_request"
        | "url_not_allowed"
        | "insufficient_credits"
        | "not_found"
        | "gone"
        | "results_not_stored"
        | "too_large"
        | "unsupported_type"
        | "unreadable_document" => ErrorKind::BadRequest,
        _ => ErrorKind::Provider,
    }
}

// ---- wire types ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct UploadWire {
    upload_id: String,
    upload_url: String,
    #[serde(default)]
    max_bytes: Option<u64>,
}

/// `ParseRecord` from the OpenAPI spec (`POST /v1/parse`, `GET /v1/parse/{id}`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ParseRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub price_version: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub page_count: Option<u32>,
    #[serde(default)]
    pub pages: Option<Vec<WirePage>>,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    #[serde(default)]
    pub charge_usd: Option<f64>,
    #[serde(default)]
    pub results_expire_at: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct TokenUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct WirePage {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub markdown: Option<String>,
    #[serde(default)]
    pub cached: Option<bool>,
    #[serde(default)]
    pub charge_usd: Option<f64>,
    #[serde(default)]
    pub error: Option<WireError>,
    #[serde(default)]
    pub layout: Option<WireLayout>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct WireError {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct WireLayout {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
    #[serde(default)]
    pub elements: Option<Vec<WireElement>>,
    #[serde(default)]
    pub error: Option<WireError>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct WireElement {
    #[serde(rename = "type", default)]
    pub kind: String,
    /// `[first, last]`, 0-based and inclusive, over the page markdown split on `"\n"`.
    #[serde(default)]
    pub lines: Option<Vec<i64>>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub boxes: Option<Vec<WireBox>>,
}

/// Fractions of the page from its top left; `r` is a clockwise angle about the centre, with
/// `x, y, w, h` the unrotated box.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub(crate) struct WireBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    #[allow(dead_code)]
    pub r: Option<f64>,
}

// ---- normalisation ---------------------------------------------------------------------------------

/// The 15 layout element types mapped onto the unified vocabulary.
fn map_block_type(t: &str) -> BlockType {
    match t {
        "title" => BlockType::Title,
        "section_header" => BlockType::SectionHeader,
        "text" | "code" | "form" | "key_value" => BlockType::Text,
        "list_item" => BlockType::List,
        "table" => BlockType::Table,
        "picture" | "chart" => BlockType::Figure,
        "formula" => BlockType::Formula,
        "caption" => BlockType::Caption,
        "footnote" => BlockType::Footnote,
        "page_header" => BlockType::Header,
        "page_footer" => BlockType::Footer,
        _ => BlockType::Other,
    }
}

/// The enclosing box of an element's pieces (unrotated boxes for slanted text).
fn union_bbox(boxes: &[WireBox]) -> Option<BBox> {
    let valid: Vec<&WireBox> = boxes
        .iter()
        .filter(|b| [b.x, b.y, b.w, b.h].iter().all(|v| v.is_finite()) && b.w >= 0.0 && b.h >= 0.0)
        .collect();
    if valid.is_empty() {
        return None;
    }
    let x0 = valid.iter().map(|b| b.x).fold(f64::INFINITY, f64::min);
    let y0 = valid.iter().map(|b| b.y).fold(f64::INFINITY, f64::min);
    let x1 = valid.iter().map(|b| b.x + b.w).fold(f64::NEG_INFINITY, f64::max);
    let y1 = valid.iter().map(|b| b.y + b.h).fold(f64::NEG_INFINITY, f64::max);
    Some(BBox { x0: x0.clamp(0.0, 1.0), y0: y0.clamp(0.0, 1.0), x1: x1.clamp(0.0, 1.0), y1: y1.clamp(0.0, 1.0) })
}

/// The markdown an element spans (`lines` over the markdown split on `"\n"` only).
fn element_markdown(lines: &[&str], range: Option<&[i64]>) -> String {
    let Some(&[first, last, ..]) = range else { return String::new() };
    if first < 0 || last < first || first as usize >= lines.len() {
        return String::new();
    }
    let last = (last as usize).min(lines.len() - 1);
    lines[first as usize..=last].join("\n").trim().to_string()
}

fn render(md: &str, fmt: OutputFormat) -> String {
    match fmt {
        OutputFormat::Markdown => md.to_string(),
        OutputFormat::Text => markdown_to_text(md),
    }
}

/// One successful page: layout elements become typed, boxed blocks; without a usable layout the
/// page is one geometry-free text block, as for the other markdown-only providers.
fn page_from_wire(p: &WirePage, fmt: OutputFormat) -> Page {
    let raw = p.markdown.as_deref().unwrap_or_default();
    let markdown = raw.trim();
    let text = markdown_to_text(markdown);
    match &p.layout {
        Some(layout) if layout.status == "ok" => {
            let lines: Vec<&str> = raw.split('\n').collect();
            let blocks = layout
                .elements
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|el| {
                    let content = element_markdown(&lines, el.lines.as_deref());
                    let bbox = union_bbox(el.boxes.as_deref().unwrap_or_default());
                    if content.is_empty() && bbox.is_none() {
                        return None;
                    }
                    Some(Block {
                        block_type: map_block_type(&el.kind),
                        text: Some(markdown_to_text(&content)),
                        content: render(&content, fmt),
                        bbox,
                        confidence: el.confidence,
                        page_number: p.page,
                    })
                })
                .collect();
            Page {
                page_number: p.page,
                width: layout.width,
                height: layout.height,
                markdown: render(markdown, fmt),
                text,
                blocks,
            }
        }
        _ if markdown.is_empty() => {
            Page { page_number: p.page, width: None, height: None, markdown: String::new(), text, blocks: vec![] }
        }
        _ => vlm::text_page(p.page, render(markdown, fmt), text),
    }
}

fn describe_page_error(e: Option<&WireError>) -> String {
    let Some(e) = e else { return "unknown error".into() };
    let mut s = if e.code.is_empty() { "error".to_string() } else { e.code.clone() };
    if let Some(m) = e.message.as_deref().filter(|m| !m.is_empty()) {
        s.push_str(&format!(": {m}"));
    }
    if let Some(r) = e.reason.as_deref().filter(|r| !r.is_empty()) {
        s.push_str(&format!(" (reason: {r})"));
    }
    s
}

/// The error for a request in which no page succeeded. Every page's code and message is kept.
fn all_pages_failed(rec: &ParseRecord, failed: &[&WirePage]) -> Error {
    let codes: Vec<&str> = failed.iter().filter_map(|p| p.error.as_ref()).map(|e| e.code.as_str()).collect();
    let kind = if let Some(code) = rec.error_code.as_deref() {
        code_kind(code)
    } else if !codes.is_empty() && codes.iter().all(|c| matches!(*c, "rate_limited" | "at_capacity")) {
        ErrorKind::RateLimit
    } else if !codes.is_empty() && codes.iter().all(|c| *c == "unreadable_page") {
        ErrorKind::BadRequest
    } else {
        ErrorKind::Provider
    };
    let code_note = rec.error_code.as_deref().map(|c| format!(", error_code {c}")).unwrap_or_default();
    let msg = if failed.is_empty() {
        format!("request ended with status '{}'{code_note} and returned no pages", rec.status)
    } else {
        const SHOWN: usize = 10;
        let mut detail: Vec<String> = failed
            .iter()
            .take(SHOWN)
            .map(|p| format!("page {}: {}", p.page, describe_page_error(p.error.as_ref())))
            .collect();
        if failed.len() > SHOWN {
            detail.push(format!("… and {} more", failed.len() - SHOWN));
        }
        format!("all {} pages failed (status '{}'{code_note}): {}", failed.len(), rec.status, detail.join("; "))
    };
    let mut e = Error::new(kind, msg).with_provider(NAME);
    if !rec.id.is_empty() {
        e = e.with_job_id(rec.id.clone());
    }
    e
}

/// A request refused before it ran (`rejected`) or whose credit hold lapsed (`expired`).
fn rejected_error(rec: &ParseRecord) -> Error {
    let code = rec.error_code.as_deref().unwrap_or(rec.status.as_str());
    let msg = match rec.status.as_str() {
        "expired" => "request expired before it settled (it is free); send it again".to_string(),
        _ => format!("request rejected before it started: {code}"),
    };
    let kind = if rec.status == "expired" { ErrorKind::Provider } else { code_kind(code) };
    Error::new(kind, msg).with_provider(NAME).with_job_id(rec.id.clone())
}

/// Map a finished record onto a [`ParseResponse`]: the successful pages, with failed pages and
/// layout errors reported in `metadata`. No successful page → `Err`.
pub(crate) fn normalize(rec: &ParseRecord, fmt: OutputFormat, model: &str) -> Result<ParseResponse> {
    if matches!(rec.status.as_str(), "rejected" | "expired") {
        return Err(rejected_error(rec));
    }
    let wire = rec.pages.as_deref().unwrap_or_default();
    let (ok, failed): (Vec<&WirePage>, Vec<&WirePage>) = wire.iter().partition(|p| p.status == "ok");
    if ok.is_empty() {
        return Err(all_pages_failed(rec, &failed));
    }

    let pages: Vec<Page> = ok.iter().map(|p| page_from_wire(p, fmt)).collect();
    let page_charges: f64 = wire.iter().filter_map(|p| p.charge_usd).sum();
    let usage = Usage {
        pages: ok.len() as u32,
        credits: None,
        provider_cost_usd: rec.charge_usd.or(Some(page_charges)).filter(|c| c.is_finite() && *c >= 0.0),
    };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    resp.provider_job_id = (!rec.id.is_empty()).then(|| rec.id.clone());

    let meta = &mut resp.metadata;
    meta.insert("opendocrouter_status".into(), json!(rec.status));
    for (key, value) in [
        ("opendocrouter_mode", &rec.mode),
        ("opendocrouter_model_version", &rec.model_version),
        ("opendocrouter_price_version", &rec.price_version),
        ("opendocrouter_results_expire_at", &rec.results_expire_at),
    ] {
        if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
            meta.insert(key.into(), json!(v));
        }
    }
    if let Some(u) = &rec.usage {
        meta.insert("opendocrouter_usage".into(), json!(u));
    }
    if let Some(n) = rec.page_count {
        meta.insert("opendocrouter_page_count".into(), json!(n));
    }
    let cached: Vec<u32> = ok.iter().filter(|p| p.cached == Some(true)).map(|p| p.page).collect();
    if !cached.is_empty() {
        meta.insert("opendocrouter_cached_pages".into(), json!(cached));
    }
    if !failed.is_empty() {
        let list: Vec<Value> = failed
            .iter()
            .map(|p| {
                let e = p.error.clone().unwrap_or_default();
                let mut v = json!({ "page": p.page, "code": e.code, "message": e.message.unwrap_or_default() });
                if let Some(r) = e.reason {
                    v["reason"] = json!(r);
                }
                v
            })
            .collect();
        tracing::warn!(
            failed = failed.len(),
            ok = ok.len(),
            "opendocrouter: some pages failed; see metadata.opendocrouter_failed_pages"
        );
        meta.insert("opendocrouter_failed_pages".into(), Value::Array(list));
    }
    let layout_errors: Vec<Value> = ok
        .iter()
        .filter_map(|p| {
            let l = p.layout.as_ref().filter(|l| l.status == "error")?;
            let e = l.error.clone().unwrap_or_default();
            Some(json!({ "page": p.page, "code": e.code, "message": e.message.unwrap_or_default() }))
        })
        .collect();
    if !layout_errors.is_empty() {
        meta.insert("opendocrouter_layout_errors".into(), Value::Array(layout_errors));
    }
    Ok(resp)
}

/// Deserialize a record and normalise it, attaching the raw payload on request.
fn finish(record: Value, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
    let rec: ParseRecord = serde_json::from_value(record.clone()).map_err(|e| {
        Error::provider(format!("unexpected response shape: {e}; body starts: {}", http::snippet(&record.to_string())))
            .with_provider(NAME)
    })?;
    let mut resp = normalize(&rec, request.output, model)?;
    if request.include_raw {
        resp.raw = Some(record);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelRef;
    use crate::testutil::{fixture, pdf_request, serve};
    use crate::types::Mode;

    fn record(name: &str) -> ParseRecord {
        serde_json::from_str(&fixture(name)).unwrap()
    }

    #[test]
    fn model_strings_keep_the_vendor_prefix() {
        let m = ModelRef::parse("opendocrouter/google/gemini-3-flash").unwrap();
        assert_eq!((m.provider.as_str(), m.model.as_str()), ("opendocrouter", "google/gemini-3-flash"));
        assert_eq!(m.qualified(), "opendocrouter/google/gemini-3-flash");
        assert_eq!(
            ModelRef::parse("OpenDocRouter/RedNote-HiLab/dots.mocr").unwrap().qualified(),
            "opendocrouter/rednote-hilab/dots.mocr"
        );
        assert_eq!(ModelRef::parse("opendocrouter").unwrap().qualified(), "opendocrouter/google/gemini-3.8-flash-low");
        assert_eq!(ModelRef::parse("odr/openai/gpt-6-luna").unwrap().qualified(), "opendocrouter/openai/gpt-6-luna");
        assert_eq!(
            ModelRef::parse_for("opendocrouter", Mode::Ocr).unwrap().qualified(),
            "opendocrouter/google/gemini-3.8-flash-low"
        );
        assert!(ModelRef::parse("opendocrouter/google").is_err());
        assert!(ModelRef::parse("opendocrouter/acme/model").is_err());
        assert!(ModelRef::parse_for("opendocrouter/google/gemini-3-flash", Mode::Extract).is_err());
        // Every registered model is priced, so `cost_usd` has an estimate even before `charge_usd`.
        for m in crate::model::provider_info(NAME).unwrap().models {
            let q = m.qualified();
            assert!(crate::pricing::price_per_page(&q, Mode::Parse).is_some(), "{q} has a parse price");
            assert_eq!(ModelRef::parse(&q).unwrap().qualified(), q);
        }
        assert_eq!(crate::model::provider_info(NAME).unwrap().models.len(), 11);
    }

    #[test]
    fn request_body_defaults_and_options() {
        let req = DocumentRequest::from_url("https://example.com/a.pdf").pages("1-3, 7");
        let body = request_body(&req, "google/gemini-3-flash").unwrap();
        assert_eq!(body, json!({"model": "google/gemini-3-flash", "layout": true, "pages": "1-3,7"}));
        assert_eq!(explicit_mode(&body).unwrap(), None);

        let req = DocumentRequest::from_url("https://example.com/a.pdf")
            .provider_options(json!({"layout": null, "cache": true, "upload": true, "document": {"url": "x"}}));
        let body = request_body(&req, "openai/gpt-6-luna").unwrap();
        assert_eq!(body, json!({"model": "openai/gpt-6-luna", "cache": true}), "local keys are not sent");
        assert!(!layout_on(&body));

        let bad = DocumentRequest::from_url("https://x/y.pdf").provider_options(json!({"mode": "later"}));
        assert_eq!(explicit_mode(&request_body(&bad, "m").unwrap()).unwrap_err().kind, ErrorKind::Input);
        let bad = DocumentRequest::from_url("https://x/y.pdf").provider_options(json!({"layout": "yes"}));
        assert_eq!(request_body(&bad, "m").unwrap_err().kind, ErrorKind::Input);
        assert_eq!(
            request_body(&DocumentRequest::from_url("https://x/y.pdf").pages("0"), "m").unwrap_err().kind,
            ErrorKind::Input
        );
    }

    #[test]
    fn async_needs_cache() {
        let mut body = json!({"model": "m", "layout": true});
        make_async(&mut body).unwrap();
        assert_eq!(body["mode"], "async");
        assert_eq!(body["cache"], true);
        let mut body = json!({"model": "m", "cache": false});
        assert_eq!(make_async(&mut body).unwrap_err().kind, ErrorKind::Input);
    }

    #[test]
    fn estimates_pages_before_sending() {
        assert_eq!(estimated_pages(Some("1-3,7"), None), Some(4));
        assert_eq!(estimated_pages(Some("1-60"), None), Some(60));
        let pdf = b"%PDF-1.4\n1 0 obj<</Type /Pages /Count 2>>endobj 2 0 obj<</Type /Page>>endobj 3 0 obj<</Type/Page>>endobj";
        assert_eq!(estimated_pages(None, Some(pdf)), Some(2));
        assert_eq!(estimated_pages(Some("2-"), Some(pdf)), Some(2), "open ranges fall back to the PDF");
        assert_eq!(estimated_pages(None, Some(b"\x89PNG....")), Some(1));
        assert_eq!(estimated_pages(None, None), None, "URLs are unknown");
    }

    #[test]
    fn rejects_unsupported_file_types() {
        let input = DocumentInput::Bytes { data: bytes::Bytes::from_static(b"hello"), filename: "a.txt".into() };
        assert_eq!(document_mime(b"hello", &input).unwrap_err().kind, ErrorKind::Input);
        let input = DocumentInput::Bytes { data: bytes::Bytes::new(), filename: "scan.jpg".into() };
        assert_eq!(document_mime(&[0xFF, 0xD8, 0xFF, 0xE0], &input).unwrap(), "image/jpeg");
        assert!(check_id("0f433d25-c2d5-40df-960f-1cb4c5ddc415").is_ok());
        assert!(check_id("../credits").is_err());
        assert!(check_id("").is_err());
    }

    #[test]
    fn normalizes_layout_fixture() {
        let resp = normalize(
            &record("opendocrouter_parse_layout.json"),
            OutputFormat::Markdown,
            "google/gemini-3.8-flash-low",
        )
        .unwrap();
        assert_eq!(resp.model, "opendocrouter/google/gemini-3.8-flash-low");
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.usage.provider_cost_usd, Some(0.00133));
        assert_eq!(resp.provider_job_id.as_deref(), Some("00000000-0000-4000-8000-000000000001"));
        let p1 = &resp.pages[0];
        assert_eq!((p1.width, p1.height), (Some(612.0), Some(792.0)));
        let types: Vec<BlockType> = p1.blocks.iter().map(|b| b.block_type).collect();
        assert_eq!(
            types,
            [
                BlockType::Title,
                BlockType::Text,
                BlockType::List,
                BlockType::Table,
                BlockType::Footer,
                BlockType::Figure
            ]
        );
        assert_eq!(p1.blocks[0].content, "# Hello LiteOCR");
        assert_eq!(p1.blocks[0].confidence, Some(0.95));
        // A paragraph printed in two pieces gets the box enclosing both.
        let bb = p1.blocks[1].bbox.unwrap();
        assert!((bb.x0 - 0.12).abs() < 1e-9 && (bb.y0 - 0.15).abs() < 1e-9, "{bb:?}");
        assert!((bb.x1 - 0.42).abs() < 1e-9 && (bb.y1 - 0.21).abs() < 1e-9, "{bb:?}");
        assert_eq!(p1.blocks[1].content, "Invoice #1234\nTotal: $56.78");
        assert_eq!(p1.blocks[2].content, "- Widget A\n- Widget B");
        assert!(p1.blocks[3].content.starts_with("| Item | Amount |"));
        // A picture the markdown doesn't mention keeps its box with no content.
        assert_eq!(p1.blocks[5].content, "");
        assert!(p1.blocks[5].bbox.is_some());
        for b in resp.pages.iter().flat_map(|p| &p.blocks) {
            if let Some(bb) = b.bbox {
                assert!([bb.x0, bb.y0, bb.x1, bb.y1].iter().all(|v| (0.0..=1.0).contains(v)), "{bb:?}");
                assert!(bb.x0 <= bb.x1 && bb.y0 <= bb.y1);
            }
        }
        let p2 = &resp.pages[1];
        let t2: Vec<BlockType> = p2.blocks.iter().map(|b| b.block_type).collect();
        assert_eq!(
            t2,
            [
                BlockType::SectionHeader,
                BlockType::Text,
                BlockType::Formula,
                BlockType::Caption,
                BlockType::Text,
                BlockType::Text
            ]
        );
        assert_eq!(p2.blocks[2].content, "$$E = mc^2$$");
        assert!(p2.blocks[5].bbox.is_none(), "an element that couldn't be placed has no box");
        // Document markdown is the page markdown joined in order.
        assert_eq!(resp.markdown, format!("{}\n\n{}", p1.markdown, p2.markdown));
        assert!(resp.markdown.starts_with("# Hello LiteOCR") && resp.markdown.ends_with("confidential"));
        assert_eq!(resp.metadata["opendocrouter_status"], "completed");
        assert_eq!(resp.metadata["opendocrouter_cached_pages"], json!([2]));
        assert_eq!(resp.metadata["opendocrouter_price_version"], "2026-10-06");
        assert_eq!(resp.metadata["opendocrouter_usage"]["input_tokens"], 1290);
        assert!(!resp.metadata.contains_key("opendocrouter_failed_pages"));
    }

    #[test]
    fn text_output_strips_markdown() {
        let resp =
            normalize(&record("opendocrouter_parse_layout.json"), OutputFormat::Text, "google/gemini-3-flash").unwrap();
        assert!(resp.pages[0].markdown.starts_with("Hello LiteOCR"));
        assert_eq!(resp.pages[0].blocks[0].content, "Hello LiteOCR");
        assert!(resp.text.contains("Widget"));
    }

    #[test]
    fn partial_keeps_ok_pages_and_reports_failures() {
        let resp = normalize(
            &record("opendocrouter_parse_partial.json"),
            OutputFormat::Markdown,
            "anthropic/claude-haiku-5-5",
        )
        .unwrap();
        let numbers: Vec<u32> = resp.pages.iter().map(|p| p.page_number).collect();
        assert_eq!(numbers, [1, 3], "pages sorted, the failed page left out");
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.usage.provider_cost_usd, Some(0.000215));
        // Without layout a page is one geometry-free text block.
        assert_eq!(resp.pages[0].blocks.len(), 1);
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Text);
        assert!(resp.pages[0].blocks[0].bbox.is_none());
        assert_eq!(resp.pages[0].markdown, "# Quarterly Report\n\nRevenue grew 12%.");
        assert_eq!(resp.metadata["opendocrouter_status"], "partial");
        assert_eq!(
            resp.metadata["opendocrouter_failed_pages"],
            json!([{"page": 2, "code": "content_filtered", "message": "The provider blocked the output", "reason": "SAFETY"}])
        );
        assert_eq!(resp.metadata["opendocrouter_layout_errors"][0]["page"], 3);
        assert_eq!(resp.metadata["opendocrouter_layout_errors"][0]["code"], "timeout");
    }

    #[test]
    fn all_failed_pages_raise_a_mapped_error() {
        let e =
            normalize(&record("opendocrouter_parse_failed.json"), OutputFormat::Markdown, "opendatalab/mineru2.5-pro")
                .unwrap_err();
        assert_eq!(e.kind, ErrorKind::RateLimit, "capacity / rate limits are retryable");
        assert!(e.retryable);
        assert!(e.message.contains("all 2 pages failed"), "{}", e.message);
        assert!(e.message.contains("page 1: at_capacity: No capacity came free"), "{}", e.message);
        assert!(e.message.contains("page 2: rate_limited"), "{}", e.message);
        assert_eq!(e.job_id.as_deref(), Some("00000000-0000-4000-8000-000000000003"));

        let mut rec = record("opendocrouter_parse_failed.json");
        for p in rec.pages.as_mut().unwrap() {
            p.error = Some(WireError { code: "unreadable_page".into(), message: Some("bad".into()), reason: None });
        }
        assert_eq!(normalize(&rec, OutputFormat::Markdown, "m").unwrap_err().kind, ErrorKind::BadRequest);
        for p in rec.pages.as_mut().unwrap() {
            p.error = Some(WireError { code: "timeout".into(), message: None, reason: None });
        }
        assert_eq!(normalize(&rec, OutputFormat::Markdown, "m").unwrap_err().kind, ErrorKind::Provider);

        let rejected = ParseRecord {
            id: "r1".into(),
            status: "rejected".into(),
            error_code: Some("insufficient_credits".into()),
            ..Default::default()
        };
        let e = normalize(&rejected, OutputFormat::Markdown, "m").unwrap_err();
        assert_eq!(e.kind, ErrorKind::BadRequest);
        assert!(e.message.contains("insufficient_credits"));
        let expired = ParseRecord { id: "r2".into(), status: "expired".into(), ..Default::default() };
        assert_eq!(normalize(&expired, OutputFormat::Markdown, "m").unwrap_err().kind, ErrorKind::Provider);
    }

    #[test]
    fn maps_http_errors() {
        let body = |code: &str, msg: &str| json!({"error": {"code": code, "message": msg}}).to_string();
        let e = map_http_error(401, &body("unauthorized", "Unknown API key"), None, Some("req_1"));
        assert_eq!(e.kind, ErrorKind::Authentication);
        assert_eq!(e.message, "unauthorized: Unknown API key [X-Request-Id: req_1]");
        assert_eq!(map_http_error(403, &body("account_paused", "paused"), None, None).kind, ErrorKind::Authentication);
        let credits = json!({"error": {"code": "insufficient_credits", "message": "Not enough credit", "required_usd": 1.5, "available_usd": 0.25}});
        let e = map_http_error(402, &credits.to_string(), None, None);
        assert_eq!(e.kind, ErrorKind::BadRequest);
        assert!(!e.retryable);
        assert_eq!(e.message, "insufficient_credits: Not enough credit (required_usd: 1.5, available_usd: 0.25)");
        let e = map_http_error(429, &body("rate_limited", "Too many requests"), Some(Duration::from_secs(7)), None);
        assert_eq!(e.kind, ErrorKind::RateLimit);
        assert!(e.message.ends_with("(Retry-After: 7s)"), "{}", e.message);
        assert!(http::is_transient(&e));
        let e = map_http_error(503, &body("model_starting", "Starting"), Some(Duration::from_secs(180)), None);
        assert_eq!((e.kind, e.status_code), (ErrorKind::Provider, Some(503)));
        assert!(http::is_transient(&e), "503 is retried");
        for (status, code) in
            [(400, "invalid_request"), (413, "too_large"), (415, "unsupported_type"), (422, "unreadable_document")]
        {
            let e = map_http_error(status, &body(code, "x"), None, None);
            assert_eq!(e.kind, ErrorKind::BadRequest, "{code}");
            assert!(e.message.starts_with(code));
        }
        assert_eq!(map_http_error(502, "<html>bad gateway</html>", None, None).message, "<html>bad gateway</html>");
    }

    #[test]
    fn merges_result_parts() {
        let mut first: Value = serde_json::from_str(&fixture("opendocrouter_get_expanded.json")).unwrap();
        merge_part(
            &mut first,
            json!({"pages": [{"page": 2, "status": "ok", "markdown": "Part two."}], "has_more": false, "next_cursor": null}),
        );
        assert_eq!(first["pages"].as_array().unwrap().len(), 2);
        assert_eq!(first["has_more"], false);
        assert!(first["next_cursor"].is_null());
        assert_eq!(first["charge_usd"], 0.000267, "null totals in a later part do not clobber");
    }

    // ---- wire tests against the loopback server ----

    #[tokio::test]
    async fn sync_inline_parse_sends_the_documented_body() {
        let (base, seen) = serve(vec![(200, fixture("opendocrouter_parse_layout.json"))]).await;
        let req = pdf_request(&base).pages("1-2").include_raw(true);
        let resp = OpenDocRouter.parse(&req, "google/gemini-3.8-flash-low").await.unwrap();
        assert_eq!(resp.pages.len(), 2);
        assert!(resp.raw.is_some());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].route(), "POST /v1/parse");
        assert_eq!(seen[0].header("authorization").as_deref(), Some("bearer test-key"));
        let body: Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(body["model"], "google/gemini-3.8-flash-low");
        assert_eq!(body["layout"], true);
        assert_eq!(body["pages"], "1-2");
        assert_eq!(body["document"]["mime_type"], "application/pdf");
        assert_eq!(body["document"]["data"], vlm::base64_encode(b"%PDF-1.4 test"));
        assert!(body.get("mode").is_none() && body.get("cache").is_none(), "sync, nothing stored");
    }

    #[tokio::test]
    async fn upload_flow_puts_the_file_without_the_key() {
        let (base, seen) = serve(vec![
            (201, json!({"upload_id": "9b2c0000-0000-4000-8000-000000000000", "upload_url": "{base}/put/9b2c", "max_bytes": 52428800, "expires_at": "2026-10-08T10:00:00Z"}).to_string()),
            (200, String::new()),
            (200, fixture("opendocrouter_parse_partial.json")),
        ])
        .await;
        let req = pdf_request(&base).provider_options(json!({"upload": true, "layout": false}));
        let resp = OpenDocRouter.parse(&req, "anthropic/claude-haiku-5-5").await.unwrap();
        assert_eq!(resp.metadata["opendocrouter_status"], "partial");
        let seen = seen.lock().unwrap();
        let routes: Vec<String> = seen.iter().map(|s| s.route()).collect();
        assert_eq!(routes, ["POST /v1/uploads", "PUT /put/9b2c", "POST /v1/parse"]);
        assert!(seen[1].header("authorization").is_none(), "the API key never goes to the upload URL");
        assert_eq!(seen[1].body, "%PDF-1.4 test");
        let body: Value = serde_json::from_str(&seen[2].body).unwrap();
        assert_eq!(body["document"], json!({"upload_id": "9b2c0000-0000-4000-8000-000000000000"}));
        assert_eq!(body["layout"], false);
        assert!(body.get("upload").is_none());
    }

    #[tokio::test]
    async fn sync_refused_as_too_large_moves_to_async() {
        let id = "00000000-0000-4000-8000-000000000004";
        let (base, seen) = serve(vec![
            (413, json!({"error": {"code": "too_large", "message": "A sync request takes at most 50 pages"}}).to_string()),
            (202, json!({"id": id, "status": "processing", "mode": "async", "pages": [], "pages_done": 0}).to_string()),
            (200, json!({"id": id, "status": "completed", "pages": [], "pages_done": 2}).to_string()),
            (200, fixture("opendocrouter_get_expanded.json")),
            (200, json!({"id": id, "status": "completed", "pages": [{"page": 2, "status": "ok", "markdown": "Part two.", "cached": false, "charge_usd": 0.000131}], "has_more": false, "next_cursor": null, "charge_usd": 0.000267}).to_string()),
        ])
        .await;
        let resp = OpenDocRouter.parse(&pdf_request(&base), "opendatalab/mineru2.5-pro").await.unwrap();
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.markdown, "# Annual Report\n\nPart one.\n\nPart two.");
        assert_eq!(resp.provider_job_id.as_deref(), Some(id));
        assert_eq!(resp.metadata["opendocrouter_mode"], "async");
        assert_eq!(resp.usage.provider_cost_usd, Some(0.000267));
        let seen = seen.lock().unwrap();
        let routes: Vec<String> = seen.iter().map(|s| s.route()).collect();
        assert_eq!(
            routes,
            [
                "POST /v1/parse".to_string(),
                "POST /v1/parse".to_string(),
                format!("GET /v1/parse/{id}"),
                format!("GET /v1/parse/{id}?expand=markdown,layout"),
                format!("GET /v1/parse/{id}?expand=markdown,layout&cursor=1"),
            ]
        );
        let second: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!((second["mode"].as_str(), second["cache"].as_bool()), (Some("async"), Some(true)));
    }

    #[tokio::test]
    async fn explicit_sync_does_not_fall_back() {
        let (base, seen) =
            serve(vec![(413, json!({"error": {"code": "too_large", "message": "over 50 pages"}}).to_string())]).await;
        let req = pdf_request(&base).provider_options(json!({"mode": "sync"}));
        let e = OpenDocRouter.parse(&req, "openai/gpt-6-luna").await.unwrap_err();
        assert_eq!((e.kind, e.status_code), (ErrorKind::BadRequest, Some(413)));
        assert_eq!(e.message, "too_large: over 50 pages");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn jobs_submit_and_retrieve() {
        let id = "00000000-0000-4000-8000-000000000004";
        let (base, seen) = serve(vec![
            (202, json!({"id": id, "status": "processing", "mode": "async", "pages": []}).to_string()),
            (200, json!({"id": id, "status": "processing", "pages": [], "pages_done": 1}).to_string()),
            (200, json!({"id": id, "status": "completed", "pages": []}).to_string()),
            (200, fixture("opendocrouter_get_expanded.json").replace("\"has_more\": true", "\"has_more\": false")),
        ])
        .await;
        let req = pdf_request(&base).model("opendocrouter/opendatalab/mineru2.5-pro");
        let handle = crate::submit_parse(req).await.unwrap();
        assert_eq!(handle.model, "opendocrouter/opendatalab/mineru2.5-pro");
        assert_eq!(handle.job_id, id);
        assert_eq!(handle.provider_state, Some(json!({"layout": true})));
        let opts = crate::RetrieveOptions {
            api_key: Some("test-key".into()),
            base_url: Some(base.clone()),
            timeout_secs: 20.0,
            max_retries: 0,
        };
        assert!(crate::retrieve_parse_with(&handle, &opts).await.unwrap().is_pending());
        let JobStatus::Succeeded(resp) = crate::retrieve_parse_with(&handle, &opts).await.unwrap() else {
            panic!("expected success")
        };
        assert_eq!(resp.model, "opendocrouter/opendatalab/mineru2.5-pro");
        assert_eq!(resp.pages.len(), 1);
        let seen = seen.lock().unwrap();
        let body: Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!((body["mode"].as_str(), body["cache"].as_bool()), (Some("async"), Some(true)));
        assert_eq!(seen[3].route(), format!("GET /v1/parse/{id}?expand=markdown,layout"));
    }

    #[tokio::test]
    async fn jobs_reject_webhooks_and_report_failures() {
        let req = pdf_request("http://127.0.0.1:9").webhook_url("https://example.com/hook");
        assert_eq!(OpenDocRouter.submit_parse(&req, "m").await.unwrap_err().kind, ErrorKind::Input);
        assert_eq!(OpenDocRouter.parse_webhook("m", &json!({})).unwrap_err().kind, ErrorKind::Input);

        let failed = fixture("opendocrouter_parse_failed.json");
        let (base, _) = serve(vec![(200, failed.clone()), (200, failed)]).await;
        let handle =
            JobHandle::new(NAME, "opendocrouter/opendatalab/mineru2.5-pro", "00000000-0000-4000-8000-000000000003");
        let opts = crate::RetrieveOptions { api_key: Some("k".into()), base_url: Some(base), ..Default::default() };
        let JobStatus::Failed(e) = crate::retrieve_parse_with(&handle, &opts).await.unwrap() else {
            panic!("expected a failure")
        };
        assert_eq!(e.kind, ErrorKind::RateLimit);
        assert_eq!(e.provider.as_deref(), Some(NAME));
    }

    /// Live smoke test. Run with:
    /// `OPEN_DOC_ROUTER_API_KEY=… cargo test -p puffinparse-core opendocrouter -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs OPEN_DOC_ROUTER_API_KEY and network"]
    async fn live_parse() {
        if std::env::var(ENV_KEY).map(|v| v.trim().is_empty()).unwrap_or(true) {
            eprintln!("skipping: {ENV_KEY} not set");
            return;
        }
        let sample =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/multipage_001.pdf");
        let req = DocumentRequest::from_path(sample).timeout_secs(240.0);
        let resp = OpenDocRouter.parse(&req, "opendatalab/mineru2.5-pro").await.expect("parse succeeds");
        assert_eq!(resp.pages.len(), 2);
        assert!(!resp.markdown.trim().is_empty());
        assert!(resp.usage.provider_cost_usd.is_some());
        assert!(resp.pages.iter().any(|p| p.blocks.iter().any(|b| b.bbox.is_some())), "layout boxes");
    }
}
