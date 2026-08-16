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
        db_pool: Option<Arc<sqlx::PgPool>>,
    ) -> Self {
        // Initialize Langfuse client from config before wrapping in Arc.
        let langfuse_client =
            crate::proxy::forward::langfuse::LangfuseClient::from_config(&config.langfuse)
                .map(Arc::new);

        // Initialize pull queue service when a Postgres pool is configured.
        let poll_interval = config.proxy.pull_queue_poll_interval_secs;
        let pull_queue = db_pool
            .as_ref()
            .map(|pool| Arc::new(PullQueueService::new(pool.clone(), poll_interval)));

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
            gpu_devices_cache: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            model_tasks: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            cookie_key: cookie::Key::generate(),
            langfuse_client: Arc::new(tokio::sync::RwLock::new(langfuse_client)),
            remote_forwarder: crate::proxy::remote::RemoteForwarder::new(),
            tamad_clients: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool,
        };

        // Spawn the queue processor background task if pull queue is configured.
        // This must be called from within a tokio runtime context (which is always true
        // in practice since ProxyState::new is only called from async functions).
        if let Some(ref _dq) = pull_queue {
            let state_clone = Arc::new(state.clone());
            tokio::spawn(async move {
                queue_processor_loop(state_clone).await;
            });
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
        let health_url = match self.db_pool() {
            Some(pool) => {
                let manager = crate::installations::InstallationManager::new(pool);
                manager
                    .get_health_check_url(&backend_config.backend, gpu_variant.variant_folder())
                    .await
            }
            None => None,
        };
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
        let pool = self
            .db_pool()
            .with_context(|| "Postgres pool not configured")?;
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
        let pool = self
            .db_pool()
            .with_context(|| "Postgres pool not configured")?;
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
    /// Returns `None` if the Postgres pool is not configured (e.g., in tests).
    ///
    /// Crate-internal ModelManager factory for proxy lifecycle code
    /// (`PullQueueService`, reload paths). The `tama` API layer uses the
    /// shared pool from `WebState` (plan-160) — do NOT add new
    /// callers there.
    pub(crate) fn model_mgr(&self) -> Option<crate::models::ModelManager> {
        self.db_pool
            .as_ref()
            .map(|pool| crate::models::ModelManager::new(pool.clone()))
    }

    /// Get cached GPU devices for a backend, or discover them on first access.
    ///
    /// Returns cached results if available (no TTL — refresh manually via
    /// `refresh_gpu_devices`). Runs `spawn_blocking` subprocess call on first hit.
    /// Cache key is `"{backend_name}:{gpu_variant}"`.
    pub async fn get_or_discover_gpu_devices(
        &self,
        backend_name: &str,
        gpu_variant: &str,
        binary_path: &std::path::Path,
    ) -> anyhow::Result<Vec<crate::gpu::GpuDeviceInfo>> {
        let cache_key = format!("{}:{}", backend_name, gpu_variant);
        // Check cache first
        {
            let cache = self.gpu_devices_cache.read().await;
            if let Some((_, devices)) = cache.get(&cache_key) {
                return Ok(devices.clone());
            }
        }

        // Not cached — discover
        self.refresh_gpu_devices(backend_name, gpu_variant, binary_path)
            .await
    }

    /// Force re-discovery of GPU devices for a backend.
    ///
    /// Runs `<binary> --list-devices` in a blocking task, parses output,
    /// and stores results in the cache. Cache key is `"{backend_name}:{gpu_variant}"`.
    pub async fn refresh_gpu_devices(
        &self,
        backend_name: &str,
        gpu_variant: &str,
        binary_path: &std::path::Path,
    ) -> anyhow::Result<Vec<crate::gpu::GpuDeviceInfo>> {
        let binary_path = binary_path.to_path_buf();
        let cache_key = format!("{}:{}", backend_name, gpu_variant);

        let devices = tokio::task::spawn_blocking(move || {
            crate::gpu::discover_devices_via_binary(&binary_path)
        })
        .await
        .context("spawn_blocking panicked")??;

        let now = std::time::Instant::now();
        let mut cache = self.gpu_devices_cache.write().await;
        cache.insert(cache_key, (now, devices.clone()));

        Ok(devices)
    }

    /// Return the names of all loaded backends that are TTS (text-to-speech) backends.
    pub async fn tts_backend_names(&self) -> Vec<String> {
        self.registry.tts_backend_names().await
    }

    /// Get a provider by name from the database.
    /// Returns None if not found or if DB is not configured.
    pub(crate) async fn get_provider(&self, name: &str) -> Option<crate::providers::Provider> {
        let pool = self.db_pool()?;
        crate::db::queries::get_provider(&pool, name)
            .await
            .ok()
            .flatten()
    }

    /// Resolve the binary path for a backend name using the same logic as `load_model`.
    ///
    /// Looks up the active installation via the Postgres pool, and returns the path.
    /// Falls back to config.path if no DB entry exists (or no pool is configured).
    pub async fn resolve_backend_binary_path(
        &self,
        backend_name: &str,
        gpu_variant: Option<&crate::gpu::GpuVariant>,
    ) -> anyhow::Result<std::path::PathBuf> {
        let config = self.config.read().await;
        let manager = self
            .db_pool()
            .map(crate::installations::InstallationManager::new);

        config
            .resolve_backend_path(backend_name, gpu_variant, manager.as_ref())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that `ProxyState::new` creates a metrics channel and that subscribing adds a receiver.
    #[test]
    fn test_proxy_state_new_creates_metrics_channel() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None, None);
        let _subscriber = state.metrics.subscribe_metrics();
        assert_eq!(state.metrics.metrics_tx.receiver_count(), 1);
    }

    // ── Alias cache tests ─────────────────────────────────────────────────────

    /// Test that resolve_alias returns the name unchanged when it is not an alias.
    #[tokio::test]
    async fn test_resolve_alias_pass_through() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None, None);
        let result = state.resolve_alias("some-model-name").await;
        assert_eq!(result, "some-model-name");
    }

    /// Test that resolve_alias returns the resolved model name for a known alias.
    #[tokio::test]
    async fn test_resolve_alias_resolves() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None, None);

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
        let state = ProxyState::new(crate::config::Config::default(), None, None);
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
