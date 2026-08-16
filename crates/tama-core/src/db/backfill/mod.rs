//! Database backfill logic
//!
//! Provides one-time migration functions, HF metadata backfill, and vLLM config backfill.

mod hf_metadata;
mod initial_backfill;
mod vllm_config;

pub use hf_metadata::*;
pub use initial_backfill::*;
pub use vllm_config::*;

// ---------------------------------------------------------------------------
// Private legacy deserialization structs (for one-time TOML migration only)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct LegacyRegistryData {
    #[serde(default)]
    backends: std::collections::HashMap<String, LegacyInstallationInfo>,
}

#[derive(serde::Deserialize)]
struct LegacyInstallationInfo {
    backend_type: crate::installations::InstallationType,
    version: String,
    path: std::path::PathBuf,
    installed_at: i64,
    #[serde(default, alias = "gpu_type")]
    #[allow(dead_code)]
    gpu_variant: Option<toml::Value>,
    #[serde(default)]
    source: Option<crate::installations::InstallationSource>,
}
