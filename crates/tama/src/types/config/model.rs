//! Model configuration (WASM mirror).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Model configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    pub backend: String,
    #[serde(default)]
    pub gpu_variant: Option<String>,
    #[serde(default)]
    pub gpu_device: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<super::SamplingParams>,
    /// Model card reference in "company/modelname" format.
    #[serde(default)]
    pub model: Option<String>,
    /// Which quant to use from the model card (e.g. "Q4_K_M").
    #[serde(default)]
    pub quant: Option<String>,
    /// Which mmproj (vision projector) to use, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<String>,
    /// Which MTP draft model to use, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtp_model: Option<String>,
    /// Custom port for this server (None = backend default)
    #[serde(default)]
    pub port: Option<u16>,
    /// Per-server health check overrides.
    #[serde(default)]
    pub health_check: Option<super::HealthCheck>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Context length for this model
    #[serde(default)]
    pub context_length: Option<u32>,
    /// Number of parallel slots for this model
    #[serde(default)]
    pub num_parallel: Option<u32>,
    /// DEPRECATED — kept for migration deserialization only.
    #[serde(default, skip_serializing)]
    pub profile: Option<String>,
    /// API name for model identifier in OpenAI API responses
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_name: Option<String>,
    /// Default GPU layers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    /// Available quantizations
    #[serde(default, skip_serializing_if = "is_btreemap_empty")]
    pub quants: BTreeMap<String, super::QuantEntry>,
    /// Model modalities (input/output types)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelModalities>,
    /// Pretty display name for UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Whether all parallel slots share a single unified KV cache pool.
    #[serde(default)]
    pub kv_unified: bool,
    /// KV cache quantization for keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_k: Option<String>,
    /// KV cache quantization for values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_v: Option<String>,
    /// Pre-allocated context KV cache size (llama.cpp --batch). None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_batch: Option<u32>,
    /// Maximum number of unique sequences to process in a single batch
    /// (llama.cpp --ubatch). None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_ubatch: Option<u32>,
    /// Forward-compatibility: preserve unknown fields
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Model modality configuration (input/output types like "text", "image").
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn is_btreemap_empty<K, V>(map: &BTreeMap<K, V>) -> bool {
    map.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ModelModalities serialization tests ───────────────────────────────

    #[test]
    fn test_model_modalities_serialization() {
        let modalities = ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["text".to_string()],
        };

        let json = serde_json::to_string(&modalities).unwrap();
        let deserialized: ModelModalities = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.input, vec!["text".to_string()]);
        assert_eq!(deserialized.output, vec!["text".to_string()]);
    }

    #[test]
    fn test_model_modalities_empty() {
        let modalities = ModelModalities {
            input: vec![],
            output: vec![],
        };

        let json = serde_json::to_string(&modalities).unwrap();
        let deserialized: ModelModalities = serde_json::from_str(&json).unwrap();

        assert!(deserialized.input.is_empty());
        assert!(deserialized.output.is_empty());
    }

    // ── is_btreemap_empty tests ───────────────────────────────────────────

    #[test]
    fn test_is_btreemap_empty_true() {
        let map: BTreeMap<String, String> = BTreeMap::new();
        assert!(is_btreemap_empty(&map));
    }

    #[test]
    fn test_is_btreemap_empty_false() {
        let mut map = BTreeMap::new();
        map.insert("key".to_string(), "value".to_string());
        assert!(!is_btreemap_empty(&map));
    }
}
