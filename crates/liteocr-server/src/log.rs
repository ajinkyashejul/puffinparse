//! Structured request log: one JSON object per line, to stdout and/or an append-only file.
//!
//! The record type below is the whole schema. It has no field that could hold document bytes,
//! document URLs, extracted data, provider error text, provider keys or virtual-key secrets —
//! only the key's `id`.

use serde::Serialize;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct RequestLog {
    pub ts: String,
    pub request_id: String,
    /// Virtual key id (`"master"`, `"anonymous"` when auth is off). Never the secret.
    pub key_id: String,
    pub method: String,
    pub path: String,
    pub mode: Option<String>,
    /// What the client asked for (alias or `provider/model`).
    pub model: Option<String>,
    /// The model that actually served the request.
    pub served_model: Option<String>,
    pub provider: Option<String>,
    pub fallback_index: Option<usize>,
    pub pages: Option<u32>,
    pub cost_usd: Option<f64>,
    pub latency_ms: u64,
    pub status: u16,
    pub error_type: Option<String>,
    /// HTTP status the provider returned, when the failure came from a provider. The provider's
    /// message is returned to the caller but not logged: some providers echo document text in
    /// error bodies.
    pub provider_status: Option<u16>,
    /// Gateway job id (`/v1/jobs`, `/v1/webhooks`), with `job_status` the state observed:
    /// `pending`, `succeeded` or `failed`. Absent for synchronous calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_status: Option<String>,
}

#[derive(Debug)]
pub struct Logger {
    stdout: bool,
    file: Option<Mutex<std::fs::File>>,
}

impl Logger {
    pub fn new(stdout: bool, file: Option<&Path>) -> Result<Self, String> {
        let file = match file {
            Some(p) => Some(Mutex::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .map_err(|e| format!("cannot open log file {}: {e}", p.display()))?,
            )),
            None => None,
        };
        Ok(Self { stdout, file })
    }

    pub fn disabled() -> Self {
        Self { stdout: false, file: None }
    }

    pub fn write(&self, record: &RequestLog) {
        if !self.stdout && self.file.is_none() {
            return;
        }
        let Ok(mut line) = serde_json::to_string(record) else { return };
        line.push('\n');
        if self.stdout {
            let _ = std::io::stdout().lock().write_all(line.as_bytes());
        }
        if let Some(f) = &self.file {
            let mut f = f.lock().unwrap_or_else(|p| p.into_inner());
            if let Err(e) = f.write_all(line.as_bytes()) {
                tracing::error!(error = %e, "liteocr-server: cannot write request log");
            }
        }
    }
}
