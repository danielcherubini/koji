//! Record types for database query results.
//!
//! These types are the canonical row representations; the API layer uses
//! them directly via `db::repository::Repository`.

use rusqlite::Row;
use serde::{Deserialize, Serialize};

/// Per-repo user configuration for a model.
///
/// This type is part of `ModelManager`'s public API and is also returned
/// directly by `Repository` methods.
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
}

impl ModelConfigRecord {
    /// All 37 columns in SELECT order (id first). Must match `from_row` index order.
    pub(crate) const COLUMNS: &str =
        "id, repo_id, display_name, backend, gpu_variant, gpu_device, enabled, selected_quant, \
         selected_mmproj, selected_mtp_model, context_length, num_parallel, kv_unified, gpu_layers, \
         cache_type_k, cache_type_v, port, args, \
         sampling, modalities, profile, api_name, health_check, \
         hf_format, hf_base_model, hf_pipeline_tag, hf_total_params, \
         hf_active_params, hf_architecture_type, hf_context_length, \
         hf_num_layers, hf_last_modified, spec_decoding, \
         created_at, updated_at, n_batch, n_ubatch";

    /// The 36 non-`id` columns in INSERT order. Must stay in sync with `COLUMNS` minus `id`.
    pub(crate) const INSERT_COLUMNS: &str =
        "repo_id, display_name, backend, gpu_variant, gpu_device, enabled, selected_quant, \
         selected_mmproj, selected_mtp_model, context_length, num_parallel, kv_unified, gpu_layers, \
         cache_type_k, cache_type_v, port, args, \
         sampling, modalities, profile, api_name, health_check, \
         hf_format, hf_base_model, hf_pipeline_tag, hf_total_params, \
         hf_active_params, hf_architecture_type, hf_context_length, \
         hf_num_layers, hf_last_modified, spec_decoding, \
         created_at, updated_at, n_batch, n_ubatch";

    /// Map a row selected with `COLUMNS` order into a record.
    pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(ModelConfigRecord {
            id: row.get(0)?,
            repo_id: row.get(1)?,
            display_name: row.get(2)?,
            backend: row.get(3)?,
            gpu_variant: row.get(4)?,
            gpu_device: row.get(5)?,
            enabled: row.get::<_, i32>(6)? != 0,
            selected_quant: row.get(7)?,
            selected_mmproj: row.get(8)?,
            selected_mtp_model: row.get(9)?,
            context_length: row.get(10)?,
            num_parallel: row.get(11)?,
            kv_unified: row.get::<_, i32>(12)? != 0,
            gpu_layers: row.get(13)?,
            cache_type_k: row.get(14)?,
            cache_type_v: row.get(15)?,
            port: row.get(16)?,
            args: row.get(17)?,
            sampling: row.get(18)?,
            modalities: row.get(19)?,
            profile: row.get(20)?,
            api_name: row.get(21)?,
            health_check: row.get(22)?,
            hf_format: row.get(23)?,
            hf_base_model: row.get(24)?,
            hf_pipeline_tag: row.get(25)?,
            hf_total_params: row.get(26)?,
            hf_active_params: row.get(27)?,
            hf_architecture_type: row.get(28)?,
            hf_context_length: row.get(29)?,
            hf_num_layers: row.get(30)?,
            hf_last_modified: row.get(31)?,
            spec_decoding: row.get(32)?,
            created_at: row.get(33)?,
            updated_at: row.get(34)?,
            n_batch: row.get(35)?,
            n_ubatch: row.get(36)?,
        })
    }
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

impl ModelFileRecord {
    /// All 11 columns in SELECT order (id first). Must match `from_row` index order.
    pub(crate) const COLUMNS: &str =
        "id, model_id, repo_id, filename, quant, lfs_oid, size_bytes, pulled_at, \
         last_verified_at, verified_ok, verify_error";

    /// Map a row selected with `COLUMNS` order into a record.
    pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let verified_ok: Option<i64> = row.get(9)?;
        Ok(ModelFileRecord {
            id: row.get(0)?,
            model_id: row.get(1)?,
            repo_id: row.get(2)?,
            filename: row.get(3)?,
            quant: row.get(4)?,
            lfs_oid: row.get(5)?,
            size_bytes: row.get(6)?,
            pulled_at: row.get(7)?,
            last_verified_at: row.get(8)?,
            verified_ok: verified_ok.map(|v| v != 0),
            verify_error: row.get(10)?,
        })
    }
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
    pub created_at: String,
    pub updated_at: String,
}

impl TtsConfigRecord {
    /// All 8 columns in SELECT order (id first). Must match `from_row` index order.
    pub(crate) const COLUMNS: &str =
        "id, engine, default_voice, speed, format, enabled, created_at, updated_at";

    /// Map a row selected with `COLUMNS` order into a record.
    pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(TtsConfigRecord {
            id: row.get(0)?,
            engine: row.get(1)?,
            default_voice: row.get(2)?,
            speed: row.get(3)?,
            format: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    }
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

impl UpdateCheckRecord {
    /// All 9 columns in SELECT order. Must match `from_row` index order.
    pub(crate) const COLUMNS: &str =
        "item_type, item_id, current_version, latest_version, update_available, \
         status, error_message, details_json, checked_at";

    /// Map a row selected with `COLUMNS` order into a record.
    pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(UpdateCheckRecord {
            item_type: row.get(0)?,
            item_id: row.get(1)?,
            current_version: row.get(2)?,
            latest_version: row.get(3)?,
            update_available: row.get::<_, i32>(4)? != 0,
            status: row.get(5)?,
            error_message: row.get(6)?,
            details_json: row.get(7)?,
            checked_at: row.get(8)?,
        })
    }
}
