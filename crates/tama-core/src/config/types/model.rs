use crate::db::queries::ModelConfigRecord;
use crate::gpu::GpuVariant;
use crate::profiles::SamplingParams;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

/// What kind of file a quant entry represents.
///
/// Used to distinguish regular GGUF model quants from auxiliary files like
/// vision projectors (mmproj) and MTP draft models. Drives both UI grouping
/// and how the file is passed on the server command line (`-m` vs `--mmproj`
/// vs `--spec-draft-model`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    /// A regular GGUF model quantization (Q4_K_M, Q8_0, F16, etc.).
    #[default]
    Model,
    /// A vision projector (mmproj-*.gguf). Passed via `--mmproj` to llama.cpp.
    Mmproj,
    /// An MTP draft model (mtp-*.gguf). Passed via --spec-draft-model to llama.cpp.
    Mtp,
}

impl QuantKind {
    /// Infer the kind from a filename. Defaults to `Model` for anything that
    /// doesn't match a known auxiliary-file pattern.
    pub fn from_filename(filename: &str) -> Self {
        let lower = filename.to_lowercase();
        if lower.starts_with("mmproj") && lower.ends_with(".gguf") {
            QuantKind::Mmproj
        } else if lower.starts_with("mtp") && lower.ends_with(".gguf") {
            QuantKind::Mtp
        } else {
            QuantKind::Model
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantEntry {
    pub file: String,
    /// What kind of file this is. Defaults to `Model` for backward compat
    /// with config files written before this field existed.
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    /// Health check endpoint URL. Overrides backend's health_check_url.
    #[serde(default)]
    pub url: Option<String>,
    /// Polling interval in milliseconds. Overrides lifecycle.health_check_interval_ms.
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// HTTP timeout in milliseconds per health check request (default: 3000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

// NOTE: Consider composing ModelConfig from BackendConfig, GpuConfig, SamplingConfig,
// SpecDecodingConfig sub-structs. Deferred to future refactor.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_variant: Option<GpuVariant>,
    /// GPU device name for this model (e.g. "ROCm0", "CUDA1").
    /// Passed as `--device` to llama.cpp backends.
    /// When None, the backend uses its default device selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_device: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    /// Model card reference in "company/modelname" format.
    #[serde(default)]
    pub model: Option<String>,
    /// Which quant to use from the model card (e.g. "Q4_K_M").
    #[serde(default)]
    pub quant: Option<String>,
    /// Which mmproj (vision projector) to use, if any. References a key in
    /// `quants` whose entry has `kind = Mmproj`. When set, the launch command
    /// gets `--mmproj <path>` injected automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<String>,
    /// Which MTP draft model to use, if any. References a key in
    /// `quants` whose entry has `kind = Mtp`. When set AND `draft-mtp`
    /// is in `spec_decoding.spec_types`, the launch command gets
    /// `--spec-draft-model <path>` injected automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtp_model: Option<String>,
    /// Custom port for this server (None = backend default)
    #[serde(default)]
    pub port: Option<u16>,
    /// Per-server health check overrides.
    #[serde(default)]
    pub health_check: Option<HealthCheck>,
    #[serde(default = "super::default_enabled")]
    pub enabled: bool,
    /// Context length for this model
    #[serde(default)]
    pub context_length: Option<u32>,
    /// Number of parallel contexts. Multiplies the effective context length.
    /// Default is Some(1). None at runtime is treated as 1.
    #[serde(default = "super::default_num_parallel")]
    pub num_parallel: Option<u32>,
    /// Whether all parallel slots share a single unified KV cache pool.
    /// When true, `-c` equals `context_length` regardless of `num_parallel`.
    /// When false, `-c = context_length * num_parallel` (each slot gets dedicated region).
    /// Default is false for backward compatibility. New models should use true.
    #[serde(default)]
    pub kv_unified: bool,
    /// DEPRECATED — kept for migration deserialization only.
    /// When present in a legacy config, the migration reads this, resolves it to
    /// concrete SamplingParams, writes those into `sampling`, and clears this field.
    /// Must NOT be serialized back (skip_serializing).
    #[serde(default, skip_serializing)]
    pub profile: Option<String>,
    /// API name for model identifier in OpenAI API responses
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_name: Option<String>,
    /// Default GPU layers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    /// KV cache data type for K head (e.g., "f16", "q4_0"). Passed as --cache-type-k.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_k: Option<String>,
    /// KV cache data type for V head (e.g., "f16", "q8_0"). Passed as --cache-type-v.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_v: Option<String>,
    /// HuggingFace model format (e.g., "transformers", "gguf").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_format: Option<String>,
    /// HuggingFace base model id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_base_model: Option<String>,
    /// HuggingFace pipeline tag (e.g., "text-generation").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_pipeline_tag: Option<String>,
    /// HuggingFace total parameters as display string (e.g., "35B").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_total_params: Option<String>,
    /// HuggingFace active parameters for MoE models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_active_params: Option<String>,
    /// HuggingFace architecture type (e.g., "MoE", "Dense").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_architecture_type: Option<String>,
    /// HuggingFace context length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_context_length: Option<u32>,
    /// HuggingFace number of layers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_num_layers: Option<u32>,
    /// HuggingFace last modified timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_last_modified: Option<String>,
    /// Available quantizations
    #[serde(default, skip_serializing_if = "super::is_btreemap_empty")]
    pub quants: BTreeMap<String, QuantEntry>,
    /// Modalities supported by this model (e.g. ["text", "image"] for input, ["text"] for output)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelModalities>,
    /// Pretty display name for UI (e.g., "Unsloth: Gemma 4 26B A4B").
    /// Derived from HF repo name when pulling, but can be overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Integer database id — set at runtime when loading from DB, never
    /// persisted via serde (TOML or JSON). Used by the status endpoint to
    /// expose the canonical integer id for API consumers.
    #[serde(default, skip)]
    pub db_id: Option<i64>,
    /// Speculative decoding configuration.
    #[serde(default)]
    pub spec_decoding: SpecDecodingConfig,
}

impl ModelConfig {
    /// Serialise to a ModelConfigRecord for DB storage.
    /// `repo_id` is the HF repo id (e.g. "unsloth/gemma-4-31B-it-GGUF").
    pub fn to_db_record(&self, repo_id: &str) -> ModelConfigRecord {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        ModelConfigRecord {
            id: 0, // auto-generated on insert
            repo_id: repo_id.to_string(),
            display_name: self.display_name.clone(),
            backend: self.backend.clone(),
            gpu_variant: self
                .gpu_variant
                .as_ref()
                .map(|v| v.variant_folder().to_string()),
            gpu_device: self.gpu_device.clone(),
            enabled: self.enabled,
            selected_quant: self.quant.clone(),
            selected_mmproj: self.mmproj.clone(),
            selected_mtp_model: self.mtp_model.clone(),
            context_length: self.context_length,
            num_parallel: self.num_parallel,
            kv_unified: self.kv_unified,
            gpu_layers: self.gpu_layers,
            cache_type_k: self.cache_type_k.clone(),
            cache_type_v: self.cache_type_v.clone(),
            port: self.port,
            args: serde_json::to_string(&self.args).ok(),
            sampling: self
                .sampling
                .as_ref()
                .and_then(|s| serde_json::to_string(s).ok()),
            modalities: self
                .modalities
                .as_ref()
                .and_then(|s| serde_json::to_string(s).ok()),
            profile: self.profile.clone(),
            api_name: self.api_name.clone(),
            health_check: self
                .health_check
                .as_ref()
                .and_then(|s| serde_json::to_string(s).ok()),
            hf_format: self.hf_format.clone(),
            hf_base_model: self.hf_base_model.clone(),
            hf_pipeline_tag: self.hf_pipeline_tag.clone(),
            hf_total_params: self.hf_total_params.clone(),
            hf_active_params: self.hf_active_params.clone(),
            hf_architecture_type: self.hf_architecture_type.clone(),
            hf_context_length: self.hf_context_length,
            hf_num_layers: self.hf_num_layers,
            hf_last_modified: self.hf_last_modified.clone(),
            spec_decoding: serde_json::to_string(&self.spec_decoding).ok(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Deserialise from a DB record. JSON fields are parsed; parse errors
    /// fall back to None / default so a bad JSON column never hard-fails.
    pub fn from_db_record(record: &ModelConfigRecord) -> Self {
        Self {
            backend: record.backend.clone(),
            gpu_variant: record.gpu_variant.as_deref().map(|s| {
                GpuVariant::from_str(s).unwrap_or_else(|_| {
                    tracing::warn!(
                        "unknown gpu_variant '{}' in model_configs row; treating as custom",
                        s
                    );
                    GpuVariant::Custom
                })
            }),
            gpu_device: record.gpu_device.clone(),
            enabled: record.enabled,
            display_name: record.display_name.clone(),
            api_name: record
                .api_name
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| Some(record.repo_id.clone())),
            port: record.port,
            context_length: record.context_length,
            num_parallel: record.num_parallel,
            kv_unified: record.kv_unified,
            gpu_layers: record.gpu_layers,
            cache_type_k: record.cache_type_k.clone(),
            cache_type_v: record.cache_type_v.clone(),
            model: Some(record.repo_id.clone()),
            quant: record.selected_quant.clone(),
            mmproj: record.selected_mmproj.clone(),
            mtp_model: record.selected_mtp_model.clone().filter(|s| !s.is_empty()),
            args: record
                .args
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default(),
            sampling: record
                .sampling
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            modalities: record
                .modalities
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            health_check: record
                .health_check
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            hf_format: record.hf_format.clone().filter(|s| !s.is_empty()),
            hf_base_model: record.hf_base_model.clone().filter(|s| !s.is_empty()),
            hf_pipeline_tag: record.hf_pipeline_tag.clone().filter(|s| !s.is_empty()),
            hf_total_params: record.hf_total_params.clone().filter(|s| !s.is_empty()),
            hf_active_params: record.hf_active_params.clone().filter(|s| !s.is_empty()),
            hf_architecture_type: record
                .hf_architecture_type
                .clone()
                .filter(|s| !s.is_empty()),
            hf_context_length: record.hf_context_length,
            hf_num_layers: record.hf_num_layers,
            hf_last_modified: record.hf_last_modified.clone().filter(|s| !s.is_empty()),
            profile: record.profile.clone(),
            quants: BTreeMap::new(), // Not stored in DB record
            db_id: Some(record.id),
            spec_decoding: record
                .spec_decoding
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default(),
        }
    }
}

/// Configuration for speculative decoding (draft model support).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpecDecodingConfig {
    /// Enabled spec types (e.g. ["draft-mtp", "ngram-simple"]).
    /// Passed as comma-separated to --spec-type. Empty = disabled.
    #[serde(default)]
    pub spec_types: Vec<String>,
    /// Draft context length (--spec-draft-n-max). Range: 1-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_max: Option<u32>,
    /// Minimum draft tokens (--spec-draft-n-min). Range: 1-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_min: Option<u32>,
    /// Draft model GPU layers (--spec-draft-ngl). MTP-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_ngl: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[cfg(test)]
impl ModelConfig {
    /// Build a minimal `ModelConfig` for tests with the given backend name.
    /// Use `..` syntax to override additional fields as needed.
    pub fn test_config(backend: &str) -> Self {
        Self {
            backend: backend.to_string(),
            ..Default::default()
        }
    }
}
