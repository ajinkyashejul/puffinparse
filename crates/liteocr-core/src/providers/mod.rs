//! Provider implementations.

pub mod extend;
pub mod llamaparse;
pub mod reducto;

use crate::error::{Error, Result};
use crate::provider::OcrProvider;
use std::sync::Arc;

/// Instantiate a provider by name.
pub fn build(name: &str) -> Result<Arc<dyn OcrProvider>> {
    match name {
        "reducto" => Ok(Arc::new(reducto::Reducto)),
        "extend" => Ok(Arc::new(extend::Extend)),
        "llamaparse" => Ok(Arc::new(llamaparse::LlamaParse)),
        other => Err(Error::unsupported_model(format!("unknown provider '{other}'"))),
    }
}
