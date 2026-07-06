//! Shared types for backend management.

use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumIs};

/// Metadata for an installed backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: String,
    pub backend_type: BackendType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(default)]
    pub source: Option<BackendSource>,
}

/// Source of a backend installation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", content = "content")]
pub enum BackendSource {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        /// Optional specific commit hash to check out after cloning.
        /// When set, the clone uses enough depth to reach the commit and
        /// then runs `git checkout <commit>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display, EnumIs)]
#[strum(serialize_all = "snake_case")]
pub enum BackendType {
    LlamaCpp,
    IkLlama,
    TtsKokoro,
    Compaction,
    Custom,
}

impl BackendType {
    pub fn is_tts(&self) -> bool {
        matches!(self, BackendType::TtsKokoro)
    }

    /// Return true for backends that are not LLM inference engines.
    /// Currently covers TTS and compaction backends.
    pub fn is_non_inference_backend(&self) -> bool {
        matches!(self, BackendType::TtsKokoro | BackendType::Compaction)
    }

    /// Return the canonical git URL for cloning this backend's source code.
    pub fn default_git_url(&self) -> &'static str {
        match self {
            BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
            BackendType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
            BackendType::TtsKokoro | BackendType::Compaction | BackendType::Custom => {
                "https://github.com/ggml-org/llama.cpp.git" // fallback, never reached in practice
            }
        }
    }
}

impl FromStr for BackendType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "llama_cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
            "ik_llama" | "ik-llama" | "ikllama" => Ok(BackendType::IkLlama),
            "tts_kokoro" | "ttskokoro" => Ok(BackendType::TtsKokoro),
            "compaction" => Ok(BackendType::Compaction),
            "custom" => Ok(BackendType::Custom),
            _ => Err(format!(
                "Unknown backend type '{}'. Supported: llama_cpp, ik_llama, tts_kokoro, compaction, custom",
                s
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_git_url() {
        assert_eq!(
            BackendType::LlamaCpp.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
        assert_eq!(
            BackendType::IkLlama.default_git_url(),
            "https://github.com/ikawrakow/ik_llama.cpp.git"
        );
        assert_eq!(
            BackendType::TtsKokoro.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
        assert_eq!(
            BackendType::Compaction.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
        assert_eq!(
            BackendType::Custom.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
    }

    #[test]
    fn test_is_non_inference_backend() {
        assert!(BackendType::TtsKokoro.is_non_inference_backend());
        assert!(BackendType::Compaction.is_non_inference_backend());
        assert!(!BackendType::LlamaCpp.is_non_inference_backend());
        assert!(!BackendType::IkLlama.is_non_inference_backend());
        assert!(!BackendType::Custom.is_non_inference_backend());
    }

    // --- Tests for derived Display / EnumIs ---

    #[test]
    fn test_display_all_variants() {
        assert_eq!(BackendType::LlamaCpp.to_string(), "llama_cpp");
        assert_eq!(BackendType::IkLlama.to_string(), "ik_llama");
        assert_eq!(BackendType::TtsKokoro.to_string(), "tts_kokoro");
        assert_eq!(BackendType::Compaction.to_string(), "compaction");
        assert_eq!(BackendType::Custom.to_string(), "custom");
    }

    #[test]
    fn test_enum_is_methods() {
        let llama_cpp = BackendType::LlamaCpp;
        let ik_llama = BackendType::IkLlama;
        let tts_kokoro = BackendType::TtsKokoro;
        let compaction = BackendType::Compaction;
        let custom = BackendType::Custom;

        assert!(llama_cpp.is_llama_cpp());
        assert!(!llama_cpp.is_ik_llama());
        assert!(!llama_cpp.is_tts_kokoro());
        assert!(!llama_cpp.is_compaction());
        assert!(!llama_cpp.is_custom());

        assert!(ik_llama.is_ik_llama());
        assert!(!ik_llama.is_llama_cpp());

        assert!(tts_kokoro.is_tts_kokoro());
        assert!(tts_kokoro.is_tts());

        assert!(compaction.is_compaction());
        assert!(compaction.is_non_inference_backend());

        assert!(custom.is_custom());
    }

    #[test]
    fn test_from_str_still_works_with_aliases() {
        use std::str::FromStr;

        assert_eq!(
            BackendType::from_str("llama_cpp").unwrap(),
            BackendType::LlamaCpp
        );
        assert_eq!(
            BackendType::from_str("llamacpp").unwrap(),
            BackendType::LlamaCpp
        );
        assert_eq!(
            BackendType::from_str("ik_llama").unwrap(),
            BackendType::IkLlama
        );
        assert_eq!(
            BackendType::from_str("ik-llama").unwrap(),
            BackendType::IkLlama
        );
        assert_eq!(
            BackendType::from_str("tts_kokoro").unwrap(),
            BackendType::TtsKokoro
        );
        assert_eq!(
            BackendType::from_str("ttskokoro").unwrap(),
            BackendType::TtsKokoro
        );
        assert_eq!(
            BackendType::from_str("compaction").unwrap(),
            BackendType::Compaction
        );
        assert_eq!(
            BackendType::from_str("custom").unwrap(),
            BackendType::Custom
        );
        assert!(BackendType::from_str("unknown").is_err());
    }

    #[test]
    fn test_display_roundtrip() {
        for variant in [
            BackendType::LlamaCpp,
            BackendType::IkLlama,
            BackendType::TtsKokoro,
            BackendType::Compaction,
            BackendType::Custom,
        ] {
            let name = variant.to_string();
            // Round-trip: Display → from_str → Display should match
            let parsed =
                BackendType::from_str(&name).expect("from_str should parse the display output");
            assert_eq!(
                parsed.to_string(),
                name,
                "round-trip failed for {variant:?}"
            );
        }
    }
}
