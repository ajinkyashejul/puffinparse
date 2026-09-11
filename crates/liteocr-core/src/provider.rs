//! The provider abstraction. Add a provider by implementing [`OcrProvider`] and registering it
//! in [`crate::model::PROVIDERS`] and [`crate::providers::build`].

use crate::error::Result;
use crate::types::{OcrRequest, OcrResponse};
use async_trait::async_trait;

/// One OCR backend.
#[async_trait]
pub trait OcrProvider: Send + Sync {
    /// Provider name, e.g. `"reducto"`.
    fn name(&self) -> &'static str;

    /// Run the request end-to-end (upload, parse, poll, download) and return a normalised response.
    /// `model` is the validated model name within this provider (e.g. `"standard"`).
    async fn ocr(&self, request: &OcrRequest, model: &str) -> Result<OcrResponse>;
}

/// Resolve the API key: explicit request override, then environment variable.
pub fn resolve_api_key(request: &OcrRequest, env_var: &str, provider: &str) -> Result<String> {
    if let Some(k) = request.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        return Ok(k.to_string());
    }
    std::env::var(env_var).ok().filter(|k| !k.trim().is_empty()).ok_or_else(|| {
        crate::error::Error::authentication(format!("no API key for {provider}: set {env_var} or pass api_key"))
            .with_provider(provider)
    })
}

/// Resolve base URL: request override, then `<PREFIX>_BASE_URL` env, then default.
pub fn resolve_base_url(request: &OcrRequest, env_var: &str, default: &str) -> String {
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
