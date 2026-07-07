mod backend;
mod compaction;
mod general;
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
pub use general::*;
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
            log_level: general_row.0,
            models_dir: general_row.1,
            logs_dir: general_row.2,
            hf_token: general_row.3,
            update_check_interval: general_row.4,
        };

        // Read proxy
        let proxy_row = crate::db::queries::get_proxy(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_proxy row not found after seeding"))?;
        let proxy = ProxyConfig {
            host: proxy_row.0,
            port: proxy_row.1,
            auto_unload: proxy_row.2,
            idle_timeout_secs: proxy_row.3,
            startup_timeout_secs: proxy_row.4,
            circuit_breaker_threshold: proxy_row.5,
            circuit_breaker_cooldown_seconds: proxy_row.6,
            metrics_retention_secs: proxy_row.7,
            download_queue_poll_interval_secs: proxy_row.8,
            max_loaded_models: proxy_row.9,
            authenticator_url: proxy_row.10,
            authenticator_skip_paths: proxy_row.11,
        };

        // Read supervisor
        let supervisor_row = crate::db::queries::get_supervisor(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_supervisor row not found after seeding"))?;
        let supervisor = Supervisor {
            restart_policy: supervisor_row.0,
            max_restarts: supervisor_row.1,
            restart_delay_ms: supervisor_row.2,
            health_check_interval_ms: supervisor_row.3,
            health_check_timeout_ms: supervisor_row.4,
            health_check_retries: supervisor_row.5,
        };

        // Read compaction
        let compaction_row = crate::db::queries::get_compaction(&conn)?
            .ok_or_else(|| anyhow::anyhow!("app_compaction row not found after seeding"))?;
        let compaction = CompactionConfig {
            enabled: compaction_row.0,
            server_path: compaction_row.1,
            device: compaction_row.2,
            port: compaction_row.3,
            request_timeout_ms: compaction_row.4,
        };

        // Read sampling templates
        let template_rows = crate::db::queries::get_all_sampling_templates(&conn)?;
        let mut sampling_templates = HashMap::new();
        for (
            name,
            temperature,
            top_k,
            top_p,
            min_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
        ) in &template_rows
        {
            sampling_templates.insert(
                name.clone(),
                SamplingParams {
                    temperature: *temperature,
                    top_k: *top_k,
                    top_p: *top_p,
                    min_p: *min_p,
                    presence_penalty: *presence_penalty,
                    frequency_penalty: *frequency_penalty,
                    repeat_penalty: *repeat_penalty,
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

        Ok(Config {
            general,
            backends,
            supervisor,
            proxy,
            compaction,
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
