pub use crate::types::quant::{QuantEntry, QuantKind};

use crate::db::queries::ModelConfigRecord;
use crate::gpu::GpuVariant;
use crate::profiles::SamplingParams;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

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
    /// Pre-allocated context KV cache size (llama.cpp --batch). None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_batch: Option<u32>,
    /// Maximum number of unique sequences to process in a single batch
    /// (llama.cpp --ubatch). None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_ubatch: Option<u32>,
}

/// Normalize legacy `-b`/`--batch-size` and `-ub`/`--ubatch-size` args into
/// explicit `n_batch` / `n_ubatch` fields.
///
/// Rules:
/// - Explicit column values (when `Some`) always take priority over any flag found in args.
/// - Only extracts a value from args when the corresponding column is `None`.
/// - Legacy `-b`/`--batch-size` and `-ub`/`--ubatch-size` flags are always removed
///   from args once processed, regardless of whether a column value was set.
/// - Supports both `--flag value` (two elements) and `--flag=value` (one element).
///   Also supports short forms `-b` and `-ub`.
///   Also supports long forms like `--batch-size` and `--ubatch-size`.
/// - Unparseable values are left in args.
///
pub fn normalize_legacy_args(
    mut args: Vec<String>,
    col_n_batch: Option<u32>,
    col_n_ubatch: Option<u32>,
) -> (Option<u32>, Option<u32>, Vec<String>) {
    let mut n_batch = col_n_batch;
    let mut n_ubatch = col_n_ubatch;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some((flag_kind, size, consumed)) = try_parse_flag(arg, &args, i) {
            match (flag_kind, size) {
                ("batch", v) if n_batch.is_none() => {
                    n_batch = Some(v);
                }
                ("ubatch", v) if n_ubatch.is_none() => {
                    n_ubatch = Some(v);
                }
                _ => {}
            }
            // Remove consumed elements (from end to preserve indices)
            for _ in 0..consumed {
                args.remove(i);
            }
        } else {
            i += 1;
        }
    }

    (n_batch, n_ubatch, args)
}

/// Try to parse a flag at position `i` in `args`.
/// Returns `(flag_kind, value_as_u32, elements_consumed)` on success,
/// where `flag_kind` is normalized to "batch" or "ubatch".
fn try_parse_flag<'a>(arg: &'a str, args: &'a [String], i: usize) -> Option<(&'a str, u32, usize)> {
    // Check for --flag=value form
    if let Some((flag, value)) = arg.split_once('=') {
        let flag_stripped = flag.strip_prefix("--").or_else(|| flag.strip_prefix('-'))?;
        let kind = normalize_flag_name(flag_stripped);
        if let Some(kind) = kind {
            if let Ok(size) = value.parse::<u32>() {
                return Some((kind, size, 1));
            }
        }
        return None;
    }

    // Check for --flag value (next element) or -b / -ub
    let flag_stripped = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-'))?;
    if let Some(kind) = normalize_flag_name(flag_stripped) {
        // Need a next element for the value
        let value = args.get(i + 1)?;
        if let Ok(size) = value.parse::<u32>() {
            return Some((kind, size, 2));
        }
    }
    None // unparseable — leave in args
}

/// Normalize a flag name to "batch" or "ubatch".
/// Handles: `b` → `batch`, `ub` → `ubatch`, `batch-size` → `batch`,
/// `ubatch-size` → `ubatch`, and exact matches like `batch`, `ubatch`.
fn normalize_flag_name(name: &str) -> Option<&'static str> {
    match name {
        "b" | "batch" | "batch-size" => Some("batch"),
        "ub" | "ubatch" | "ubatch-size" => Some("ubatch"),
        _ => None,
    }
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
            n_batch: self.n_batch.map(|v| v as i32),
            n_ubatch: self.n_ubatch.map(|v| v as i32),
        }
    }

    /// Deserialise from a DB record. JSON fields are parsed; parse errors
    /// fall back to None / default so a bad JSON column never hard-fails.
    ///
    /// Legacy args normalization: when `n_batch` or `n_ubatch` is `None` in the
    /// DB row, scans the parsed `args` array for `-b`/`--batch-size` and `-ub`/
    /// `--ubatch-size` flags (both `--flag value` and `--flag=value` forms),
    /// populates the corresponding field from the parsed u32, and removes those
    /// elements from `args`. Explicit column values always win over args flags.
    pub fn from_db_record(record: &ModelConfigRecord) -> Self {
        // Parse args from JSON
        let args: Vec<String> = record
            .args
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        // Extract explicit column values (these always win)
        let col_n_batch = record.n_batch.map(|v| v as u32);
        let col_n_ubatch = record.n_ubatch.map(|v| v as u32);

        // Normalize legacy args into new fields when columns are None
        let (n_batch, n_ubatch, args) = normalize_legacy_args(args, col_n_batch, col_n_ubatch);

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
            args,
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
            n_batch,
            n_ubatch,
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
