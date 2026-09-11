//! # liteocr-core
//!
//! One API for every OCR / document-parsing provider.
//!
//! ```no_run
//! use liteocr_core::{parse, DocumentRequest};
//!
//! # async fn run() -> liteocr_core::Result<()> {
//! let resp = parse(DocumentRequest::from_path("invoice.pdf").model("reducto/standard")).await?;
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
pub use model::{list_models, list_models_for, model_info, ModelInfo, ModelRef, ProviderInfo, PROVIDERS};
pub use provider::Provider;
pub use router::{Router, RouterConfig, Strategy};
pub use types::{
    BBox, Block, BlockType, Citation, DocumentInput, DocumentRequest, ExtractRequest, ExtractResponse, FieldInfo, Line,
    Mode, OutputFormat, Page, ParseResponse, TextPage, TextResponse, Usage, Word,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run a `parse`-mode request (layout-aware markdown + blocks) with the model in `request.model`.
///
/// This is the entry point used by the Python SDK, the CLI and the [`Router`].
pub async fn parse(request: DocumentRequest) -> Result<ParseResponse> {
    let (provider, model_ref) = resolve(&request.model, Mode::Parse)?;
    let started = std::time::Instant::now();
    let qualified = model_ref.qualified();
    tracing::info!(mode = "parse", model = %qualified, input = %request.input.describe(), "liteocr: starting");
    let result = provider.parse(&request, &model_ref.model).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(mut resp) => {
            resp.model = qualified.clone();
            resp.provider = model_ref.provider.clone();
            resp.latency_ms = latency_ms;
            resp.metadata.extend(request.metadata.clone());
            if resp.cost_usd.is_none() {
                resp.cost_usd = resp
                    .usage
                    .provider_cost_usd
                    .or_else(|| pricing::estimate_cost(&qualified, Mode::Parse, resp.usage.pages));
            }
            if !request.include_raw {
                resp.raw = None;
            }
            log_done("parse", &qualified, resp.usage.pages, latency_ms, resp.cost_usd);
            Ok(resp)
        }
        Err(e) => Err(log_failed("parse", &qualified, latency_ms, e, &model_ref.provider)),
    }
}

/// Run an `ocr`-mode request (plain text + word/line boxes) with the model in `request.model`.
pub async fn ocr(request: DocumentRequest) -> Result<TextResponse> {
    let (provider, model_ref) = resolve(&request.model, Mode::Ocr)?;
    let started = std::time::Instant::now();
    let qualified = model_ref.qualified();
    tracing::info!(mode = "ocr", model = %qualified, input = %request.input.describe(), "liteocr: starting");
    let result = provider.ocr(&request, &model_ref.model).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(mut resp) => {
            resp.model = qualified.clone();
            resp.provider = model_ref.provider.clone();
            resp.latency_ms = latency_ms;
            resp.metadata.extend(request.metadata.clone());
            if resp.cost_usd.is_none() {
                resp.cost_usd = resp
                    .usage
                    .provider_cost_usd
                    .or_else(|| pricing::estimate_cost(&qualified, Mode::Ocr, resp.usage.pages));
            }
            if !request.include_raw {
                resp.raw = None;
            }
            log_done("ocr", &qualified, resp.usage.pages, latency_ms, resp.cost_usd);
            Ok(resp)
        }
        Err(e) => Err(log_failed("ocr", &qualified, latency_ms, e, &model_ref.provider)),
    }
}

/// Run an `extract`-mode request (JSON schema → structured data) with the model in `request.document.model`.
pub async fn extract(request: ExtractRequest) -> Result<ExtractResponse> {
    if !request.schema.is_object() {
        return Err(Error::input("extract: schema must be a JSON object (JSON Schema)"));
    }
    let (provider, model_ref) = resolve(&request.document.model, Mode::Extract)?;
    let started = std::time::Instant::now();
    let qualified = model_ref.qualified();
    tracing::info!(mode = "extract", model = %qualified, input = %request.document.input.describe(), "liteocr: starting");
    let result = provider.extract(&request, &model_ref.model).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(mut resp) => {
            resp.model = qualified.clone();
            resp.provider = model_ref.provider.clone();
            resp.latency_ms = latency_ms;
            resp.metadata.extend(request.document.metadata.clone());
            if resp.cost_usd.is_none() {
                resp.cost_usd = resp
                    .usage
                    .provider_cost_usd
                    .or_else(|| pricing::estimate_cost(&qualified, Mode::Extract, resp.usage.pages));
            }
            if !request.document.include_raw {
                resp.raw = None;
            }
            log_done("extract", &qualified, resp.usage.pages, latency_ms, resp.cost_usd);
            Ok(resp)
        }
        Err(e) => Err(log_failed("extract", &qualified, latency_ms, e, &model_ref.provider)),
    }
}

fn resolve(model: &str, mode: Mode) -> Result<(std::sync::Arc<dyn Provider>, ModelRef)> {
    let model_ref = ModelRef::parse_for(model, mode)?;
    let provider = providers::build(&model_ref.provider)?;
    Ok((provider, model_ref))
}

fn log_done(mode: &str, model: &str, pages: u32, latency_ms: u64, cost_usd: Option<f64>) {
    tracing::info!(mode, model, pages, latency_ms, cost_usd = ?cost_usd, "liteocr: done");
}

fn log_failed(mode: &str, model: &str, latency_ms: u64, e: Error, provider: &str) -> Error {
    let e = if e.provider.is_none() { e.with_provider(provider) } else { e };
    tracing::warn!(mode, model, latency_ms, error = %e, "liteocr: failed");
    e
}

/// Blocking wrapper around [`parse`] for non-async callers. Builds a small runtime per call.
pub fn parse_blocking(request: DocumentRequest) -> Result<ParseResponse> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::provider(format!("failed to start runtime: {e}")))?;
    rt.block_on(parse(request))
}
