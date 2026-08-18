//! Proxy state sub-structs and the main `ProxyState` implementation.
//!
//! `ProxyState` is composed from three domain sub-structs:
//! - `RegistryState` — models, model_configs, aliases caches
//! - `MetricsState` — counters, system_metrics, metrics_tx, inference_stats channels
//! - `PullState` — pull_jobs, in_flight_pulls, pull_queue service, repo_pulls

mod metrics;
mod pull;
mod registry;
pub(crate) mod repo_pull;

pub(crate) use metrics::MetricsState;
pub(crate) use pull::PullState;
pub(crate) use registry::RegistryState;
pub(crate) use repo_pull::*;

use anyhow::Context;
use std::sync::Arc;
use std::time::Duration;

use super::pull_queue::{queue_processor_loop, PullQueueService};
use super::types::{BackendState, ProxyState};

impl ProxyState {
    pub fn new(
        config: crate::config::Config,
        db_dir: Option<std::path::PathBuf>,
        db_pool: Arc<sqlx::PgPool>,
    ) -> Self {
        // Initialize Langfuse client from config before wrapping in Arc.
        let langfuse_client =
            crate::proxy::forward::langfuse::LangfuseClient::from_config(&config.langfuse)
                .map(Arc::new);

        // Initialize pull queue service from the Postgres pool.
        let poll_interval = config.proxy.pull_queue_poll_interval_secs;
        let pull_queue = Some(Arc::new(PullQueueService::new(
            db_pool.clone(),
            poll_interval,
        )));

        let state = Self {
            registry: RegistryState::new(),
            metrics: MetricsState::new(),
            pull: PullState::new(pull_queue.clone()),
            config: Arc::new(tokio::sync::RwLock::new(config)),
            client: reqwest::Client::builder()
                // Only set a connect timeout — not an overall timeout.
                // The overall timeout covers the entire response lifetime
                // including streaming bodies, which would kill long SSE
                // streams from LLM backends.
                .connect_timeout(Duration::from_secs(30))
                .build()
                // reqwest Client::build() only fails if TLS backend init fails,
                // which is not recoverable — panic is acceptable here.
                .expect("failed to build HTTP client"),
            db_dir,
            config_write_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            backend_logs: crate::installations::log_stream::BackendLogManager::default(),
            cookie_key: cookie::Key::generate(),
            langfuse_client: Arc::new(tokio::sync::RwLock::new(langfuse_client)),
            remote_forwarder: crate::proxy::remote::RemoteForwarder::new(),
            tamad_pool: Arc::new(crate::tamad::pool::TamadPool::new(db_pool.clone())),
            started_at: std::time::Instant::now(),
            db_pool,
        };

        // Spawn the queue processor background task.
        // This must be called from within a tokio runtime context (production
        // always is; plain `#[test]` contexts skip it via try_current).
        if let Some(ref _dq) = pull_queue {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let state_clone = Arc::new(state.clone());
                handle.spawn(async move {
                    queue_processor_loop(state_clone).await;
                });
            }
        }

        state
    }

    /// Get the backend URL for a backend name.
    pub async fn get_backend_url(&self, backend_name: &str) -> anyhow::Result<String> {
        let config = self.config.read().await;
        let model_configs = self.registry.model_configs.read().await;
        let backend_config = config
            .resolve_backend(&model_configs, backend_name)
            .with_context(|| format!("Backend '{}' not found", backend_name))?
            .0;

        // Look up the health_check_url from the Postgres pool (plan-190 Task 8).
        let gpu_variant = backend_config
            .gpu_variant
            .clone()
            .unwrap_or(crate::gpu::GpuVariant::CpuOnly);
        let pool = self.db_pool();
        let manager = crate::installations::InstallationManager::new(pool);
        let health_url = manager
            .get_health_check_url(&backend_config.backend, gpu_variant.variant_folder())
            .await;
        let backend_url = config
            .resolve_backend_url(backend_config, health_url.as_deref())
            .with_context(|| format!("No backend URL resolved for backend '{}'", backend_name))?;

        Ok(backend_url)
    }

    /// Get the state of a loaded model (backend).
    pub async fn get_model_state(&self, backend_name: &str) -> Option<BackendState> {
        self.registry.get_model_state(backend_name).await
    }

    /// Find an available loaded backend for a given model name.
    pub async fn get_available_backend_for_model(&self, model_name: &str) -> Option<String> {
        let config = self.config.read().await;
        self.registry
            .get_available_backend_for_model(&config, model_name)
            .await
    }

    /// Update the last accessed time for a backend.
    pub async fn update_last_accessed(&self, backend_name: &str) {
        self.registry.update_last_accessed(backend_name).await
    }

    /// Get the model TOML for a model name.
    pub async fn get_model_toml(&self, model_name: &str) -> Option<crate::models::ModelToml> {
        let configs_dir = self.config.read().await.configs_dir().ok()?;

        // Try to find the model card file.
        // Format: configs/<slug>.toml — slug is case-preserving (see card_slug).
        //
        // `card_slug` (a simple `replace('/', "--")`) is equivalent to the old
        // `split_once('/').unwrap_or(("", model_name))` logic for well-formed
        // repo_ids (no leading/trailing slash). The only divergence is the
        // degenerate case of a leading slash (e.g. "/name"): the old code
        // dropped it ("name.toml") while `card_slug` preserves it ("--name.toml").
        // This never occurs with real HF repo_ids.
        let card_filename = format!("{}.toml", crate::models::card_slug(model_name));
        let card_path = configs_dir.join(card_filename);

        let content = tokio::fs::read_to_string(&card_path).await.ok()?;
        let model_toml: crate::models::ModelToml = toml::from_str(&content).ok()?;
        Some(model_toml)
    }

    /// Reload model configurations from the database.
    ///
    /// This ensures that the in-memory registry stays in sync with mutations
    /// made via the web API or CLI.
    pub async fn reload_model_configs(&self) -> anyhow::Result<()> {
        let pool = self.db_pool();
        let configs = crate::db::load_model_configs(&pool).await?;
        self.registry.reload_model_configs(configs).await;
        Ok(())
    }

    /// Read from the live config without exposing the lock. The closure runs
    /// under a read guard that is dropped before this method returns.
    pub async fn with_config<R>(&self, f: impl FnOnce(&crate::config::Config) -> R) -> R {
        let config = self.config.read().await;
        f(&config)
    }

    /// Mutate the live config without exposing the lock. Returns the closure's
    /// result (e.g. a cloned `Config` to persist) after the write guard drops.
    pub async fn with_config_mut<R>(&self, f: impl FnOnce(&mut crate::config::Config) -> R) -> R {
        let mut config = self.config.write().await;
        f(&mut config)
    }

    /// Replace the live config and refresh config-derived clients (Langfuse).
    /// Mirrors what the config PATCH endpoint did inline (`sync_proxy_config`).
    pub async fn replace_config(&self, new_config: crate::config::Config) {
        {
            let mut config = self.config.write().await;
            *config = new_config;
        }
        self.refresh_langfuse_client().await;
    }

    /// Refresh the Langfuse client from the current config.
    ///
    /// Called after config is updated via PATCH so that langfuse config changes
    /// (enabled flag, keys, host) take effect without requiring a Tama restart.
    /// If langfuse is disabled or credentials are missing in the new config,
    /// the client is set to None (disabling tracing).
    pub async fn refresh_langfuse_client(&self) {
        let langfuse_cfg = self.config.read().await.langfuse.clone();
        let new_client =
            crate::proxy::forward::langfuse::LangfuseClient::from_config(&langfuse_cfg)
                .map(Arc::new);
        let mut current = self.langfuse_client.write().await;
        *current = new_client;
    }

    /// Reload alias cache from the database.
    ///
    /// Populates the in-memory alias map with enabled aliases only.
    /// Disabled aliases are filtered out by the DB query.
    pub async fn reload_aliases(&self) -> anyhow::Result<()> {
        let pool = self.db_pool();
        let pairs = crate::db::queries::load_aliases_for_cache(&pool).await?;
        self.registry.reload_aliases(pairs).await;
        Ok(())
    }

    /// Resolve a model name through the alias registry.
    /// - If `name` is an alias → returns the resolved model name (api_name or repo_id)
    /// - If `name` is not an alias → returns `name` unchanged (pass-through)
    pub async fn resolve_alias(&self, name: &str) -> String {
        self.registry.resolve_alias(name).await
    }

    /// Open a ModelManager for model-related database operations.
    ///
    /// Crate-internal ModelManager factory for proxy lifecycle code
    /// (`PullQueueService`, reload paths). The `tama` API layer uses the
    /// shared pool from `WebState` (plan-160) — do NOT add new
    /// callers there.
    pub(crate) fn model_mgr(&self) -> crate::models::ModelManager {
        crate::models::ModelManager::new(self.db_pool.clone())
    }

    /// Return the names of all loaded backends that are TTS (text-to-speech) backends.
    pub async fn tts_backend_names(&self) -> Vec<String> {
        self.registry.tts_backend_names().await
    }

    /// Get a provider by name from the database.
    /// Returns None if not found or if DB is not configured.
    pub(crate) async fn get_provider(&self, name: &str) -> Option<crate::providers::Provider> {
        crate::db::queries::get_provider(&self.db_pool, name)
            .await
            .ok()
            .flatten()
    }

    /// Remove the local BackendState mirror entry (if any) for the model
    /// with this name (plan-191 Task 5). The mirror is a staging cache of
    /// the tamad's process table — keyed by config backend name, carrying
    /// the model name — so the lookup is by `model_name()`.
    pub async fn remove_mirror_by_model(&self, model_name: &str) {
        let mut models = self.registry.models.write().await;
        let stale: Vec<String> = models
            .iter()
            .filter(|(_, s)| s.model_name() == model_name)
            .map(|(k, _)| k.clone())
            .collect();
        for key in &stale {
            models.remove(key);
        }
        drop(models);
        self.metrics.modify_inference_stats(|map| {
            for key in &stale {
                map.remove(key);
            }
        });
        let pool = self.db_pool();
        for key in &stale {
            let _ = crate::db::queries::remove_active_model(&pool, key).await;
        }
    }

    /// Sync the local BackendState mirror with one tamad's live process
    /// snapshot (plan-191 Task 5, staging mirror):
    ///
    /// - upsert `Ready` entries for every live process (so the forward
    ///   path and the management API see live endpoints),
    /// - drop mirror entries for models on this tamad that are neither
    ///   alive nor desired.
    pub async fn sync_tamad_mirror(
        &self,
        processes: &[crate::tamad::ProcessInfo],
        desired: &[String],
    ) {
        let config = self.config.read().await.clone();

        // Build the upsert plan before taking the mirror lock (no nested
        // acquisition of registry locks).
        let model_configs = self.registry.model_configs.read().await;
        let mut upserts: Vec<(String, String, String, u32, String)> = Vec::new();
        for p in processes {
            if !p.alive {
                continue;
            }
            let Some(backend_name) = config
                .resolve_backends_for_model(&model_configs, &p.model_name)
                .into_iter()
                .next()
                .map(|(name, _, _)| name)
            else {
                continue;
            };
            let backend = model_configs
                .get(&backend_name)
                .map(|c| c.backend.clone())
                .unwrap_or_else(|| p.provider_name.clone());
            upserts.push((
                backend_name,
                backend,
                p.model_name.clone(),
                p.pid.max(0) as u32,
                p.endpoint_url.clone(),
            ));
        }
        drop(model_configs);

        let mut models = self.registry.models.write().await;

        // Upsert/update live processes.
        for (backend_name, backend, model_name, pid, url) in upserts {
            match models.get_mut(&backend_name) {
                Some(BackendState::Ready {
                    backend_pid,
                    backend_url,
                    ..
                }) => {
                    *backend_pid = pid;
                    if !url.is_empty() {
                        *backend_url = url;
                    }
                }
                _ => {
                    models.insert(
                        backend_name,
                        BackendState::Ready {
                            model_name,
                            backend,
                            backend_pid: pid,
                            backend_url: url,
                            load_time: std::time::SystemTime::now(),
                            last_accessed: std::time::Instant::now(),
                            consecutive_failures: std::sync::Arc::new(
                                std::sync::atomic::AtomicU32::new(0),
                            ),
                            failure_timestamp: None,
                            restart_count: 0,
                            is_docker: false,
                        },
                    );
                }
            }
        }

        // Drop mirror entries for models that are gone (dead or evicted)
        // and not desired.
        let mut stale: Vec<String> = Vec::new();
        for (key, s) in models.iter() {
            let model = s.model_name();
            let still_wanted = desired.iter().any(|d| d == model);
            let still_alive = processes.iter().any(|p| p.model_name == model && p.alive);
            if !still_wanted && !still_alive {
                stale.push(key.clone());
            }
        }
        for key in &stale {
            models.remove(key);
        }
        drop(models);

        self.metrics.modify_inference_stats(|map| {
            for key in &stale {
                map.remove(key);
            }
        });
        let pool = self.db_pool();
        for key in &stale {
            let _ = crate::db::queries::remove_active_model(&pool, key).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that `ProxyState::new` creates a metrics channel and that subscribing adds a receiver.
    #[tokio::test]
    async fn test_proxy_state_new_creates_metrics_channel() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None, crate::db::pool::test_dummy_pool());
        let _subscriber = state.metrics.subscribe_metrics();
        assert_eq!(state.metrics.metrics_tx.receiver_count(), 1);
    }

    // ── Alias cache tests ─────────────────────────────────────────────────────

    /// Test that resolve_alias returns the name unchanged when it is not an alias.
    #[tokio::test]
    async fn test_resolve_alias_pass_through() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None, crate::db::pool::test_dummy_pool());
        let result = state.resolve_alias("some-model-name").await;
        assert_eq!(result, "some-model-name");
    }

    /// Test that resolve_alias returns the resolved model name for a known alias.
    #[tokio::test]
    async fn test_resolve_alias_resolves() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None, crate::db::pool::test_dummy_pool());

        // Manually populate the alias cache
        {
            let mut aliases = state.registry.aliases.write().await;
            aliases.insert("my-alias".to_string(), "owner--real-model".to_string());
        }

        let result = state.resolve_alias("my-alias").await;
        assert_eq!(result, "owner--real-model");
    }

    /// Test that `with_config` / `with_config_mut` / `replace_config` work correctly.
    #[tokio::test]
    async fn test_with_config_and_replace_config() {
        let state = ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        );
        let port = state.with_config(|c| c.proxy.port).await;
        assert_eq!(port, crate::config::Config::default().proxy.port);
        let mut new_config = crate::config::Config::default();
        new_config.proxy.port = 19999;
        state.replace_config(new_config).await;
        assert_eq!(state.with_config(|c| c.proxy.port).await, 19999);
        state.with_config_mut(|c| c.proxy.port = 18888).await;
        assert_eq!(state.with_config(|c| c.proxy.port).await, 18888);
    }

    // `reload_aliases` disabled-filtering moved to the Postgres harness
    // (plan-190 Task 5): `crates/tama-core/tests/proxy_state_registry.rs`.
}
