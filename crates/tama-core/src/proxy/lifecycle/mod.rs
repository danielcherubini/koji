//! Proxy-side model lifecycle (plan-191 Tasks 5 and 10).
//!
//! After Task 10 the proxy is a pure control plane (ADR-0010, enforced by
//! the dependency graph): this module only *resolves* launch specs from the
//! central DB and *dispatches* them to the model's provider tamad. All
//! process spawning, health polling, killing, and host sampling live in the
//! tamad crate. The proxy keeps a staging `BackendState` mirror of the
//! tamad's process table (see `ProxyState::sync_tamad_mirror`) so the
//! forward path and management API see live endpoints.

use anyhow::{anyhow, Result};
use tracing::{debug, info, warn};

use super::types::{BackendState, ProxyState};

mod idle_timeout;

pub mod spec;

/// Ensure a model is loaded and return its backend name.
///
/// Shared flow used by multiple handlers: resolve alias → remote-provider
/// check (sentinel) → already-loaded mirror check → load via the model's
/// provider's tamad (plan-191 Task 5) → update last_accessed.
///
/// The proxy never spawns the backend process itself (ADR-0010): the launch
/// spec is resolved from the central DB, sent to the tamad via `LoadModel`,
/// and the model is marked *desired* so the reconciler keeps it alive.
///
/// Callers provide an `on_load_error` closure to handle the case where loading
/// fails (e.g., returning an error response). The closure receives the
/// resolved model name and the error, and returns the fallback backend name
/// (or an error if no fallback is possible).
pub async fn ensure_model_loaded(
    state: &std::sync::Arc<ProxyState>,
    model_name: &str,
    on_load_error: impl FnOnce(&str, anyhow::Error) -> Result<String> + Send,
) -> Result<String> {
    // Resolve alias before routing
    let resolved_model = state.resolve_alias(model_name).await;

    // Check if model has a provider_name that resolves to a remote provider.
    // When set, this overrides the `backend` field for routing.
    let provider_name: Option<String> = {
        let model_configs = state.registry.model_configs.read().await;
        model_configs
            .get(&resolved_model)
            .and_then(|c| c.provider_name.clone())
    };

    if let Some(ref name) = provider_name {
        if let Some(provider) = state.get_provider(name).await {
            if provider.provider_type.is_remote() {
                // Return sentinel indicating remote provider
                return Ok(format!("remote:{}", provider.id));
            }
        }
    }

    // Already loaded (staging mirror of the tamad's process table) → fast
    // path: no RPC, no desired write.
    if let Some(backend_name) = state.get_available_backend_for_model(&resolved_model).await {
        state.update_last_accessed(&backend_name).await;
        return Ok(backend_name);
    }

    // Load via the model's provider's tamad (the proxy spawns nothing).
    let backend_name = match spec::load_model_on_tamad(state, &resolved_model).await {
        Ok(name) => name,
        Err(e) => on_load_error(&resolved_model, e)?,
    };

    state.update_last_accessed(&backend_name).await;
    Ok(backend_name)
}

impl ProxyState {
    // ─── Other public methods ──────────────────────────────────────

    /// Evict the least-recently-used Ready model on the target GPU if the proxy
    /// is at capacity for that device.
    pub async fn evict_lru_if_needed(
        &self,
        target_gpu_device: Option<String>,
    ) -> Result<Option<String>> {
        let config = self.config.read().await;
        let max = config.proxy.max_loaded_models;

        if max == 0 {
            return Ok(None);
        }

        let models = self.registry.models.write().await;
        let ready_backends: Vec<String> = models
            .iter()
            .filter(|(_, s)| matches!(s, BackendState::Ready { .. }))
            .map(|(name, _)| name.clone())
            .collect();

        let non_inference_backends: std::collections::HashSet<String> = models
            .iter()
            .filter(|(_, s)| s.is_non_inference_backend())
            .map(|(name, _)| name.clone())
            .collect();

        drop(models);

        let model_configs = self.registry.model_configs.read().await;
        let llm_count = ready_backends
            .iter()
            .filter(|backend_name| {
                if model_configs
                    .get(backend_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    || non_inference_backends.contains(backend_name.as_str())
                {
                    return false;
                }
                let model_gpu = model_configs
                    .get(backend_name.as_str())
                    .and_then(|mc| mc.gpu_device.as_ref());
                model_gpu == target_gpu_device.as_ref()
            })
            .count();

        if llm_count < max as usize {
            return Ok(None);
        }

        let mut models = self.registry.models.write().await;
        let lru = ready_backends
            .iter()
            .filter(|backend_name| {
                if model_configs
                    .get(backend_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    || non_inference_backends.contains(backend_name.as_str())
                {
                    return false;
                }
                let model_gpu = model_configs
                    .get(backend_name.as_str())
                    .and_then(|mc| mc.gpu_device.as_ref());
                model_gpu == target_gpu_device.as_ref()
            })
            .filter_map(|backend_name| models.get(backend_name).map(|s| (backend_name, s)))
            .min_by_key(|(_, s)| s.last_accessed())
            .map(|(name, s)| (name.to_string(), s.model_name().to_string()));

        if let Some((ref name, _)) = lru {
            if let Some(state) = models.get_mut(name) {
                if let BackendState::Ready {
                    model_name,
                    backend,
                    backend_pid,
                    backend_url,
                    last_accessed,
                    consecutive_failures,
                    failure_timestamp,
                    restart_count,
                    load_time: _,
                    is_docker,
                } = std::mem::take(state)
                {
                    *state = BackendState::Unloading {
                        model_name,
                        backend,
                        backend_pid,
                        backend_url,
                        last_accessed,
                        consecutive_failures,
                        failure_timestamp,
                        restart_count,
                        is_docker,
                    };
                }
            }
        }

        drop(models);

        if let Some((name, _model_name)) = lru {
            // The model is being evicted; unload it locally. Lifecycle
            // truth now comes from the live tamad rows, so there is no
            // desired/desired_models row to clear here.
            self.unload_model(&name).await?;
            Ok(Some(name))
        } else {
            Ok(None)
        }
    }

    /// Unload a backend (plan-191 Task 5 re-route): the *physical* kill
    /// happens on the model's provider tamad via `UnloadModel`; the proxy
    /// clears its own state (mirror, inference stats, active_models row).
    ///
    /// The tamad RPC is best-effort: on failure the local state is cleared
    /// anyway and the reconciler converges the tamad's process table to the
    /// desired set on its next tick.
    pub async fn unload_model(&self, backend_name: &str) -> Result<()> {
        debug!("Unloading backend: {}", backend_name);

        // plan-193 T4 flip: first confirm the model is currently live on the
        // wire (no host => nothing to unload). The mirror's richer state is
        // no longer the source for this presence check.
        let live = crate::proxy::live_rows(self.tamad_pool().as_ref()).await;
        let row = live.row(backend_name);
        if row.is_none() {
            anyhow::bail!("Backend '{}' not loaded", backend_name);
        }
        let row = row.unwrap();
        if row.status != "ready" && row.status != "starting" && row.status != "restarting" {
            return Err(anyhow!(
                "Backend '{}' is not ready (state: {})",
                backend_name,
                row.status
            ));
        }
        let model_name = backend_name.to_string();

        // Physical unload on the tamad (best-effort).
        match spec::unload_model_on_tamad(self, &model_name).await {
            Ok(true) => info!(model = %model_name, "backend unloaded on tamad"),
            Ok(false) => debug!(model = %model_name, "backend not loaded on tamad"),
            Err(e) => warn!(
                model = %model_name,
                error = %e,
                "UnloadModel RPC failed; clearing local state anyway (reconciler will converge)"
            ),
        }

        // Remove from the mirror.
        let mut models = self.registry.models.write().await;
        models.remove(backend_name);

        // Clear stale inference stats.
        self.metrics.modify_inference_stats(|map| {
            map.remove(backend_name);
        });

        // Best-effort DB cleanup.
        let pool = self.db_pool();
        let _ = crate::db::queries::remove_active_model(&pool, backend_name).await;

        info!("Backend '{}' unloaded", backend_name);
        self.metrics
            .counters
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
