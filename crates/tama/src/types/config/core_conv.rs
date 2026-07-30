//! SSR-only conversions between tama-core config types and the mirror types.
//!
//! This module is compiled only when the `ssr` feature is enabled.
//! It provides `From` impls for converting between `tama_core::config::*` types
//! and the mirror types defined in this module.

use std::str::FromStr;

use super::*;

// ── Config ─────────────────────────────────────────────────────────────────

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

// ── StructuredConfigBody ───────────────────────────────────────────────────

/// Convert from StructuredConfigBody to CoreConfig.
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

// ── General ────────────────────────────────────────────────────────────────

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

// ── Lifecycle ──────────────────────────────────────────────────────────────

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

// ── ProxyConfig ────────────────────────────────────────────────────────────

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

// ── CompactionConfig ───────────────────────────────────────────────────────

/// Convert from mirror CompactionConfig to tama_core::config::CompactionConfig.
impl From<CompactionConfig> for tama_core::config::CompactionConfig {
    fn from(c: CompactionConfig) -> Self {
        Self {
            enabled: c.enabled,
            server_path: c.server_path,
            device: c.device,
            port: c.port,
            request_timeout_ms: c.request_timeout_ms,
        }
    }
}

/// Convert from tama_core::config::CompactionConfig to mirror CompactionConfig.
impl From<tama_core::config::CompactionConfig> for CompactionConfig {
    fn from(c: tama_core::config::CompactionConfig) -> Self {
        Self {
            enabled: c.enabled,
            server_path: c.server_path,
            device: c.device,
            port: c.port,
            request_timeout_ms: c.request_timeout_ms,
        }
    }
}

// ── HealthCheck ────────────────────────────────────────────────────────────

/// Convert from tama_core::config::HealthCheck to mirror type.
impl From<tama_core::config::HealthCheck> for HealthCheck {
    fn from(h: tama_core::config::HealthCheck) -> Self {
        Self {
            url: h.url,
            interval_ms: h.interval_ms,
            timeout_ms: h.timeout_ms,
        }
    }
}

/// Convert from mirror HealthCheck to tama_core::config::HealthCheck.
impl From<HealthCheck> for tama_core::config::HealthCheck {
    fn from(h: HealthCheck) -> Self {
        Self {
            url: h.url,
            interval_ms: h.interval_ms,
            timeout_ms: h.timeout_ms,
        }
    }
}

// ── SamplingParams ─────────────────────────────────────────────────────────

/// Convert from tama_core::profiles::SamplingParams to mirror type.
impl From<tama_core::profiles::SamplingParams> for SamplingParams {
    fn from(s: tama_core::profiles::SamplingParams) -> Self {
        Self {
            temperature: s.temperature,
            top_k: s.top_k,
            top_p: s.top_p,
            min_p: s.min_p,
            presence_penalty: s.presence_penalty,
            frequency_penalty: s.frequency_penalty,
            repeat_penalty: s.repeat_penalty,
        }
    }
}

/// Convert from mirror SamplingParams to tama_core::profiles::SamplingParams.
impl From<SamplingParams> for tama_core::profiles::SamplingParams {
    fn from(s: SamplingParams) -> Self {
        Self {
            temperature: s.temperature,
            top_k: s.top_k,
            top_p: s.top_p,
            min_p: s.min_p,
            presence_penalty: s.presence_penalty,
            frequency_penalty: s.frequency_penalty,
            repeat_penalty: s.repeat_penalty,
        }
    }
}

// ── LangfuseConfig ─────────────────────────────────────────────────────────

impl From<tama_core::config::LangfuseConfig> for LangfuseConfig {
    fn from(c: tama_core::config::LangfuseConfig) -> Self {
        Self {
            enabled: c.enabled,
            public_key: c.public_key,
            secret_key: c.secret_key,
            host: c.host,
            environment: c.environment,
            capture_input: c.capture_input,
            capture_output: c.capture_output,
            capture_streaming: c.capture_streaming,
            telemetry_max_bytes: c.telemetry_max_bytes,
            electricity_price_per_kwh: c.electricity_price_per_kwh,
        }
    }
}

impl From<LangfuseConfig> for tama_core::config::LangfuseConfig {
    fn from(c: LangfuseConfig) -> Self {
        Self {
            enabled: c.enabled,
            public_key: c.public_key,
            secret_key: c.secret_key,
            host: c.host,
            environment: c.environment,
            capture_input: c.capture_input,
            capture_output: c.capture_output,
            capture_streaming: c.capture_streaming,
            telemetry_max_bytes: c.telemetry_max_bytes,
            electricity_price_per_kwh: c.electricity_price_per_kwh,
        }
    }
}

// ── BackendConfig ──────────────────────────────────────────────────────────

/// Convert from CoreBackendConfig to mirror type.
impl From<tama_core::config::BackendConfig> for BackendConfig {
    fn from(b: tama_core::config::BackendConfig) -> Self {
        Self {
            path: b.path,
            version: b.version,
            gpu_variant: b.gpu_variant.map(|v| v.variant_folder().to_string()),
        }
    }
}

/// Convert from mirror BackendConfig to CoreBackendConfig.
impl From<BackendConfig> for tama_core::config::BackendConfig {
    fn from(b: BackendConfig) -> Self {
        Self {
            path: b.path,
            version: b.version,
            gpu_variant: b.gpu_variant.map(|s| {
                tama_core::gpu::GpuVariant::from_str(&s).unwrap_or_else(|_| {
                    tracing::warn!(
                        "unknown gpu_variant '{}' in backend config; treating as custom",
                        s
                    );
                    tama_core::gpu::GpuVariant::Custom
                })
            }),
        }
    }
}

// ── ModelModalities ────────────────────────────────────────────────────────

/// Convert from CoreModelModalities to mirror type.
impl From<tama_core::config::ModelModalities> for ModelModalities {
    fn from(m: tama_core::config::ModelModalities) -> Self {
        Self {
            input: m.input,
            output: m.output,
        }
    }
}

/// Convert from mirror ModelModalities to core type.
impl From<ModelModalities> for tama_core::config::ModelModalities {
    fn from(m: ModelModalities) -> Self {
        Self {
            input: m.input,
            output: m.output,
        }
    }
}

// ── ModelConfig ────────────────────────────────────────────────────────────

/// Convert from tama_core::config::ModelConfig to mirror type.
impl From<tama_core::config::ModelConfig> for ModelConfig {
    fn from(m: tama_core::config::ModelConfig) -> Self {
        Self {
            backend: m.backend,
            gpu_variant: m.gpu_variant.map(|v| v.variant_folder().to_string()),
            gpu_device: m.gpu_device,
            args: m.args,
            sampling: m.sampling.map(Into::into),
            model: m.model,
            quant: m.quant,
            mmproj: m.mmproj,
            mtp_model: m.mtp_model,
            port: m.port,
            health_check: m.health_check.map(Into::into),
            enabled: m.enabled,
            context_length: m.context_length,
            num_parallel: m.num_parallel,
            profile: None, // Skip serializing - deprecated field
            api_name: m.api_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants,
            modalities: m.modalities.map(Into::into),
            display_name: m.display_name,
            kv_unified: m.kv_unified,
            cache_type_k: m.cache_type_k,
            cache_type_v: m.cache_type_v,
            n_batch: m.n_batch,
            n_ubatch: m.n_ubatch,
            extra: None, // Forward-compat field - preserve unknown fields on POST
        }
    }
}

/// Convert from mirror ModelConfig to tama_core::config::ModelConfig.
///
/// This conversion is intentionally lossy — the following fields are NOT
/// carried through because they are DB-only metadata populated by the model
/// pull/verify pipeline, not editable through the config:
/// - `hf_*` fields (format, base_model, pipeline_tag, params, etc.)
/// - `db_id` (auto-generated primary key)
/// - `spec_decoding` (managed through the model CRUD endpoints)
///
/// In practice, this conversion path is only used for the structured config
/// save endpoint, which does NOT persist models (models are DB-only).
/// The model CRUD endpoints use `ModelBody` → `apply_model_body()` instead.
impl From<ModelConfig> for tama_core::config::ModelConfig {
    fn from(m: ModelConfig) -> Self {
        Self {
            backend: m.backend,
            gpu_variant: m.gpu_variant.map(|s| {
                tama_core::gpu::GpuVariant::from_str(&s).unwrap_or_else(|_| {
                    tracing::warn!(
                        "unknown gpu_variant '{}' in model config; treating as custom",
                        s
                    );
                    tama_core::gpu::GpuVariant::Custom
                })
            }),
            gpu_device: m.gpu_device,
            args: m.args,
            sampling: m.sampling.map(Into::into),
            model: m.model,
            quant: m.quant,
            mmproj: m.mmproj,
            mtp_model: m.mtp_model,
            port: m.port,
            health_check: m.health_check.map(Into::into),
            enabled: m.enabled,
            context_length: m.context_length,
            num_parallel: m.num_parallel,
            profile: None, // Skip serializing - deprecated field
            api_name: m.api_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants,
            modalities: m.modalities.map(Into::into),
            display_name: m.display_name,
            kv_unified: m.kv_unified,
            cache_type_k: m.cache_type_k,
            cache_type_v: m.cache_type_v,
            // DB-only metadata — not editable through config, populated by pull/verify pipeline
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            db_id: None,                       // Auto-generated primary key
            spec_decoding: Default::default(), // Managed through model CRUD endpoints
            n_batch: m.n_batch,
            n_ubatch: m.n_ubatch,
        }
    }
}
