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
// Shared helpers for the vision-LLM providers (gemini, openai, anthropic).
pub(crate) mod vlm;

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
        "textract" => Ok(Arc::new(textract::Textract)),
        "gemini" => Ok(Arc::new(gemini::Gemini)),
        "openai" => Ok(Arc::new(openai::OpenAi)),
        "anthropic" => Ok(Arc::new(anthropic::Anthropic)),
        "mathpix" => Ok(Arc::new(mathpix::Mathpix)),
        "datalab" => Ok(Arc::new(datalab::Datalab)),
        "unstructured" => Ok(Arc::new(unstructured::Unstructured)),
        "upstage" => Ok(Arc::new(upstage::Upstage)),
        "landingai" => Ok(Arc::new(landingai::LandingAi)),
        "google_documentai" => Ok(Arc::new(google_documentai::GoogleDocumentAi)),
        other => Err(Error::unsupported_model(format!("unknown provider '{other}'"))),
    }
}
