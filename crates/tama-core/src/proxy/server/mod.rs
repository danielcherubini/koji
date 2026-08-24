pub mod listener;
pub mod metrics;
pub mod router;

use crate::proxy::ProxyState;
use std::sync::Arc;

/// The proxy server, owning shared state and background tasks.
pub struct ProxyServer {
    state: Arc<ProxyState>,
    /// Handle for the idle timeout checker task. Kept to prevent task cancellation.
    #[allow(dead_code)]
    idle_timeout_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle for the system metrics collection task. Kept to prevent task cancellation.
    #[allow(dead_code)]
    metrics_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyServer {
    /// Create a new proxy server with the given shared state.
    ///
    /// Starts a background task that periodically checks for idle models
    /// and unloads them.
    pub async fn new(state: Arc<ProxyState>) -> Self {
        // Populate in-memory model registry from Postgres (plan-190 Task 5)
        let pool = state.db_pool.as_ref();
        // Repair model_configs rows whose model_files were wiped by the
        // v9 FK-cascade bug. No-op when rows are intact.
        {
            let config = state.config.read().await;
            match config.models_dir() {
                Ok(models_dir) => {
                    if let Err(e) =
                        crate::db::backfill::repair_orphaned_model_files(pool, &models_dir).await
                    {
                        tracing::warn!("repair_orphaned_model_files failed: {}", e);
                    }
                }
                Err(e) => tracing::debug!("models_dir unavailable for repair scan: {}", e),
            }
        }

        match crate::db::load_model_configs(pool).await {
            Ok(db_models) if !db_models.is_empty() => {
                tracing::info!("Loaded {} models from database", db_models.len());
                *state.registry.model_configs.write().await = db_models;
            }
            Ok(_) => {}
            Err(e) => tracing::error!("Failed to load model configs from database: {}", e),
        }

        // Load aliases into the in-memory cache.
        // Without this, /v1/models and /v1/opencode/models return zero aliases
        // because the cache is never populated at startup.
        match crate::db::queries::load_aliases_for_cache(pool).await {
            Ok(pairs) => {
                if !pairs.is_empty() {
                    tracing::info!("Loaded {} aliases from database", pairs.len());
                }
                *state.registry.aliases.write().await = pairs.into_iter().collect();
            }
            Err(e) => tracing::error!("Failed to load aliases from database: {}", e),
        }

        // Check if any models need HF metadata backfill (after migration v19).
        // If so, spawn a background task to fetch and populate the columns.
        let needs_backfill: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM model_configs WHERE hf_format IS NULL")
                .fetch_one(pool)
                .await
                .unwrap_or(0);
        if needs_backfill > 0 {
            let pool = state.db_pool.clone();
            // Fire-and-forget background task — doesn't block startup.
            // JoinHandle is intentionally dropped; backfill errors are logged internally.
            let _handle = tokio::spawn(async move {
                if let Err(e) = crate::db::backfill::backfill_hf_metadata(&pool).await {
                    tracing::warn!("HF metadata backfill failed: {}", e);
                }
            });
        }

        // Check if any models need vLLM config backfill (args → vllm_config migration).
        // Intentionally NOT in run_initial_backfill because that is first-run-only
        // and would never fire on existing databases.
        // Gate: args contains at least one managed vLLM flag.
        let needs_vllm_backfill: i64 =
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM model_configs WHERE hf_format = 'transformers' AND (\n                     args LIKE '%--quantization%' OR \
                 args LIKE '%--kv-cache-dtype%' OR \
                 args LIKE '%--tensor-parallel-size%' OR \
                 args LIKE '%--gpu-memory-utilization%' OR \
                 args LIKE '%--max-model-len%' OR \
                 args LIKE '%--max-num-batched-tokens%' OR \
                 args LIKE '%--enable-prefix-caching%' OR \
                 args LIKE '%--trust-remote-code%')",
            )
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        if needs_vllm_backfill > 0 {
            let pool = state.db_pool.clone();
            // Fire-and-forget background task — doesn't block startup.
            // JoinHandle is intentionally dropped; backfill errors are logged internally.
            let _handle = tokio::spawn(async move {
                if let Err(e) = crate::db::backfill::backfill_vllm_config(&pool).await {
                    tracing::warn!("vLLM config backfill failed: {}", e);
                }
            });
        }

        // Note: dead-process cleanup and Docker container reconciliation were
        // removed in plan-191 Task 10 — the proxy no longer sees local backends
        // (ADR-0010). The proxy never spawns backends: each tamad's process table
        // the desired set, and the idle checker cleans up Failed mirror entries.
        let idle_timeout_handle = Self::start_idle_timeout_checker(state.clone());

        // Spawn background task to refresh system metrics every 2s.
        let metrics_handle = metrics::start_metrics_collector(state.clone());

        Self {
            state,
            idle_timeout_handle: Some(idle_timeout_handle),
            metrics_handle: Some(metrics_handle),
        }
    }

    /// Spawn the idle timeout checker task.
    /// Always spawns — the task reads config each iteration and respects runtime
    /// changes to auto_unload (e.g., via web UI) without requiring a restart.
    /// check_idle_timeouts is always called so Failed backends get cleaned up
    /// even when auto_unload is disabled; the idle-unload logic inside it is
    /// gated on the auto_unload flag.
    fn start_idle_timeout_checker(state: Arc<ProxyState>) -> tokio::task::JoinHandle<()> {
        use std::time::Duration;

        tokio::spawn(async move {
            loop {
                // Re-read config each iteration so runtime changes (e.g., via web UI)
                // take effect without a restart.
                let idle_timeout_secs = state.config.read().await.proxy.idle_timeout_secs;
                let interval = if idle_timeout_secs > 0 {
                    Duration::from_secs((idle_timeout_secs / 2).max(1))
                } else {
                    Duration::from_secs(30)
                };
                tokio::time::sleep(interval).await;
                // Always called — cleans up Failed backends even when auto_unload is off.
                let _ = state.check_idle_timeouts().await;
            }
        })
    }

    /// Consume the server and return a configured axum Router.
    pub async fn into_router(self) -> axum::Router {
        router::build_router(self.state).await
    }

    /// Consume the server and return a unified axum Router that merges
    /// proxy routes with extra routes (e.g., web UI routes from `tama-web`).
    ///
    /// Proxy-specific routes are defined before extra routes to ensure
    /// correct route priority in axum.
    ///
    /// # Example
    /// ```ignore
    /// let web_routes = tama_web::router::build_web_routes();
    /// let app = server.into_unified_router(web_routes);
    /// ```
    #[cfg(feature = "web-ui")]
    pub async fn into_unified_router(
        self,
        extra_routes: axum::Router<Arc<ProxyState>>,
    ) -> axum::Router {
        router::build_unified_router(self.state, extra_routes).await
    }

    /// Start serving on the given address.
    ///
    /// Builds the router and delegates to the listener module.
    /// If `shutdown_tx` is provided, the shutdown signal is broadcast to
    /// other servers (e.g. the web UI) so they shut down simultaneously.
    pub async fn run(
        self,
        addr: std::net::SocketAddr,
        shutdown_tx: Option<tokio::sync::watch::Sender<()>>,
    ) -> anyhow::Result<()> {
        // Clone state for shutdown cleanup (unloads TTS backends)
        let cleanup_state = Arc::clone(&self.state);
        let app = self.into_router().await;
        let on_shutdown = async move {
            let live = crate::proxy::live_rows(cleanup_state.tamad_pool().as_ref()).await;
            let tts_backs: Vec<String> = live
                .all()
                .iter()
                .filter(|r| r.key.starts_with("tts_"))
                .map(|r| r.key.clone())
                .collect();
            for name in tts_backs {
                if let Err(e) = cleanup_state.unload_model(&name).await {
                    tracing::warn!("Failed to unload TTS backend '{}': {}", name, e);
                }
            }
        };
        listener::run(app, addr, Some(on_shutdown), shutdown_tx).await
    }
}

#[cfg(test)]
mod tests;
