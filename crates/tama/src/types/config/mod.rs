//! Mirror types for Config that can be used from WASM.
//!
//! These types mirror the tama-core config types but use BTreeMap instead of HashMap
//! for deterministic JSON serialization. They are designed to be serialized/deserialized
//! with serde_json for the WASM frontend.

mod backend;
mod compaction;
mod general;
mod health;
mod langfuse;
mod lifecycle;
mod model;
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

// ── PATCH types for /tama/v1/config/structured (PATCH) ──────────────────────

mod patch;

pub use patch::CompactionConfigPatch;
pub use patch::ConfigPatchBody;
pub use patch::GeneralPatch;
pub use patch::LangfuseConfigPatch;
pub use patch::LifecyclePatch;
pub use patch::OAuth2ConfigPatch;
pub use patch::ProxyConfigPatch;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Request body for POST /tama/v1/config/structured.
///
/// Mirrors the shape of `Config` but lives here so the API layer
/// (`api.rs`) doesn't need a reverse dependency into `types::config`.
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

/// Convert from CoreConfig to mirror type.
impl From<tama_core::config::Config> for Config {
    fn from(c: tama_core::config::Config) -> Self {
        Self {
            general: c.general.into(),
            backends: c.backends.into_iter().map(|(k, v)| (k, v.into())).collect(),
            lifecycle: c.lifecycle.into(),
            sampling_templates: c
                .sampling_templates
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            proxy: c.proxy.into(),
            compaction: c.compaction.into(),
            langfuse: c.langfuse.into(),
        }
    }
}

/// Convert from mirror Config to CoreConfig.
impl From<StructuredConfigBody> for tama_core::config::Config {
    fn from(b: StructuredConfigBody) -> Self {
        Self {
            general: b.general.into(),
            backends: b.backends.into_iter().map(|(k, v)| (k, v.into())).collect(),
            lifecycle: b.lifecycle.into(),
            sampling_templates: b
                .sampling_templates
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            proxy: b.proxy.into(),
            compaction: b.compaction.into(),
            langfuse: b.langfuse.into(),
        }
    }
}

/// Convert from mirror Config to CoreConfig.
impl From<Config> for tama_core::config::Config {
    fn from(c: Config) -> Self {
        Self {
            general: c.general.into(),
            backends: c.backends.into_iter().map(|(k, v)| (k, v.into())).collect(),
            lifecycle: c.lifecycle.into(),
            sampling_templates: c
                .sampling_templates
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            proxy: c.proxy.into(),
            compaction: c.compaction.into(),
            langfuse: c.langfuse.into(),
        }
    }
}

/// Convert from CoreGeneral to mirror type.
impl From<tama_core::config::General> for General {
    fn from(g: tama_core::config::General) -> Self {
        Self {
            log_level: g.log_level,
            models_dir: g.models_dir,
            logs_dir: g.logs_dir,
            hf_token: g.hf_token,
            update_check_interval: g.update_check_interval,
        }
    }
}

/// Convert from mirror General to CoreGeneral.
impl From<General> for tama_core::config::General {
    fn from(g: General) -> Self {
        Self {
            log_level: g.log_level,
            models_dir: g.models_dir,
            logs_dir: g.logs_dir,
            hf_token: g.hf_token,
            update_check_interval: g.update_check_interval,
        }
    }
}

/// Convert from CoreLifecycle to mirror type.
impl From<tama_core::config::Lifecycle> for Lifecycle {
    fn from(s: tama_core::config::Lifecycle) -> Self {
        Self {
            restart_policy: s.restart_policy,
            max_restarts: s.max_restarts,
            restart_delay_ms: s.restart_delay_ms,
            health_check_interval_ms: s.health_check_interval_ms,
            health_check_timeout_ms: s.health_check_timeout_ms,
            health_check_retries: s.health_check_retries,
        }
    }
}

/// Convert from mirror Lifecycle to CoreLifecycle.
impl From<Lifecycle> for tama_core::config::Lifecycle {
    fn from(s: Lifecycle) -> Self {
        Self {
            restart_policy: s.restart_policy,
            max_restarts: s.max_restarts,
            restart_delay_ms: s.restart_delay_ms,
            health_check_interval_ms: s.health_check_interval_ms,
            health_check_timeout_ms: s.health_check_timeout_ms,
            health_check_retries: s.health_check_retries,
        }
    }
}

/// Convert from CoreProxyConfig to mirror type.
impl From<tama_core::config::ProxyConfig> for ProxyConfig {
    fn from(p: tama_core::config::ProxyConfig) -> Self {
        Self {
            host: p.host,
            port: p.port,
            auto_unload: p.auto_unload,
            idle_timeout_secs: p.idle_timeout_secs,
            startup_timeout_secs: p.startup_timeout_secs,
            circuit_breaker_threshold: p.circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds: p.circuit_breaker_cooldown_seconds,
            metrics_retention_secs: p.metrics_retention_secs,
            pull_queue_poll_interval_secs: p.pull_queue_poll_interval_secs,
            max_loaded_models: p.max_loaded_models,
            authenticator_url: p.authenticator_url,
            authenticator_skip_paths: p.authenticator_skip_paths,
            oauth2: OAuth2Config {
                enabled: p.oauth2.enabled,
                client_id: p.oauth2.client_id,
                client_secret: p.oauth2.client_secret,
                authorize_url: p.oauth2.authorize_url,
                token_url: p.oauth2.token_url,
                userinfo_url: p.oauth2.userinfo_url,
                logout_url: p.oauth2.logout_url,
                redirect_uri: p.oauth2.redirect_uri,
                scopes: p.oauth2.scopes,
                session_ttl_secs: p.oauth2.session_ttl_secs,
            },
            api_keys_enabled: p.api_keys_enabled,
        }
    }
}

/// Convert from mirror ProxyConfig to CoreProxyConfig.
impl From<ProxyConfig> for tama_core::config::ProxyConfig {
    fn from(p: ProxyConfig) -> Self {
        Self {
            host: p.host,
            port: p.port,
            auto_unload: p.auto_unload,
            idle_timeout_secs: p.idle_timeout_secs,
            startup_timeout_secs: p.startup_timeout_secs,
            circuit_breaker_threshold: p.circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds: p.circuit_breaker_cooldown_seconds,
            metrics_retention_secs: p.metrics_retention_secs,
            pull_queue_poll_interval_secs: p.pull_queue_poll_interval_secs,
            max_loaded_models: p.max_loaded_models,
            authenticator_url: p.authenticator_url,
            authenticator_skip_paths: p.authenticator_skip_paths,
            oauth2: tama_core::config::OAuth2Config {
                enabled: p.oauth2.enabled,
                client_id: p.oauth2.client_id,
                client_secret: p.oauth2.client_secret,
                authorize_url: p.oauth2.authorize_url,
                token_url: p.oauth2.token_url,
                userinfo_url: p.oauth2.userinfo_url,
                logout_url: p.oauth2.logout_url,
                redirect_uri: p.oauth2.redirect_uri,
                scopes: p.oauth2.scopes,
                session_ttl_secs: p.oauth2.session_ttl_secs,
            },
            api_keys_enabled: p.api_keys_enabled,
        }
    }
}
