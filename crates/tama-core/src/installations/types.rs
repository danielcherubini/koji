//! Shared types for backend management.

use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumIs};

use super::docker::DockerConfig;

/// Metadata for an installed backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallationInfo {
    pub name: String,
    pub backend_type: InstallationType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_variant: String,
    #[serde(default)]
    pub source: Option<InstallationSource>,
    /// Docker configuration for Docker-based backends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_config: Option<DockerConfig>,
}

/// Source of a backend installation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", content = "content")]
pub enum InstallationSource {
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
pub enum InstallationType {
    LlamaCpp,
    IkLlama,
    TtsKokoro,
    Compaction,
    Custom,
    Docker,
}

impl InstallationType {
    pub fn is_tts(&self) -> bool {
        matches!(self, InstallationType::TtsKokoro)
    }

    /// Return true for backends that are not LLM inference engines.
    /// Currently covers TTS and compaction backends.
    pub fn is_non_inference_backend(&self) -> bool {
        matches!(
            self,
            InstallationType::TtsKokoro | InstallationType::Compaction
        )
    }

    /// Return the canonical git URL for cloning this backend's source code.
    pub fn default_git_url(&self) -> &'static str {
        match self {
            InstallationType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
            InstallationType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
            InstallationType::TtsKokoro
            | InstallationType::Compaction
            | InstallationType::Custom
            | InstallationType::Docker => {
                "https://github.com/ggml-org/llama.cpp.git" // fallback, never reached in practice
            }
        }
    }
}

impl FromStr for InstallationType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "llama_cpp" | "llamacpp" => Ok(InstallationType::LlamaCpp),
            "ik_llama" | "ik-llama" | "ikllama" => Ok(InstallationType::IkLlama),
            "tts_kokoro" | "ttskokoro" => Ok(InstallationType::TtsKokoro),
            "compaction" => Ok(InstallationType::Compaction),
            "custom" => Ok(InstallationType::Custom),
            "docker" => Ok(InstallationType::Docker),
            _ => Err(format!(
                "Unknown backend type '{}'. Supported: llama_cpp, ik_llama, tts_kokoro, compaction, custom, docker",
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
            InstallationType::LlamaCpp.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
        assert_eq!(
            InstallationType::IkLlama.default_git_url(),
            "https://github.com/ikawrakow/ik_llama.cpp.git"
        );
        assert_eq!(
            InstallationType::TtsKokoro.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
        assert_eq!(
            InstallationType::Compaction.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
        assert_eq!(
            InstallationType::Custom.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git"
        );
        assert_eq!(
            InstallationType::Docker.default_git_url(),
            "https://github.com/ggml-org/llama.cpp.git" // fallback, never reached
        );
    }

    #[test]
    fn test_is_non_inference_backend() {
        assert!(InstallationType::TtsKokoro.is_non_inference_backend());
        assert!(InstallationType::Compaction.is_non_inference_backend());
        assert!(!InstallationType::LlamaCpp.is_non_inference_backend());
        assert!(!InstallationType::IkLlama.is_non_inference_backend());
        assert!(!InstallationType::Custom.is_non_inference_backend());
        assert!(!InstallationType::Docker.is_non_inference_backend());
    }

    // --- Tests for derived Display / EnumIs ---

    #[test]
    fn test_display_all_variants() {
        assert_eq!(InstallationType::LlamaCpp.to_string(), "llama_cpp");
        assert_eq!(InstallationType::IkLlama.to_string(), "ik_llama");
        assert_eq!(InstallationType::TtsKokoro.to_string(), "tts_kokoro");
        assert_eq!(InstallationType::Compaction.to_string(), "compaction");
        assert_eq!(InstallationType::Custom.to_string(), "custom");
        assert_eq!(InstallationType::Docker.to_string(), "docker");
    }

    #[test]
    fn test_enum_is_methods() {
        let llama_cpp = InstallationType::LlamaCpp;
        let ik_llama = InstallationType::IkLlama;
        let tts_kokoro = InstallationType::TtsKokoro;
        let compaction = InstallationType::Compaction;
        let custom = InstallationType::Custom;
        let docker = InstallationType::Docker;

        assert!(llama_cpp.is_llama_cpp());
        assert!(!llama_cpp.is_ik_llama());
        assert!(!llama_cpp.is_tts_kokoro());
        assert!(!llama_cpp.is_compaction());
        assert!(!llama_cpp.is_custom());
        assert!(!llama_cpp.is_docker());

        assert!(ik_llama.is_ik_llama());
        assert!(!ik_llama.is_llama_cpp());

        assert!(tts_kokoro.is_tts_kokoro());
        assert!(tts_kokoro.is_tts());

        assert!(compaction.is_compaction());
        assert!(compaction.is_non_inference_backend());

        assert!(custom.is_custom());

        assert!(docker.is_docker());
        assert!(!docker.is_llama_cpp());
        assert!(!docker.is_ik_llama());
    }

    #[test]
    fn test_from_str_still_works_with_aliases() {
        use std::str::FromStr;

        assert_eq!(
            InstallationType::from_str("llama_cpp").unwrap(),
            InstallationType::LlamaCpp
        );
        assert_eq!(
            InstallationType::from_str("llamacpp").unwrap(),
            InstallationType::LlamaCpp
        );
        assert_eq!(
            InstallationType::from_str("ik_llama").unwrap(),
            InstallationType::IkLlama
        );
        assert_eq!(
            InstallationType::from_str("ik-llama").unwrap(),
            InstallationType::IkLlama
        );
        assert_eq!(
            InstallationType::from_str("tts_kokoro").unwrap(),
            InstallationType::TtsKokoro
        );
        assert_eq!(
            InstallationType::from_str("ttskokoro").unwrap(),
            InstallationType::TtsKokoro
        );
        assert_eq!(
            InstallationType::from_str("compaction").unwrap(),
            InstallationType::Compaction
        );
        assert_eq!(
            InstallationType::from_str("custom").unwrap(),
            InstallationType::Custom
        );
        assert_eq!(
            InstallationType::from_str("docker").unwrap(),
            InstallationType::Docker
        );
        assert!(InstallationType::from_str("unknown").is_err());
    }

    #[test]
    fn test_display_roundtrip() {
        for variant in [
            InstallationType::LlamaCpp,
            InstallationType::IkLlama,
            InstallationType::TtsKokoro,
            InstallationType::Compaction,
            InstallationType::Custom,
            InstallationType::Docker,
        ] {
            let name = variant.to_string();
            // Round-trip: Display → from_str → Display should match
            let parsed = InstallationType::from_str(&name)
                .expect("from_str should parse the display output");
            assert_eq!(
                parsed.to_string(),
                name,
                "round-trip failed for {variant:?}"
            );
        }
    }
}
