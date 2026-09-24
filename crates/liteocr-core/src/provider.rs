//! The provider abstraction. Add a provider by implementing [`Provider`] and registering it
//! in [`crate::model::PROVIDERS`] and [`crate::providers::build`].

use crate::error::{Error, Result};
use crate::jobs::{JobHandle, JobStatus, WebhookEvent};
use crate::types::{DocumentRequest, ExtractRequest, ExtractResponse, Mode, ParseResponse, TextResponse};
use async_trait::async_trait;

/// One document-AI backend. Implement the methods for the modes the provider's models declare
/// in [`crate::model::PROVIDERS`]; the defaults return `UnsupportedModel`.
///
/// `model` is always the validated model name within this provider (e.g. `"standard"`), and the
/// registry guarantees it declares the mode being called.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider name, e.g. `"reducto"`.
    fn name(&self) -> &'static str;

    /// `parse` mode: layout-aware parsing to markdown + typed blocks.
    async fn parse(&self, _request: &DocumentRequest, model: &str) -> Result<ParseResponse> {
        Err(unsupported(self.name(), model, Mode::Parse))
    }

    /// `ocr` mode: plain text with word/line geometry. The default derives it from [`Self::parse`],
    /// which is correct for layout providers; native OCR endpoints should override it.
    async fn ocr(&self, request: &DocumentRequest, model: &str) -> Result<TextResponse> {
        let parsed = self.parse(request, model).await?;
        Ok(TextResponse::from_parse(&parsed))
    }

    /// `extract` mode: schema-driven structured extraction.
    async fn extract(&self, _request: &ExtractRequest, model: &str) -> Result<ExtractResponse> {
        Err(unsupported(self.name(), model, Mode::Extract))
    }

    /// Start a `parse` job without waiting for it (see [`crate::jobs`]). Only providers with a
    /// job queue implement it; the default returns `UnsupportedModel`.
    async fn submit_parse(&self, _request: &DocumentRequest, model: &str) -> Result<JobHandle> {
        Err(no_jobs(self.name(), model))
    }

    /// Check a submitted `parse` job once. `request` carries the credentials, deadline and output
    /// preferences (built from the handle and the caller's options); `model` is the bare model name.
    async fn retrieve_parse(&self, _job: &JobHandle, _request: &DocumentRequest, model: &str) -> Result<JobStatus> {
        Err(no_jobs(self.name(), model))
    }

    /// Interpret the JSON body this provider POSTs to a webhook. Pure: no network.
    fn parse_webhook(&self, model: &str, _payload: &serde_json::Value) -> Result<WebhookEvent> {
        Err(no_jobs(self.name(), model))
    }
}

fn no_jobs(provider: &str, model: &str) -> Error {
    Error::unsupported_model(format!(
        "{provider}/{model} has no asynchronous job API in LiteOCR; use parse() \
         (providers with jobs: reducto, extend, llamaparse)"
    ))
    .with_provider(provider)
}

/// The request's `webhook_url`, validated as an absolute http(s) URL.
pub fn webhook_url(request: &DocumentRequest) -> Result<Option<&str>> {
    let Some(raw) = request.webhook_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) else {
        return Ok(None);
    };
    let parsed = url::Url::parse(raw).map_err(|e| Error::input(format!("invalid webhook_url {raw}: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Error::input(format!("webhook_url must be http(s), got {raw}")));
    }
    Ok(Some(raw))
}

fn unsupported(provider: &str, model: &str, mode: Mode) -> Error {
    Error::unsupported_model(format!("{provider}/{model} does not implement mode '{mode}'")).with_provider(provider)
}

/// Resolve the API key: explicit request override, then environment variable.
pub fn resolve_api_key(request: &DocumentRequest, env_var: &str, provider: &str) -> Result<String> {
    if let Some(k) = request.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        return Ok(k.to_string());
    }
    std::env::var(env_var).ok().filter(|k| !k.trim().is_empty()).ok_or_else(|| {
        crate::error::Error::authentication(format!("no API key for {provider}: set {env_var} or pass api_key"))
            .with_provider(provider)
    })
}

/// Resolve base URL: request override, then `<PREFIX>_BASE_URL` env, then default.
pub fn resolve_base_url(request: &DocumentRequest, env_var: &str, default: &str) -> String {
    request
        .base_url
        .clone()
        .or_else(|| std::env::var(env_var).ok())
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Load the document bytes for upload (path or bytes). URLs return `None`.
pub async fn load_bytes(input: &crate::types::DocumentInput) -> Result<Option<bytes::Bytes>> {
    match input {
        crate::types::DocumentInput::Path { path } => {
            let data = tokio::fs::read(path)
                .await
                .map_err(|e| crate::error::Error::input(format!("cannot read {}: {e}", path.display())))?;
            if data.is_empty() {
                return Err(crate::error::Error::input(format!("{} is empty", path.display())));
            }
            Ok(Some(bytes::Bytes::from(data)))
        }
        crate::types::DocumentInput::Bytes { data, filename } => {
            if data.is_empty() {
                return Err(crate::error::Error::input("input bytes are empty"));
            }
            if filename.trim().is_empty() {
                return Err(crate::error::Error::input("filename is required for bytes input"));
            }
            Ok(Some(data.clone()))
        }
        crate::types::DocumentInput::Url { url } => {
            url::Url::parse(url).map_err(|e| crate::error::Error::input(format!("invalid URL {url}: {e}")))?;
            Ok(None)
        }
    }
}

/// Build a multipart file part with the right filename and MIME type.
pub fn file_part(data: bytes::Bytes, input: &crate::types::DocumentInput) -> reqwest::multipart::Part {
    let mime = input.mime_type();
    let part = reqwest::multipart::Part::stream(data).file_name(input.filename());
    // `mime_guess` only produces well-formed MIME strings; fall back to octet-stream defensively.
    match part.mime_str(&mime) {
        Ok(p) => p,
        Err(_) => reqwest::multipart::Part::stream(bytes::Bytes::new())
            .file_name(input.filename())
            .mime_str("application/octet-stream")
            .expect("static mime is valid"),
    }
}
