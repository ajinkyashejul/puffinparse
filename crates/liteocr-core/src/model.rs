//! Model-string parsing (`"<provider>/<model>"`) and the provider registry.

use crate::error::{Error, Result};
use crate::types::Mode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    pub provider: &'static str,
    pub model: &'static str,
    pub description: &'static str,
    /// Default model for its provider **within each mode it supports**.
    pub default: bool,
    /// Modes this model can serve. Providers can only be swapped within a mode.
    pub modes: &'static [Mode],
}

impl ModelInfo {
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }

    pub fn supports(&self, mode: Mode) -> bool {
        self.modes.contains(&mode)
    }
}

/// Shorthand for the common "layout parse, and plain text derived from it" pair.
pub const PARSE_OCR: &[Mode] = &[Mode::Parse, Mode::Ocr];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderInfo {
    pub name: &'static str,
    pub display_name: &'static str,
    pub env_var: &'static str,
    pub base_url: &'static str,
    pub docs: &'static str,
    pub models: &'static [ModelInfo],
}

pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        name: "reducto",
        display_name: "Reducto",
        env_var: "REDUCTO_API_KEY",
        base_url: "https://platform.reducto.ai",
        docs: "https://docs.reducto.ai",
        models: &[
            ModelInfo {
                provider: "reducto",
                model: "standard",
                description: "Reducto Parse with account-default model (legacy standard)",
                default: true,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "reducto",
                model: "r-1",
                description: "Reducto Parse with settings.model=r-1 (newest model, cheaper)",
                default: false,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "reducto",
                model: "agentic",
                description: "Reducto Parse with agentic text+table enhancement (highest accuracy, 2x cost)",
                default: false,
                modes: PARSE_OCR,
            },
        ],
    },
    ProviderInfo {
        name: "extend",
        display_name: "Extend",
        env_var: "EXTEND_API_KEY",
        base_url: "https://api.extend.ai",
        docs: "https://docs.extend.ai",
        models: &[
            ModelInfo {
                provider: "extend",
                model: "parse_performance",
                description: "Extend engine=parse_performance (highest accuracy)",
                default: true,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "extend",
                model: "parse_light",
                description: "Extend engine=parse_light (fast, cheap, digital-native docs)",
                default: false,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "extend",
                model: "parse_auto",
                description: "Extend engine=parse_auto (picks light or performance per page)",
                default: false,
                modes: PARSE_OCR,
            },
        ],
    },
    ProviderInfo {
        name: "llamaparse",
        display_name: "LlamaParse (LlamaCloud)",
        env_var: "LLAMA_API_KEY",
        base_url: "https://api.cloud.llamaindex.ai",
        docs: "https://docs.cloud.llamaindex.ai/llamaparse",
        models: &[
            ModelInfo {
                provider: "llamaparse",
                model: "fast",
                description: "LlamaParse tier=fast (text extraction, no OCR of images)",
                default: false,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "llamaparse",
                model: "cost_effective",
                description: "LlamaParse tier=cost_effective",
                default: true,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "llamaparse",
                model: "agentic",
                description: "LlamaParse tier=agentic",
                default: false,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "llamaparse",
                model: "agentic_plus",
                description: "LlamaParse tier=agentic_plus (highest accuracy)",
                default: false,
                modes: PARSE_OCR,
            },
        ],
    },
    ProviderInfo {
        name: "mistral",
        display_name: "Mistral Document AI",
        env_var: "MISTRAL_API_KEY",
        base_url: "https://api.mistral.ai",
        docs: "https://docs.mistral.ai/capabilities/OCR/basic_ocr",
        models: &[
            ModelInfo {
                provider: "mistral",
                model: "ocr-latest",
                description: "Mistral OCR, latest alias (mistral-ocr-latest; currently OCR 4.1)",
                default: true,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "mistral",
                model: "ocr-4-1",
                description: "Mistral OCR 4.1 pinned (mistral-ocr-4-1; blocks + block confidence scores)",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "mistral",
                model: "ocr-4-0",
                description: "Mistral OCR 4.0 pinned (mistral-ocr-4-0; paragraph blocks, no block confidence)",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "mistral",
                model: "ocr-2512",
                description: "Mistral OCR 3 pinned (mistral-ocr-2512; cheaper, no paragraph blocks)",
                default: false,
                modes: Mode::ALL,
            },
        ],
    },
    ProviderInfo {
        name: "azure",
        display_name: "Azure AI Document Intelligence",
        env_var: "AZURE_DOCUMENT_INTELLIGENCE_KEY",
        // Display only: Azure endpoints are per resource. The provider reads the real endpoint from
        // AZURE_DOCUMENT_INTELLIGENCE_ENDPOINT, or `base_url` on the request, and errors without one.
        base_url: "https://<resource>.cognitiveservices.azure.com",
        docs: "https://learn.microsoft.com/azure/ai-services/document-intelligence/",
        models: &[
            ModelInfo {
                provider: "azure",
                model: "read",
                description: "Azure prebuilt-read: native OCR, words/lines with confidence (cheapest)",
                default: true,
                modes: &[Mode::Ocr],
            },
            ModelInfo {
                provider: "azure",
                model: "layout",
                description: "Azure prebuilt-layout: markdown, paragraphs with roles, tables, polygons",
                default: true,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "azure",
                model: "invoice",
                description: "Azure prebuilt-invoice: fixed invoice schema",
                default: true,
                modes: &[Mode::Extract],
            },
            ModelInfo {
                provider: "azure",
                model: "receipt",
                description: "Azure prebuilt-receipt: fixed receipt schema",
                default: false,
                modes: &[Mode::Extract],
            },
            ModelInfo {
                provider: "azure",
                model: "id_document",
                description: "Azure prebuilt-idDocument: fixed ID document schema",
                default: false,
                modes: &[Mode::Extract],
            },
            ModelInfo {
                provider: "azure",
                model: "tax_us_w2",
                description: "Azure prebuilt-tax.us.w2: fixed W-2 schema",
                default: false,
                modes: &[Mode::Extract],
            },
            ModelInfo {
                provider: "azure",
                model: "custom",
                description: "Azure custom model, id via provider_options.model_id",
                default: false,
                modes: Mode::ALL,
            },
        ],
    },
    ProviderInfo {
        name: "textract",
        display_name: "AWS Textract",
        // Also reads AWS_SECRET_ACCESS_KEY (required), AWS_SESSION_TOKEN (optional),
        // AWS_REGION / AWS_DEFAULT_REGION (default us-east-1). `api_key` overrides the key id only.
        env_var: "AWS_ACCESS_KEY_ID",
        base_url: "https://textract.us-east-1.amazonaws.com",
        docs: "https://docs.aws.amazon.com/textract/latest/dg/what-is.html",
        models: &[
            ModelInfo {
                provider: "textract",
                model: "detect-text",
                description: "Textract DetectDocumentText: raw OCR, lines + words with boxes (cheapest)",
                default: true,
                modes: &[Mode::Ocr],
            },
            ModelInfo {
                provider: "textract",
                model: "layout",
                description: "Textract AnalyzeDocument LAYOUT + TABLES: reading-order markdown + tables",
                default: false,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "textract",
                model: "queries",
                description: "Textract AnalyzeDocument QUERIES: one natural-language query per schema field",
                default: true,
                modes: &[Mode::Extract],
            },
            ModelInfo {
                provider: "textract",
                model: "forms",
                description: "Textract AnalyzeDocument FORMS: key-value pairs matched to schema fields",
                default: false,
                modes: &[Mode::Extract],
            },
        ],
    },
    ProviderInfo {
        name: "gemini",
        display_name: "Google Gemini",
        env_var: "GEMINI_API_KEY",
        base_url: "https://generativelanguage.googleapis.com",
        docs: "https://ai.google.dev/gemini-api/docs",
        models: &[
            ModelInfo {
                provider: "gemini",
                model: "2.5-flash",
                description: "Gemini 2.5 Flash: vision-LLM transcription, the price/quality default",
                default: true,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "gemini",
                model: "2.5-pro",
                description: "Gemini 2.5 Pro: highest accuracy, ~4x the cost of Flash",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "gemini",
                model: "2.5-flash-lite",
                description: "Gemini 2.5 Flash-Lite: cheapest and fastest, clean documents",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "gemini",
                model: "3.5-flash",
                description: "Gemini 3.5 Flash: frontier Flash generation (GA 2026-05-19)",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "gemini",
                model: "3.5-flash-lite",
                description: "Gemini 3.5 Flash-Lite: low-latency 3.x tier (GA 2026-07-21)",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "gemini",
                model: "3.8-flash",
                description: "Gemini 3.8 Flash: newest Flash model (GA 2026-09-02, introductory pricing)",
                default: false,
                modes: Mode::ALL,
            },
        ],
    },
    ProviderInfo {
        name: "openai",
        display_name: "OpenAI",
        env_var: "OPENAI_API_KEY",
        base_url: "https://api.openai.com",
        docs: "https://developers.openai.com/api/docs",
        models: &[
            ModelInfo {
                provider: "openai",
                model: "gpt-5.6-luna",
                description: "OpenAI Responses API, gpt-5.6-luna (cheapest current vision model)",
                default: true,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "openai",
                model: "gpt-5.6-terra",
                description: "OpenAI Responses API, gpt-5.6-terra (balanced capability/price)",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "openai",
                model: "gpt-5.6-sol",
                description: "OpenAI Responses API, gpt-5.6-sol (flagship GPT-5.6)",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "openai",
                model: "gpt-6-astra",
                description: "OpenAI Responses API, gpt-6-astra (most capable, most expensive)",
                default: false,
                modes: Mode::ALL,
            },
        ],
    },
    ProviderInfo {
        name: "anthropic",
        display_name: "Anthropic (Claude)",
        env_var: "ANTHROPIC_API_KEY",
        base_url: "https://api.anthropic.com",
        docs: "https://platform.claude.com/docs",
        models: &[
            ModelInfo {
                provider: "anthropic",
                model: "claude-sonnet-5",
                description: "Claude Messages API, claude-sonnet-5 (balanced vision transcription)",
                default: true,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "anthropic",
                model: "claude-haiku-4-5",
                description: "Claude Messages API, claude-haiku-4-5 (cheapest, 200K context)",
                default: false,
                modes: Mode::ALL,
            },
            ModelInfo {
                provider: "anthropic",
                model: "claude-opus-5",
                description: "Claude Messages API, claude-opus-5 (highest accuracy)",
                default: false,
                modes: Mode::ALL,
            },
        ],
    },
    ProviderInfo {
        name: "mathpix",
        display_name: "Mathpix",
        // Also requires MATHPIX_APP_ID (read by the provider, or provider_options.app_id).
        env_var: "MATHPIX_APP_KEY",
        base_url: "https://api.mathpix.com",
        docs: "https://docs.mathpix.com",
        models: &[
            ModelInfo {
                provider: "mathpix",
                model: "pdf",
                description: "Mathpix v3/pdf document OCR (image inputs auto-routed to v3/text); MMD + line polygons",
                default: true,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "mathpix",
                model: "text",
                description: "Mathpix v3/text single-image OCR (line + word polygons, per-image billing)",
                default: false,
                modes: PARSE_OCR,
            },
        ],
    },
    ProviderInfo {
        name: "datalab",
        display_name: "Datalab (Marker)",
        env_var: "DATALAB_API_KEY",
        base_url: "https://www.datalab.to",
        docs: "https://documentation.datalab.to",
        models: &[
            ModelInfo {
                provider: "datalab",
                model: "fast",
                description: "Datalab Convert mode=fast (lowest latency, digital-native documents)",
                default: false,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "datalab",
                model: "balanced",
                description: "Datalab Convert mode=balanced (Datalab's recommended default)",
                default: true,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "datalab",
                model: "accurate",
                description: "Datalab Convert mode=accurate (scans, dense layouts, complex tables)",
                default: false,
                modes: PARSE_OCR,
            },
        ],
    },
    ProviderInfo {
        name: "unstructured",
        display_name: "Unstructured",
        env_var: "UNSTRUCTURED_API_KEY",
        base_url: "https://api.unstructuredapp.io",
        docs: "https://docs.unstructured.io/api-reference/partition/overview",
        models: &[
            ModelInfo {
                provider: "unstructured",
                model: "hi_res",
                description: "Unstructured strategy=hi_res (layout model + OCR; coordinates, table HTML, confidence)",
                default: true,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "unstructured",
                model: "fast",
                description: "Unstructured strategy=fast (text-layer extraction, no OCR, rejects images)",
                default: false,
                modes: PARSE_OCR,
            },
            ModelInfo {
                provider: "unstructured",
                model: "auto",
                description: "Unstructured strategy=auto (routes each page to fast / hi_res / VLM)",
                default: false,
                modes: PARSE_OCR,
            },
        ],
    },
];

/// A parsed, validated model reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    /// Parse `"provider/model"` or `"provider"` (→ default model) without checking modes.
    /// Aliases: `llama`, `llama_parse`, `llamacloud` → `llamaparse`.
    pub fn parse(s: &str) -> Result<Self> {
        Self::resolve(s, None)
    }

    /// Parse and require that the model supports `mode`. A bare provider name resolves to the
    /// provider's default model **for that mode**.
    pub fn parse_for(s: &str, mode: Mode) -> Result<Self> {
        Self::resolve(s, Some(mode))
    }

    fn resolve(s: &str, mode: Option<Mode>) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::unsupported_model("model must not be empty"));
        }
        let (prov, model) = match s.split_once('/') {
            Some((p, m)) => (p.trim().to_ascii_lowercase(), Some(m.trim().to_ascii_lowercase())),
            None => (s.to_ascii_lowercase(), None),
        };
        let prov = match prov.as_str() {
            "llama" | "llama_parse" | "llama-parse" | "llamacloud" | "llama_cloud" => "llamaparse".to_string(),
            "gpt" | "oai" => "openai".to_string(),
            "claude" => "anthropic".to_string(),
            other => other.to_string(),
        };
        let info = provider_info(&prov).ok_or_else(|| {
            Error::unsupported_model(format!(
                "unknown provider '{prov}'. Known providers: {}",
                PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
            ))
        })?;
        let model = match model {
            Some(m) => {
                let Some(mi) = info.models.iter().find(|mi| mi.model == m) else {
                    return Err(Error::unsupported_model(format!(
                        "unknown model '{m}' for provider '{prov}'. Known: {}",
                        info.models.iter().map(|mi| mi.model).collect::<Vec<_>>().join(", ")
                    )));
                };
                if let Some(mode) = mode {
                    if !mi.supports(mode) {
                        return Err(Error::unsupported_model(format!(
                            "model '{prov}/{m}' does not support mode '{mode}' (supports: {}). Models for '{mode}' from {prov}: {}",
                            mi.modes.iter().map(Mode::as_str).collect::<Vec<_>>().join(", "),
                            info.models.iter().filter(|x| x.supports(mode)).map(|x| x.model).collect::<Vec<_>>().join(", ")
                        )));
                    }
                }
                m
            }
            None => {
                let candidates = info.models.iter().filter(|m| mode.map(|md| m.supports(md)).unwrap_or(true));
                let mut candidates: Vec<&ModelInfo> = candidates.collect();
                if candidates.is_empty() {
                    return Err(Error::unsupported_model(format!(
                        "provider '{prov}' has no model for mode '{}'",
                        mode.map(|m| m.as_str()).unwrap_or("any")
                    )));
                }
                candidates.sort_by_key(|m| !m.default);
                candidates[0].model.to_string()
            }
        };
        Ok(Self { provider: prov, model })
    }

    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

pub fn provider_info(name: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS.iter().find(|p| p.name == name)
}

/// All fully-qualified model names.
pub fn list_models() -> Vec<String> {
    PROVIDERS.iter().flat_map(|p| p.models.iter().map(|m| m.qualified())).collect()
}

/// Fully-qualified model names that support `mode`.
pub fn list_models_for(mode: Mode) -> Vec<String> {
    PROVIDERS.iter().flat_map(|p| p.models.iter().filter(|m| m.supports(mode)).map(|m| m.qualified())).collect()
}

/// Look up a model's registry entry.
pub fn model_info(provider: &str, model: &str) -> Option<&'static ModelInfo> {
    provider_info(provider)?.models.iter().find(|m| m.model == model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_aliases() {
        assert_eq!(ModelRef::parse("reducto").unwrap().qualified(), "reducto/standard");
        assert_eq!(ModelRef::parse("Reducto/Agentic").unwrap().qualified(), "reducto/agentic");
        assert_eq!(ModelRef::parse("reducto/R-1").unwrap().qualified(), "reducto/r-1");
        assert_eq!(ModelRef::parse("llama").unwrap().qualified(), "llamaparse/cost_effective");
        assert_eq!(ModelRef::parse("llama_parse/fast").unwrap().qualified(), "llamaparse/fast");
        assert!(ModelRef::parse("extend/bogus").is_err());
        assert!(ModelRef::parse("nope").is_err());
        assert!(ModelRef::parse("").is_err());
    }

    #[test]
    fn mode_aware_resolution() {
        assert_eq!(ModelRef::parse_for("reducto", Mode::Ocr).unwrap().qualified(), "reducto/standard");
        let e = ModelRef::parse_for("reducto/standard", Mode::Extract).unwrap_err();
        assert!(e.to_string().contains("does not support mode 'extract'"), "{e}");
        assert!(ModelRef::parse_for("extend", Mode::Extract).is_err());
        assert!(list_models_for(Mode::Parse).len() < list_models().len());
        assert!(!list_models_for(Mode::Parse).contains(&"azure/read".to_string()));
        assert_eq!(ModelRef::parse_for("azure", Mode::Ocr).unwrap().qualified(), "azure/read");
        assert_eq!(ModelRef::parse_for("azure", Mode::Parse).unwrap().qualified(), "azure/layout");
        assert_eq!(ModelRef::parse_for("azure", Mode::Extract).unwrap().qualified(), "azure/invoice");
        assert!(list_models_for(Mode::Extract).iter().all(|m| model_info(
            m.split('/').next().unwrap(),
            m.split('/').nth(1).unwrap()
        )
        .unwrap()
        .supports(Mode::Extract)));
        assert!(list_models_for(Mode::Extract).contains(&"mistral/ocr-latest".to_string()));
    }

    #[test]
    fn lists_models() {
        let m = list_models();
        assert!(m.contains(&"extend/parse_performance".to_string()));
        assert_eq!(m.len(), 46);
    }
}
