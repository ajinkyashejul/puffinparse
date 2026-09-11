//! # liteocr-core
//!
//! One API for every OCR / document-parsing provider.
//!
//! ```no_run
//! use liteocr_core::{ocr, OcrRequest};
//!
//! # async fn run() -> liteocr_core::Result<()> {
//! let resp = ocr(OcrRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
//! println!("{}", resp.markdown);
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod bench;
pub mod error;
pub mod http;
pub mod model;
pub mod pricing;
pub mod provider;
pub mod providers;
pub mod router;
pub mod types;
pub mod util;

pub use error::{Error, ErrorKind, Result};
pub use model::{list_models, ModelRef, ProviderInfo, PROVIDERS};
pub use provider::OcrProvider;
pub use router::{Router, RouterConfig, Strategy};
pub use types::{BBox, Block, BlockType, DocumentInput, OcrRequest, OcrResponse, OutputFormat, Page, Usage};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run an OCR request against the provider selected by `request.model`.
///
/// This is the single entry point used by the Python SDK, the CLI and the [`Router`].
pub async fn ocr(request: OcrRequest) -> Result<OcrResponse> {
    let model_ref = ModelRef::parse(&request.model)?;
    let provider = providers::build(&model_ref.provider)?;
    let started = std::time::Instant::now();
    let qualified = model_ref.qualified();
    tracing::info!(model = %qualified, input = %request.input.describe(), "liteocr: starting");

    let result = provider.ocr(&request, &model_ref.model).await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(mut resp) => {
            resp.model = qualified.clone();
            resp.provider = model_ref.provider.clone();
            resp.latency_ms = latency_ms;
            resp.metadata.extend(request.metadata.clone());
            if resp.cost_usd.is_none() {
                resp.cost_usd =
                    resp.usage.provider_cost_usd.or_else(|| pricing::estimate_cost(&qualified, resp.usage.pages));
            }
            if !request.include_raw {
                resp.raw = None;
            }
            tracing::info!(
                model = %qualified,
                pages = resp.usage.pages,
                latency_ms,
                cost_usd = ?resp.cost_usd,
                "liteocr: done"
            );
            Ok(resp)
        }
        Err(e) => {
            let e = if e.provider.is_none() { e.with_provider(&model_ref.provider) } else { e };
            tracing::warn!(model = %qualified, latency_ms, error = %e, "liteocr: failed");
            Err(e)
        }
    }
}

/// Blocking wrapper around [`ocr`] for non-async callers. Builds a small runtime per call.
pub fn ocr_blocking(request: OcrRequest) -> Result<OcrResponse> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::provider(format!("failed to start runtime: {e}")))?;
    rt.block_on(ocr(request))
}
