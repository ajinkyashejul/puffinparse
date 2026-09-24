//! # liteocr-server
//!
//! The LiteOCR gateway: one HTTP endpoint in front of every OCR provider, with model aliases and
//! fallbacks, virtual keys with per-key model allow-lists, monthly budgets and rate limits,
//! JSON-lines request logs and Prometheus metrics. See `docs/SERVER.md`.
//!
//! ```no_run
//! # async fn run() -> Result<(), String> {
//! let config = liteocr_server::Config::load(std::path::Path::new("liteocr.toml"))?;
//! liteocr_server::serve(config).await
//! # }
//! ```

#![forbid(unsafe_code)]

mod api;
pub mod config;
pub mod error;
pub mod log;
pub mod metrics;
pub mod usage;

pub use config::Config;

use config::{resolve_secret, AliasConfig, Target};
use liteocr_core::{ErrorKind, Strategy};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

/// A provider call target with credentials resolved.
#[derive(Debug, Clone)]
pub(crate) struct Deployment {
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Alias {
    pub targets: Vec<Deployment>,
    pub strategy: Strategy,
    pub fallback_on: Vec<ErrorKind>,
    pub cursor: AtomicUsize,
}

#[derive(Debug)]
pub(crate) struct VirtualKey {
    pub id: String,
    pub secret: String,
    pub models: Vec<String>,
    pub monthly_budget_usd: Option<f64>,
    pub rpm: Option<u32>,
}

/// Everything a request handler needs. Built once from a [`Config`].
#[derive(Debug)]
pub struct AppState {
    pub(crate) server: config::ServerConfig,
    pub(crate) master_key: Option<String>,
    /// Provider name → (api_key, base_url) from `[providers.*]`.
    pub(crate) providers: BTreeMap<String, (Option<String>, Option<String>)>,
    pub(crate) aliases: BTreeMap<String, Alias>,
    pub(crate) keys: Vec<VirtualKey>,
    pub(crate) usage: usage::UsageStore,
    pub(crate) metrics: metrics::Metrics,
    pub(crate) logger: log::Logger,
}

pub(crate) fn default_fallback_on() -> Vec<ErrorKind> {
    vec![ErrorKind::Provider, ErrorKind::RateLimit, ErrorKind::Timeout, ErrorKind::Network]
}

fn secret(field: &str, value: Option<&String>) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(v) => {
            let resolved = resolve_secret(v).map_err(|e| format!("{field}: {e}"))?;
            if resolved.is_none() {
                // Name the variable, never a value (a literal that resolved to nothing is empty).
                eprintln!("warning: {field} = \"{v}\" resolved to nothing (unset or empty)");
            }
            Ok(resolved)
        }
    }
}

impl AppState {
    /// Resolve secrets, open the log sink and load persisted usage.
    pub fn from_config(cfg: Config) -> Result<Self, String> {
        Self::build(cfg, None)
    }

    /// Like [`AppState::from_config`] but with request logging switched off (tests).
    pub fn from_config_quiet(cfg: Config) -> Result<Self, String> {
        Self::build(cfg, Some(log::Logger::disabled()))
    }

    fn build(cfg: Config, logger: Option<log::Logger>) -> Result<Self, String> {
        let master_key = secret("master_key", cfg.master_key.as_ref())?;
        let mut providers = BTreeMap::new();
        for (name, p) in &cfg.providers {
            let key = secret(&format!("providers.{name}.api_key"), p.api_key.as_ref())?;
            providers.insert(name.clone(), (key, p.base_url.clone()));
        }
        let mut aliases = BTreeMap::new();
        for AliasConfig { name, targets, strategy, fallback_on } in &cfg.models {
            let targets = targets
                .iter()
                .map(|t| {
                    Ok(match t {
                        Target::Model(m) => Deployment { model: m.clone(), api_key: None, base_url: None },
                        Target::Detailed { model, api_key, base_url } => Deployment {
                            model: model.clone(),
                            api_key: secret(&format!("models.{name}.api_key"), api_key.as_ref())?,
                            base_url: base_url.clone(),
                        },
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            aliases.insert(
                name.clone(),
                Alias {
                    targets,
                    strategy: *strategy,
                    fallback_on: fallback_on.clone().unwrap_or_else(default_fallback_on),
                    cursor: AtomicUsize::new(0),
                },
            );
        }
        let mut keys = Vec::new();
        for k in &cfg.keys {
            let Some(secret) = secret(&format!("keys.{}.key", k.id), Some(&k.key))? else {
                return Err(format!("keys.{}.key resolved to an empty value", k.id));
            };
            keys.push(VirtualKey {
                id: k.id.clone(),
                secret,
                models: k.models.clone(),
                monthly_budget_usd: k.monthly_budget_usd,
                rpm: k.rpm,
            });
        }
        if cfg.master_key.is_some() && master_key.is_none() {
            return Err("master_key is set but resolved to an empty value".into());
        }
        let logger = match logger {
            Some(l) => l,
            None => log::Logger::new(cfg.server.log_stdout, cfg.server.log_file.as_deref())?,
        };
        let usage = usage::UsageStore::new(cfg.server.state_file.clone())?;
        Ok(Self {
            server: cfg.server,
            master_key,
            providers,
            aliases,
            keys,
            usage,
            metrics: metrics::Metrics::default(),
            logger,
        })
    }

    /// Whether requests must carry a bearer token.
    pub fn auth_enabled(&self) -> bool {
        self.master_key.is_some() || !self.keys.is_empty()
    }
}

/// The axum application (all routes and layers), for embedding or for tests.
pub fn app(state: Arc<AppState>) -> axum::Router {
    api::router(state)
}

/// Bind `server.host:server.port` and serve until Ctrl-C / SIGTERM.
pub async fn serve(cfg: Config) -> Result<(), String> {
    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let state = Arc::new(AppState::from_config(cfg)?);
    if !state.auth_enabled() {
        eprintln!("warning: no master_key or [[keys]] configured; the gateway is open to anyone who can reach it");
    }
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| format!("cannot bind {addr}: {e}"))?;
    let local = listener.local_addr().map(|a| a.to_string()).unwrap_or(addr);
    eprintln!(
        "liteocr gateway listening on http://{local} (auth {}; {} alias(es), {} key(s))",
        if state.auth_enabled() { "on" } else { "OFF" },
        state.aliases.len(),
        state.keys.len()
    );
    axum::serve(listener, app(state)).with_graceful_shutdown(shutdown()).await.map_err(|e| format!("server error: {e}"))
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = term => {} }
}
