//! Extend (extend.ai) document parsing and extraction.
//!
//! Parse flow: upload (multipart `POST /files/upload`) unless the input is a URL → `POST /parse_runs`
//! → poll `GET /parse_runs/{id}` until `PROCESSED` / `FAILED`.
//! Extract flow: the same file reference → `POST /extract_runs` → poll `GET /extract_runs/{id}`.
//! API version pinned via the mandatory `x-extend-api-version` header.
//!
//! Jobs API (`crate::submit_parse` / `retrieve_parse`): the same `POST /parse_runs`, then one
//! `GET /parse_runs/{id}` per retrieve. `provider_options.responseType = "url"` asks for the output
//! as a presigned URL (fetched without auth) instead of inline.

use crate::error::{Error, ErrorKind, Result};
use crate::http::{self, Deadline, Retry};
use crate::jobs::{JobHandle, JobStatus, WebhookEvent, WebhookStatus};
use crate::provider::{self, Provider};
use crate::types::{
    BBox, Block, BlockType, Citation, DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, FieldInfo,
    OutputFormat, Page, ParseResponse, Usage,
};
use crate::util::deep_merge;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME: &str = "extend";
pub const API_VERSION: &str = "2026-02-09";
const ENV_KEY: &str = "EXTEND_API_KEY";
const ENV_BASE: &str = "EXTEND_BASE_URL";
const DEFAULT_BASE: &str = "https://api.extend.ai";

#[derive(Debug, Default, Clone, Copy)]
pub struct Extend;

#[async_trait]
impl Provider for Extend {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let doc = &request.document;
        let api_key = provider::resolve_api_key(doc, ENV_KEY, NAME)?;
        let base = provider::resolve_base_url(doc, ENV_BASE, DEFAULT_BASE);
        let deadline = Deadline::new(doc.timeout_secs);
        let retry = Retry::new(doc.max_retries);
        let client = http::client();
        let headers = |rb: reqwest::RequestBuilder| {
            let rb = rb.bearer_auth(&api_key).header("x-extend-api-version", API_VERSION);
            match doc.option("workspace_id").and_then(Value::as_str) {
                Some(ws) => rb.header("x-extend-workspace-id", ws),
                None => rb,
            }
        };

        // 1. File reference: URL directly, or upload bytes.
        let file_ref = resolve_file_ref(doc, client, &base, &headers, retry, &deadline).await?;

        // 2. Config (schema is adapted to Extend's JSON Schema subset).
        let body = build_extract_body(request, model, file_ref)?;

        // 3. Create the async run.
        let run: ExtractRun = http::with_retry(NAME, retry, &deadline, || {
            let rb =
                headers(client.post(format!("{base}/extract_runs"))).timeout(deadline.request_timeout()).json(&body);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await?;
        let run_id = run.id.clone();
        tracing::debug!(run_id = %run_id, status = %run.status, "extend: extract run created");

        // 4. Poll.
        let run = if is_terminal(&run.status) {
            run
        } else {
            http::poll_until(NAME, &deadline, Duration::from_secs(1), Duration::from_secs(10), || {
                let rb =
                    headers(client.get(format!("{base}/extract_runs/{run_id}"))).timeout(deadline.request_timeout());
                async move {
                    let run: ExtractRun = http::read_json(NAME, rb.send().await?).await?;
                    Ok(if is_terminal(&run.status) { Some(run) } else { None })
                }
            })
            .await
            .map_err(|e| e.with_job_id(run_id.clone()))?
        };
        if run.status != "PROCESSED" {
            let reason = run.failure_reason.clone().unwrap_or_else(|| run.status.clone());
            let msg = run.failure_message.clone().unwrap_or_default();
            return Err(Error::new(failure_kind(&reason), format!("extract run failed: {reason}: {msg}"))
                .with_provider(NAME)
                .with_job_id(run_id));
        }
        let output = run.output.clone().ok_or_else(|| {
            Error::provider("extract run finished without output").with_provider(NAME).with_job_id(run_id.clone())
        })?;

        let raw = if doc.include_raw { Some(serde_json::to_value(&run)?) } else { None };
        let mut resp = normalize_extract(&run, &output, model);
        resp.provider_job_id = Some(run_id);
        resp.raw = raw;
        Ok(resp)
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let ctx = Ctx::new(request)?;
        let headers = |rb: reqwest::RequestBuilder| ctx.headers(rb);

        // 1. File reference: URL directly, or upload bytes.
        let file_ref = resolve_file_ref(request, ctx.client, &ctx.base, &headers, ctx.retry, &ctx.deadline).await?;

        // 2. Build config.
        let body = build_body(request, model, file_ref)?;

        // 3. Create async run.
        let run = ctx.create(&body).await?;
        let run_id = run.id.clone();
        tracing::debug!(run_id = %run_id, status = %run.status, "extend: run created");

        // 4. Poll.
        let run = if run.status == "PROCESSED" || run.status == "FAILED" {
            run
        } else {
            http::poll_until(NAME, &ctx.deadline, Duration::from_secs(1), Duration::from_secs(10), || async {
                let run = ctx.get_run_once(&run_id).await?;
                Ok(match run.status.as_str() {
                    "PROCESSED" | "FAILED" => Some(run),
                    _ => None,
                })
            })
            .await
            .map_err(|e| e.with_job_id(run_id.clone()))?
        };

        if run.status == "FAILED" {
            return Err(parse_run_failure(&run));
        }

        // 5. Output (inline, or via presigned URL when responseType=url was requested).
        ctx.finish_parse(run, request).await
    }

    /// `POST /parse_runs` and return; Extend has no per-run webhook (see `docs/providers/extend.md`).
    async fn submit_parse(&self, request: &DocumentRequest, model: &str) -> Result<JobHandle> {
        if provider::webhook_url(request)?.is_some() {
            return Err(Error::input(
                "extend has no per-run webhook URL: register an endpoint once (POST /webhook_endpoints or the \
                 dashboard) subscribed to parse_run.processed / parse_run.failed, then hand its bodies to \
                 liteocr parse_webhook / resolve_webhook",
            )
            .with_provider(NAME));
        }
        let ctx = Ctx::new(request)?;
        let headers = |rb: reqwest::RequestBuilder| ctx.headers(rb);
        let file_ref = resolve_file_ref(request, ctx.client, &ctx.base, &headers, ctx.retry, &ctx.deadline).await?;
        let body = build_body(request, model, file_ref)?;
        let run = ctx.create(&body).await?;
        tracing::debug!(run_id = %run.id, status = %run.status, "extend: run submitted");
        let mut job = JobHandle::new(NAME, model, run.id);
        for key in ["workspace_id", RESPONSE_TYPE] {
            if let Some(v) = request.option(key) {
                job = job.with_state(key, v.clone());
            }
        }
        Ok(job)
    }

    /// One `GET /parse_runs/{id}` (with `?responseType=url` when the run was submitted that way).
    async fn retrieve_parse(&self, job: &JobHandle, request: &DocumentRequest, _model: &str) -> Result<JobStatus> {
        let ctx = Ctx::new(request)?;
        let run = http::with_retry(NAME, ctx.retry, &ctx.deadline, || ctx.get_run_once(&job.job_id))
            .await
            .map_err(|e| e.with_job_id(job.job_id.clone()))?;
        match run.status.as_str() {
            "PROCESSED" => Ok(JobStatus::Succeeded(Box::new(ctx.finish_parse(run, request).await?))),
            "FAILED" | "CANCELLED" => Ok(JobStatus::Failed(parse_run_failure(&run))),
            _ => Ok(JobStatus::Pending),
        }
    }

    /// `{"eventId", "eventType": "parse_run.processed" | "parse_run.failed", "payload": {…}}` where
    /// `payload` is a `parse_run_status` (id, status, failureReason, failureMessage, metadata).
    fn parse_webhook(&self, model: &str, payload: &Value) -> Result<WebhookEvent> {
        let event_type = payload.get("eventType").and_then(Value::as_str).unwrap_or_default();
        if !event_type.is_empty() && !event_type.starts_with("parse_run.") {
            return Err(Error::input(format!("extend webhook event '{event_type}' is not a parse run event"))
                .with_provider(NAME));
        }
        let inner = payload.get("payload").unwrap_or(payload);
        if inner.get("id").is_none() && inner.get("data").and_then(Value::as_str).is_some() {
            return Err(Error::input(
                "extend webhook body is a signed download URL (payload.data): download it and pass the JSON it \
                 holds instead",
            )
            .with_provider(NAME));
        }
        let run: ParseRun = serde_json::from_value(inner.clone()).map_err(|e| {
            Error::input(format!("extend webhook payload is not a parse run status: {e}")).with_provider(NAME)
        })?;
        let status = match run.status.as_str() {
            "PROCESSED" => match run.output.clone() {
                // A full run object (not the minimal status) carries the output inline.
                Some(output) => {
                    let mut resp = normalize(&run, output, OutputFormat::Markdown);
                    resp.provider_job_id = Some(run.id.clone());
                    WebhookStatus::Succeeded(Box::new(resp))
                }
                None => WebhookStatus::Finished,
            },
            "FAILED" | "CANCELLED" => WebhookStatus::Failed(parse_run_failure(&run)),
            _ => WebhookStatus::Pending,
        };
        Ok(WebhookEvent { job: Some(JobHandle::new(NAME, model, run.id)), status })
    }
}

/// `provider_options` key (Extend's own query-parameter name) asking for the output as a presigned
/// URL (`GET /parse_runs/{id}?responseType=url`) instead of inline. Consumed by LiteOCR, never sent
/// in the body.
const RESPONSE_TYPE: &str = "responseType";

/// Credentials, endpoint and limits resolved once per call.
struct Ctx {
    api_key: String,
    base: String,
    workspace_id: Option<String>,
    response_type: Option<String>,
    deadline: Deadline,
    retry: Retry,
    client: &'static reqwest::Client,
}

impl Ctx {
    fn new(request: &DocumentRequest) -> Result<Self> {
        Ok(Self {
            api_key: provider::resolve_api_key(request, ENV_KEY, NAME)?,
            base: provider::resolve_base_url(request, ENV_BASE, DEFAULT_BASE),
            workspace_id: request.option("workspace_id").and_then(Value::as_str).map(String::from),
            response_type: request.option(RESPONSE_TYPE).and_then(Value::as_str).map(String::from),
            deadline: Deadline::new(request.timeout_secs),
            retry: Retry::new(request.max_retries),
            client: http::client(),
        })
    }

    /// Bearer key, pinned API version, and the workspace header when one was given.
    fn headers(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let rb = rb.bearer_auth(&self.api_key).header("x-extend-api-version", API_VERSION);
        match &self.workspace_id {
            Some(ws) => rb.header("x-extend-workspace-id", ws),
            None => rb,
        }
    }

    /// `POST /parse_runs`.
    async fn create(&self, body: &Value) -> Result<ParseRun> {
        let url = format!("{}/parse_runs", self.base);
        http::with_retry(NAME, self.retry, &self.deadline, || {
            let rb = self.headers(self.client.post(&url)).timeout(self.deadline.request_timeout()).json(body);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await
    }

    /// One `GET /parse_runs/{id}`, adding `?responseType=…` when asked for.
    async fn get_run_once(&self, run_id: &str) -> Result<ParseRun> {
        let mut url = format!("{}/parse_runs/{run_id}", self.base);
        if let Some(rt) = &self.response_type {
            url.push_str(&format!("?responseType={rt}"));
        }
        let rb = self.headers(self.client.get(url)).timeout(self.deadline.request_timeout());
        http::read_json(NAME, rb.send().await?).await
    }

    /// Output inline, or downloaded from `outputUrl` (plain GET, no auth header: it is presigned).
    async fn finish_parse(&self, run: ParseRun, request: &DocumentRequest) -> Result<ParseResponse> {
        let run_id = run.id.clone();
        let output = match (run.output.clone(), run.output_url.as_deref()) {
            (Some(o), _) => o,
            (None, Some(url)) => {
                let resp = self.client.get(url).timeout(self.deadline.request_timeout()).send().await?;
                http::read_json::<Output>(NAME, resp).await.map_err(|e| e.with_job_id(run_id.clone()))?
            }
            (None, None) => {
                return Err(Error::provider("run finished without output").with_provider(NAME).with_job_id(run_id))
            }
        };
        let raw = if request.include_raw { Some(serde_json::to_value(&run)?) } else { None };
        let mut resp = normalize(&run, output, request.output);
        resp.provider_job_id = Some(run_id);
        resp.raw = raw;
        Ok(resp)
    }
}

/// A `FAILED` parse run → error: `OCR_ERROR` / `INTERNAL_ERROR` → provider, `OUT_OF_CREDITS` →
/// authentication, anything else → bad request. Message and run id are always kept.
fn parse_run_failure(run: &ParseRun) -> Error {
    let reason = run.failure_reason.clone().unwrap_or_else(|| "UNKNOWN".into());
    let msg = run.failure_message.clone().unwrap_or_default();
    let kind = match reason.as_str() {
        "OCR_ERROR" | "INTERNAL_ERROR" => ErrorKind::Provider,
        "OUT_OF_CREDITS" => ErrorKind::Authentication,
        _ => ErrorKind::BadRequest,
    };
    Error::new(kind, format!("parse run failed: {reason}: {msg}")).with_provider(NAME).with_job_id(run.id.clone())
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "PROCESSED" | "FAILED" | "CANCELLED")
}

fn failure_kind(reason: &str) -> ErrorKind {
    match reason {
        "OCR_ERROR"
        | "INTERNAL_ERROR"
        | "FAILED_TO_PROCESS_FILE"
        | "PARSING_ERROR"
        | "PRE_PROCESSING_FAILURE"
        | "POST_PROCESSING_FAILURE" => ErrorKind::Provider,
        "OUT_OF_CREDITS" => ErrorKind::Authentication,
        _ => ErrorKind::BadRequest,
    }
}

/// Resolve the document to Extend's `file` reference: URLs are passed through, bytes and paths are
/// uploaded to `POST /files/upload` and referenced by id.
async fn resolve_file_ref<F>(
    request: &DocumentRequest,
    client: &reqwest::Client,
    base: &str,
    headers: &F,
    retry: Retry,
    deadline: &Deadline,
) -> Result<Value>
where
    F: Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
{
    match provider::load_bytes(&request.input).await? {
        None => {
            let DocumentInput::Url { url } = &request.input else { unreachable!() };
            Ok(json!({ "url": url, "name": request.input.filename() }))
        }
        Some(data) => {
            let upload: UploadResponse = http::with_retry(NAME, retry, deadline, || {
                let form =
                    reqwest::multipart::Form::new().part("file", provider::file_part(data.clone(), &request.input));
                let rb = headers(client.post(format!("{base}/files/upload")))
                    .timeout(deadline.request_timeout())
                    .multipart(form);
                async move { http::read_json(NAME, rb.send().await?).await }
            })
            .await?;
            tracing::debug!(file_id = %upload.id, "extend: uploaded");
            Ok(json!({ "id": upload.id }))
        }
    }
}

fn build_body(request: &DocumentRequest, model: &str, file_ref: Value) -> Result<Value> {
    let engine = match model {
        "parse_performance" | "parse_light" | "parse_auto" => model,
        other => return Err(Error::unsupported_model(format!("extend: unknown engine '{other}'"))),
    };
    // Tables are always requested as markdown; `OutputFormat::Text` is derived from it afterwards.
    let table_format = "markdown";
    let mut config = json!({
        "target": "markdown",
        "chunkingStrategy": { "type": "page" },
        "engine": engine,
        "blockOptions": { "tables": { "targetFormat": table_format } },
    });
    if let Some(pages) = &request.pages {
        let ranges: Vec<Value> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) => json!({ "start": s, "end": e }),
                None => json!({ "start": s, "end": 750 }),
            })
            .collect();
        config["advancedOptions"] = json!({ "pageRanges": ranges });
    }
    let mut body = json!({ "file": file_ref, "config": config });
    if let Some(opts) = &request.provider_options {
        // Options may target `config` directly or be given at the top level (metadata, dataRetention).
        const CONFIG_KEYS: &[&str] =
            &["target", "chunkingStrategy", "engine", "engineVersion", "blockOptions", "advancedOptions"];
        let mut patch = opts.clone();
        if let Value::Object(o) = &mut patch {
            o.remove("workspace_id");
            o.remove(RESPONSE_TYPE);
            let mut cfg = serde_json::Map::new();
            for k in CONFIG_KEYS {
                if let Some(v) = o.remove(*k) {
                    cfg.insert((*k).to_string(), v);
                }
            }
            if !cfg.is_empty() {
                let mut merged = o.remove("config").unwrap_or_else(|| json!({}));
                deep_merge(&mut merged, &Value::Object(cfg));
                o.insert("config".into(), merged);
            }
        }
        deep_merge(&mut body, &patch);
    }
    Ok(body)
}

fn build_extract_body(request: &ExtractRequest, model: &str, file_ref: Value) -> Result<Value> {
    let doc = &request.document;
    let (base_processor, parse_engine) = match model {
        "extraction_performance" => ("extraction_performance", "parse_performance"),
        "extraction_light" => ("extraction_light", "parse_light"),
        other => return Err(Error::unsupported_model(format!("extend: unknown extract processor '{other}'"))),
    };
    let mut config = json!({
        "baseProcessor": base_processor,
        "schema": adapt_schema(&request.schema, false),
        // An extract run implicitly parses the file first; keep the parser in the same weight class.
        "parseConfig": { "engine": parse_engine },
    });
    if let Some(rules) = request.instructions.as_deref().filter(|s| !s.trim().is_empty()) {
        config["extractionRules"] = json!(rules);
    }
    if request.citations {
        config["advancedOptions"] = json!({ "citationsEnabled": true });
    }
    if let Some(pages) = &doc.pages {
        let ranges: Vec<Value> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| json!({ "start": s, "end": e.unwrap_or(750) }))
            .collect();
        let advanced = config["advancedOptions"].as_object_mut();
        match advanced {
            Some(o) => {
                o.insert("pageRanges".into(), Value::Array(ranges));
            }
            None => config["advancedOptions"] = json!({ "pageRanges": ranges }),
        }
    }
    let mut body = json!({ "file": file_ref, "config": config });
    if let Some(opts) = &doc.provider_options {
        // Options may target `config` directly or be given at the top level (metadata, dataRetention).
        const CONFIG_KEYS: &[&str] =
            &["baseProcessor", "baseVersion", "extractionRules", "schema", "advancedOptions", "parseConfig"];
        let mut patch = opts.clone();
        if let Value::Object(o) = &mut patch {
            o.remove("workspace_id");
            let mut cfg = serde_json::Map::new();
            for k in CONFIG_KEYS {
                if let Some(v) = o.remove(*k) {
                    cfg.insert((*k).to_string(), v);
                }
            }
            if !cfg.is_empty() {
                let mut merged = o.remove("config").unwrap_or_else(|| json!({}));
                deep_merge(&mut merged, &Value::Object(cfg));
                o.insert("config".into(), merged);
            }
        }
        deep_merge(&mut body, &patch);
    }
    Ok(body)
}

/// Adapt a plain JSON Schema to Extend's subset: every primitive type must be nullable
/// (`"type": ["string", "null"]`) and every enum must offer `null`, or the API answers `400`.
/// Items of arrays of primitives are the documented exception and stay non-nullable.
fn adapt_schema(schema: &Value, in_primitive_items: bool) -> Value {
    let Value::Object(map) = schema else { return schema.clone() };
    let mut out = map.clone();
    if let Some(Value::Object(props)) = out.get_mut("properties") {
        let adapted: serde_json::Map<String, Value> =
            props.iter().map(|(k, v)| (k.clone(), adapt_schema(v, false))).collect();
        *props = adapted;
    }
    if let Some(items) = out.get("items").cloned() {
        let primitive_items = matches!(items.get("type").and_then(Value::as_str), Some(t) if is_primitive(t));
        out.insert("items".into(), adapt_schema(&items, primitive_items));
    }
    if in_primitive_items {
        return Value::Object(out);
    }
    if let Some(Value::String(t)) = out.get("type").cloned() {
        if is_primitive(&t) {
            // The union member is the *string* "null"; enums below use a real JSON null instead.
            out.insert("type".into(), json!([t, "null"]));
        }
    }
    if let Some(Value::Array(values)) = out.get_mut("enum") {
        if !values.iter().any(Value::is_null) {
            values.push(Value::Null);
        }
    }
    Value::Object(out)
}

fn is_primitive(t: &str) -> bool {
    matches!(t, "string" | "number" | "integer" | "boolean")
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UploadResponse {
    id: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParseRun {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub output: Option<Output>,
    #[serde(default)]
    pub output_url: Option<String>,
    #[serde(default)]
    pub metrics: Option<Metrics>,
    #[serde(default)]
    pub usage: Option<RunUsage>,
    #[serde(default)]
    pub file: Option<Value>,
    #[serde(default)]
    pub config: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Output {
    #[serde(default)]
    pub chunks: Vec<Chunk>,
    #[serde(default)]
    pub metadata: Option<OutputMetadata>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputMetadata {
    #[serde(default)]
    pub pages: Option<Vec<PageMeta>>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageMeta {
    pub number: u32,
    #[serde(default)]
    pub original_page_width: Option<f64>,
    #[serde(default)]
    pub original_page_height: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chunk {
    #[serde(rename = "type", default)]
    pub chunk_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub metadata: Option<ChunkMeta>,
    #[serde(default)]
    pub blocks: Vec<WireBlock>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChunkMeta {
    #[serde(default)]
    pub page_range: Option<PageRange>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct PageRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireBlock {
    #[serde(rename = "type", default)]
    pub block_type: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub metadata: Option<BlockMeta>,
    #[serde(default)]
    pub bounding_box: Option<WireBBox>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlockMeta {
    #[serde(default)]
    pub page: Option<BlockPage>,
    #[serde(default)]
    pub avg_ocr_confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct BlockPage {
    pub number: u32,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct WireBBox {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

// ---- extract wire types -------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtractRun {
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub output: Option<ExtractOutput>,
    #[serde(default)]
    pub usage: Option<ExtractUsage>,
    #[serde(default)]
    pub file: Option<Value>,
    #[serde(default)]
    pub parse_run_id: Option<String>,
    #[serde(default)]
    pub dashboard_url: Option<String>,
    #[serde(default)]
    pub reviewed: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct ExtractOutput {
    /// The extracted object, shaped by the request schema.
    #[serde(default)]
    pub value: Value,
    /// Per-field details keyed by path notation (`line_items[0].description`).
    #[serde(default)]
    pub metadata: BTreeMap<String, FieldMeta>,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldMeta {
    #[serde(default)]
    pub ocr_confidence: Option<f64>,
    #[serde(default)]
    pub logprobs_confidence: Option<f64>,
    #[serde(default)]
    pub review_agent_score: Option<f64>,
    #[serde(default)]
    pub citations: Vec<WireCitation>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireCitation {
    #[serde(default)]
    pub page: Option<CitationPage>,
    #[serde(default)]
    pub reference_text: Option<String>,
    #[serde(default)]
    pub polygon: Vec<Point>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct CitationPage {
    pub number: u32,
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub height: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtractUsage {
    #[serde(default)]
    pub credits: Option<f64>,
    /// Includes the parse run an extract run triggers on a fresh file.
    #[serde(default)]
    pub total_credits: Option<f64>,
    #[serde(default)]
    pub breakdown: Vec<UsageEntry>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct UsageEntry {
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub credits: Option<f64>,
    #[serde(default)]
    pub charges: Vec<UsageCharge>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub(crate) struct UsageCharge {
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub quantity: Option<f64>,
    #[serde(default)]
    pub credits: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Metrics {
    #[serde(default)]
    pub page_count: Option<f64>,
    #[serde(default)]
    pub processing_time_ms: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunUsage {
    #[serde(default)]
    pub credits: Option<f64>,
}

// ---- normalisation -----------------------------------------------------------------------------

fn map_block_type(t: &str) -> BlockType {
    match t {
        "text" | "key_value" => BlockType::Text,
        "heading" => BlockType::Title,
        "section_heading" => BlockType::SectionHeader,
        "table" | "table_head" | "table_cell" => BlockType::Table,
        "figure" => BlockType::Figure,
        "formula" => BlockType::Formula,
        "header" => BlockType::Header,
        "footer" => BlockType::Footer,
        _ => BlockType::Other,
    }
}

pub(crate) fn normalize(run: &ParseRun, output: Output, fmt: OutputFormat) -> ParseResponse {
    let mut page_dims: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
    let mut page_md: BTreeMap<u32, String> = BTreeMap::new();
    let mut blocks: Vec<Block> = Vec::new();

    for chunk in &output.chunks {
        let range = chunk.metadata.as_ref().and_then(|m| m.page_range.as_ref());
        if let Some(r) = range {
            if r.start == r.end && chunk.chunk_type == "page" {
                page_md.insert(r.start, chunk.content.clone());
            }
        }
        let fallback_page = range.map(|r| r.start).unwrap_or(1);
        for b in &chunk.blocks {
            let (page_number, dims) = b
                .metadata
                .as_ref()
                .and_then(|m| m.page.as_ref())
                .map(|p| (p.number, p.width.zip(p.height)))
                .unwrap_or((fallback_page, None));
            if let Some((w, h)) = dims {
                page_dims.entry(page_number).or_insert((w, h));
            }
            let bbox = b.bounding_box.as_ref().and_then(|bb| {
                let (w, h) = page_dims.get(&page_number).copied()?;
                BBox::from_xywh(bb.left, bb.top, bb.right - bb.left, bb.bottom - bb.top, w, h)
            });
            let content = match fmt {
                OutputFormat::Markdown => b.content.clone(),
                OutputFormat::Text => crate::types::markdown_to_text(&b.content),
            };
            blocks.push(Block {
                block_type: map_block_type(&b.block_type),
                content,
                text: None,
                bbox,
                confidence: b.metadata.as_ref().and_then(|m| m.avg_ocr_confidence),
                page_number,
            });
        }
    }

    let mut pages: Vec<Page> = crate::types::pages_from_blocks(blocks, &page_dims);
    // Prefer the chunk-level page markdown (Extend's own page rendering) when available.
    for p in &mut pages {
        if let Some(md) = page_md.get(&p.page_number) {
            p.markdown = md.clone();
            p.text = crate::types::markdown_to_text(md);
        }
    }
    // Pages that only had chunk content but no blocks.
    for (n, md) in &page_md {
        if !pages.iter().any(|p| p.page_number == *n) {
            pages.push(Page {
                page_number: *n,
                width: page_dims.get(n).map(|d| d.0),
                height: page_dims.get(n).map(|d| d.1),
                markdown: md.clone(),
                text: crate::types::markdown_to_text(md),
                blocks: vec![],
            });
        }
    }
    if let OutputFormat::Text = fmt {
        for p in &mut pages {
            p.markdown = p.text.clone();
        }
    }

    let billed_pages = run
        .metrics
        .as_ref()
        .and_then(|m| m.page_count)
        .map(|p| p.round() as u32)
        .filter(|&p| p > 0)
        .unwrap_or(pages.len() as u32);
    let usage =
        Usage { pages: billed_pages, credits: run.usage.as_ref().and_then(|u| u.credits), provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(
        NAME,
        &format!(
            "{NAME}/{}",
            run.config.as_ref().and_then(|c| c.get("engine")).and_then(Value::as_str).unwrap_or("parse_performance")
        ),
        pages,
        usage,
    );
    if let Some(ms) = run.metrics.as_ref().and_then(|m| m.processing_time_ms) {
        resp.metadata.insert("extend_processing_time_ms".into(), json!(ms));
    }
    resp
}

// ---- extract normalisation -----------------------------------------------------------------------

/// Convert Extend's path notation (`line_items[0].description`) to a JSON pointer
/// (`/line_items/0/description`), escaping `~` and `/` inside names (RFC 6901).
fn key_to_pointer(key: &str) -> String {
    let mut out = String::new();
    let mut push = |seg: &str| {
        if !seg.is_empty() {
            out.push('/');
            out.push_str(&seg.replace('~', "~0").replace('/', "~1"));
        }
    };
    for part in key.split('.') {
        let (name, mut rest) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        push(name);
        while let Some(end) = rest.strip_prefix('[').and_then(|r| r.find(']').map(|i| (r, i))) {
            let (inner, i) = end;
            push(&inner[..i]);
            rest = &inner[i + 1..];
        }
    }
    out
}

/// Extend polygons are page-pixel points; take their axis-aligned bounds and normalise.
fn polygon_bbox(c: &WireCitation) -> Option<BBox> {
    let page = c.page.as_ref()?;
    let (w, h) = (page.width?, page.height?);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &c.polygon {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    if x1 < x0 || y1 < y0 {
        return None;
    }
    BBox::from_xywh(x0, y0, x1 - x0, y1 - y0, w, h)
}

pub(crate) fn normalize_extract(run: &ExtractRun, output: &ExtractOutput, model: &str) -> ExtractResponse {
    let mut fields: BTreeMap<String, FieldInfo> = BTreeMap::new();
    let mut max_cited_page = 0u32;
    for (key, meta) in &output.metadata {
        let citations: Vec<Citation> = meta
            .citations
            .iter()
            .map(|c| {
                let page_number = c.page.as_ref().map(|p| p.number).unwrap_or(1);
                max_cited_page = max_cited_page.max(page_number);
                Citation {
                    page_number,
                    bbox: polygon_bbox(c),
                    text: c.reference_text.clone().filter(|t| !t.is_empty()),
                }
            })
            .collect();
        // `logprobsConfidence` is being phased out (null on newer processors); prefer OCR confidence.
        let confidence = meta.ocr_confidence.or(meta.logprobs_confidence);
        if confidence.is_some() || !citations.is_empty() {
            fields.insert(key_to_pointer(key), FieldInfo { confidence, citations });
        }
    }

    // Pages billed: the `page`-unit charges on the run's own usage line, else the file's page count,
    // else the highest cited page.
    let usage_pages = run
        .usage
        .as_ref()
        .map(|u| {
            u.breakdown
                .iter()
                .flat_map(|e| e.charges.iter())
                .filter(|c| c.unit.as_deref() == Some("page"))
                .filter_map(|c| c.quantity)
                .fold(0.0_f64, f64::max)
                .round() as u32
        })
        .unwrap_or(0);
    let file_pages = run
        .file
        .as_ref()
        .and_then(|f| f.get("metadata"))
        .and_then(|m| m.get("pageCount"))
        .and_then(Value::as_f64)
        .map(|p| p.round() as u32)
        .unwrap_or(0);
    let pages = [usage_pages, file_pages, max_cited_page, 1].into_iter().find(|&p| p > 0).unwrap_or(1);

    let usage = Usage {
        pages,
        credits: run.usage.as_ref().and_then(|u| u.total_credits.or(u.credits)),
        provider_cost_usd: None,
    };
    let mut resp = ExtractResponse::new(NAME, &format!("{NAME}/{model}"), output.value.clone(), usage);
    resp.fields = fields;
    if let Some(id) = &run.parse_run_id {
        resp.metadata.insert("extend_parse_run_id".into(), json!(id));
    }
    if let Some(url) = &run.dashboard_url {
        resp.metadata.insert("extend_dashboard_url".into(), json!(url));
    }
    if let Some(true) = run.reviewed {
        resp.metadata.insert("extend_reviewed".into(), json!(true));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fixture() {
        let raw = include_str!("../../tests/fixtures/extend_parse_run.json");
        let run: ParseRun = serde_json::from_str(raw).unwrap();
        let output = run.output.clone().unwrap();
        let resp = normalize(&run, output, OutputFormat::Markdown);
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(resp.usage.pages, 2);
        assert_eq!(resp.usage.credits, Some(4.0));
        assert!(resp.pages[0].markdown.contains("# Hello LiteOCR"));
        assert!(resp.pages[0].markdown.contains("| Item | Amount |"));
        assert_eq!(resp.pages[1].page_number, 2);
        let first = &resp.pages[0].blocks[0];
        assert_eq!(first.block_type, BlockType::Title);
        let bb = first.bbox.unwrap();
        assert!(bb.x0 > 0.07 && bb.x0 < 0.08, "{bb:?}");
        assert!(bb.y1 < 0.1);
        assert!(first.confidence.unwrap() > 0.9);
        assert!(resp.markdown.contains("Reference: ABC-9876"));
        assert_eq!(resp.pages[0].width, Some(1241.0));
    }

    #[test]
    fn text_output_strips_markdown() {
        let raw = include_str!("../../tests/fixtures/extend_parse_run.json");
        let run: ParseRun = serde_json::from_str(raw).unwrap();
        let output = run.output.clone().unwrap();
        let resp = normalize(&run, output, OutputFormat::Text);
        assert!(!resp.pages[0].markdown.starts_with('#'));
        assert!(resp.text.starts_with("Hello LiteOCR"));
    }

    #[test]
    fn normalizes_extract_fixture() {
        let raw = include_str!("../../tests/fixtures/extend_extract_run.json");
        let run: ExtractRun = serde_json::from_str(raw).unwrap();
        let output = run.output.clone().unwrap();
        let resp = normalize_extract(&run, &output, "extraction_performance");

        assert_eq!(resp.data["invoice_number"], "INV-9865");
        assert_eq!(resp.data["total"], "$14,667.43");
        assert_eq!(resp.data["line_items"][0]["description"], "Hydraulic fluid, 5 gal");
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.usage.credits, Some(3.0));
        assert_eq!(resp.model, "extend/extraction_performance");
        assert!(resp.metadata.contains_key("extend_parse_run_id"));

        let f = &resp.fields["/invoice_number"];
        assert!(f.confidence.unwrap() > 0.9);
        assert_eq!(f.citations.len(), 1);
        assert_eq!(f.citations[0].page_number, 1);
        assert_eq!(f.citations[0].text.as_deref(), Some("Invoice #: INV-9865"));
        // Polygon points (page pixels) become a normalised, axis-aligned box.
        let bb = f.citations[0].bbox.unwrap();
        assert!(bb.x0 > 0.0 && bb.x1 <= 1.0 && bb.y0 > 0.0 && bb.y1 <= 1.0, "{bb:?}");
        assert!(bb.x1 > bb.x0 && bb.y1 > bb.y0, "{bb:?}");
        // Array paths become JSON pointers.
        assert!(resp.fields.contains_key("/line_items/0/amount"), "{:?}", resp.fields.keys().collect::<Vec<_>>());
        assert!(resp.fields.contains_key("/line_items/1/description"));
    }

    #[test]
    fn converts_extend_paths_to_json_pointers() {
        assert_eq!(key_to_pointer("invoice_number"), "/invoice_number");
        assert_eq!(key_to_pointer("line_items[0]"), "/line_items/0");
        assert_eq!(key_to_pointer("line_items[0].description"), "/line_items/0/description");
        assert_eq!(key_to_pointer("a[0][1].b"), "/a/0/1/b");
        assert_eq!(key_to_pointer("a/b.c~d"), "/a~1b/c~0d");
        assert_eq!(key_to_pointer(""), "");
    }

    #[test]
    fn page_count_falls_back_through_usage_file_and_citations() {
        let run: ExtractRun = serde_json::from_str(
            r#"{"id":"exr_1","status":"PROCESSED","file":{"metadata":{"pageCount":7}},
                "usage":{"credits":9,"totalCredits":14,"breakdown":[
                  {"object":"extract_run","credits":9,"charges":[{"unit":"page","quantity":3,"credits":9}]},
                  {"object":"parse_run","credits":5,"charges":[{"unit":"page","quantity":3,"credits":5}]}]}}"#,
        )
        .unwrap();
        let output = ExtractOutput { value: json!({}), metadata: BTreeMap::new() };
        let resp = normalize_extract(&run, &output, "extraction_performance");
        assert_eq!(resp.usage.pages, 3, "page charges win over the file page count");
        assert_eq!(resp.usage.credits, Some(14.0), "credits include the triggered parse run");

        let run: ExtractRun =
            serde_json::from_str(r#"{"id":"exr_1","status":"PROCESSED","file":{"metadata":{"pageCount":7}}}"#).unwrap();
        assert_eq!(normalize_extract(&run, &output, "extraction_light").usage.pages, 7);

        let run: ExtractRun = serde_json::from_str(r#"{"id":"exr_1","status":"PROCESSED"}"#).unwrap();
        let cited = ExtractOutput {
            value: json!({"a": 1}),
            metadata: BTreeMap::from([(
                "a".to_string(),
                serde_json::from_value(json!({"citations": [{"page": {"number": 4}}]})).unwrap(),
            )]),
        };
        let resp = normalize_extract(&run, &cited, "extraction_light");
        assert_eq!(resp.usage.pages, 4);
        assert_eq!(resp.fields["/a"].citations[0].bbox, None, "no page dims ⇒ no box");
    }

    #[test]
    fn adapts_schema_to_extends_subset() {
        let schema = json!({
            "type": "object",
            "properties": {
                "invoice_number": {"type": "string"},
                "paid": {"type": "boolean"},
                "status": {"type": "string", "enum": ["paid", "due"]},
                "tags": {"type": "array", "items": {"type": "string"}},
                "line_items": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"amount": {"type": "number"}},
                    "required": ["amount"]
                }}
            },
            "required": ["invoice_number"]
        });
        let a = adapt_schema(&schema, false);
        assert_eq!(a["type"], "object", "objects and arrays stay as they are");
        assert_eq!(a["properties"]["invoice_number"]["type"], json!(["string", "null"]));
        assert_eq!(a["properties"]["paid"]["type"], json!(["boolean", "null"]));
        assert_eq!(a["properties"]["status"]["enum"], json!(["paid", "due", null]));
        assert_eq!(a["properties"]["tags"]["items"]["type"], "string", "primitive array items stay non-nullable");
        assert_eq!(a["properties"]["line_items"]["items"]["properties"]["amount"]["type"], json!(["number", "null"]));
        assert_eq!(a["required"], json!(["invoice_number"]));
        // Already-nullable schemas are left alone.
        assert_eq!(adapt_schema(&json!({"type": ["string", "null"]}), false), json!({"type": ["string", "null"]}));
    }

    #[test]
    fn extract_body_maps_model_instructions_and_citations() {
        let doc = DocumentRequest::from_url("https://x/y.pdf").pages("1-2,5");
        let req = ExtractRequest::new(doc, json!({"type": "object", "properties": {"a": {"type": "string"}}}))
            .instructions("Totals are USD")
            .citations(true);
        let b = build_extract_body(&req, "extraction_light", json!({"url": "https://x/y.pdf"})).unwrap();
        assert_eq!(b["config"]["baseProcessor"], "extraction_light");
        assert_eq!(b["config"]["parseConfig"]["engine"], "parse_light");
        assert_eq!(b["config"]["extractionRules"], "Totals are USD");
        assert_eq!(b["config"]["schema"]["properties"]["a"]["type"], json!(["string", "null"]));
        assert_eq!(b["config"]["advancedOptions"]["citationsEnabled"], true);
        assert_eq!(b["config"]["advancedOptions"]["pageRanges"][1]["start"], 5);
        assert_eq!(b["config"]["advancedOptions"]["pageRanges"][1]["end"], 5);

        let plain = ExtractRequest::new(
            DocumentRequest::from_url("u").provider_options(
                json!({"advancedOptions": {"reviewAgent": {"enabled": true}}, "metadata": {"k": "v"}}),
            ),
            json!({"type": "object"}),
        );
        let b = build_extract_body(&plain, "extraction_performance", json!({"id": "file_1"})).unwrap();
        assert_eq!(b["config"]["baseProcessor"], "extraction_performance");
        assert_eq!(b["config"]["parseConfig"]["engine"], "parse_performance");
        assert!(b["config"].get("advancedOptions").and_then(|o| o.get("citationsEnabled")).is_none());
        assert_eq!(b["config"]["advancedOptions"]["reviewAgent"]["enabled"], true);
        assert_eq!(b["metadata"]["k"], "v");
        assert!(build_extract_body(&plain, "parse_light", json!({})).is_err());
    }

    #[test]
    fn body_merges_provider_options_and_pages() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .pages("1-2,5")
            .provider_options(json!({"blockOptions": {"figures": {"enabled": false}}, "metadata": {"k": "v"}}));
        let body = build_body(&req, "parse_light", json!({"url": "https://x/y.pdf"})).unwrap();
        assert_eq!(body["config"]["engine"], "parse_light");
        assert_eq!(body["config"]["blockOptions"]["tables"]["targetFormat"], "markdown");
        assert_eq!(body["config"]["blockOptions"]["figures"]["enabled"], false);
        assert_eq!(body["config"]["advancedOptions"]["pageRanges"][1]["start"], 5);
        assert_eq!(body["metadata"]["k"], "v");
        assert!(build_body(&req, "nope", json!({})).is_err());
    }

    // ---- live ---------------------------------------------------------------------------------

    const LIVE_DOC: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/invoice_001.png");

    #[tokio::test]
    #[ignore = "needs EXTEND_API_KEY and network"]
    async fn live_extract() {
        if std::env::var("EXTEND_API_KEY").map(|v| v.trim().is_empty()).unwrap_or(true) {
            eprintln!("skipping extend live extract: EXTEND_API_KEY not set");
            return;
        }
        let schema = json!({
            "type": "object",
            "properties": {
                "invoice_number": {"type": "string"},
                "total": {"type": "string"},
                "date": {"type": "string"},
                "vendor": {"type": "string"}
            },
            "required": ["invoice_number", "total"]
        });
        let doc = DocumentRequest::from_path(LIVE_DOC).timeout_secs(240.0);
        let req = ExtractRequest::new(doc, schema)
            .instructions("Extract the invoice header fields exactly as printed.")
            .citations(true);
        let resp = Extend.extract(&req, "extraction_light").await.expect("extend extract succeeds");
        assert_eq!(resp.data["invoice_number"], "INV-9865");
        assert!(resp.data["total"].as_str().unwrap().contains("14,667.43"), "{}", resp.data["total"]);
        assert_eq!(resp.data["vendor"], "Cedar Ridge Supply");
        assert_eq!(resp.usage.pages, 1);
        assert!(resp.provider_job_id.is_some());
        let f = &resp.fields["/invoice_number"];
        assert_eq!(f.citations[0].page_number, 1);
        assert!(f.citations[0].bbox.is_some());
        assert!(f.confidence.unwrap() > 0.5);
    }
}

// ---- loopback transport tests --------------------------------------------------------------------

/// End-to-end tests against a local HTTP server: the real requests the provider builds and the
/// real response paths, including `responseType=url` output downloads.
#[cfg(test)]
mod wire {
    use super::*;
    use crate::jobs::RetrieveOptions;
    use crate::testutil::{fixture, pdf_request, serve};

    const STORAGE: &str = "https://example-storage.invalid";
    const RUN: &str = "pr_Q7uRlTestUrlRun0000001";

    fn upload() -> (u16, String) {
        (200, json!({"object": "file", "id": "file_up1", "name": "doc.pdf"}).to_string())
    }

    fn processing(id: &str) -> (u16, String) {
        (
            200,
            json!({"object": "parse_run", "id": id, "status": "PROCESSING", "output": null, "outputUrl": null})
                .to_string(),
        )
    }

    /// The captured `GET /parse_runs/{id}?responseType=url` answer, pointed at the loopback server.
    fn url_run() -> String {
        fixture("extend_parse_run_url.json").replace(STORAGE, "{base}")
    }

    fn opts(base: &str) -> RetrieveOptions {
        RetrieveOptions {
            api_key: Some("test-key".into()),
            base_url: Some(base.into()),
            max_retries: 0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn parse_downloads_a_url_output() {
        let (base, seen) = serve(vec![
            upload(),
            processing(RUN),
            (200, url_run()),
            (200, fixture("extend_parse_run_url_output.json")),
        ])
        .await;
        let req = pdf_request(&base).provider_options(json!({"responseType": "url", "workspace_id": "ws_1"}));
        let resp = Extend.parse(&req, "parse_light").await.expect("parse");

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 4);
        assert_eq!(seen[0].route(), "POST /files/upload");
        assert_eq!(seen[1].route(), "POST /parse_runs");
        let body: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert!(body.get("responseType").is_none(), "responseType is a query parameter, not a body key");
        assert!(body.get("workspace_id").is_none());
        assert_eq!(body["config"]["engine"], "parse_light");
        assert_eq!(seen[2].route(), format!("GET /parse_runs/{RUN}?responseType=url"));
        assert_eq!(seen[2].header("x-extend-api-version").as_deref(), Some(API_VERSION));
        assert_eq!(seen[2].header("x-extend-workspace-id").as_deref(), Some("ws_1"));
        assert!(seen[3].route().starts_with("GET /org__REDACTED/ws_REDACTED/parser-runs/"));
        assert_eq!(seen[3].header("authorization"), None, "presigned URLs are fetched without the key");

        assert_eq!(resp.provider_job_id.as_deref(), Some(RUN));
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.usage.credits, Some(0.5));
        assert!(resp.markdown.contains("Cedar Ridge Supply"));
        assert_eq!(resp.pages[0].blocks.len(), 7);
        assert_eq!(resp.pages[0].width, Some(1240.0));
        assert!(resp.pages[0].blocks.iter().all(|b| b.bbox.is_some()));
    }

    #[tokio::test]
    async fn a_run_with_neither_output_nor_url_is_an_error() {
        let mut run: Value = serde_json::from_str(&url_run()).unwrap();
        run["outputUrl"] = Value::Null;
        let (base, _) = serve(vec![upload(), (200, run.to_string())]).await;
        let e = Extend.parse(&pdf_request(&base), "parse_light").await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Provider);
        assert!(e.message.contains("without output"));
        assert_eq!(e.job_id.as_deref(), Some(RUN));
    }

    // ---- jobs API --------------------------------------------------------------------------------

    #[tokio::test]
    async fn submit_then_retrieve_until_done() {
        let (base, seen) = serve(vec![
            upload(),
            processing("pr_job1"),
            processing("pr_job1"),
            (200, url_run()),
            (200, fixture("extend_parse_run_url_output.json")),
        ])
        .await;
        let req = pdf_request(&base)
            .model("extend/parse_light")
            .provider_options(json!({"workspace_id": "ws_1", "responseType": "url", "metadata": {"k": "v"}}));
        let handle = crate::submit_parse(req).await.expect("submit");
        assert_eq!(handle.job_id, "pr_job1");
        assert_eq!(handle.model, "extend/parse_light");
        assert_eq!(handle.provider_state, Some(json!({"workspace_id": "ws_1", "responseType": "url"})));

        assert!(crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap().is_pending());
        let JobStatus::Succeeded(resp) = crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap() else {
            panic!("expected success")
        };
        assert_eq!(resp.model, "extend/parse_light");
        assert!(resp.markdown.contains("Cedar Ridge Supply"));
        assert!(resp.cost_usd.is_some());

        let seen = seen.lock().unwrap();
        let body: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(body["metadata"], json!({"k": "v"}), "extend's own run metadata still goes through");
        assert_eq!(seen[2].route(), "GET /parse_runs/pr_job1?responseType=url");
        assert_eq!(seen[2].header("x-extend-workspace-id").as_deref(), Some("ws_1"), "state survives the handle");
    }

    #[tokio::test]
    async fn retrieve_reports_a_failed_run() {
        let failed = json!({"object": "parse_run", "id": "pr_f", "status": "FAILED",
            "failureReason": "PASSWORD_PROTECTED_FILE", "failureMessage": "The file is password protected."});
        let (base, _) = serve(vec![(200, failed.to_string())]).await;
        let handle = crate::JobHandle::new(NAME, "extend/parse_performance", "pr_f");
        let JobStatus::Failed(e) = crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap() else {
            panic!("expected failure")
        };
        assert_eq!(e.kind, ErrorKind::BadRequest);
        assert!(e.message.contains("PASSWORD_PROTECTED_FILE") && e.message.contains("password protected"));
        assert_eq!(e.job_id.as_deref(), Some("pr_f"));
    }

    #[tokio::test]
    async fn webhook_url_is_rejected_before_any_request() {
        let (base, seen) = serve(vec![]).await;
        let req = pdf_request(&base).model("extend").webhook_url("https://hooks.example.com/x");
        let e = crate::submit_parse(req).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Input);
        assert!(e.message.contains("webhook_endpoints"), "{e}");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn webhook_events_map_to_statuses() {
        let failed = json!({"eventId": "evt_1", "eventType": "parse_run.failed", "payload": {
            "object": "parse_run_status", "id": "pr_9", "status": "FAILED",
            "failureReason": "OUT_OF_CREDITS", "failureMessage": "No credits left.", "metadata": null}});
        let event = crate::parse_webhook("extend", &failed).unwrap();
        let WebhookStatus::Failed(e) = event.status else { panic!("expected failure") };
        assert_eq!(e.kind, ErrorKind::Authentication);
        assert!(e.message.contains("No credits left."));
        assert_eq!(event.job.unwrap().job_id, "pr_9");

        let processed = json!({"eventId": "evt_2", "eventType": "parse_run.processed", "payload": {
            "object": "parse_run_status", "id": RUN, "status": "PROCESSED", "failureReason": null}});
        let event = crate::parse_webhook("extend/parse_light", &processed).unwrap();
        assert!(matches!(event.status, WebhookStatus::Finished));
        assert_eq!(event.job.as_ref().unwrap().model, "extend/parse_light");

        let signed = json!({"eventId": "e", "eventType": "parse_run.processed", "payload": {"data": "https://x"}});
        assert_eq!(crate::parse_webhook("extend", &signed).unwrap_err().kind, ErrorKind::Input);
        let other = json!({"eventId": "e", "eventType": "extract_run.processed", "payload": {"id": "exr_1"}});
        assert_eq!(crate::parse_webhook("extend", &other).unwrap_err().kind, ErrorKind::Input);

        let (base, seen) = serve(vec![(200, fixture("extend_parse_run.json"))]).await;
        let status = crate::resolve_webhook("extend/parse_light", &processed, &opts(&base)).await.unwrap();
        let JobStatus::Succeeded(resp) = status else { panic!("expected success") };
        assert_eq!(resp.pages.len(), 2);
        assert_eq!(seen.lock().unwrap()[0].route(), format!("GET /parse_runs/{RUN}"));
    }
}
