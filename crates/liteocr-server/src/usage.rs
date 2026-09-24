//! Per-key accounting: monthly spend, request counts and the sliding-window rate limiter.
//!
//! State lives in memory. With `server.state_file` set, spend and counts are written to a small
//! JSON file after every billed request (temp file + rename) and reloaded on startup, so a restart
//! does not reset budgets. Rate-limit windows are deliberately not persisted.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Spend and volume for one key in one calendar month (UTC).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyUsage {
    /// `YYYY-MM` the counters belong to; they reset when the month changes.
    pub month: String,
    pub spend_usd: f64,
    pub requests: u64,
    pub pages: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Persisted {
    keys: BTreeMap<String, KeyUsage>,
}

#[derive(Debug, Default)]
struct Inner {
    usage: BTreeMap<String, KeyUsage>,
    windows: HashMap<String, VecDeque<Instant>>,
}

#[derive(Debug)]
pub struct UsageStore {
    inner: Mutex<Inner>,
    state_file: Option<PathBuf>,
    /// Serialises file writes so an older snapshot never overwrites a newer one.
    write_lock: Mutex<()>,
}

pub fn current_month() -> String {
    chrono::Utc::now().format("%Y-%m").to_string()
}

impl UsageStore {
    pub fn new(state_file: Option<PathBuf>) -> Result<Self, String> {
        let mut inner = Inner::default();
        if let Some(path) = &state_file {
            match std::fs::read_to_string(path) {
                Ok(text) => {
                    let p: Persisted = serde_json::from_str(&text)
                        .map_err(|e| format!("state file {} is not valid: {e}", path.display()))?;
                    inner.usage = p.keys;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot read state file {}: {e}", path.display())),
            }
        }
        Ok(Self { inner: Mutex::new(inner), state_file, write_lock: Mutex::new(()) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// This month's usage for `key_id` (zeroed if the stored month is stale).
    pub fn get(&self, key_id: &str) -> KeyUsage {
        let month = current_month();
        match self.lock().usage.get(key_id) {
            Some(u) if u.month == month => u.clone(),
            _ => KeyUsage { month, ..KeyUsage::default() },
        }
    }

    /// Sliding 60-second window. `Err(secs)` carries a Retry-After hint.
    pub fn check_rate(&self, key_id: &str, rpm: u32) -> Result<(), u64> {
        self.check_rate_at(key_id, rpm, Instant::now())
    }

    fn check_rate_at(&self, key_id: &str, rpm: u32, now: Instant) -> Result<(), u64> {
        let window = Duration::from_secs(60);
        let mut inner = self.lock();
        let q = inner.windows.entry(key_id.to_string()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        if q.len() >= rpm as usize {
            let oldest = q.front().copied().unwrap_or(now);
            let wait = window.saturating_sub(now.duration_since(oldest));
            return Err(wait.as_secs().max(1));
        }
        q.push_back(now);
        Ok(())
    }

    /// Add one finished request to `key_id`'s month and persist if configured.
    pub fn record(&self, key_id: &str, cost_usd: f64, pages: u32) {
        let month = current_month();
        {
            let mut inner = self.lock();
            let u = inner.usage.entry(key_id.to_string()).or_default();
            if u.month != month {
                *u = KeyUsage { month, ..KeyUsage::default() };
            }
            u.spend_usd += cost_usd;
            u.requests += 1;
            u.pages += u64::from(pages);
        }
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = &self.state_file else { return };
        let _guard = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        let snapshot = Persisted { keys: self.lock().usage.clone() };
        let text = match serde_json::to_string_pretty(&snapshot) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, "liteocr-server: cannot serialise usage state");
                return;
            }
        };
        let tmp = path.with_extension("tmp");
        let result = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = result {
            tracing::error!(error = %e, path = %path.display(), "liteocr-server: cannot write usage state");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_window_slides() {
        let s = UsageStore::new(None).unwrap();
        let t0 = Instant::now();
        assert!(s.check_rate_at("k", 2, t0).is_ok());
        assert!(s.check_rate_at("k", 2, t0 + Duration::from_secs(1)).is_ok());
        let wait = s.check_rate_at("k", 2, t0 + Duration::from_secs(2)).unwrap_err();
        assert_eq!(wait, 58);
        assert!(s.check_rate_at("other", 2, t0).is_ok());
        assert!(s.check_rate_at("k", 2, t0 + Duration::from_secs(60)).is_ok());
    }

    #[test]
    fn persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("liteocr-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let s = UsageStore::new(Some(path.clone())).unwrap();
        s.record("a", 0.25, 3);
        s.record("a", 0.5, 1);
        let reloaded = UsageStore::new(Some(path)).unwrap();
        let u = reloaded.get("a");
        assert_eq!((u.requests, u.pages), (2, 4));
        assert!((u.spend_usd - 0.75).abs() < 1e-9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_month_resets() {
        let s = UsageStore::new(None).unwrap();
        s.lock().usage.insert("a".into(), KeyUsage { month: "1999-01".into(), spend_usd: 9.0, requests: 9, pages: 9 });
        assert_eq!(s.get("a").spend_usd, 0.0);
        s.record("a", 1.0, 1);
        assert_eq!(s.get("a").requests, 1);
    }
}
