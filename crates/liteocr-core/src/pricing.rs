//! Embedded, overridable per-mode price table used to compute `cost_usd` on every response.

use crate::types::Mode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

/// Prices for one model: USD per page for each mode it is priced in.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PriceEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

impl PriceEntry {
    pub fn for_mode(&self, mode: Mode) -> Option<f64> {
        match mode {
            Mode::Parse => self.parse,
            Mode::Ocr => self.ocr.or(self.parse),
            Mode::Extract => self.extract,
        }
    }

    fn set_mode(&mut self, mode: Mode, price: f64) {
        match mode {
            Mode::Parse => self.parse = Some(price),
            Mode::Ocr => self.ocr = Some(price),
            Mode::Extract => self.extract = Some(price),
        }
    }
}

const EMBEDDED: &str = include_str!("pricing.json");

fn table() -> &'static RwLock<BTreeMap<String, PriceEntry>> {
    static TABLE: OnceLock<RwLock<BTreeMap<String, PriceEntry>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(parse_embedded()))
}

fn parse_embedded() -> BTreeMap<String, PriceEntry> {
    let v: serde_json::Value = serde_json::from_str(EMBEDDED).expect("embedded pricing.json is valid");
    v.as_object()
        .expect("pricing.json is an object")
        .iter()
        .filter(|(k, _)| !k.starts_with('_'))
        .map(|(k, v)| {
            let e: PriceEntry = serde_json::from_value(v.clone()).expect("valid price entry");
            (k.clone(), e)
        })
        .collect()
}

/// Price per page for a fully-qualified model in `mode`, if known.
pub fn price_per_page(model: &str, mode: Mode) -> Option<f64> {
    table().read().ok()?.get(model).and_then(|e| e.for_mode(mode))
}

/// Snapshot of the whole table.
pub fn all_prices() -> BTreeMap<String, PriceEntry> {
    table().read().map(|t| t.clone()).unwrap_or_default()
}

/// Insert or replace per-page prices for `mode` (used by `liteocr.set_pricing`).
pub fn set_prices(prices: BTreeMap<String, f64>, mode: Mode) {
    if let Ok(mut t) = table().write() {
        for (k, v) in prices {
            let e = t.entry(k).or_default();
            e.set_mode(mode, v);
            e.source = Some("user".into());
        }
    }
}

/// Reset to the embedded defaults.
pub fn reset_prices() {
    if let Ok(mut t) = table().write() {
        *t = parse_embedded();
    }
}

/// Compute the cost for `pages` at `model`'s rate in `mode`.
pub fn estimate_cost(model: &str, mode: Mode, pages: u32) -> Option<f64> {
    price_per_page(model, mode).map(|p| p * f64::from(pages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_table_loads() {
        assert!(price_per_page("reducto/standard", Mode::Parse).is_some());
        assert!(price_per_page("reducto/standard", Mode::Extract).is_none());
        assert!(price_per_page("nope/none", Mode::Parse).is_none());
        assert!((estimate_cost("llamaparse/fast", Mode::Ocr, 10).unwrap() - 0.0125).abs() < 1e-9);
    }

    #[test]
    fn override_and_reset() {
        set_prices(BTreeMap::from([("custom/x".to_string(), 1.5)]), Mode::Extract);
        assert_eq!(price_per_page("custom/x", Mode::Extract), Some(1.5));
        assert_eq!(price_per_page("custom/x", Mode::Parse), None);
        reset_prices();
        assert!(price_per_page("custom/x", Mode::Extract).is_none());
    }

    #[test]
    fn every_registered_model_has_a_price_for_each_mode() {
        for p in crate::model::PROVIDERS {
            for m in p.models {
                for mode in m.modes {
                    assert!(price_per_page(&m.qualified(), *mode).is_some(), "missing price: {} {mode}", m.qualified());
                }
            }
        }
    }
}
