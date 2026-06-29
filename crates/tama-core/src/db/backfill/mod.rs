//! Database backfill logic
//!
//! Provides one-time migration functions and HF metadata backfill.

mod hf_metadata;
mod initial_backfill;
mod migrate_toml_to_db;

pub use hf_metadata::*;
pub use initial_backfill::*;
pub use migrate_toml_to_db::*;

// ---------------------------------------------------------------------------
// Private legacy deserialization structs (for one-time TOML migration only)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct LegacyRegistryData {
    #[serde(default)]
    backends: std::collections::HashMap<String, LegacyBackendInfo>,
}

#[derive(serde::Deserialize)]
struct LegacyBackendInfo {
    backend_type: crate::backends::BackendType,
    version: String,
    path: std::path::PathBuf,
    installed_at: i64,
    gpu_type: Option<crate::gpu::GpuType>,
    source: Option<crate::backends::BackendSource>,
}
