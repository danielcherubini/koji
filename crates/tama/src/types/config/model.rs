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

/// Convert from CoreModelModalities to mirror type.
impl From<tama_core::config::ModelModalities> for ModelModalities {
    fn from(m: tama_core::config::ModelModalities) -> Self {
        Self {
            input: m.input,
            output: m.output,
        }
    }
}

/// Convert from mirror ModelModalities to core type.
impl From<ModelModalities> for tama_core::config::ModelModalities {
    fn from(m: ModelModalities) -> Self {
        Self {
            input: m.input,
            output: m.output,
        }
    }
}

/// Convert from tama_core::config::ModelConfig to mirror type.
impl From<tama_core::config::ModelConfig> for ModelConfig {
    fn from(m: tama_core::config::ModelConfig) -> Self {
        Self {
            backend: m.backend,
            gpu_variant: m.gpu_variant.map(|v| v.variant_folder().to_string()),
            gpu_device: m.gpu_device,
            args: m.args,
            sampling: m.sampling.map(Into::into),
            model: m.model,
            quant: m.quant,
            mmproj: m.mmproj,
            mtp_model: m.mtp_model,
            port: m.port,
            health_check: m.health_check.map(Into::into),
            enabled: m.enabled,
            context_length: m.context_length,
            num_parallel: m.num_parallel,
            profile: None, // Skip serializing - deprecated field
            api_name: m.api_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants.into_iter().map(|(k, v)| (k, v.into())).collect(),
            modalities: m.modalities.map(Into::into),
            display_name: m.display_name,
            kv_unified: m.kv_unified,
            cache_type_k: m.cache_type_k,
            cache_type_v: m.cache_type_v,
            extra: None, // Forward-compat field - preserve unknown fields on POST
        }
    }
}

/// Convert from mirror ModelConfig to tama_core::config::ModelConfig.
///
/// This conversion is intentionally lossy — the following fields are NOT
/// carried through because they are DB-only metadata populated by the model
/// pull/verify pipeline, not editable through the config:
/// - `hf_*` fields (format, base_model, pipeline_tag, params, etc.)
/// - `db_id` (auto-generated primary key)
/// - `spec_decoding` (managed through the model CRUD endpoints)
///
/// In practice, this conversion path is only used for the structured config
/// save endpoint, which does NOT persist models (models are DB-only).
/// The model CRUD endpoints use `ModelBody` → `apply_model_body()` instead.
impl From<ModelConfig> for tama_core::config::ModelConfig {
    fn from(m: ModelConfig) -> Self {
        use std::str::FromStr;
        Self {
            backend: m.backend,
            gpu_variant: m.gpu_variant.map(|s| {
                tama_core::gpu::GpuType::from_str(&s).unwrap_or_else(|_| {
                    tracing::warn!(
                        "unknown gpu_variant '{}' in model config; treating as custom",
                        s
                    );
                    tama_core::gpu::GpuType::Custom
                })
            }),
            gpu_device: m.gpu_device,
            args: m.args,
            sampling: m.sampling.map(Into::into),
            model: m.model,
            quant: m.quant,
            mmproj: m.mmproj,
            mtp_model: m.mtp_model,
            port: m.port,
            health_check: m.health_check.map(Into::into),
            enabled: m.enabled,
            context_length: m.context_length,
            num_parallel: m.num_parallel,
            profile: None, // Skip serializing - deprecated field
            api_name: m.api_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants.into_iter().map(|(k, v)| (k, v.into())).collect(),
            modalities: m.modalities.map(Into::into),
            display_name: m.display_name,
            kv_unified: m.kv_unified,
            cache_type_k: m.cache_type_k,
            cache_type_v: m.cache_type_v,
            // DB-only metadata — not editable through config, populated by pull/verify pipeline
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            db_id: None,                       // Auto-generated primary key
            spec_decoding: Default::default(), // Managed through model CRUD endpoints
        }
    }
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
