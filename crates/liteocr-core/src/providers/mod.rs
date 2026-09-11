//! Provider implementations.

pub mod extend;
pub mod llamaparse;
pub mod reducto;
// Providers below are being implemented in parallel; each file is owned by one contributor.
pub mod anthropic;
pub mod azure;
pub mod datalab;
pub mod gemini;
pub mod google_documentai;
pub mod landingai;
pub mod mathpix;
pub mod mistral;
pub mod openai;
pub mod textract;
pub mod unstructured;
pub mod upstage;

use crate::error::{Error, Result};
use crate::provider::Provider;
use std::sync::Arc;

/// Instantiate a provider by name.
pub fn build(name: &str) -> Result<Arc<dyn Provider>> {
    match name {
        "reducto" => Ok(Arc::new(reducto::Reducto)),
        "extend" => Ok(Arc::new(extend::Extend)),
        "llamaparse" => Ok(Arc::new(llamaparse::LlamaParse)),
        "mistral" => Ok(Arc::new(mistral::Mistral)),
        "azure" => Ok(Arc::new(azure::Azure)),
        other => Err(Error::unsupported_model(format!("unknown provider '{other}'"))),
    }
}
