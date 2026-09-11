//! Model-string parsing (`"<provider>/<model>"`) and the provider registry.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    pub provider: &'static str,
    pub model: &'static str,
    pub description: &'static str,
    pub default: bool,
}

impl ModelInfo {
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

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
            },
            ModelInfo {
                provider: "reducto",
                model: "r-1",
                description: "Reducto Parse with settings.model=r-1 (newest model, cheaper)",
                default: false,
            },
            ModelInfo {
                provider: "reducto",
                model: "agentic",
                description: "Reducto Parse with agentic text+table enhancement (highest accuracy, 2x cost)",
                default: false,
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
            },
            ModelInfo {
                provider: "extend",
                model: "parse_light",
                description: "Extend engine=parse_light (fast, cheap, digital-native docs)",
                default: false,
            },
            ModelInfo {
                provider: "extend",
                model: "parse_auto",
                description: "Extend engine=parse_auto (picks light or performance per page)",
                default: false,
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
            },
            ModelInfo {
                provider: "llamaparse",
                model: "cost_effective",
                description: "LlamaParse tier=cost_effective",
                default: true,
            },
            ModelInfo {
                provider: "llamaparse",
                model: "agentic",
                description: "LlamaParse tier=agentic",
                default: false,
            },
            ModelInfo {
                provider: "llamaparse",
                model: "agentic_plus",
                description: "LlamaParse tier=agentic_plus (highest accuracy)",
                default: false,
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
    /// Parse `"provider/model"` or `"provider"` (→ default model). Aliases: `llama`, `llama_parse`,
    /// `llamacloud` → `llamaparse`.
    pub fn parse(s: &str) -> Result<Self> {
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
                if !info.models.iter().any(|mi| mi.model == m) {
                    return Err(Error::unsupported_model(format!(
                        "unknown model '{m}' for provider '{prov}'. Known: {}",
                        info.models.iter().map(|mi| mi.model).collect::<Vec<_>>().join(", ")
                    )));
                }
                m
            }
            None => info
                .models
                .iter()
                .find(|m| m.default)
                .or_else(|| info.models.first())
                .map(|m| m.model.to_string())
                .expect("every provider has at least one model"),
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
    fn lists_models() {
        let m = list_models();
        assert!(m.contains(&"extend/parse_performance".to_string()));
        assert_eq!(m.len(), 10);
    }
}
