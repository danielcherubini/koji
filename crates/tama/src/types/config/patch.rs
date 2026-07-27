//! PATCH types for /tama/v1/config/structured.
//!
//! Each `*Patch` struct mirrors the corresponding non-patch struct with all
//! fields as `Option<T>`, enabling deep recursive field-level merge.

use serde::{Deserialize, Serialize};
use tama_core::config::{
    CompactionDevice as CoreCompactionDevice, LogLevel as CoreLogLevel,
    RestartPolicy as CoreRestartPolicy,
};

/// PATCH body for General section.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralPatch {
    pub log_level: Option<CoreLogLevel>,
    pub models_dir: Option<String>,
    pub logs_dir: Option<String>,
    pub hf_token: Option<String>,
    pub update_check_interval: Option<u32>,
}

/// PATCH body for Lifecycle section.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LifecyclePatch {
    pub restart_policy: Option<CoreRestartPolicy>,
    pub max_restarts: Option<u32>,
    pub restart_delay_ms: Option<u64>,
    pub health_check_interval_ms: Option<u64>,
    pub health_check_timeout_ms: Option<u64>,
    pub health_check_retries: Option<u32>,
}

/// PATCH body for ProxyConfig section.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfigPatch {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub auto_unload: Option<bool>,
    pub idle_timeout_secs: Option<u64>,
    pub startup_timeout_secs: Option<u64>,
    pub circuit_breaker_threshold: Option<u32>,
    pub circuit_breaker_cooldown_seconds: Option<u64>,
    pub metrics_retention_secs: Option<u64>,
    #[serde(alias = "download_queue_poll_interval_secs")]
    pub pull_queue_poll_interval_secs: Option<u64>,
    pub max_loaded_models: Option<u32>,
    pub authenticator_url: Option<String>,
    pub authenticator_skip_paths: Option<Vec<String>>,
    pub oauth2: Option<OAuth2ConfigPatch>,
    pub api_keys_enabled: Option<bool>,
}

/// PATCH body for OAuth2Config section.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuth2ConfigPatch {
    pub enabled: Option<bool>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub logout_url: Option<String>,
    pub redirect_uri: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub session_ttl_secs: Option<u64>,
}

/// PATCH body for CompactionConfig section.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfigPatch {
    pub enabled: Option<bool>,
    pub server_path: Option<String>,
    pub device: Option<CoreCompactionDevice>,
    pub port: Option<u16>,
    pub request_timeout_ms: Option<u64>,
}

/// PATCH body for LangfuseConfig section.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LangfuseConfigPatch {
    pub enabled: Option<bool>,
    pub public_key: Option<String>,
    pub secret_key: Option<String>,
    pub host: Option<String>,
    pub environment: Option<String>,
    pub capture_input: Option<bool>,
    pub capture_output: Option<bool>,
    pub capture_streaming: Option<bool>,
    pub telemetry_max_bytes: Option<usize>,
    pub electricity_price_per_kwh: Option<f64>,
}

/// Top-level PATCH body for /tama/v1/config/structured.
///
/// Each section is `Option<SectionPatch>` — if `None`, the existing section
/// is preserved entirely. If `Some`, field-by-field deep merge is applied.
///
/// `backends` is intentionally omitted — `Config.backends` is read-only
/// (not persisted by `to_db`).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ConfigPatchBody {
    #[serde(default)]
    pub general: Option<GeneralPatch>,
    // backends intentionally omitted — Config.backends is read-only (not persisted by to_db)
    #[serde(default, alias = "supervisor")]
    pub lifecycle: Option<LifecyclePatch>,
    #[serde(default)]
    pub sampling_templates:
        Option<std::collections::BTreeMap<String, crate::types::config::SamplingParams>>,
    #[serde(default)]
    pub proxy: Option<ProxyConfigPatch>,
    #[serde(default)]
    pub compaction: Option<CompactionConfigPatch>,
    #[serde(default)]
    pub langfuse: Option<LangfuseConfigPatch>,
}
