use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::gpu_types::{
    CompactionDevice as CoreCompactionDevice, LogLevel as CoreLogLevel,
    RestartPolicy as CoreRestartPolicy,
};

// ─── WASM-safe JSON mirror types ──────────────────────────────────────────
// These match the shape served by /api/config/structured and accepted by POST.
//
// NOTE: These types duplicate `crate::types::config::*` but use WASM-compatible
// enums from `gpu_types` (LogLevel, RestartPolicy, CompactionDevice) instead of
// `tama_core::config::*`. If you add/remove fields here, mirror the change in
// `types/config/` to keep them in sync. The two Config structs must remain
// structurally identical for the structured config API to work correctly.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendConfig>,
    #[serde(default)]
    pub supervisor: Supervisor,
    #[serde(default)]
    pub sampling_templates: BTreeMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
}

// Note: `models` is intentionally excluded from this Config struct.
// Model configs are stored in the SQLite database and managed through
// the /tama/v1/models/:id CRUD endpoints (pages/model_editor/), not
// through the structured config endpoint.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct General {
    #[serde(default)]
    pub log_level: CoreLogLevel,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    #[serde(default)]
    pub update_check_interval: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

// Note: `default_args` and `health_check_url` are intentionally excluded.
// These are per-backend settings stored in the SQLite `backend_configs` table
// and managed through the /tama/v1/backends/:name/default-args endpoint,
// not through the structured config endpoint.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Supervisor {
    #[serde(default)]
    pub restart_policy: CoreRestartPolicy,
    #[serde(default)]
    pub max_restarts: u32,
    #[serde(default)]
    pub restart_delay_ms: u64,
    #[serde(default)]
    pub health_check_interval_ms: u64,
    #[serde(default)]
    pub health_check_timeout_ms: u64,
    #[serde(default)]
    pub health_check_retries: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub auto_unload: bool,
    #[serde(default)]
    pub idle_timeout_secs: u64,
    #[serde(default)]
    pub startup_timeout_secs: u64,
    #[serde(default)]
    pub circuit_breaker_threshold: u32,
    #[serde(default)]
    pub circuit_breaker_cooldown_seconds: u64,
    #[serde(default)]
    pub metrics_retention_secs: u64,
    #[serde(default)]
    pub max_loaded_models: u32,
    #[serde(default)]
    pub download_queue_poll_interval_secs: u64,
    #[serde(default)]
    pub authenticator_url: Option<String>,
    #[serde(default)]
    pub authenticator_skip_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server_path: Option<String>,
    #[serde(default = "default_compaction_device")]
    pub device: CoreCompactionDevice,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_compaction_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

fn default_compaction_device() -> CoreCompactionDevice {
    CoreCompactionDevice::Cpu
}

fn default_compaction_request_timeout_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SamplingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
}
