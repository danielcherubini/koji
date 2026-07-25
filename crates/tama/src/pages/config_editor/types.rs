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
    #[serde(default)]
    pub langfuse: LangfuseConfig,
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
pub struct OAuth2Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub authorize_url: String,
    #[serde(default)]
    pub token_url: String,
    #[serde(default)]
    pub userinfo_url: Option<String>,
    #[serde(default)]
    pub logout_url: Option<String>,
    #[serde(default)]
    pub redirect_uri: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub session_ttl_secs: u64,
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
    #[serde(default)]
    pub oauth2: OAuth2Config,
    /// Whether API key authentication is enabled.
    /// Mirrors `tama_core::config::ProxyConfig::api_keys_enabled`.
    /// MUST stay in sync with the core type — if it's missing here, every
    /// config save silently disables API key auth.
    #[serde(default)]
    pub api_keys_enabled: bool,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LangfuseConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub capture_input: bool,
    #[serde(default)]
    pub capture_output: bool,
    #[serde(default)]
    pub capture_streaming: bool,
    #[serde(default)]
    pub telemetry_max_bytes: usize,
    #[serde(default)]
    pub electricity_price_per_kwh: f64,
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

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a JSON shape that mirrors what the server returns from
    /// `GET /tama/v1/config/structured`. Every field is set to a non-default
    /// value so any missing field in the form's mirror type round-trip can
    /// be observed.
    fn server_response_with_all_fields() -> serde_json::Value {
        json!({
            "general": {
                "log_level": "debug",
                "models_dir": "/mnt/models",
                "logs_dir": "/var/log/tama",
                "hf_token": "hf_testtoken",
                "update_check_interval": 6,
            },
            "backends": {},
            "supervisor": {
                "restart_policy": "on-failure",
                "max_restarts": 5,
                "restart_delay_ms": 1000,
                "health_check_interval_ms": 2000,
                "health_check_timeout_ms": 5000,
                "health_check_retries": 3,
            },
            "sampling_templates": {},
            "proxy": {
                "host": "127.0.0.1",
                "port": 18910,
                "auto_unload": true,
                "idle_timeout_secs": 600,
                "startup_timeout_secs": 90,
                "circuit_breaker_threshold": 7,
                "circuit_breaker_cooldown_seconds": 120,
                "metrics_retention_secs": 172_800,
                "max_loaded_models": 2,
                "download_queue_poll_interval_secs": 3,
                "authenticator_url": "https://auth.example.com",
                "authenticator_skip_paths": ["/health", "/metrics"],
                "oauth2": {
                    "enabled": true,
                    "client_id": "test-client",
                    "client_secret": "test-secret",
                    "authorize_url": "https://auth.example.com/authorize",
                    "token_url": "https://auth.example.com/token",
                    "userinfo_url": "https://auth.example.com/userinfo",
                    "logout_url": "https://auth.example.com/logout",
                    "redirect_uri": "http://localhost:11434/login/callback",
                    "scopes": ["openid", "profile"],
                    "session_ttl_secs": 7200,
                },
                "api_keys_enabled": true,
            },
            "compaction": {
                "enabled": true,
                "server_path": "/opt/compaction/main.py",
                "device": "cuda",
                "port": 8081,
                "request_timeout_ms": 60_000,
            },
            "langfuse": {
                "enabled": true,
                "public_key": "pk-lf-test123",
                "secret_key": "sk-lf-test456",
                "host": "https://cloud.langfuse.com",
                "environment": "production",
                "capture_input": true,
                "capture_output": true,
                "capture_streaming": true,
                "telemetry_max_bytes": 1048576,
                "electricity_price_per_kwh": 0.12,
            },
        })
    }

    /// Regression: `api_keys_enabled` was being dropped on every config save
    /// because the form's mirror type did not include the field. serde silently
    /// ignores unknown fields on deserialize, so the in-memory form ended up
    /// with `api_keys_enabled = false` (default) and POSTed that to the server.
    #[test]
    fn test_api_keys_enabled_round_trips_through_form_config() {
        let server_json = server_response_with_all_fields();
        let form_cfg: Config = serde_json::from_value(server_json.clone())
            .expect("form should accept the server's full structured config");

        let round_trip: serde_json::Value =
            serde_json::to_value(&form_cfg).expect("form should re-serialize its config");

        assert_eq!(
            round_trip["proxy"]["api_keys_enabled"], true,
            "api_keys_enabled was dropped on the form's mirror type — \
             this is the bug that silently disables API key auth on every config save"
        );
    }

    /// Regression: every field in the form's mirror type must round-trip,
    /// not just `api_keys_enabled`. Catches future drift if a new field is
    /// added to the core config and forgotten in the form.
    #[test]
    fn test_full_config_round_trip_preserves_every_field() {
        let original = server_response_with_all_fields();
        let form_cfg: Config = serde_json::from_value(original.clone())
            .expect("form should deserialize server's structured config");
        let round_trip: serde_json::Value =
            serde_json::to_value(&form_cfg).expect("form should re-serialize its config");

        assert_eq!(
            original, round_trip,
            "form mirror type did not round-trip all fields — any missing \
             field will be silently dropped on save"
        );
    }
}
