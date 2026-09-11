//! Embedded, overridable price table used to compute `OcrResponse::cost_usd`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceEntry {
    pub per_page_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
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

/// Price per page for a fully-qualified model, if known.
pub fn price_per_page(model: &str) -> Option<f64> {
    table().read().ok()?.get(model).map(|e| e.per_page_usd)
}

/// Snapshot of the whole table.
pub fn all_prices() -> BTreeMap<String, PriceEntry> {
    table().read().map(|t| t.clone()).unwrap_or_default()
}

/// Insert or replace prices (used by `liteocr.set_pricing`).
pub fn set_prices(prices: BTreeMap<String, f64>) {
    if let Ok(mut t) = table().write() {
        for (k, v) in prices {
            t.insert(k, PriceEntry { per_page_usd: v, source: Some("user".into()), updated: None });
        }
    }
}

/// Reset to the embedded defaults.
pub fn reset_prices() {
    if let Ok(mut t) = table().write() {
        *t = parse_embedded();
    }
}

/// Compute the cost for `pages` at `model`'s rate.
pub fn estimate_cost(model: &str, pages: u32) -> Option<f64> {
    price_per_page(model).map(|p| p * f64::from(pages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_table_loads() {
        assert!(price_per_page("reducto/standard").is_some());
        assert!(price_per_page("nope/none").is_none());
        assert!((estimate_cost("llamaparse/fast", 10).unwrap() - 0.0125).abs() < 1e-9);
    }

    #[test]
    fn override_and_reset() {
        set_prices(BTreeMap::from([("custom/x".to_string(), 1.5)]));
        assert_eq!(price_per_page("custom/x"), Some(1.5));
        reset_prices();
        assert!(price_per_page("custom/x").is_none());
    }
}
