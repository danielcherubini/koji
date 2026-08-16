//! Record types for database query results.
//!
//! These types are the canonical row representations; the API layer uses
//! them directly via the async `db::queries` functions.

use serde::{Deserialize, Serialize};

/// Per-repo user configuration for a model.
///
/// This type is part of `ModelManager`'s public API and is also returned
/// directly by the `db::queries` functions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfigRecord {
    pub id: i64,         // auto-increment primary key
    pub repo_id: String, // HF repo name
    pub display_name: Option<String>,
    pub backend: String,
    pub gpu_variant: Option<String>,
    pub gpu_device: Option<String>,
    pub enabled: bool,
    pub selected_quant: Option<String>,
    pub selected_mmproj: Option<String>,
    pub selected_mtp_model: Option<String>,
    pub context_length: Option<u32>,
    pub num_parallel: Option<u32>,
    pub kv_unified: bool,
    pub gpu_layers: Option<u32>,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
    pub port: Option<u16>,
    pub args: Option<String>,       // raw JSON string
    pub sampling: Option<String>,   // raw JSON string
    pub modalities: Option<String>, // raw JSON string
    pub profile: Option<String>,
    pub api_name: Option<String>,
    pub health_check: Option<String>, // raw JSON string
    pub hf_format: Option<String>,
    pub hf_base_model: Option<String>,
    pub hf_pipeline_tag: Option<String>,
    pub hf_total_params: Option<String>,
    pub hf_active_params: Option<String>,
    pub hf_architecture_type: Option<String>,
    pub hf_context_length: Option<u32>,
    pub hf_num_layers: Option<u32>,
    pub hf_last_modified: Option<String>,
    pub spec_decoding: Option<String>, // raw JSON string
    pub created_at: String,
    pub updated_at: String,
    /// Pre-allocated context KV cache size (llama.cpp --batch). None = backend default.
    pub n_batch: Option<i32>,
    /// Maximum number of unique sequences to process in a single batch
    /// (llama.cpp --ubatch). None = backend default.
    pub n_ubatch: Option<i32>,
    /// vLLM-specific launch settings as raw JSON string.
    pub vllm_config: Option<String>,
    /// Name of the remote provider that serves this model. When set, overrides
    /// the `backend` field for routing — request is forwarded via `RemoteForwarder`
    /// to the provider's `base_url` with the provider's `api_key` as Bearer token.
    /// NULL means use normal local backend routing (legacy behaviour).
    pub provider_name: Option<String>,
    pub reasoning_levels: Option<String>, // raw JSON string
}

/// A stored pull record for a HuggingFace repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPullRecord {
    pub id: i64,         // auto-increment primary key
    pub model_id: i64,   // FK to model_configs.id
    pub repo_id: String, // cached
    pub commit_sha: String,
    pub pulled_at: String, // ISO 8601 from SQLite
}

/// A stored file record for a pulled GGUF.
///
/// This type is part of `ModelManager`'s public API and is also returned
/// directly by `Repository` methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFileRecord {
    pub id: i64,         // auto-increment primary key
    pub model_id: i64,   // FK to model_configs.id
    pub repo_id: String, // cached
    pub filename: String,
    pub quant: Option<String>,
    pub lfs_oid: Option<String>,
    pub size_bytes: Option<i64>,
    pub pulled_at: String,
    /// ISO 8601 timestamp of the most recent verification attempt. None if never verified.
    pub last_verified_at: Option<String>,
    /// Some(true) = hash matched. Some(false) = mismatch. None = never verified
    /// or no upstream hash available to compare against.
    pub verified_ok: Option<bool>,
    /// Short human-readable error when `verified_ok = Some(false)` or when verification
    /// could not complete (e.g. "no upstream hash", "hash mismatch: expected X got Y").
    pub verify_error: Option<String>,
}

/// An entry in the pull log (append-only).
pub struct PullLogEntry {
    pub repo_id: String,
    pub filename: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub success: bool,
    pub error_message: Option<String>,
}

/// An active model entry tracking a running backend process.
pub struct ActiveModelRecord {
    pub server_name: String,
    pub model_name: String,
    pub backend: String,
    pub pid: i64,
    pub port: i64,
    pub backend_url: String,
    pub loaded_at: String,
    pub last_accessed: String,
}

/// TTS engine configuration record.
pub struct TtsConfigRecord {
    pub id: i64,        // auto-increment primary key
    pub engine: String, // TTS engine name (e.g., 'kokoro')
    pub default_voice: Option<String>,
    pub speed: f32,     // 0.5 to 2.0
    pub format: String, // mp3, wav, ogg
    pub enabled: bool,
    pub created_at: sqlx::types::time::OffsetDateTime,
    pub updated_at: sqlx::types::time::OffsetDateTime,
}

/// A stored update check record for a backend or model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckRecord {
    pub item_type: String, // "backend" or "model"
    pub item_id: String,   // backend name or model config key
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String, // "unknown", "up_to_date", "update_available", "error"
    pub error_message: Option<String>,
    pub details_json: Option<String>, // JSON blob for model file changes
    pub checked_at: i64,              // unix timestamp
}
