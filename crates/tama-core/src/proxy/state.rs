use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::pull_queue::{queue_processor_loop, PullQueueService};
use super::types::{BackendState, ProxyMetrics, ProxyState};

impl ProxyState {
    pub fn new(config: crate::config::Config, db_dir: Option<std::path::PathBuf>) -> Self {
        let (metrics_tx, _) = tokio::sync::broadcast::channel(3);

        // Initialize Langfuse client from config before wrapping in Arc.
        let langfuse_client =
            crate::proxy::forward::langfuse::LangfuseClient::from_config(&config.langfuse)
                .map(Arc::new);

        // Initialize pull queue service if db_dir is configured.
        let poll_interval = config.proxy.download_queue_poll_interval_secs;
        let pull_queue = db_dir.as_ref().and_then(|dir| {
            crate::models::ModelManager::open(dir)
                .ok()
                .map(|mm| Arc::new(PullQueueService::new(mm, poll_interval)))
        });

        let state = Self {
            config: Arc::new(tokio::sync::RwLock::new(config)),
            model_configs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            aliases: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            models: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
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
            metrics: Arc::new(ProxyMetrics::default()),
            db_dir,
            pull_jobs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            system_metrics: Arc::new(tokio::sync::RwLock::new(
                crate::gpu::SystemMetrics::default(),
            )),
            in_flight_pulls: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            metrics_tx,
            pull_queue: pull_queue.clone(),
            config_write_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            backend_logs: crate::backends::log_stream::BackendLogManager::default(),
            inference_stats: tokio::sync::watch::channel(std::collections::HashMap::new()).0,
            gpu_devices_cache: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            model_tasks: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            cookie_key: cookie::Key::generate(),
            langfuse_client: Arc::new(tokio::sync::RwLock::new(langfuse_client)),
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
    pub async fn get_backend_url(&self, backend_name: &str) -> Result<String> {
        let config = self.config.read().await;
        let model_configs = self.model_configs.read().await;
        let backend_config = config
            .resolve_backend(&model_configs, backend_name)
            .with_context(|| format!("Backend '{}' not found", backend_name))?
            .0;

        // Open BackendManager for health_check_url lookup
        let manager = self
            .db_dir
            .as_ref()
            .and_then(|dir| crate::backends::BackendManager::open(dir).ok())
            .unwrap_or_else(|| {
                crate::backends::BackendManager::open_in_memory()
                    .expect("in-memory BackendManager must always open")
            });
        let gpu_variant = backend_config
            .gpu_variant
            .clone()
            .unwrap_or(crate::gpu::GpuType::CpuOnly);
        let health_url =
            manager.get_health_check_url(&backend_config.backend, gpu_variant.variant_folder());
        let backend_url = config
            .resolve_backend_url(backend_config, health_url.as_deref())
            .with_context(|| format!("No backend URL resolved for backend '{}'", backend_name))?;

        Ok(backend_url)
    }

    /// Get the state of a loaded model (backend).
    pub async fn get_model_state(&self, backend_name: &str) -> Option<BackendState> {
        let models = self.models.read().await;
        models.get(backend_name).cloned()
    }

    /// Find an available loaded backend for a given model name.
    pub async fn get_available_backend_for_model(&self, model_name: &str) -> Option<String> {
        let (backend_names, circuit_breaker_threshold) = {
            let config = self.config.read().await;
            let model_configs = self.model_configs.read().await;
            // Collect just the backend names (owned Strings) so we can drop the lock.
            let names: Vec<String> = config
                .resolve_backends_for_model(&model_configs, model_name)
                .into_iter()
                .map(|(name, _, _)| name)
                .collect();
            let threshold = config.proxy.circuit_breaker_threshold;
            (names, threshold)
        };

        let models = self.models.read().await;

        // Simple round-robin or first available
        for backend_name in backend_names {
            if let Some(state) = models.get(&backend_name) {
                if (state.is_ready() || matches!(state, BackendState::Starting { .. }))
                    && state
                        .consecutive_failures()
                        .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(0)
                        < circuit_breaker_threshold
                {
                    return Some(backend_name);
                }
            }
        }

        None
    }

    /// Update the last accessed time for a backend.
    pub async fn update_last_accessed(&self, backend_name: &str) {
        let mut models = self.models.write().await;
        if let Some(state) = models.get_mut(backend_name) {
            match state {
                BackendState::Starting { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                BackendState::Ready { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                BackendState::Unloading { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                BackendState::Failed { .. } => {}
            }
        }
    }

    /// Get the model card for a model name.
    pub async fn get_model_card(&self, model_name: &str) -> Option<crate::models::card::ModelCard> {
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
        let card: crate::models::card::ModelCard = toml::from_str(&content).ok()?;
        Some(card)
    }

    /// Reload model configurations from the database.
    ///
    /// This ensures that the in-memory registry stays in sync with mutations
    /// made via the web API or CLI.
    pub async fn reload_model_configs(&self) -> Result<()> {
        let mgr = self
            .model_mgr()
            .with_context(|| "Database directory not configured")?;
        let configs = crate::db::load_model_configs(mgr.conn())?;
        let mut model_configs = self.model_configs.write().await;
        *model_configs = configs;
        Ok(())
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
    pub async fn reload_aliases(&self) -> Result<()> {
        let mgr = self
            .model_mgr()
            .with_context(|| "Database directory not configured")?;
        let pairs = crate::db::queries::load_aliases_for_cache(mgr.conn())?;
        let mut aliases = self.aliases.write().await;
        *aliases = pairs.into_iter().collect();
        Ok(())
    }

    /// Resolve a model name through the alias registry.
    /// - If `name` is an alias → returns the resolved model name (api_name or repo_id)
    /// - If `name` is not an alias → returns `name` unchanged (pass-through)
    pub async fn resolve_alias(&self, name: &str) -> String {
        let aliases = self.aliases.read().await;
        if let Some(resolved) = aliases.get(name) {
            return resolved.clone();
        }
        name.to_string()
    }

    /// Open a ModelManager for model-related database operations.
    ///
    /// Returns `None` if `db_dir` is not configured (e.g., in tests).
    ///
    /// Each call opens a fresh `ModelManager` (and thus a fresh `rusqlite::Connection`).
    /// This is deliberate: `Connection` is `Send` but not `Sync`, so we cannot
    /// share a single instance across threads via `Arc`. For persistent reuse,
    /// see `PullQueueService` which wraps `ModelManager` in `Mutex`.
    pub fn model_mgr(&self) -> Option<crate::models::ModelManager> {
        self.db_dir
            .as_ref()
            .and_then(|dir| crate::models::ModelManager::open(dir).ok())
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
    ) -> Result<Vec<crate::gpu::GpuDeviceInfo>> {
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
    ) -> Result<Vec<crate::gpu::GpuDeviceInfo>> {
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

    /// Resolve the binary path for a backend name using the same logic as `load_model`.
    ///
    /// Opens the BackendManager, looks up the active installation, and returns the path.
    /// Falls back to config.path if no DB entry exists.
    pub async fn resolve_backend_binary_path(
        &self,
        backend_name: &str,
        gpu_variant: Option<&crate::gpu::GpuType>,
    ) -> Result<std::path::PathBuf> {
        let config = self.config.read().await;
        let manager = self
            .db_dir
            .as_ref()
            .and_then(|dir| crate::backends::BackendManager::open(dir).ok())
            .unwrap_or_else(|| {
                crate::backends::BackendManager::open_in_memory()
                    .expect("in-memory BackendManager must always open")
            });

        config.resolve_backend_path(backend_name, gpu_variant, &manager)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that `ProxyState::new` creates a metrics channel and that subscribing adds a receiver.
    #[test]
    fn test_proxy_state_new_creates_metrics_channel() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);
        let _subscriber = state.metrics_tx.subscribe();
        assert_eq!(state.metrics_tx.receiver_count(), 1);
    }

    // ── Alias cache tests ─────────────────────────────────────────────────────

    /// Test that resolve_alias returns the name unchanged when it is not an alias.
    #[tokio::test]
    async fn test_resolve_alias_pass_through() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);
        let result = state.resolve_alias("some-model-name").await;
        assert_eq!(result, "some-model-name");
    }

    /// Test that resolve_alias returns the resolved model name for a known alias.
    #[tokio::test]
    async fn test_resolve_alias_resolves() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);

        // Manually populate the alias cache
        {
            let mut aliases = state.aliases.write().await;
            aliases.insert("my-alias".to_string(), "owner--real-model".to_string());
        }

        let result = state.resolve_alias("my-alias").await;
        assert_eq!(result, "owner--real-model");
    }

    /// Test that reload_aliases populates the cache from the database.
    #[tokio::test]
    async fn test_reload_aliases_populates_cache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, Some(temp_dir.path().to_path_buf()));

        // Insert a model config and an alias directly into the DB
        let mgr = state.model_mgr().expect("DB should be configured");
        let conn = mgr.conn();
        conn.execute(
            "INSERT INTO model_configs (repo_id, api_name, backend) VALUES (?, ?, ?)",
            rusqlite::params!["test-owner/test-model", "TestModel", "llama_cpp"],
        )
        .unwrap();
        let model_id: i64 = conn
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO model_aliases (name, model_id, enabled) VALUES (?, ?, 1)",
            rusqlite::params!["short-name", model_id],
        )
        .unwrap();

        // Cache should be empty before reload
        assert!(state.aliases.read().await.is_empty());

        // Reload
        state.reload_aliases().await.unwrap();

        // Cache should now contain the alias
        let aliases = state.aliases.read().await;
        assert!(
            aliases.contains_key("short-name"),
            "alias 'short-name' should be in cache"
        );
        assert_eq!(
            aliases.get("short-name"),
            Some(&"TestModel".to_string()),
            "resolved name should be the api_name"
        );
    }

    /// Test that disabled aliases are not included in the cache after reload.
    #[tokio::test]
    async fn test_reload_aliases_filters_disabled() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, Some(temp_dir.path().to_path_buf()));

        // Insert a model config and two aliases (one enabled, one disabled)
        let mgr = state.model_mgr().expect("DB should be configured");
        let conn = mgr.conn();
        conn.execute(
            "INSERT INTO model_configs (repo_id, api_name, backend) VALUES (?, ?, ?)",
            rusqlite::params!["owner/model1", "ModelOne", "llama_cpp"],
        )
        .unwrap();
        let model_id: i64 = conn
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO model_aliases (name, model_id, enabled) VALUES (?, ?, 1)",
            rusqlite::params!["enabled-alias", model_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_aliases (name, model_id, enabled) VALUES (?, ?, 0)",
            rusqlite::params!["disabled-alias", model_id],
        )
        .unwrap();

        // Reload
        state.reload_aliases().await.unwrap();

        let aliases = state.aliases.read().await;
        assert!(
            aliases.contains_key("enabled-alias"),
            "enabled alias should be in cache"
        );
        assert!(
            !aliases.contains_key("disabled-alias"),
            "disabled alias should NOT be in cache"
        );
    }
}
