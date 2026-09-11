//! Multi-model routing: ordered fallbacks and round-robin, with per-model stats.

use crate::error::{Error, ErrorKind, Result};
use crate::model::ModelRef;
use crate::types::{DocumentRequest, ExtractRequest, ExtractResponse, Mode, ParseResponse, TextResponse};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Always start with the first model; fall back in order.
    #[default]
    Ordered,
    /// Rotate the starting model per call; fall back in order from there.
    RoundRobin,
}

impl std::str::FromStr for Strategy {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().replace('-', "_").as_str() {
            "ordered" | "fallback" => Ok(Strategy::Ordered),
            "round_robin" | "roundrobin" => Ok(Strategy::RoundRobin),
            other => Err(Error::input(format!("unknown router strategy '{other}'"))),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelStats {
    pub successes: u64,
    pub failures: u64,
    pub total_latency_ms: u64,
    pub total_cost_usd: f64,
    pub total_pages: u64,
}

impl ModelStats {
    pub fn avg_latency_ms(&self) -> Option<f64> {
        if self.successes == 0 {
            None
        } else {
            Some(self.total_latency_ms as f64 / self.successes as f64)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub models: Vec<String>,
    /// The mode every model must support; calls in another mode are rejected.
    #[serde(default = "default_mode")]
    pub mode: Mode,
    #[serde(default)]
    pub strategy: Strategy,
    /// Error kinds that trigger a fallback to the next model.
    #[serde(default = "default_fallback_on")]
    pub fallback_on: Vec<ErrorKind>,
}

fn default_mode() -> Mode {
    Mode::Parse
}

fn default_fallback_on() -> Vec<ErrorKind> {
    vec![ErrorKind::Provider, ErrorKind::RateLimit, ErrorKind::Timeout, ErrorKind::Network]
}

impl RouterConfig {
    pub fn new(models: Vec<String>) -> Self {
        Self { models, mode: Mode::Parse, strategy: Strategy::Ordered, fallback_on: default_fallback_on() }
    }

    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }
}

/// Routes requests across several models.
pub struct Router {
    models: Vec<ModelRef>,
    mode: Mode,
    strategy: Strategy,
    fallback_on: Vec<ErrorKind>,
    cursor: AtomicUsize,
    stats: Mutex<BTreeMap<String, ModelStats>>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("models", &self.models.iter().map(ModelRef::qualified).collect::<Vec<_>>())
            .field("mode", &self.mode)
            .field("strategy", &self.strategy)
            .finish()
    }
}

impl Router {
    pub fn new(config: RouterConfig) -> Result<Self> {
        if config.models.is_empty() {
            return Err(Error::input("router needs at least one model"));
        }
        let models = config.models.iter().map(|m| ModelRef::parse_for(m, config.mode)).collect::<Result<Vec<_>>>()?;
        let stats = models.iter().map(|m| (m.qualified(), ModelStats::default())).collect();
        Ok(Self {
            models,
            mode: config.mode,
            strategy: config.strategy,
            fallback_on: config.fallback_on,
            cursor: AtomicUsize::new(0),
            stats: Mutex::new(stats),
        })
    }

    pub fn models(&self) -> Vec<String> {
        self.models.iter().map(ModelRef::qualified).collect()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    fn check_mode(&self, wanted: Mode) -> Result<()> {
        if self.mode == wanted {
            Ok(())
        } else {
            Err(Error::input(format!("router is configured for mode '{}', cannot serve '{wanted}'", self.mode)))
        }
    }

    /// Order in which models will be tried for the next call.
    pub fn plan(&self) -> Vec<String> {
        let n = self.models.len();
        let start = match self.strategy {
            Strategy::Ordered => 0,
            Strategy::RoundRobin => self.cursor.fetch_add(1, Ordering::Relaxed) % n,
        };
        (0..n).map(|i| self.models[(start + i) % n].qualified()).collect()
    }

    /// Run a `parse` request, trying models per the strategy. The request's own `model` is ignored.
    pub async fn parse(&self, request: &DocumentRequest) -> Result<ParseResponse> {
        self.check_mode(Mode::Parse)?;
        self.run(|model| {
            let mut req = request.clone();
            req.model = model;
            async move { crate::parse(req).await.map(|r| (r.latency_ms, r.cost_usd, r.usage.pages, r)) }
        })
        .await
        .map(|(resp, i, err)| annotate(resp, i, err, |r| &mut r.metadata))
    }

    /// Run an `ocr` request across the configured models.
    pub async fn ocr(&self, request: &DocumentRequest) -> Result<TextResponse> {
        self.check_mode(Mode::Ocr)?;
        self.run(|model| {
            let mut req = request.clone();
            req.model = model;
            async move { crate::ocr(req).await.map(|r| (r.latency_ms, r.cost_usd, r.usage.pages, r)) }
        })
        .await
        .map(|(resp, i, err)| annotate(resp, i, err, |r| &mut r.metadata))
    }

    /// Run an `extract` request across the configured models.
    pub async fn extract(&self, request: &ExtractRequest) -> Result<ExtractResponse> {
        self.check_mode(Mode::Extract)?;
        self.run(|model| {
            let mut req = request.clone();
            req.document.model = model;
            async move { crate::extract(req).await.map(|r| (r.latency_ms, r.cost_usd, r.usage.pages, r)) }
        })
        .await
        .map(|(resp, i, err)| annotate(resp, i, err, |r| &mut r.metadata))
    }

    /// Shared fallback loop. `call` returns `(latency_ms, cost_usd, pages, response)`.
    async fn run<T, F, Fut>(&self, mut call: F) -> Result<(T, usize, Option<Error>)>
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Result<(u64, Option<f64>, u32, T)>>,
    {
        let plan = self.plan();
        let mut last_err: Option<Error> = None;
        for (i, model) in plan.iter().enumerate() {
            match call(model.clone()).await {
                Ok((latency_ms, cost_usd, pages, resp)) => {
                    self.record_success(model, latency_ms, cost_usd, pages);
                    return Ok((resp, i, last_err));
                }
                Err(e) => {
                    self.record_failure(model);
                    let can_fallback = self.fallback_on.contains(&e.kind) && i + 1 < plan.len();
                    tracing::warn!(model, error = %e, can_fallback, "router: model failed");
                    if !can_fallback {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| Error::provider("router: no models tried")))
    }

    fn record_success(&self, model: &str, latency_ms: u64, cost_usd: Option<f64>, pages: u32) {
        if let Ok(mut s) = self.stats.lock() {
            let e = s.entry(model.to_string()).or_default();
            e.successes += 1;
            e.total_latency_ms += latency_ms;
            e.total_cost_usd += cost_usd.unwrap_or(0.0);
            e.total_pages += u64::from(pages);
        }
    }

    fn record_failure(&self, model: &str) {
        if let Ok(mut s) = self.stats.lock() {
            s.entry(model.to_string()).or_default().failures += 1;
        }
    }

    pub fn stats(&self) -> BTreeMap<String, ModelStats> {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Record which fallback served the call in the response metadata.
fn annotate<T>(
    mut resp: T,
    index: usize,
    err: Option<Error>,
    meta: impl FnOnce(&mut T) -> &mut BTreeMap<String, serde_json::Value>,
) -> T {
    if index > 0 {
        let m = meta(&mut resp);
        m.insert("liteocr_fallback_index".into(), serde_json::json!(index));
        if let Some(e) = err {
            m.insert("liteocr_fallback_from_error".into(), serde_json::json!(e.to_string()));
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_rotates_for_round_robin() {
        let r = Router::new(RouterConfig {
            models: vec!["reducto".into(), "extend".into(), "llamaparse/fast".into()],
            mode: Mode::Parse,
            strategy: Strategy::RoundRobin,
            fallback_on: default_fallback_on(),
        })
        .unwrap();
        assert_eq!(r.plan(), vec!["reducto/standard", "extend/parse_performance", "llamaparse/fast"]);
        assert_eq!(r.plan(), vec!["extend/parse_performance", "llamaparse/fast", "reducto/standard"]);
        let o = Router::new(RouterConfig::new(vec!["extend".into(), "reducto".into()])).unwrap();
        assert_eq!(o.plan(), o.plan());
    }

    #[test]
    fn rejects_bad_config() {
        assert!(Router::new(RouterConfig::new(vec![])).is_err());
        assert!(Router::new(RouterConfig::new(vec!["nope/x".into()])).is_err());
        // every model must support the router's mode
        assert!(Router::new(RouterConfig::new(vec!["reducto".into()]).mode(Mode::Extract)).is_err());
    }

    #[tokio::test]
    async fn rejects_calls_in_another_mode() {
        let r = Router::new(RouterConfig::new(vec!["reducto".into()])).unwrap();
        let req = DocumentRequest::from_bytes(bytes::Bytes::from_static(b"x"), "a.pdf");
        assert_eq!(r.ocr(&req).await.unwrap_err().kind, ErrorKind::Input);
    }

    #[tokio::test]
    async fn falls_back_on_auth_error_only_if_configured() {
        // No API keys in the test env → AuthenticationError, which is not a fallback kind by default.
        std::env::remove_var("REDUCTO_API_KEY");
        let r = Router::new(RouterConfig::new(vec!["reducto".into(), "extend".into()])).unwrap();
        let req = DocumentRequest::from_bytes(bytes::Bytes::from_static(b"x"), "a.pdf").api_key("");
        let err = r.parse(&req).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Authentication);
        assert_eq!(r.stats()["reducto/standard"].failures, 1);
        assert_eq!(r.stats()["extend/parse_performance"].failures, 0);
    }
}
