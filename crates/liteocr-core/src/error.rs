//! Error types shared by every provider.

use std::fmt;

/// Classification of an error, mirrored 1:1 by the Python exception hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 401 / 403 from the provider, or missing API key.
    Authentication,
    /// 429 from the provider after retries were exhausted.
    RateLimit,
    /// Any other 4xx: the request itself is wrong.
    BadRequest,
    /// 5xx, malformed provider payload, or a job that ended in a failed state.
    Provider,
    /// The overall deadline (upload + polling) was exceeded.
    Timeout,
    /// Unknown provider / model string.
    UnsupportedModel,
    /// Unreadable input, bytes without filename, empty body, …
    Input,
    /// Network / TLS / DNS failure after retries.
    Network,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ErrorKind::Authentication => "authentication_error",
            ErrorKind::RateLimit => "rate_limit_error",
            ErrorKind::BadRequest => "bad_request_error",
            ErrorKind::Provider => "provider_error",
            ErrorKind::Timeout => "timeout_error",
            ErrorKind::UnsupportedModel => "unsupported_model_error",
            ErrorKind::Input => "input_error",
            ErrorKind::Network => "network_error",
        };
        f.write_str(s)
    }
}

/// The single error type returned by LiteOCR.
#[derive(Debug, Clone, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[error("{kind}{}: {message}{}", provider.as_deref().map(|p| format!(" [{p}]")).unwrap_or_default(), status_code.map(|c| format!(" (HTTP {c})")).unwrap_or_default())]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub provider: Option<String>,
    pub status_code: Option<u16>,
    pub job_id: Option<String>,
    /// Whether a router may try a fallback model for this error.
    pub retryable: bool,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let retryable =
            matches!(kind, ErrorKind::RateLimit | ErrorKind::Provider | ErrorKind::Timeout | ErrorKind::Network);
        Self { kind, message: message.into(), provider: None, status_code: None, job_id: None, retryable }
    }

    pub fn with_provider(mut self, provider: &str) -> Self {
        self.provider = Some(provider.to_string());
        self
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status_code = Some(status);
        self
    }

    pub fn with_job_id(mut self, job_id: impl Into<String>) -> Self {
        self.job_id = Some(job_id.into());
        self
    }

    pub fn input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Input, message)
    }

    pub fn unsupported_model(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::UnsupportedModel, message)
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Provider, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Timeout, message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    pub fn authentication(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Authentication, message)
    }

    /// Build an error from an HTTP status + body returned by a provider.
    pub fn from_http(provider: &str, status: u16, body: &str) -> Self {
        let kind = match status {
            401 | 403 => ErrorKind::Authentication,
            429 => ErrorKind::RateLimit,
            400..=499 => ErrorKind::BadRequest,
            _ => ErrorKind::Provider,
        };
        let message = extract_message(body).unwrap_or_else(|| truncate(body, 500));
        Self::new(kind, message).with_provider(provider).with_status(status)
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            Error::timeout(e.to_string())
        } else if e.is_decode() {
            Error::provider(format!("failed to decode provider response: {e}"))
        } else {
            Error::network(e.to_string())
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::provider(format!("failed to parse provider JSON: {e}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::input(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Try to pull a human-readable message out of a JSON error body.
fn extract_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    for key in ["message", "detail", "error", "error_message", "msg"] {
        match v.get(key) {
            Some(serde_json::Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            Some(serde_json::Value::Object(o)) => {
                if let Some(serde_json::Value::String(s)) = o.get("message") {
                    return Some(s.clone());
                }
                return Some(serde_json::Value::Object(o.clone()).to_string());
            }
            Some(serde_json::Value::Array(a)) => {
                return Some(serde_json::Value::Array(a.clone()).to_string());
            }
            _ => {}
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_status_to_kind() {
        assert_eq!(Error::from_http("x", 401, "").kind, ErrorKind::Authentication);
        assert_eq!(Error::from_http("x", 429, "").kind, ErrorKind::RateLimit);
        assert_eq!(Error::from_http("x", 422, "").kind, ErrorKind::BadRequest);
        assert_eq!(Error::from_http("x", 503, "").kind, ErrorKind::Provider);
    }

    #[test]
    fn extracts_json_message() {
        let e = Error::from_http("x", 400, r#"{"detail":"bad thing"}"#);
        assert_eq!(e.message, "bad thing");
        let e = Error::from_http("x", 400, r#"{"error":{"message":"nested"}}"#);
        assert_eq!(e.message, "nested");
        let e = Error::from_http("x", 400, "plain text");
        assert_eq!(e.message, "plain text");
    }

    #[test]
    fn display_includes_context() {
        let e = Error::from_http("reducto", 500, "boom");
        assert_eq!(e.to_string(), "provider_error [reducto]: boom (HTTP 500)");
    }
}
