//! Proxy-side model lifecycle (plan-191 Tasks 5 and 10).
//!
//! After Task 10 the proxy is a pure control plane (ADR-0010, enforced by
//! the dependency graph): this module only *resolves* launch specs from the
//! central DB and *dispatches* them to the model's provider tamad. All
//! process spawning, health polling, killing, and host sampling live in the
//! tamad crate. The forward path and management API see live model state
//! from the tamads' process tables over the 1 Hz wire (plan 193 T4+
//! T5, `crate::proxy::live_rows`-backed reads), not from any local cache.

use anyhow::{anyhow, Result};
use tracing::{debug, info, warn};

use super::types::ProxyState;

mod idle_timeout;

pub mod spec;

/// Marked error: the model's tamad-side restart budget is exhausted.
/// Its `budget_exhausted` row is the tamad's signal that it will not
/// respawn the model for ~60s (plan-193 T5c).
///
/// Typed variant (not a string match): `ensure_model_loaded` returns it
/// in its `Err` arm when the row is `budget_exhausted`; the HTTP
/// callers check `err.is::<BudgetExhausted>()` and map it to
/// [`budget_exhausted_response`]. No handler may recover the mark
/// from error text, and the wire string 'the model exhausted its
/// restarts; retry in 60 seconds' is only ever built inside
/// [`budget_exhausted_response`].
#[derive(Debug)]
pub struct BudgetExhausted;

impl std::fmt::Display for BudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("model restart budget exhausted; retry in 60 seconds")
    }
}

impl std::error::Error for BudgetExhausted {}

/// Build the HTTP 503 response returned when a model is in the tamad's
/// `budget_exhausted` lifecycle state (plan-193 T5c): its restart budget is
/// spent, so the tamad will refuse to respawn it for ~60s. The body and
/// `retry-after` header are part of the wire contract.
pub fn budget_exhausted_response() -> axum::response::Response {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
        .header("retry-after", "60")
        .body(axum::body::Body::from(
            "the model exhausted its restarts; retry in 60 seconds",
        ))
        .unwrap()
}

/// The HTTP-layer transformation for a typed mark: `Some(503)` when
/// `err` is the `BudgetExhausted` variant (a typed check — never a
/// string match), otherwise `None`. If possible, we go through the same
/// fn (the forward/chat handler and unit tests below); the marker stays a
/// single source.
pub fn budget_exhausted_response_for(err: &anyhow::Error) -> Option<axum::response::Response> {
    err.is::<BudgetExhausted>().then(budget_exhausted_response)
}

/// Ensure a model is loaded and return its backend name.
///
/// Shared flow used by multiple handlers: resolve alias → remote-provider
/// check (sentinel) → already-loaded mirror check → load via the model's
/// provider's tamad (plan-191 Task 5) → update last_accessed.
///
/// The proxy never spawns the backend process itself (ADR-0010): the launch
/// spec is resolved from the central DB, sent to the tamad via `LoadModel`,
/// and the model is marked *desired* (host-side lifetime, ADR-0011).
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

    // plan-193 T5c: a model whose restart budget is exhausted stays on
    // the wire as a `budget_exhausted` row (the tamad holds that state
    // with the process dead, re-warming in ~60s). It cannot respawn:
    // surface 503 + retry-after via the typed BudgetExhausted mark
    // instead of looping a load.
    {
        let rows = crate::proxy::live_rows(state.tamad_pool().as_ref()).await;
        if rows
            .row(&resolved_model)
            .is_some_and(|r| r.status == "budget_exhausted")
        {
            return Err(anyhow::Error::new(BudgetExhausted));
        }
    }

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

    // Already loaded (the live wire row says so) → fast path: no RPC,
    // no desired write.
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

        // Live wire rows are the lifecycle truth (plan-193 T4): evict
        // from the live eligible rows, not from a staging mirror. The LRU
        // key is the row's `last_seen_ms` (wire recency) — the mirror's
        // request `last_accessed` disappeared with the mirror.
        let model_configs = self.registry.model_configs.read().await;
        let rows = crate::proxy::live_rows(self.tamad_pool().as_ref()).await;
        let candidates: Vec<&crate::proxy::ModelRow> = rows
            .all()
            .iter()
            .filter(|r| r.status == "ready")
            .filter(|r| {
                let mc = model_configs.get(&r.key);
                let backend = mc.map(|c| c.backend.as_str()).unwrap_or("");
                if backend.starts_with("tts_") || backend == "compaction" {
                    return false;
                }
                mc.and_then(|c| c.gpu_device.as_ref()) == target_gpu_device.as_ref()
            })
            .collect();

        if candidates.len() < max as usize {
            return Ok(None);
        }

        // Least-recently-ACCESSED first (per-key access map; plan 193
        // T5c): a candidate with no access entry never touched the
        // proxy and sits LRU-frontmost. Ties break on the wire
        // recency (`last_seen_ms`), then the key — orderings that
        // differ across racing concurrent callers, so two
        // simultaneous calls at capacity do not double-evict the
        // same model.
        let lru = self.registry.last_accessed.read().await;
        let mut ordered = candidates;
        ordered.sort_by(|a, b| {
            let ka = lru.get(&a.key).cloned();
            let kb = lru.get(&b.key).cloned();
            if ka == kb {
                a.last_seen_ms
                    .cmp(&b.last_seen_ms)
                    .then_with(|| a.key.cmp(&b.key))
            } else {
                ka.cmp(&kb)
            }
        });
        drop(lru);
        let name = ordered[0].key.clone();
        self.unload_model(&name).await?;
        Ok(Some(name))
    }

    /// Unload a backend (plan-191 Task 5 re-route): the *physical* kill
    /// happens on the model's provider tamad via `UnloadModel`; the proxy
    /// clears its own state (inference stats, access-map entry).
    ///
    /// The tamad RPC is best-effort: on failure the local state is cleared
    /// anyway and the tamad's own lifecycle (sweep / reaper) converges its
    /// process table next window.
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
                "UnloadModel RPC failed; clearing local state anyway (host-side store keeps the truth)"
            ),
        }

        // Clear stale inference stats.
        self.metrics.modify_inference_stats(|map| {
            map.remove(backend_name);
        });

        // Drop the per-key access entry: the model is off the rows, so
        // the LRU / idle decisions must not keep a dead entry around.
        self.registry
            .last_accessed
            .write()
            .await
            .remove(backend_name);

        info!("Backend '{}' unloaded", backend_name);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
