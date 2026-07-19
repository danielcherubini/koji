mod backend;
mod compaction;
mod enums;
mod general;
mod langfuse;
mod model;
mod proxy;
mod supervisor;

#[cfg(test)]
mod compaction_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod general_tests;
#[cfg(test)]
mod model_tests;

pub use backend::*;
pub use compaction::*;
pub use enums::*;
pub use general::*;
pub use langfuse::*;
pub use model::*;
pub use proxy::*;
pub use supervisor::*;

use crate::profiles::SamplingParams;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: General,
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,
    #[serde(default)]
    pub supervisor: Supervisor,
    #[serde(default)]
    pub sampling_templates: HashMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default)]
    pub langfuse: LangfuseConfig,
}

impl Config {
    /// Get the configs directory for model cards.
    pub fn configs_dir(&self) -> anyhow::Result<std::path::PathBuf> {
        Ok(Self::config_dir()?.join("configs"))
    }

    /// Get the models directory for this config.
    /// Uses `general.models_dir` if set, otherwise `<config_dir>/models/`.
    pub fn models_dir(&self) -> anyhow::Result<std::path::PathBuf> {
        if let Some(models_dir) = &self.general.models_dir {
            return Ok(std::path::PathBuf::from(models_dir));
        }
        Ok(Self::config_dir()?.join("models"))
    }

    /// Load a complete `Config` from a SQLite database at the given path.
    ///
    /// If the database is empty (no rows in any config table), seeds defaults
    /// via `app_config_queries::seed_defaults` before reading.
    /// Runs migrations if the database has not been initialized yet.
    pub fn from_db(db_path: &std::path::Path) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)
            .with_context(|| format!("Failed to open DB at {}", db_path.display()))?;

        // Run migrations if needed (skips quickly if already at latest version)
        crate::db::migrations::run(&conn)?;

        // Seed defaults if tables are empty (idempotent — no-op if rows exist)
        crate::db::queries::seed_defaults(&conn)?;

        // Read general
        let general_row = crate::db::queries::get_general(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_general row not found after seeding"))?;
        let general = General {
            log_level: crate::config::types::LogLevel::from_str(&general_row.log_level)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        log_level = %general_row.log_level,
                        "Invalid log_level in DB, falling back to default (info)"
                    );
                    crate::config::types::LogLevel::default()
                }),
            models_dir: general_row.models_dir,
            logs_dir: general_row.logs_dir,
            hf_token: general_row.hf_token,
            update_check_interval: general_row.update_check_interval,
        };

        // Read proxy
        let proxy_row = crate::db::queries::get_proxy(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_proxy row not found after seeding"))?;

        // Derive `api_keys_enabled` from the actual `api_keys` table. The flag
        // is a derived value — the source of truth is the `api_keys` table.
        // Treating it as a stored field allowed it to drift on every config
        // save; re-deriving on load ensures a stale DB value can never lock
        // the operator out of their own proxy after a restart.
        let active_keys = crate::db::queries::count_active_keys(&conn)?;
        let api_keys_enabled = active_keys > 0;

        let mut proxy = ProxyConfig {
            host: proxy_row.host,
            port: proxy_row.port,
            auto_unload: proxy_row.auto_unload,
            idle_timeout_secs: proxy_row.idle_timeout_secs,
            startup_timeout_secs: proxy_row.startup_timeout_secs,
            circuit_breaker_threshold: proxy_row.circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds: proxy_row.circuit_breaker_cooldown_seconds,
            metrics_retention_secs: proxy_row.metrics_retention_secs,
            download_queue_poll_interval_secs: proxy_row.download_queue_poll_interval_secs,
            max_loaded_models: proxy_row.max_loaded_models,
            authenticator_url: proxy_row.authenticator_url,
            authenticator_skip_paths: proxy_row.authenticator_skip_paths,
            oauth2: crate::config::types::OAuth2Config {
                enabled: proxy_row.oauth2_enabled,
                client_id: proxy_row.oauth2_client_id,
                client_secret: proxy_row.oauth2_client_secret,
                authorize_url: proxy_row.oauth2_authorize_url,
                token_url: proxy_row.oauth2_token_url,
                userinfo_url: proxy_row.oauth2_userinfo_url,
                logout_url: proxy_row.oauth2_logout_url,
                redirect_uri: proxy_row.oauth2_redirect_uri,
                scopes: proxy_row.oauth2_scopes,
                session_ttl_secs: proxy_row.oauth2_session_ttl_secs,
            },
            api_keys_enabled,
        };

        // Resolve env var references in OAuth2 config
        proxy.resolve_env_vars();

        // Read supervisor
        let supervisor_row = crate::db::queries::get_supervisor(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_supervisor row not found after seeding"))?;
        let supervisor = Supervisor {
            restart_policy: crate::config::types::RestartPolicy::from_str(
                &supervisor_row.restart_policy,
            )
            .unwrap_or_else(|| {
                tracing::warn!(
                    restart_policy = %supervisor_row.restart_policy,
                    "Invalid restart_policy in DB, falling back to default (always)"
                );
                crate::config::types::RestartPolicy::default()
            }),
            max_restarts: supervisor_row.max_restarts,
            restart_delay_ms: supervisor_row.restart_delay_ms,
            health_check_interval_ms: supervisor_row.health_check_interval_ms,
            health_check_timeout_ms: supervisor_row.health_check_timeout_ms,
            health_check_retries: supervisor_row.health_check_retries,
        };

        // Read compaction
        let compaction_row = crate::db::queries::get_compaction(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_compaction row not found after seeding"))?;
        let compaction = CompactionConfig {
            enabled: compaction_row.enabled,
            server_path: compaction_row.server_path,
            device: crate::config::types::CompactionDevice::from_str(&compaction_row.device)
                .unwrap_or_else(|| {
                    tracing::warn!(
                        device = %compaction_row.device,
                        "Invalid compaction device in DB, falling back to default (cpu)"
                    );
                    crate::config::types::CompactionDevice::default()
                }),
            port: compaction_row.port,
            request_timeout_ms: compaction_row.request_timeout_ms,
        };

        // Read sampling templates
        let template_rows = crate::db::queries::get_all_sampling_templates(&conn)?;
        let mut sampling_templates = HashMap::new();
        for template in &template_rows {
            sampling_templates.insert(
                template.name.clone(),
                SamplingParams {
                    temperature: template.temperature,
                    top_k: template.top_k,
                    top_p: template.top_p,
                    min_p: template.min_p,
                    presence_penalty: template.presence_penalty,
                    frequency_penalty: template.frequency_penalty,
                    repeat_penalty: template.repeat_penalty,
                },
            );
        }

        // Read backends from backend_configs table.
        // Note: BackendConfig (TOML struct) fields `path` and `version` are
        // not stored in the DB — backend resolution is exclusively DB-managed
        // via backend_configs + backend_installations tables.
        let backend_rows = crate::db::queries::list_backend_configs(&conn)?;
        let mut backends: HashMap<String, BackendConfig> = HashMap::new();
        for record in &backend_rows {
            backends.insert(
                record.name.clone(),
                BackendConfig {
                    path: None,
                    version: None,
                    gpu_variant: Some(record.gpu_variant.clone()),
                },
            );
        }

        // Read langfuse
        let langfuse_row = crate::db::queries::get_langfuse(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_langfuse row not found after seeding"))?;
        let langfuse = LangfuseConfig {
            enabled: langfuse_row.enabled,
            public_key: langfuse_row.public_key,
            secret_key: langfuse_row.secret_key,
            host: langfuse_row.host,
            environment: langfuse_row.environment,
            capture_input: langfuse_row.capture_input,
            capture_output: langfuse_row.capture_output,
            capture_streaming: langfuse_row.capture_streaming,
            telemetry_max_bytes: langfuse_row.telemetry_max_bytes,
            electricity_price_per_kwh: langfuse_row.electricity_price_per_kwh,
        };

        Ok(Config {
            general,
            backends,
            supervisor,
            proxy,
            compaction,
            langfuse,
            sampling_templates,
        })
    }

    /// Persist a `Config` to a SQLite database at the given path.
    ///
    /// Upserts each config section into its corresponding table. Sampling
    /// templates are deleted first then re-inserted to ensure a clean state.
    /// Runs migrations if the database has not been initialized yet.
    pub fn to_db(&self, db_path: &std::path::Path) -> anyhow::Result<()> {
        let conn = rusqlite::Connection::open(db_path)
            .with_context(|| format!("Failed to open DB at {}", db_path.display()))?;

        // Run migrations to ensure tables exist
        crate::db::migrations::run(&conn)?;

        // Upsert general
        crate::db::queries::upsert_general(
            &conn,
            &self.general.log_level,
            self.general.models_dir.as_deref(),
            self.general.logs_dir.as_deref(),
            self.general.hf_token.as_deref(),
            self.general.update_check_interval,
        )?;

        // Derive `api_keys_enabled` from the actual `api_keys` table. The flag
        // is a derived value — it must always reflect whether at least one
        // active (non-revoked, non-expired) key exists. Treating it as a
        // user-editable config field allowed it to drift to `false` on every
        // config save whenever the form's mirror type was missing the field
        // (and even when it didn't, an explicit `false` from a stale client
        // could lock the operator out of their own proxy).
        let active_keys = crate::db::queries::count_active_keys(&conn)?;
        let api_keys_enabled = active_keys > 0;

        // Upsert proxy
        crate::db::queries::upsert_proxy(
            &conn,
            &self.proxy.host,
            self.proxy.port,
            self.proxy.auto_unload,
            self.proxy.idle_timeout_secs,
            self.proxy.startup_timeout_secs,
            self.proxy.circuit_breaker_threshold,
            self.proxy.circuit_breaker_cooldown_seconds,
            self.proxy.metrics_retention_secs,
            self.proxy.download_queue_poll_interval_secs,
            self.proxy.max_loaded_models,
            self.proxy.authenticator_url.as_deref(),
            &self.proxy.authenticator_skip_paths,
            self.proxy.oauth2.enabled,
            &self.proxy.oauth2.client_id,
            &self.proxy.oauth2.client_secret,
            &self.proxy.oauth2.authorize_url,
            &self.proxy.oauth2.token_url,
            self.proxy.oauth2.userinfo_url.as_deref(),
            self.proxy.oauth2.logout_url.as_deref(),
            &self.proxy.oauth2.redirect_uri,
            &self.proxy.oauth2.scopes,
            self.proxy.oauth2.session_ttl_secs,
            api_keys_enabled,
        )?;

        // Upsert supervisor
        crate::db::queries::upsert_supervisor(
            &conn,
            &self.supervisor.restart_policy,
            self.supervisor.max_restarts,
            self.supervisor.restart_delay_ms,
            self.supervisor.health_check_interval_ms,
            self.supervisor.health_check_timeout_ms,
            self.supervisor.health_check_retries,
        )?;

        // Upsert compaction
        crate::db::queries::upsert_compaction(
            &conn,
            self.compaction.enabled,
            self.compaction.server_path.as_deref(),
            &self.compaction.device,
            self.compaction.port,
            self.compaction.request_timeout_ms,
        )?;

        // Upsert sampling templates (delete all first, then re-insert)
        crate::db::queries::delete_all_sampling_templates(&conn)?;
        for (name, params) in &self.sampling_templates {
            crate::db::queries::upsert_sampling_template(
                &conn,
                name,
                params.temperature,
                params.top_k,
                params.top_p,
                params.min_p,
                params.presence_penalty,
                params.frequency_penalty,
                params.repeat_penalty,
            )?;
        }

        // Upsert langfuse
        crate::db::queries::upsert_langfuse(
            &conn,
            &crate::db::queries::LangfuseRecord {
                enabled: self.langfuse.enabled,
                public_key: self.langfuse.public_key.clone(),
                secret_key: self.langfuse.secret_key.clone(),
                host: self.langfuse.host.clone(),
                environment: self.langfuse.environment.clone(),
                capture_input: self.langfuse.capture_input,
                capture_output: self.langfuse.capture_output,
                capture_streaming: self.langfuse.capture_streaming,
                telemetry_max_bytes: self.langfuse.telemetry_max_bytes,
                electricity_price_per_kwh: self.langfuse.electricity_price_per_kwh,
            },
        )?;

        Ok(())
    }
}

/// Helper function to check if a BTreeMap is empty.
/// Used in `skip_serializing_if` attributes to avoid serializing empty maps.
fn is_btreemap_empty<K, V>(map: &BTreeMap<K, V>) -> bool {
    map.is_empty()
}

fn default_enabled() -> bool {
    true
}

pub fn default_num_parallel() -> Option<u32> {
    // 0 = auto (don't set -np flag), 1+ = explicitly set -np N
    Some(0)
}

/// Maximum request body size in bytes (16 MB)
pub const MAX_REQUEST_BODY_SIZE: usize = 16 * 1024 * 1024;
