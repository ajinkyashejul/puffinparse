//! Native-format compatibility: render a unified response in a vendor's own response shape.
//!
//! A caller already integrated with Reducto, Extend or LlamaParse can switch the underlying
//! provider and keep parsing the JSON it already knows: ask for `output_format="reducto"` and the
//! unified [`ParseResponse`] is rendered into Reducto's `ParseResponse` JSON, whatever provider
//! actually produced it.
//!
//! ```
//! use puffinparse_core::compat::{render_parse, Format};
//! # fn demo(resp: &puffinparse_core::ParseResponse) {
//! let native = render_parse(resp, Format::Reducto);
//! assert_eq!(native["response_type"], "parse");
//! # }
//! ```
//!
//! What is guaranteed is **structural fidelity**, not semantic identity: the key set, the nesting,
//! the chunk/page/block counts, the content strings, the block-type vocabulary and the coordinate
//! units are the vendor's. Fields PuffinParse never models (presigned URLs, studio links, per-product
//! billing breakdowns, OCR word layers) are rendered as `null` or empty. See `docs/COMPAT.md`.

mod extend;
mod llamaparse;
mod reducto;

#[cfg(test)]
mod roundtrip;

use crate::error::{Error, Result};
use crate::types::{ExtractResponse, Page, ParseResponse};
use serde_json::Value;

/// Page dimensions used when the unified response carries none. Vendors that report block boxes in
/// page units need *some* page size; 1000x1000 keeps the numbers readable and makes the
/// normalised 0..1 coordinates recoverable by dividing by 1000.
pub const SYNTHETIC_PAGE_DIM: f64 = 1000.0;

/// A response shape PuffinParse can render into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Format {
    /// PuffinParse's own unified shape (the default).
    #[default]
    #[serde(rename = "puffinparse")]
    Puffinparse,
    #[serde(rename = "reducto")]
    Reducto,
    #[serde(rename = "extend")]
    Extend,
    #[serde(rename = "llamaparse")]
    LlamaParse,
}

impl Format {
    /// Every format, in documentation order. Useful for CLI help and exhaustive tests.
    pub const ALL: &'static [Format] = &[Format::Puffinparse, Format::Reducto, Format::Extend, Format::LlamaParse];

    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Puffinparse => "puffinparse",
            Format::Reducto => "reducto",
            Format::Extend => "extend",
            Format::LlamaParse => "llamaparse",
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "puffinparse" | "unified" | "native" => Ok(Format::Puffinparse),
            "reducto" => Ok(Format::Reducto),
            "extend" => Ok(Format::Extend),
            "llamaparse" | "llama" | "llama_parse" | "llama_index" | "llamaindex" => Ok(Format::LlamaParse),
            other => Err(Error::input(format!(
                "unknown output_format '{other}' (puffinparse | reducto | extend | llamaparse)"
            ))),
        }
    }
}

/// Render a unified parse response in `format`'s native JSON shape.
///
/// Never fails: unknown values become `null`, missing geometry falls back to
/// [`SYNTHETIC_PAGE_DIM`].
pub fn render_parse(resp: &ParseResponse, format: Format) -> Value {
    match format {
        // The unified types are plain structs with string map keys, so this cannot fail.
        Format::Puffinparse => serde_json::to_value(resp).unwrap_or(Value::Null),
        Format::Reducto => reducto::render_parse(resp),
        Format::Extend => extend::render_parse(resp),
        Format::LlamaParse => llamaparse::render_parse(resp),
    }
}

/// Render a unified extract response in `format`'s native JSON shape.
///
/// **Best effort.** The extract surfaces differ far more between vendors than the parse ones
/// (schemas, citation shapes and per-field metadata are all vendor-specific), so this reproduces
/// the documented envelope and the citation/confidence placement, not every optional field.
pub fn render_extract(resp: &ExtractResponse, format: Format) -> Value {
    match format {
        Format::Puffinparse => serde_json::to_value(resp).unwrap_or(Value::Null),
        Format::Reducto => reducto::render_extract(resp),
        Format::Extend => extend::render_extract(resp),
        Format::LlamaParse => llamaparse::render_extract(resp),
    }
}

// ---- shared helpers ------------------------------------------------------------------------------

/// Page size in the vendor's coordinate units, plus whether it had to be invented.
pub(crate) fn page_dims(page: &Page) -> (f64, f64, bool) {
    match (page.width.filter(|w| *w > 0.0), page.height.filter(|h| *h > 0.0)) {
        (Some(w), Some(h)) => (w, h, false),
        _ => (SYNTHETIC_PAGE_DIM, SYNTHETIC_PAGE_DIM, true),
    }
}

/// True when any page needed synthetic dimensions.
pub(crate) fn any_synthetic_dims(resp: &ParseResponse) -> bool {
    resp.pages.iter().any(|p| page_dims(p).2)
}

/// The vendor job id a caller would have seen, falling back to PuffinParse's own response id so the
/// field is never null (integrations key off it).
pub(crate) fn job_id(id: &str, provider_job_id: Option<&String>) -> String {
    provider_job_id.cloned().unwrap_or_else(|| id.to_string())
}

/// Mean of the confidences present, or `None` when nothing reported one.
pub(crate) fn mean_confidence(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let present: Vec<f64> = values.flatten().collect();
    if present.is_empty() {
        return None;
    }
    Some(present.iter().sum::<f64>() / present.len() as f64)
}

/// Smallest confidence present, or `None`.
pub(crate) fn min_confidence(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    values.flatten().fold(None::<f64>, |acc, c| Some(acc.map_or(c, |a| a.min(c))))
}

/// `serde_json` renders `Option<f64>` as `null` when absent, which is what every vendor does.
pub(crate) fn num(v: Option<f64>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn format_parses_aliases() {
        assert_eq!(Format::from_str("Reducto").unwrap(), Format::Reducto);
        assert_eq!(Format::from_str(" extend ").unwrap(), Format::Extend);
        for alias in ["llamaparse", "llama", "llama_parse", "llama-parse", "LlamaIndex"] {
            assert_eq!(Format::from_str(alias).unwrap(), Format::LlamaParse, "{alias}");
        }
        assert_eq!(Format::from_str("puffinparse").unwrap(), Format::Puffinparse);
        assert_eq!(Format::default(), Format::Puffinparse);
        let err = Format::from_str("nope").unwrap_err();
        assert!(err.to_string().contains("llamaparse"), "{err}");
    }

    #[test]
    fn format_roundtrips_through_string_and_serde() {
        for f in Format::ALL {
            assert_eq!(Format::from_str(f.as_str()).unwrap(), *f);
            assert_eq!(serde_json::to_value(f).unwrap(), Value::from(f.as_str()));
            assert_eq!(f.to_string(), f.as_str());
        }
        assert_eq!(Format::ALL.len(), 4);
    }
}
