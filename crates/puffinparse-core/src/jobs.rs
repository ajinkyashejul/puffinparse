//! Asynchronous jobs: submit a `parse` now, collect the result later.
//!
//! [`crate::parse`] blocks until the provider is done, polling for job-queue providers. That is the
//! right default, but it ties a worker to one document for minutes. The primitives here split the
//! call in two, so the caller owns the waiting:
//!
//! * [`crate::submit_parse`] uploads the document, starts the provider job and returns a
//!   serialisable [`JobHandle`] straight away (optionally registering a provider webhook through
//!   [`crate::DocumentRequest::webhook_url`]);
//! * [`crate::retrieve_parse`] asks the provider once and returns a [`JobStatus`] — `Pending`,
//!   `Succeeded(ParseResponse)` (normalised exactly like `parse`) or `Failed(Error)`;
//! * [`crate::parse_webhook`] / [`crate::resolve_webhook`] turn the body a provider POSTs to your
//!   webhook into the same vocabulary. An SDK cannot receive webhooks itself; your web handler
//!   passes the JSON body in.
//!
//! A [`JobHandle`] never contains a secret: API keys are resolved again at retrieve time (env var
//! or [`RetrieveOptions::api_key`]), and only the non-secret provider options a later call needs
//! (e.g. Extend's `workspace_id`) are kept in [`JobHandle::provider_state`].

use crate::error::Error;
use crate::types::{DocumentInput, DocumentRequest, OutputFormat, ParseResponse};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A submitted provider job. Serialise it (JSON) to hand it to another process or store it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobHandle {
    /// Provider name, e.g. `"reducto"`.
    pub provider: String,
    /// Fully-qualified model, e.g. `"reducto/standard"`.
    pub model: String,
    /// The provider's own job / run id.
    pub job_id: String,
    /// RFC 3339 UTC timestamp of the submission.
    pub submitted_at: String,
    /// Block content preference carried over from the request.
    #[serde(default)]
    pub output: OutputFormat,
    /// Attach the provider's raw payload to the eventual response.
    #[serde(default)]
    pub include_raw: bool,
    /// Base URL override used at submit time, reused by retrieve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Non-secret, provider-specific options that retrieve needs (never API keys or passwords).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<Value>,
    /// The request's free-form metadata, echoed into the final response.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl JobHandle {
    /// A handle with only the identifying fields set; the rest is filled by [`crate::submit_parse`].
    pub fn new(provider: &str, model: &str, job_id: impl Into<String>) -> Self {
        Self {
            provider: provider.to_string(),
            model: model.to_string(),
            job_id: job_id.into(),
            submitted_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            output: OutputFormat::Markdown,
            include_raw: false,
            base_url: None,
            provider_state: None,
            metadata: BTreeMap::new(),
        }
    }

    /// Record a provider option that a later retrieve needs.
    pub fn with_state(mut self, key: &str, value: Value) -> Self {
        let state = self.provider_state.get_or_insert_with(|| Value::Object(Default::default()));
        if let Value::Object(map) = state {
            map.insert(key.to_string(), value);
        }
        self
    }

    /// The request the provider's retrieve path runs with: credentials, base URL and deadline
    /// from `opts` (falling back to the handle), output preferences and provider state from the
    /// handle. The input is a placeholder; retrieve never uploads anything.
    pub(crate) fn request(&self, opts: &RetrieveOptions) -> DocumentRequest {
        let mut req = DocumentRequest::new(DocumentInput::Url { url: format!("puffinparse-job:{}", self.job_id) })
            .model(self.model.clone())
            .output(self.output)
            .include_raw(self.include_raw)
            .timeout_secs(opts.timeout_secs)
            .max_retries(opts.max_retries);
        req.api_key = opts.api_key.clone();
        req.base_url = opts.base_url.clone().or_else(|| self.base_url.clone());
        req.provider_options = self.provider_state.clone();
        req.metadata = self.metadata.clone();
        req
    }

    /// Milliseconds since submission (0 if the timestamp does not parse).
    pub(crate) fn elapsed_ms(&self) -> u64 {
        chrono::DateTime::parse_from_rfc3339(&self.submitted_at)
            .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_milliseconds().max(0) as u64)
            .unwrap_or(0)
    }
}

/// Credentials and limits for one [`crate::retrieve_parse_with`] call.
#[derive(Clone, Serialize, Deserialize)]
pub struct RetrieveOptions {
    /// API key override; otherwise the provider's env var (`REDUCTO_API_KEY`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Base URL override; otherwise the handle's, then the env var, then the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Deadline for this status check, including a result download.
    #[serde(default = "default_retrieve_timeout")]
    pub timeout_secs: f64,
    /// Retries on 429 / 5xx / network errors.
    #[serde(default = "default_retrieve_retries")]
    pub max_retries: u32,
}

fn default_retrieve_timeout() -> f64 {
    120.0
}

fn default_retrieve_retries() -> u32 {
    2
}

impl Default for RetrieveOptions {
    fn default() -> Self {
        Self {
            api_key: None,
            base_url: None,
            timeout_secs: default_retrieve_timeout(),
            max_retries: default_retrieve_retries(),
        }
    }
}

/// Never print the key, not even through `{:?}`.
impl std::fmt::Debug for RetrieveOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetrieveOptions")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

/// Where a submitted job stands.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub enum JobStatus {
    /// Queued or running; ask again later.
    Pending,
    /// Finished; the response is normalised exactly like [`crate::parse`]'s.
    Succeeded(Box<ParseResponse>),
    /// The provider reported a failure. The error carries its message and the job id.
    Failed(Error),
}

impl JobStatus {
    pub fn is_pending(&self) -> bool {
        matches!(self, JobStatus::Pending)
    }
}

/// What a provider webhook body says, before any extra HTTP call.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookEvent {
    /// The job the event is about, when the payload names it.
    pub job: Option<JobHandle>,
    pub status: WebhookStatus,
}

/// State reported by a webhook body.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", content = "result", rename_all = "snake_case")]
pub enum WebhookStatus {
    /// The job is queued or running.
    Pending,
    /// The job reached a terminal state but the body carries neither the result nor the error
    /// detail: call [`crate::retrieve_parse`] on [`WebhookEvent::job`] (what
    /// [`crate::resolve_webhook`] does).
    Finished,
    /// The body carried the whole result.
    Succeeded(Box<ParseResponse>),
    /// The body carried the failure (reason and message).
    Failed(Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn handle_round_trips_and_builds_a_retrieve_request() {
        let h = JobHandle::new("extend", "extend/parse_light", "pr_1").with_state("workspace_id", json!("ws_1"));
        let s = serde_json::to_string(&h).unwrap();
        let back: JobHandle = serde_json::from_str(&s).unwrap();
        assert_eq!(back, h);
        assert!(!s.contains("api_key"));
        let opts = RetrieveOptions { api_key: Some("k".into()), ..Default::default() };
        assert!(!format!("{opts:?}").contains("\"k\""));
        let req = h.request(&opts);
        assert_eq!(req.option("workspace_id"), Some(&json!("ws_1")));
        assert_eq!(req.api_key.as_deref(), Some("k"));
        assert_eq!(req.timeout_secs, 120.0);
        assert!(h.elapsed_ms() < 60_000);
    }

    #[test]
    fn status_serialises_with_a_tag() {
        assert_eq!(serde_json::to_value(JobStatus::Pending).unwrap(), json!({"status": "pending"}));
        let failed = serde_json::to_value(JobStatus::Failed(Error::provider("boom"))).unwrap();
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["result"]["message"], "boom");
    }
}
