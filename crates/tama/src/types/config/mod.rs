//! Mirror types for Config that can be used from WASM.
//!
//! These types mirror the tama-core config types but use BTreeMap instead of HashMap
//! for deterministic JSON serialization. They are designed to be serialized/deserialized
//! with serde_json for the WASM frontend.

mod backend;
mod compaction;
#[cfg(feature = "ssr")]
mod core_conv;
mod general;
mod health;
mod langfuse;
mod lifecycle;
mod model;
#[cfg(feature = "ssr")]
mod patch;
mod proxy;
mod quant;
mod sampling;

pub use backend::*;
pub use compaction::*;
pub use general::*;
pub use health::*;
pub use langfuse::*;
pub use lifecycle::*;
pub use model::*;
pub use proxy::*;
pub use quant::*;
pub use sampling::*;

#[cfg(feature = "ssr")]
pub use patch::CompactionConfigPatch;
#[cfg(feature = "ssr")]
pub use patch::ConfigPatchBody;
#[cfg(feature = "ssr")]
pub use patch::GeneralPatch;
#[cfg(feature = "ssr")]
pub use patch::LangfuseConfigPatch;
#[cfg(feature = "ssr")]
pub use patch::LifecyclePatch;
#[cfg(feature = "ssr")]
pub use patch::OAuth2ConfigPatch;
#[cfg(feature = "ssr")]
pub use patch::ProxyConfigPatch;

// ── PATCH types for /tama/v1/config/structured (PATCH) ──────────────────────

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Main configuration struct.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub general: General,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendConfig>,
    #[serde(default, alias = "supervisor")]
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub sampling_templates: BTreeMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub langfuse: LangfuseConfig,
}

/// Request body for POST /tama/v1/config/structured.
///
/// Mirrors the shape of `Config` but lives here so the API layer
/// (`api.rs`) doesn't need a reverse dependency into `types::config`.
#[cfg(feature = "ssr")]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StructuredConfigBody {
    pub general: General,
    #[serde(default)]
    pub backends: std::collections::BTreeMap<String, BackendConfig>,
    #[serde(default, alias = "supervisor")]
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub sampling_templates: std::collections::BTreeMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub langfuse: LangfuseConfig,
}

// ── Regression tests (pure serde, compile on both feature sets) ────────────

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
                "log_directives": "tama_core=debug",
                "log_retention_days": 14,
                "log_retention_rows": 25_000,
                "log_retention_max_mb": 128,
            },
            "backends": {},
            "lifecycle": {
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
                "pull_queue_poll_interval_secs": 3,
                "pull_backend": "tamad-123",
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
