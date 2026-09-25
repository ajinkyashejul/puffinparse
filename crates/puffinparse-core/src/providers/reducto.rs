//! Reducto (reducto.ai) document parsing and extraction.
//!
//! Parse flow: `POST /upload` (multipart) unless the input is a URL → `POST /parse` (sync, default)
//! or `POST /parse_async` + `GET /job/{id}` when `provider_options.async == true`.
//! Results with `result.type == "url"` are fetched from the presigned URL.
//!
//! Extract flow: the same upload step → `POST /extract` (or `/extract_async` + `GET /job/{id}`) with
//! `instructions.schema` and `settings.citations`.
//!
//! Jobs API (`crate::submit_parse` / `retrieve_parse`): `POST /parse_async` (with
//! `async.webhook` for `webhook_url`) → one `GET /job/{id}` per retrieve.

use crate::error::{Error, Result};
use crate::http::{self, Deadline, Retry};
use crate::jobs::{JobHandle, JobStatus, WebhookEvent, WebhookStatus};
use crate::provider::{self, Provider};
use crate::types::{
    BBox, Block, BlockType, Citation, DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, FieldInfo,
    OutputFormat, Page, ParseResponse, Usage,
};
use crate::util::deep_merge;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

pub const NAME: &str = "reducto";
const ENV_KEY: &str = "REDUCTO_API_KEY";
const ENV_BASE: &str = "REDUCTO_BASE_URL";
const DEFAULT_BASE: &str = "https://platform.reducto.ai";

#[derive(Debug, Default, Clone, Copy)]
pub struct Reducto;

#[async_trait]
impl Provider for Reducto {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn parse(&self, request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        let ctx = Ctx::new(request)?;

        // 1. Input reference.
        let input = resolve_input(request, ctx.client, &ctx.base, &ctx.api_key, ctx.retry, &ctx.deadline).await?;

        // 2. Body.
        let use_async = request.option("async").and_then(Value::as_bool).unwrap_or(false);
        let body = build_body(request, model, &input)?;

        // 3. Parse (sync or async + poll).
        let parsed: WireParseResponse = if use_async {
            let job_id = ctx.submit("parse_async", &body).await?;
            tracing::debug!(job_id = %job_id, "reducto: async job submitted");
            let result = ctx.wait(&job_id).await?;
            parse_job_result(result, &job_id)?
        } else {
            ctx.post("parse", &body).await?
        };

        // 4. Large results come back as a presigned URL holding the same `FullResult` object.
        ctx.finish_parse(parsed, request, model).await
    }

    async fn extract(&self, request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        let doc = &request.document;
        let ctx = Ctx::new(doc)?;

        // 1. Input reference (identical to the parse path: URL through, everything else uploaded).
        let input = resolve_input(doc, ctx.client, &ctx.base, &ctx.api_key, ctx.retry, &ctx.deadline).await?;

        // 2. Body.
        let use_async = doc.option("async").and_then(Value::as_bool).unwrap_or(false);
        let body = build_extract_body(request, model, &input)?;

        // 3. Extract (sync, or async + poll on the shared /job endpoint).
        let wire: WireExtractResponse = if use_async {
            let job_id = ctx.submit("extract_async", &body).await?;
            tracing::debug!(job_id = %job_id, "reducto: async extract job submitted");
            let result = ctx.wait(&job_id).await?;
            serde_json::from_value(result).map_err(|e| {
                Error::provider(format!("unexpected job result shape: {e}")).with_provider(NAME).with_job_id(job_id)
            })?
        } else {
            ctx.post("extract", &body).await?
        };

        // 4. `settings.force_url_result` (and large results) return a presigned URL holding the
        //    bare result value.
        let result = match url_result(&wire.result) {
            None => wire.result.clone(),
            Some(url) => {
                let resp = ctx.client.get(url).timeout(ctx.deadline.request_timeout()).send().await?;
                let body = http::read_response(NAME, resp).await?;
                serde_json::from_str(&body).map_err(|e| {
                    Error::provider(format!(
                        "unexpected result URL payload: {e}; body starts: {}",
                        http::snippet(&body)
                    ))
                    .with_provider(NAME)
                })?
            }
        };

        let raw = if doc.include_raw { Some(serde_json::to_value(&wire)?) } else { None };
        let mut resp = normalize_extract(&wire, &result, model);
        resp.provider_job_id = wire.job_id.clone();
        resp.raw = raw;
        Ok(resp)
    }

    /// `POST /parse_async` with the same body as `parse`, plus `async.webhook` for `webhook_url`.
    async fn submit_parse(&self, request: &DocumentRequest, model: &str) -> Result<JobHandle> {
        let ctx = Ctx::new(request)?;
        let input = resolve_input(request, ctx.client, &ctx.base, &ctx.api_key, ctx.retry, &ctx.deadline).await?;
        let body = build_submit_body(request, model, &input)?;
        let job_id = ctx.submit("parse_async", &body).await?;
        tracing::debug!(job_id = %job_id, "reducto: job submitted");
        Ok(JobHandle::new(NAME, model, job_id))
    }

    /// One `GET /job/{id}`; a completed job's result is followed through a presigned URL if needed.
    async fn retrieve_parse(&self, job: &JobHandle, request: &DocumentRequest, model: &str) -> Result<JobStatus> {
        let ctx = Ctx::new(request)?;
        let job_id = job.job_id.as_str();
        let wire = ctx.job(job_id).await?;
        match job_outcome(wire, job_id) {
            JobOutcome::Running => Ok(JobStatus::Pending),
            JobOutcome::Failed(e) => Ok(JobStatus::Failed(e)),
            JobOutcome::Completed(result) => {
                let parsed = parse_job_result(result, job_id)?;
                let resp = ctx.finish_parse(parsed, request, model).await.map_err(|e| e.with_job_id(job_id))?;
                Ok(JobStatus::Succeeded(Box::new(resp)))
            }
        }
    }

    /// Direct webhooks POST `{"status": "Completed" | "Failed", "job_id": …, "metadata": …}`.
    /// The body carries neither the result nor a failure reason, so both are `Finished`.
    fn parse_webhook(&self, model: &str, payload: &Value) -> Result<WebhookEvent> {
        let job_id = payload
            .get("job_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::input("reducto webhook payload has no job_id").with_provider(NAME))?;
        let status = match payload.get("status").and_then(Value::as_str) {
            Some("Completed" | "Failed" | "Cancelled") => WebhookStatus::Finished,
            _ => WebhookStatus::Pending,
        };
        Ok(WebhookEvent { job: Some(JobHandle::new(NAME, model, job_id)), status })
    }
}

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

    /// `POST {base}/{path}` with retries, decoding the JSON answer.
    async fn post<T: serde::de::DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        let url = format!("{}/{path}", self.base);
        http::with_retry(NAME, self.retry, &self.deadline, || {
            let rb =
                self.client.post(&url).bearer_auth(&self.api_key).timeout(self.deadline.request_timeout()).json(body);
            async move { http::read_json(NAME, rb.send().await?).await }
        })
        .await
    }

    /// Submit to an `*_async` endpoint and return the job id.
    async fn submit(&self, path: &str, body: &Value) -> Result<String> {
        let submit: AsyncParseResponse = self.post(path, body).await?;
        Ok(submit.job_id)
    }

    /// One `GET {base}/job/{id}` (no retry: the polling loop is the retry).
    async fn job_once(&self, job_id: &str) -> Result<JobResponse> {
        let rb = self
            .client
            .get(format!("{}/job/{job_id}", self.base))
            .bearer_auth(&self.api_key)
            .timeout(self.deadline.request_timeout());
        http::read_json(NAME, rb.send().await?).await
    }

    /// `GET {base}/job/{id}` with retries on transient errors.
    async fn job(&self, job_id: &str) -> Result<JobResponse> {
        http::with_retry(NAME, self.retry, &self.deadline, || self.job_once(job_id))
            .await
            .map_err(|e| e.with_job_id(job_id))
    }

    /// Poll the job until it is terminal; returns a completed job's `result`.
    async fn wait(&self, job_id: &str) -> Result<Value> {
        let job = http::poll_until(NAME, &self.deadline, Duration::from_secs(1), Duration::from_secs(8), || async {
            let job = self.job_once(job_id).await?;
            Ok(match job.status.as_str() {
                "Completed" | "Failed" | "Cancelled" => Some(job),
                _ => None,
            })
        })
        .await
        .map_err(|e| e.with_job_id(job_id))?;
        match job_outcome(job, job_id) {
            JobOutcome::Completed(result) => Ok(result),
            JobOutcome::Failed(e) => Err(e),
            JobOutcome::Running => unreachable!("poll_until only returns terminal jobs"),
        }
    }

    /// Follow a `url` result if needed, then normalise.
    async fn finish_parse(
        &self,
        parsed: WireParseResponse,
        request: &DocumentRequest,
        model: &str,
    ) -> Result<ParseResponse> {
        let job_id = parsed.job_id.clone();
        let full = match &parsed.result {
            ParseResult::Full(f) => f.clone(),
            ParseResult::Url { url, .. } => {
                let resp = self.client.get(url).timeout(self.deadline.request_timeout()).send().await?;
                match http::read_json::<ParseResult>(NAME, resp).await.map_err(|e| e.with_job_id(job_id.clone()))? {
                    ParseResult::Full(f) => f,
                    ParseResult::Url { .. } => {
                        return Err(Error::provider("result URL returned another URL")
                            .with_provider(NAME)
                            .with_job_id(job_id))
                    }
                }
            }
        };
        let raw = if request.include_raw { Some(serde_json::to_value(&parsed)?) } else { None };
        let mut resp = normalize(&parsed, &full, request.output, model);
        resp.provider_job_id = Some(job_id);
        resp.raw = raw;
        Ok(resp)
    }
}

/// Where a `GET /job/{id}` answer leaves the job.
enum JobOutcome {
    Running,
    Completed(Value),
    Failed(Error),
}

/// `Completed` → the result; `Failed` / `Cancelled` → a provider error carrying `reason` (or
/// `error.message`) and the job id; anything else (`Pending`, `Idle`, `InProgress`, `Completing`)
/// is still running.
fn job_outcome(job: JobResponse, job_id: &str) -> JobOutcome {
    match job.status.as_str() {
        "Completed" => match job.result {
            Some(result) => JobOutcome::Completed(result),
            None => JobOutcome::Failed(
                Error::provider("completed job has no result").with_provider(NAME).with_job_id(job_id),
            ),
        },
        "Failed" | "Cancelled" => {
            let reason = job
                .reason
                .or_else(|| job.error.as_ref().and_then(|e| e.get("message")).and_then(Value::as_str).map(String::from))
                .unwrap_or_else(|| "unknown".into());
            JobOutcome::Failed(
                Error::provider(format!("job {}: {reason}", job.status)).with_provider(NAME).with_job_id(job_id),
            )
        }
        _ => JobOutcome::Running,
    }
}

/// A completed parse job's `result` is the same object the sync `/parse` returns.
fn parse_job_result(result: Value, job_id: &str) -> Result<WireParseResponse> {
    serde_json::from_value(result).map_err(|e| {
        Error::provider(format!("unexpected job result shape: {e}")).with_provider(NAME).with_job_id(job_id)
    })
}

/// The `/parse_async` body: the `parse` body, Reducto's own `async` object if the caller passed
/// one, and `async.webhook = {"mode": "direct", "url": …}` for `webhook_url`.
fn build_submit_body(request: &DocumentRequest, model: &str, input: &str) -> Result<Value> {
    let mut body = build_body(request, model, input)?;
    if let Some(Value::Object(native)) = request.option("async") {
        body["async"] = Value::Object(native.clone());
    }
    if let Some(url) = provider::webhook_url(request)? {
        if !body["async"].is_object() {
            body["async"] = json!({});
        }
        body["async"]["webhook"] = json!({ "mode": "direct", "url": url });
    }
    Ok(body)
}

/// Resolve the document to a Reducto `input` string: URLs are passed through, bytes and paths are
/// uploaded to `POST /upload` and referenced by their `reducto://` id.
async fn resolve_input(
    request: &DocumentRequest,
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    retry: Retry,
    deadline: &Deadline,
) -> Result<String> {
    match provider::load_bytes(&request.input).await? {
        None => {
            let DocumentInput::Url { url } = &request.input else { unreachable!() };
            Ok(url.clone())
        }
        Some(data) => {
            let upload: UploadResponse = http::with_retry(NAME, retry, deadline, || {
                let form =
                    reqwest::multipart::Form::new().part("file", provider::file_part(data.clone(), &request.input));
                let rb = client
                    .post(format!("{base}/upload"))
                    .bearer_auth(api_key)
                    .timeout(deadline.request_timeout())
                    .multipart(form);
                async move { http::read_json(NAME, rb.send().await?).await }
            })
            .await?;
            tracing::debug!(file_id = %upload.file_id, "reducto: uploaded");
            Ok(upload.file_id)
        }
    }
}

fn build_body(request: &DocumentRequest, model: &str, input: &str) -> Result<Value> {
    let mut body = json!({
        "input": input,
        "retrieval": { "chunking": { "chunk_mode": "page" } },
        "formatting": { "table_output_format": "md" },
        "settings": {},
    });
    match model {
        "standard" => {}
        "r-1" => body["settings"]["model"] = json!("r-1"),
        "agentic" => body["enhance"] = json!({ "agentic": [{ "scope": "text" }, { "scope": "table" }] }),
        other => return Err(Error::unsupported_model(format!("reducto: unknown model '{other}'"))),
    }
    if let Some(pages) = &request.pages {
        let ranges: Vec<Value> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) => json!({ "start": s, "end": e }),
                None => json!({ "start": s }),
            })
            .collect();
        body["settings"]["page_range"] = Value::Array(ranges);
    }
    if let Some(opts) = &request.provider_options {
        let mut patch = opts.clone();
        if let Value::Object(o) = &mut patch {
            o.remove("async");
        }
        deep_merge(&mut body, &patch);
    }
    Ok(body)
}

fn build_extract_body(request: &ExtractRequest, model: &str, input: &str) -> Result<Value> {
    let doc = &request.document;
    let mut body = json!({
        "input": input,
        "instructions": { "schema": request.schema },
        "settings": {},
    });
    if let Some(prompt) = request.instructions.as_deref().filter(|s| !s.trim().is_empty()) {
        body["instructions"]["system_prompt"] = json!(prompt);
    }
    match model {
        "extract" => {}
        // Deep Extract: agentic loop that iteratively refines the output (2x the list price).
        "deep_extract" => body["settings"]["deep_extract"] = json!(true),
        other => return Err(Error::unsupported_model(format!("reducto: unknown extract model '{other}'"))),
    }
    if request.citations {
        body["settings"]["citations"] = json!({ "enabled": true });
    }
    if let Some(pages) = &doc.pages {
        let ranges: Vec<Value> = crate::util::parse_page_ranges(pages)?
            .into_iter()
            .map(|(s, e)| match e {
                Some(e) => json!({ "start": s, "end": e }),
                None => json!({ "start": s }),
            })
            .collect();
        body["settings"]["page_range"] = Value::Array(ranges);
    }
    if let Some(opts) = &doc.provider_options {
        let mut patch = opts.clone();
        if let Value::Object(o) = &mut patch {
            o.remove("async");
        }
        deep_merge(&mut body, &patch);
    }
    Ok(body)
}

// ---- wire types -------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UploadResponse {
    file_id: String,
}

#[derive(Debug, Deserialize)]
struct AsyncParseResponse {
    job_id: String,
}

#[derive(Debug, Deserialize)]
struct JobResponse {
    status: String,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireParseResponse {
    pub job_id: String,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub usage: Option<ParseUsage>,
    pub result: ParseResult,
    #[serde(default)]
    pub studio_link: Option<String>,
}

/// `POST /extract` — `response_type` is `"extract"` without citations (`result` is a list) and
/// `"v3_extract"` with them (`result` is an object of `{value, citations}` leaves).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireExtractResponse {
    #[serde(default)]
    pub response_type: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub usage: Option<ExtractUsage>,
    #[serde(default)]
    pub studio_link: Option<String>,
    /// Document-level Deep Extract confidence label (`"high"` / `"low"`).
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub confidence_reason: Option<String>,
    #[serde(default)]
    pub result: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ExtractUsage {
    #[serde(default)]
    pub num_pages: u32,
    #[serde(default)]
    pub num_fields: Option<u32>,
    #[serde(default)]
    pub credits: Option<f64>,
    #[serde(default)]
    pub extract_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct ParseUsage {
    #[serde(default)]
    pub num_pages: u32,
    #[serde(default)]
    pub credits: Option<f64>,
    #[serde(default)]
    pub credit_breakdown: Option<BTreeMap<String, f64>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum ParseResult {
    Full(FullResult),
    Url {
        url: String,
        #[serde(default)]
        result_id: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct FullResult {
    #[serde(default)]
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Chunk {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub blocks: Vec<WireBlock>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireBlock {
    #[serde(rename = "type", default)]
    pub block_type: String,
    #[serde(default)]
    pub bbox: Option<WireBBox>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub confidence: Option<String>,
    #[serde(default)]
    pub granular_confidence: Option<GranularConfidence>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct GranularConfidence {
    #[serde(default)]
    pub parse_confidence: Option<f64>,
    /// Only set on extract citations.
    #[serde(default)]
    pub extract_confidence: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WireBBox {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default = "one")]
    pub page: u32,
    #[serde(default)]
    pub original_page: Option<u32>,
}

fn one() -> u32 {
    1
}

// ---- normalisation -----------------------------------------------------------------------------

fn map_block_type(t: &str) -> BlockType {
    match t {
        "Text" | "Key Value" | "Comment" => BlockType::Text,
        "Title" => BlockType::Title,
        "Section Header" => BlockType::SectionHeader,
        "List Item" => BlockType::List,
        "Table" => BlockType::Table,
        "Figure" => BlockType::Figure,
        "Header" => BlockType::Header,
        "Footer" => BlockType::Footer,
        "Footnote" => BlockType::Footnote,
        "Caption" => BlockType::Caption,
        "Formula" | "Equation" => BlockType::Formula,
        _ => BlockType::Other,
    }
}

pub(crate) fn normalize(
    parsed: &WireParseResponse,
    full: &FullResult,
    fmt: OutputFormat,
    model: &str,
) -> ParseResponse {
    let mut blocks: Vec<Block> = Vec::new();
    // Chunk content is the provider's own markdown rendering; keep it when a chunk maps to one page.
    let mut page_md: BTreeMap<u32, String> = BTreeMap::new();
    for chunk in &full.chunks {
        let pages: std::collections::BTreeSet<u32> =
            chunk.blocks.iter().filter_map(|b| b.bbox.as_ref().map(|bb| bb.page)).collect();
        if pages.len() == 1 && !chunk.content.trim().is_empty() {
            let p = *pages.iter().next().unwrap();
            page_md
                .entry(p)
                .and_modify(|s| {
                    s.push_str("\n\n");
                    s.push_str(&chunk.content)
                })
                .or_insert_with(|| chunk.content.clone());
        }
        for b in &chunk.blocks {
            let page_number = b.bbox.as_ref().map(|bb| bb.page).unwrap_or(1);
            let bbox = b.bbox.as_ref().map(|bb| BBox::from_normalized_ltwh(bb.left, bb.top, bb.width, bb.height));
            let coarse = match b.confidence.as_deref() {
                Some("high") => Some(0.9),
                Some("low") => Some(0.5),
                _ => None,
            };
            let confidence = b.granular_confidence.as_ref().and_then(|g| g.parse_confidence).or(coarse);
            let content = match fmt {
                OutputFormat::Markdown => b.content.clone(),
                OutputFormat::Text => crate::types::markdown_to_text(&b.content),
            };
            blocks.push(Block {
                block_type: map_block_type(&b.block_type),
                content,
                text: None,
                bbox,
                confidence,
                page_number,
            });
        }
    }
    let mut pages: Vec<Page> = crate::types::pages_from_blocks(blocks, &BTreeMap::new());
    for p in &mut pages {
        if let Some(md) = page_md.get(&p.page_number) {
            p.markdown = md.clone();
            p.text = crate::types::markdown_to_text(md);
        }
    }
    for (n, md) in &page_md {
        if !pages.iter().any(|p| p.page_number == *n) {
            pages.push(Page {
                page_number: *n,
                width: None,
                height: None,
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
    let num_pages = parsed.usage.as_ref().map(|u| u.num_pages).filter(|&n| n > 0).unwrap_or(pages.len() as u32);
    let usage =
        Usage { pages: num_pages, credits: parsed.usage.as_ref().and_then(|u| u.credits), provider_cost_usd: None };
    let mut resp = ParseResponse::from_pages(NAME, &format!("{NAME}/{model}"), pages, usage);
    if let Some(d) = parsed.duration {
        resp.metadata.insert("reducto_duration_s".into(), json!(d));
    }
    if let Some(link) = &parsed.studio_link {
        resp.metadata.insert("reducto_studio_link".into(), json!(link));
    }
    resp
}

// ---- extract normalisation ---------------------------------------------------------------------

/// `{"type":"url","url":…}` stands in for a result that was written to object storage.
fn url_result(result: &Value) -> Option<&str> {
    match result.get("type").and_then(Value::as_str) {
        Some("url") => result.get("url").and_then(Value::as_str),
        _ => None,
    }
}

/// Append one unescaped segment to a JSON pointer (RFC 6901: `~` → `~0`, `/` → `~1`).
fn push_pointer(base: &str, segment: &str) -> String {
    format!("{base}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

/// A citation leaf is exactly `{"value": …, "citations": [...]}` — Reducto wraps every *leaf* of the
/// schema when `settings.citations.enabled` is on, including inside arrays and nested objects.
fn citation_leaf(map: &serde_json::Map<String, Value>) -> Option<&Value> {
    if map.len() == 2 && map.contains_key("citations") {
        map.get("value")
    } else {
        None
    }
}

/// Strip the citation wrappers out of `node`, recording each leaf's citations under its JSON pointer.
fn unwrap_citations(node: &Value, pointer: &str, fields: &mut BTreeMap<String, FieldInfo>) -> Value {
    match node {
        Value::Object(map) => {
            if let Some(inner) = citation_leaf(map) {
                let info = field_info(map.get("citations"));
                if info.confidence.is_some() || !info.citations.is_empty() {
                    fields.insert(pointer.to_string(), info);
                }
                return unwrap_citations(inner, pointer, fields);
            }
            Value::Object(
                map.iter().map(|(k, v)| (k.clone(), unwrap_citations(v, &push_pointer(pointer, k), fields))).collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, v)| unwrap_citations(v, &push_pointer(pointer, &i.to_string()), fields))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Turn a Reducto `citations` array (parse blocks) into unified citations + a confidence.
fn field_info(citations: Option<&Value>) -> FieldInfo {
    let blocks: Vec<WireBlock> = citations
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| serde_json::from_value(c.clone()).ok()).collect())
        .unwrap_or_default();
    let confidence = blocks
        .iter()
        .filter_map(|b| {
            b.granular_confidence.as_ref().and_then(|g| g.extract_confidence).or(match b.confidence.as_deref() {
                Some("high") => Some(0.9),
                Some("low") => Some(0.5),
                _ => None,
            })
        })
        .fold(None::<f64>, |acc, c| Some(acc.map_or(c, |a| a.max(c))));
    let citations = blocks
        .iter()
        .map(|b| Citation {
            page_number: b.bbox.as_ref().map(|bb| bb.page).unwrap_or(1),
            // Reducto boxes are already normalised 0..1 with a top-left origin.
            bbox: b.bbox.as_ref().map(|bb| BBox::from_normalized_ltwh(bb.left, bb.top, bb.width, bb.height)),
            text: Some(b.content.clone()).filter(|c| !c.is_empty()),
        })
        .collect();
    FieldInfo { confidence, citations }
}

pub(crate) fn normalize_extract(wire: &WireExtractResponse, result: &Value, model: &str) -> ExtractResponse {
    let mut fields = BTreeMap::new();
    // Without chunking Reducto returns a single-element list; unwrap it so the data matches the
    // caller's schema. Longer lists (one entry per chunk) are kept as an array.
    let data = match result {
        Value::Array(items) if items.len() == 1 => unwrap_citations(&items[0], "", &mut fields),
        other => unwrap_citations(other, "", &mut fields),
    };
    let usage = Usage {
        pages: wire.usage.as_ref().map(|u| u.num_pages).unwrap_or(0),
        credits: wire.usage.as_ref().and_then(|u| u.credits),
        provider_cost_usd: None,
    };
    let mut resp = ExtractResponse::new(NAME, &format!("{NAME}/{model}"), data, usage);
    resp.fields = fields;
    if let Some(link) = &wire.studio_link {
        resp.metadata.insert("reducto_studio_link".into(), json!(link));
    }
    if let Some(u) = &wire.usage {
        if let Some(n) = u.num_fields {
            resp.metadata.insert("reducto_num_fields".into(), json!(n));
        }
        if let Some(m) = &u.extract_mode {
            resp.metadata.insert("reducto_extract_mode".into(), json!(m));
        }
    }
    if let Some(c) = &wire.confidence {
        resp.metadata.insert("reducto_confidence".into(), json!(c));
    }
    if let Some(r) = &wire.confidence_reason {
        resp.metadata.insert("reducto_confidence_reason".into(), json!(r));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fixture() {
        let raw = include_str!("../../tests/fixtures/reducto_parse.json");
        let parsed: WireParseResponse = serde_json::from_str(raw).unwrap();
        let ParseResult::Full(full) = &parsed.result else { panic!("expected full") };
        let resp = normalize(&parsed, full, OutputFormat::Markdown, "standard");
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.usage.credits, Some(1.0));
        assert_eq!(resp.pages[0].blocks.len(), 3);
        assert_eq!(resp.pages[0].blocks[0].block_type, BlockType::Title);
        assert!(resp.pages[0].markdown.starts_with("# Hello LiteOCR"));
        let bb = resp.pages[0].blocks[0].bbox.unwrap();
        assert!((bb.x0 - 0.1193).abs() < 1e-3 && (bb.x1 - (0.1193 + 0.2475)).abs() < 1e-3);
        assert!(resp.pages[0].blocks[0].confidence.unwrap() > 0.9);
        assert!(resp.metadata.contains_key("reducto_duration_s"));
    }

    #[test]
    fn url_result_variant_parses() {
        let v: ParseResult =
            serde_json::from_str(r#"{"type":"url","url":"https://x/y.json","result_id":"abc"}"#).unwrap();
        assert!(matches!(v, ParseResult::Url { .. }));
    }

    #[test]
    fn normalizes_extract_fixture_with_citations() {
        let raw = include_str!("../../tests/fixtures/reducto_extract.json");
        let wire: WireExtractResponse = serde_json::from_str(raw).unwrap();
        let result = wire.result.clone();
        let resp = normalize_extract(&wire, &result, "extract");

        // Data is the caller's schema shape, with every citation wrapper stripped.
        assert_eq!(resp.data["invoice_number"], "INV-9865");
        assert_eq!(resp.data["vendor_address"]["city"], "Akron");
        assert_eq!(resp.data["line_items"][0]["description"], "Hydraulic fluid, 5 gal");
        assert_eq!(resp.data["line_items"].as_array().unwrap().len(), 2);
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.usage.credits, Some(3.333333));
        assert_eq!(resp.metadata["reducto_num_fields"], 15);
        assert_eq!(resp.metadata["reducto_extract_mode"], "extract");

        // Fields are keyed by JSON pointer, including array indices and nested objects.
        let f = &resp.fields["/invoice_number"];
        assert!((f.confidence.unwrap() - 0.996).abs() < 1e-9);
        assert_eq!(f.citations.len(), 1);
        assert_eq!(f.citations[0].page_number, 1);
        assert_eq!(f.citations[0].text.as_deref(), Some("INV-9865"));
        let bb = f.citations[0].bbox.unwrap();
        assert!((bb.x0 - 0.1572).abs() < 1e-3 && (bb.x1 - (0.1572 + 0.0919)).abs() < 1e-3, "{bb:?}");
        assert!(resp.fields.contains_key("/line_items/0/amount"));
        assert!(resp.fields.contains_key("/vendor_address/city"));
        assert!(!resp.fields.contains_key("/line_items"), "only leaves carry citations");
    }

    #[test]
    fn normalizes_extract_fixture_without_citations() {
        let raw = include_str!("../../tests/fixtures/reducto_extract_plain.json");
        let wire: WireExtractResponse = serde_json::from_str(raw).unwrap();
        let result = wire.result.clone();
        let resp = normalize_extract(&wire, &result, "extract");
        // The single-element list Reducto returns without chunking is unwrapped to the object.
        assert_eq!(resp.data["invoice_number"], "INV-9865");
        assert_eq!(resp.data["total"], "$14,667.43");
        assert!(resp.fields.is_empty());
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.model, "reducto/extract");
    }

    #[test]
    fn multi_chunk_extract_results_stay_an_array() {
        let wire: WireExtractResponse =
            serde_json::from_str(r#"{"usage":{"num_pages":2},"result":[{"a":1},{"a":2}]}"#).unwrap();
        let result = wire.result.clone();
        let resp = normalize_extract(&wire, &result, "extract");
        assert_eq!(resp.data, json!([{"a": 1}, {"a": 2}]));
        assert_eq!(resp.usage.pages, 2);
    }

    #[test]
    fn pointer_segments_are_escaped() {
        let mut fields = BTreeMap::new();
        let node = json!({"a/b": {"value": "x", "citations": [{"type":"Text","content":"x"}]}});
        let data = unwrap_citations(&node, "", &mut fields);
        assert_eq!(data["a/b"], "x");
        assert!(fields.contains_key("/a~1b"), "{:?}", fields.keys().collect::<Vec<_>>());
    }

    #[test]
    fn detects_url_results() {
        assert_eq!(url_result(&json!({"type": "url", "url": "https://x/y.json"})), Some("https://x/y.json"));
        assert_eq!(url_result(&json!({"total": {"value": 1}})), None);
        assert_eq!(url_result(&json!([{"total": 1}])), None);
    }

    #[test]
    fn extract_body_variants() {
        let doc = DocumentRequest::from_url("https://x/y.pdf")
            .pages("2-3")
            .provider_options(json!({"async": true, "settings": {"optimize_for_latency": true}}));
        let req = ExtractRequest::new(doc, json!({"type": "object", "properties": {"a": {"type": "string"}}}))
            .instructions("Be careful")
            .citations(true);
        let b = build_extract_body(&req, "deep_extract", "https://x/y.pdf").unwrap();
        assert_eq!(b["input"], "https://x/y.pdf");
        assert_eq!(b["instructions"]["schema"]["properties"]["a"]["type"], "string");
        assert_eq!(b["instructions"]["system_prompt"], "Be careful");
        assert_eq!(b["settings"]["deep_extract"], true);
        assert_eq!(b["settings"]["citations"]["enabled"], true);
        assert_eq!(b["settings"]["page_range"][0]["end"], 3);
        assert_eq!(b["settings"]["optimize_for_latency"], true);
        assert!(b.get("async").is_none());

        let plain = ExtractRequest::new(DocumentRequest::from_url("u"), json!({"type": "object"}));
        let b = build_extract_body(&plain, "extract", "u").unwrap();
        assert!(b["settings"].get("deep_extract").is_none());
        assert!(b["settings"].get("citations").is_none());
        assert!(b["instructions"].get("system_prompt").is_none());
        assert!(build_extract_body(&plain, "nope", "u").is_err());
    }

    #[test]
    fn body_variants() {
        let req = DocumentRequest::from_url("https://x/y.pdf")
            .pages("2-3")
            .provider_options(json!({"async": true, "settings": {"ocr_system": "legacy"}}));
        let b = build_body(&req, "agentic", "https://x/y.pdf").unwrap();
        assert_eq!(b["input"], "https://x/y.pdf");
        assert_eq!(b["enhance"]["agentic"][0]["scope"], "text");
        assert_eq!(b["settings"]["page_range"][0]["start"], 2);
        assert_eq!(b["settings"]["ocr_system"], "legacy");
        assert!(b.get("async").is_none());
        let b = build_body(&DocumentRequest::from_url("u"), "r-1", "u").unwrap();
        assert_eq!(b["settings"]["model"], "r-1");
        assert!(build_body(&DocumentRequest::from_url("u"), "nope", "u").is_err());
    }

    // ---- live ---------------------------------------------------------------------------------

    const LIVE_DOC: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmark/datasets/synthetic-v1/docs/invoice_001.png");

    fn live_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "invoice_number": {"type": "string"},
                "total": {"type": "string"},
                "date": {"type": "string"},
                "vendor": {"type": "string"}
            },
            "required": ["invoice_number", "total"]
        })
    }

    #[tokio::test]
    #[ignore = "needs REDUCTO_API_KEY and network"]
    async fn live_extract() {
        if std::env::var("REDUCTO_API_KEY").map(|v| v.trim().is_empty()).unwrap_or(true) {
            eprintln!("skipping reducto live extract: REDUCTO_API_KEY not set");
            return;
        }
        let doc = DocumentRequest::from_path(LIVE_DOC).timeout_secs(240.0);
        let req = ExtractRequest::new(doc, live_schema())
            .instructions("Extract the invoice header fields exactly as printed.")
            .citations(true);
        let resp = Reducto.extract(&req, "extract").await.expect("reducto extract succeeds");
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
/// real response paths, including following a presigned `result.type == "url"` link.
#[cfg(test)]
mod wire {
    use super::*;
    use crate::jobs::RetrieveOptions;
    use crate::testutil::{fixture, pdf_request, serve};
    use crate::ErrorKind;

    const STORAGE: &str = "https://example-storage.invalid";

    fn upload() -> (u16, String) {
        (200, json!({"file_id": "reducto://abc.pdf", "presigned_url": null}).to_string())
    }

    /// The captured `/parse` envelope, with its presigned URL pointed at the loopback server.
    fn url_envelope() -> String {
        fixture("reducto_parse_url.json").replace(STORAGE, "{base}")
    }

    #[tokio::test]
    async fn parse_follows_a_url_result() {
        let (base, seen) =
            serve(vec![upload(), (200, url_envelope()), (200, fixture("reducto_parse_url_result.json"))]).await;
        let resp = Reducto.parse(&pdf_request(&base).include_raw(true), "standard").await.expect("parse");

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].route(), "POST /upload");
        assert_eq!(seen[1].route(), "POST /parse");
        assert!(seen[2].route().starts_with("GET /org/REDACTED/"), "{}", seen[2].line);
        assert!(seen[2].line.contains("X-Amz-Signature=REDACTED"), "query string kept: {}", seen[2].line);
        assert_eq!(seen[1].header("authorization").as_deref(), Some("bearer test-key"));
        assert_eq!(seen[2].header("authorization"), None, "presigned URLs are fetched without the key");

        assert_eq!(resp.provider_job_id.as_deref(), Some("5b0e8a52-3f1c-4d7e-9a61-2c4f0d9e7b13"));
        assert_eq!(resp.pages.len(), 1);
        assert_eq!(resp.usage.pages, 1);
        assert_eq!(resp.usage.credits, Some(2.0));
        assert!(resp.markdown.contains("Cedar Ridge Supply"));
        assert!(resp.markdown.contains("Total Due: $14,667.43"));
        assert_eq!(resp.pages[0].blocks.len(), 7);
        assert!(resp.pages[0].blocks.iter().all(|b| b.bbox.is_some()));
        assert_eq!(resp.raw.as_ref().unwrap()["result"]["type"], "url", "raw keeps the envelope");
    }

    #[tokio::test]
    async fn a_url_that_returns_another_url_is_an_error() {
        let (base, _) = serve(vec![
            upload(),
            (200, url_envelope()),
            (200, json!({"type": "url", "url": "x", "result_id": "y"}).to_string()),
        ])
        .await;
        let e = Reducto.parse(&pdf_request(&base), "standard").await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Provider);
        assert!(e.message.contains("another URL"));
        assert_eq!(e.job_id.as_deref(), Some("5b0e8a52-3f1c-4d7e-9a61-2c4f0d9e7b13"));
    }

    #[tokio::test]
    async fn an_expired_result_url_keeps_the_storage_error() {
        let expired =
            "<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code><Message>Request has expired</Message></Error>";
        let (base, _) = serve(vec![upload(), (200, url_envelope()), (403, expired.into())]).await;
        let e = Reducto.parse(&pdf_request(&base), "standard").await.unwrap_err();
        assert_eq!(e.status_code, Some(403));
        assert!(e.message.contains("Request has expired"), "the storage message is surfaced: {e}");
    }

    #[tokio::test]
    async fn extract_follows_a_url_result_holding_the_bare_value() {
        let plain: Value = serde_json::from_str(&fixture("reducto_extract_plain.json")).unwrap();
        let mut envelope = plain.clone();
        envelope["result"] =
            json!({"type": "url", "url": "{base}/results/r1.json?X-Amz-Signature=REDACTED", "result_id": "r1"});
        let (base, seen) = serve(vec![upload(), (200, envelope.to_string()), (200, plain["result"].to_string())]).await;
        let doc = pdf_request(&base).provider_options(json!({"settings": {"force_url_result": true}}));
        let schema = json!({"type": "object", "properties": {"invoice_number": {"type": "string"}}});
        let resp = Reducto.extract(&ExtractRequest::new(doc, schema), "extract").await.expect("extract");

        let seen = seen.lock().unwrap();
        assert_eq!(seen[1].route(), "POST /extract");
        let sent: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(sent["settings"]["force_url_result"], true);
        assert!(seen[2].route().starts_with("GET /results/r1.json"));
        assert_eq!(seen[2].header("authorization"), None);
        assert_eq!(resp.data["invoice_number"], "INV-9865", "single-element list unwrapped");
        assert_eq!(resp.usage.pages, 1);
    }

    // ---- jobs API --------------------------------------------------------------------------------

    fn job(status: &str, result: Option<Value>) -> (u16, String) {
        let mut j = json!({"status": status, "progress": null, "reason": null});
        if let Some(r) = result {
            j["result"] = r;
        }
        (200, j.to_string())
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
    async fn submit_then_retrieve_until_done() {
        let parsed: Value = serde_json::from_str(&fixture("reducto_parse.json")).unwrap();
        let (base, seen) = serve(vec![
            upload(),
            (200, json!({"job_id": "job-123"}).to_string()),
            job("Pending", None),
            job("InProgress", None),
            job("Completed", Some(parsed)),
        ])
        .await;
        let req = pdf_request(&base)
            .model("reducto/r-1")
            .webhook_url("https://hooks.example.com/reducto")
            .provider_options(json!({"async": {"metadata": {"tenant": "t1"}}}));
        let mut req = req;
        req.metadata.insert("run".into(), json!("r1"));
        let handle = crate::submit_parse(req).await.expect("submit");
        assert_eq!(handle.provider, "reducto");
        assert_eq!(handle.model, "reducto/r-1");
        assert_eq!(handle.job_id, "job-123");
        assert_eq!(handle.base_url.as_deref(), Some(base.as_str()));
        assert!(!serde_json::to_string(&handle).unwrap().contains("test-key"), "no secret in the handle");

        assert!(crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap().is_pending());
        assert!(crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap().is_pending());
        let JobStatus::Succeeded(resp) = crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap() else {
            panic!("expected success")
        };
        assert_eq!(resp.model, "reducto/r-1");
        assert_eq!(resp.provider, "reducto");
        assert!(resp.markdown.starts_with("# Hello LiteOCR"));
        assert_eq!(resp.metadata["run"], "r1", "request metadata is echoed");
        assert!(resp.cost_usd.is_some(), "priced like parse()");
        assert!(resp.raw.is_none());

        let seen = seen.lock().unwrap();
        assert_eq!(seen[1].route(), "POST /parse_async");
        let body: Value = serde_json::from_str(&seen[1].body).unwrap();
        assert_eq!(body["async"]["webhook"], json!({"mode": "direct", "url": "https://hooks.example.com/reducto"}));
        assert_eq!(body["async"]["metadata"]["tenant"], "t1", "reducto's own async object is kept");
        assert_eq!(body["settings"]["model"], "r-1");
        assert_eq!(seen[2].route(), "GET /job/job-123");
        assert_eq!(seen[4].header("authorization").as_deref(), Some("bearer test-key"));
    }

    #[tokio::test]
    async fn retrieve_follows_a_url_result_and_reports_failures() {
        let envelope: Value = serde_json::from_str(&url_envelope()).unwrap();
        let (base, _) = serve(vec![
            job("Completed", Some(envelope)),
            (200, fixture("reducto_parse_url_result.json")),
            (200, json!({"status": "Failed", "reason": "Password-protected document"}).to_string()),
        ])
        .await;
        let handle = crate::JobHandle::new(NAME, "reducto/standard", "job-9");
        let JobStatus::Succeeded(resp) = crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap() else {
            panic!("expected success")
        };
        assert!(resp.markdown.contains("Cedar Ridge Supply"));
        let JobStatus::Failed(e) = crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap() else {
            panic!("expected failure")
        };
        assert_eq!(e.kind, ErrorKind::Provider);
        assert!(e.message.contains("Password-protected document"));
        assert_eq!(e.job_id.as_deref(), Some("job-9"));
        assert_eq!(e.provider.as_deref(), Some("reducto"));
    }

    #[tokio::test]
    async fn retrieve_maps_http_errors() {
        let (base, _) = serve(vec![(401, r#"{"detail":"Invalid access token"}"#.into())]).await;
        let handle = crate::JobHandle::new(NAME, "reducto/standard", "job-1");
        let e = crate::retrieve_parse_with(&handle, &opts(&base)).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Authentication);
        assert_eq!(e.job_id.as_deref(), Some("job-1"));
    }

    #[tokio::test]
    async fn webhook_names_the_job_and_resolve_fetches_it() {
        let body = json!({"status": "Completed", "job_id": "job-7", "metadata": {"tenant": "t1"}});
        let event = crate::parse_webhook("reducto", &body).unwrap();
        assert!(matches!(event.status, WebhookStatus::Finished));
        let handle = event.job.unwrap();
        assert_eq!((handle.model.as_str(), handle.job_id.as_str()), ("reducto/standard", "job-7"));
        let pending = crate::parse_webhook("reducto", &json!({"status": "Pending", "job_id": "job-7"})).unwrap();
        assert!(matches!(pending.status, WebhookStatus::Pending));
        assert_eq!(
            crate::parse_webhook("reducto", &json!({"status": "Completed"})).unwrap_err().kind,
            ErrorKind::Input
        );

        let parsed: Value = serde_json::from_str(&fixture("reducto_parse.json")).unwrap();
        let (base, seen) = serve(vec![job("Completed", Some(parsed))]).await;
        let status = crate::resolve_webhook("reducto", &body, &opts(&base)).await.unwrap();
        assert!(matches!(status, JobStatus::Succeeded(_)));
        assert_eq!(seen.lock().unwrap()[0].route(), "GET /job/job-7");
    }
}
