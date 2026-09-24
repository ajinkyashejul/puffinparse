//! `liteocr.toml`: server settings, provider credentials, model aliases and virtual keys.
//!
//! Secrets (provider keys, virtual keys, the master key) may be written as `"env:VAR"` to read
//! them from the environment at startup; literal values work too but should stay out of VCS.

use liteocr_core::model::ModelRef;
use liteocr_core::{ErrorKind, Strategy};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    /// Bearer token with full access (all models, no budget or rate limit). `env:VAR` allowed.
    #[serde(default)]
    pub master_key: Option<String>,
    /// Per-provider credentials and base URLs, keyed by provider name (`reducto`, `llamaparse`, ...).
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Named aliases that route to one or more provider models.
    #[serde(default)]
    pub models: Vec<AliasConfig>,
    /// Virtual keys handed to clients.
    #[serde(default)]
    pub keys: Vec<KeyConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Append JSON-lines request logs to this file (in addition to stdout).
    #[serde(default)]
    pub log_file: Option<PathBuf>,
    /// Print JSON-lines request logs to stdout.
    #[serde(default = "yes")]
    pub log_stdout: bool,
    /// Persist per-key spend and request counts here (JSON), reloaded on startup.
    #[serde(default)]
    pub state_file: Option<PathBuf>,
    /// Largest accepted request body (multipart upload or base64 JSON), in MiB.
    #[serde(default = "default_max_body_mb")]
    pub max_body_mb: usize,
    /// Upper bound for a client-supplied `timeout` (seconds); also the default.
    #[serde(default = "default_timeout")]
    pub max_timeout_secs: f64,
    /// Retries per provider call when the client does not send `max_retries`.
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Allow requests naming a registry model (`reducto/standard`) that is not an alias.
    #[serde(default = "yes")]
    pub allow_direct_models: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            log_file: None,
            log_stdout: true,
            state_file: None,
            max_body_mb: default_max_body_mb(),
            max_timeout_secs: default_timeout(),
            max_retries: default_retries(),
            allow_direct_models: true,
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    4000
}
fn yes() -> bool {
    true
}
fn default_max_body_mb() -> usize {
    50
}
fn default_timeout() -> f64 {
    300.0
}
fn default_retries() -> u32 {
    2
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// API key, usually `env:REDUCTO_API_KEY`. Unset → the core reads the provider's default env var.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Base URL override (self-hosted / regional endpoints, or a mock in tests).
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AliasConfig {
    /// What clients send as `model`, e.g. `"invoices"`.
    pub name: String,
    /// Provider models behind the alias, tried per `strategy`.
    pub targets: Vec<Target>,
    #[serde(default)]
    pub strategy: Strategy,
    /// Error kinds that move on to the next target (default: provider, rate_limit, timeout, network).
    #[serde(default)]
    pub fallback_on: Option<Vec<ErrorKind>>,
}

/// One deployment behind an alias: a bare model string, or a table with per-target overrides.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Target {
    Model(String),
    Detailed {
        model: String,
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        base_url: Option<String>,
    },
}

impl Target {
    pub fn model(&self) -> &str {
        match self {
            Target::Model(m) | Target::Detailed { model: m, .. } => m,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyConfig {
    /// Stable, non-secret identifier used in logs, metrics and the state file.
    pub id: String,
    /// The bearer token clients send. `env:VAR` allowed.
    pub key: String,
    /// Aliases or `provider/model` strings this key may call (`provider/*` wildcards allowed).
    /// Empty or absent → every model.
    #[serde(default)]
    pub models: Vec<String>,
    /// Spend cap per calendar month (UTC), in USD of estimated list-price cost.
    #[serde(default)]
    pub monthly_budget_usd: Option<f64>,
    /// Requests per minute (sliding 60 s window).
    #[serde(default)]
    pub rpm: Option<u32>,
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| format!("invalid config: {e}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_toml(&text)
    }

    fn validate(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        for a in &self.models {
            if a.name.trim().is_empty() {
                return Err("models: alias name must not be empty".into());
            }
            if !names.insert(a.name.as_str()) {
                return Err(format!("models: duplicate alias '{}'", a.name));
            }
            if a.targets.is_empty() {
                return Err(format!("models: alias '{}' has no targets", a.name));
            }
            for t in &a.targets {
                ModelRef::parse(t.model()).map_err(|e| format!("models: alias '{}': {}", a.name, e.message))?;
            }
        }
        for p in self.providers.keys() {
            if liteocr_core::model::provider_info(p).is_none() {
                return Err(format!("providers: unknown provider '{p}'"));
            }
        }
        let mut ids = BTreeSet::new();
        for k in &self.keys {
            if !ids.insert(k.id.as_str()) {
                return Err(format!("keys: duplicate id '{}'", k.id));
            }
            if k.key.trim().is_empty() {
                return Err(format!("keys: '{}' has an empty key", k.id));
            }
            if k.monthly_budget_usd.is_some_and(|b| b.is_nan() || b < 0.0) {
                return Err(format!("keys: '{}' monthly_budget_usd must be >= 0", k.id));
            }
        }
        if self.server.max_timeout_secs.is_nan() || self.server.max_timeout_secs <= 0.0 {
            return Err("server.max_timeout_secs must be > 0".into());
        }
        Ok(())
    }
}

/// Resolve `env:VAR` references. Returns `Ok(None)` when the variable is unset or empty.
pub fn resolve_secret(value: &str) -> Result<Option<String>, String> {
    match value.strip_prefix("env:") {
        Some(var) => {
            let var = var.trim();
            if var.is_empty() {
                return Err("'env:' reference without a variable name".into());
            }
            Ok(std::env::var(var).ok().filter(|v| !v.trim().is_empty()))
        }
        None => Ok(Some(value.to_string()).filter(|v| !v.trim().is_empty())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample() {
        let cfg = Config::from_toml(
            r#"
            master_key = "env:LITEOCR_MASTER_KEY"
            [server]
            port = 8080
            [providers.reducto]
            api_key = "env:REDUCTO_API_KEY"
            [[models]]
            name = "invoices"
            targets = ["reducto/standard", { model = "extend/parse_performance", base_url = "http://x" }]
            strategy = "round_robin"
            [[keys]]
            id = "team-a"
            key = "sk-a"
            models = ["invoices", "llamaparse/*"]
            monthly_budget_usd = 10.0
            rpm = 60
            "#,
        )
        .unwrap();
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.models[0].strategy, Strategy::RoundRobin);
        assert_eq!(cfg.models[0].targets[1].model(), "extend/parse_performance");
        assert_eq!(cfg.keys[0].rpm, Some(60));
    }

    #[test]
    fn example_config_is_valid() {
        let cfg = Config::from_toml(include_str!("../../../examples/server/liteocr.toml")).unwrap();
        assert_eq!(cfg.models.len(), 3);
        assert_eq!(cfg.keys.len(), 2);
    }

    #[test]
    fn rejects_bad_configs() {
        assert!(Config::from_toml("[[models]]\nname='a'\ntargets=['nope/x']").is_err());
        assert!(Config::from_toml("[[models]]\nname='a'\ntargets=[]").is_err());
        assert!(Config::from_toml("[providers.nope]\napi_key='x'").is_err());
        assert!(Config::from_toml("[[keys]]\nid='a'\nkey='x'\n[[keys]]\nid='a'\nkey='y'").is_err());
        assert!(Config::from_toml("unknown = 1").is_err());
    }

    #[test]
    fn resolves_env_secrets() {
        std::env::set_var("LITEOCR_TEST_SECRET_XYZ", "s3cret");
        assert_eq!(resolve_secret("env:LITEOCR_TEST_SECRET_XYZ").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(resolve_secret("env:LITEOCR_TEST_UNSET_XYZ").unwrap(), None);
        assert_eq!(resolve_secret("literal").unwrap().as_deref(), Some("literal"));
        assert!(resolve_secret("env:").is_err());
    }
}
