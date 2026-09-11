//! Shared HTTP plumbing: a lazily-built `reqwest::Client`, retry with backoff, deadlines.

use crate::error::{Error, ErrorKind, Result};
use std::future::Future;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Process-wide HTTP client (connection pooling across calls).
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("liteocr/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("reqwest client builds")
    })
}

/// A whole-call deadline shared by upload, polling and result download.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    start: Instant,
    total: Duration,
}

impl Deadline {
    pub fn new(secs: f64) -> Self {
        Self { start: Instant::now(), total: Duration::from_secs_f64(secs.max(0.001)) }
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn remaining(&self) -> Duration {
        self.total.saturating_sub(self.start.elapsed())
    }

    pub fn expired(&self) -> bool {
        self.remaining().is_zero()
    }

    /// Timeout to use for a single HTTP request, capped by the remaining budget.
    pub fn request_timeout(&self) -> Duration {
        self.remaining().max(Duration::from_millis(1))
    }

    pub fn check(&self, provider: &str, what: &str) -> Result<()> {
        if self.expired() {
            Err(Error::timeout(format!("deadline exceeded while {what}")).with_provider(provider))
        } else {
            Ok(())
        }
    }
}

/// Retry policy for transient failures.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self { max_retries: 2, base_delay: Duration::from_millis(500), max_delay: Duration::from_secs(20) }
    }
}

impl Retry {
    pub fn new(max_retries: u32) -> Self {
        Self { max_retries, ..Default::default() }
    }

    /// Exponential backoff with full jitter: `rand(0, min(max, base * 2^attempt))`.
    pub fn delay(&self, attempt: u32) -> Duration {
        let exp = self.base_delay.saturating_mul(2u32.saturating_pow(attempt.min(16)));
        let cap = exp.min(self.max_delay);
        let jitter: f64 = rand::random::<f64>();
        cap.mul_f64(0.5 + 0.5 * jitter)
    }
}

/// Run `op` with retries on retryable errors, respecting the deadline.
pub async fn with_retry<T, F, Fut>(provider: &str, retry: Retry, deadline: &Deadline, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0u32;
    loop {
        deadline.check(provider, "sending request")?;
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) if is_transient(&e) && attempt < retry.max_retries => {
                let delay = retry.delay(attempt).min(deadline.remaining());
                tracing::warn!(provider, attempt, ?delay, error = %e, "retrying after transient error");
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_transient(e: &Error) -> bool {
    matches!(e.kind, ErrorKind::RateLimit | ErrorKind::Network)
        || (e.kind == ErrorKind::Provider && matches!(e.status_code, Some(500 | 502 | 503 | 504)))
}

/// Turn a response into `(status, body)`; map non-2xx into `Error`.
pub async fn read_response(provider: &str, resp: reqwest::Response) -> Result<String> {
    let status = resp.status();
    let body = resp.text().await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(Error::from_http(provider, status.as_u16(), &body))
    }
}

/// Convenience: read + parse JSON.
pub async fn read_json<T: serde::de::DeserializeOwned>(provider: &str, resp: reqwest::Response) -> Result<T> {
    let body = read_response(provider, resp).await?;
    serde_json::from_str(&body).map_err(|e| {
        Error::provider(format!("unexpected response shape: {e}; body starts: {}", snippet(&body)))
            .with_provider(provider)
    })
}

pub fn snippet(s: &str) -> String {
    let t: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        format!("{t}…")
    } else {
        t
    }
}

/// Polling helper: calls `poll` until it returns `Some(T)` or the deadline expires.
/// Interval grows from `initial` to `max` (×1.5 each round).
pub async fn poll_until<T, F, Fut>(
    provider: &str,
    deadline: &Deadline,
    initial: Duration,
    max: Duration,
    mut poll: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>>>,
{
    let mut interval = initial;
    loop {
        deadline.check(provider, "waiting for job to finish")?;
        if let Some(v) = poll().await? {
            return Ok(v);
        }
        let sleep = interval.min(deadline.remaining());
        tokio::time::sleep(sleep).await;
        interval = interval.mul_f64(1.5).min(max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded() {
        let r = Retry::default();
        for a in 0..10 {
            let d = r.delay(a);
            assert!(d <= r.max_delay);
            assert!(d >= Duration::from_millis(1));
        }
    }

    #[tokio::test]
    async fn retries_transient_then_succeeds() {
        let dl = Deadline::new(5.0);
        let mut n = 0;
        let r = with_retry("t", Retry { base_delay: Duration::from_millis(1), ..Retry::new(3) }, &dl, || {
            n += 1;
            let k = n;
            async move {
                if k < 3 {
                    Err(Error::network("flaky"))
                } else {
                    Ok(k)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(r, 3);
    }

    #[tokio::test]
    async fn does_not_retry_bad_request() {
        let dl = Deadline::new(5.0);
        let mut n = 0;
        let r: Result<()> = with_retry("t", Retry::new(3), &dl, || {
            n += 1;
            async { Err(Error::from_http("t", 400, "no")) }
        })
        .await;
        assert!(r.is_err());
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn poll_respects_deadline() {
        let dl = Deadline::new(0.05);
        let r: Result<()> =
            poll_until("t", &dl, Duration::from_millis(10), Duration::from_millis(10), || async { Ok(None) }).await;
        assert_eq!(r.unwrap_err().kind, ErrorKind::Timeout);
    }
}
