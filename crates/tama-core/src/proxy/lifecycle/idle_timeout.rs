//! Idle-timeout unload (plan-191 Task 10 slim-down).
//!
//! The proxy no longer inspects processes (ADR-0010): dead/back-crashed
//! detection and restarts are the reconciler's job (it converges each
//! tamad's process-table snapshot to the desired set). What remains here is
//! the purely proxy-side bookkeeping: unloading *idle* Ready models (the
//! mirror's `last_accessed` is proxy-owned, so the idle decision stays here)
//! and cleaning up `Failed` mirror entries.

use std::time::{Duration, Instant};
use tracing::{debug, warn};

use super::ProxyState;
use crate::proxy::types::BackendState;

impl ProxyState {
    /// Unload backends that have been idle longer than the configured
    /// timeout (gated on `auto_unload`) and drop Failed mirror entries.
    ///
    /// Unloading clears the model's *desired* state and issues `UnloadModel`
    /// to its tamad, so the reconciler will not re-load an idle model.
    pub async fn check_idle_timeouts(&self) -> Vec<String> {
        let now = Instant::now();
        let mut to_unload = Vec::new();
        let mut failed_to_remove = Vec::new();

        let (auto_unload, idle_timeout_secs) = {
            let cfg = self.config.read().await;
            (cfg.proxy.auto_unload, cfg.proxy.idle_timeout_secs)
        };
        let idle_timeout = Duration::from_secs(idle_timeout_secs);

        // === Collect candidates under read lock (fast only) ===
        let models = self.registry.models.read().await;
        for (backend_name, state) in models.iter() {
            // Skip Unloading — already being handled.
            if matches!(state, BackendState::Unloading { .. }) {
                continue;
            }

            // Skip non-inference backends (TTS, compaction) — separate
            // lifecycle (they are not auto-unloaded while idle).
            if state.is_non_inference_backend() {
                continue;
            }

            // Ready models — check idle timeout (mirror-only signal).
            if state.is_ready() {
                if let Some(last) = state.last_accessed() {
                    let idle_duration = now.saturating_duration_since(last);
                    if auto_unload && idle_duration > idle_timeout {
                        warn!(
                            "Backend '{}' idle for {}s (timeout: {}s)",
                            backend_name,
                            idle_duration.as_secs(),
                            idle_timeout_secs
                        );
                        to_unload.push(backend_name.clone());
                    }
                }
            }

            // Failed models — mark for cleanup.
            if matches!(state, BackendState::Failed { .. }) {
                debug!(
                    "Backend '{}' in Failed state, marking for cleanup",
                    backend_name
                );
                failed_to_remove.push(backend_name.clone());
            }
        }
        drop(models); // Release read lock

        // === Mutations ===

        // Remove Failed models
        if !failed_to_remove.is_empty() {
            let mut models = self.registry.models.write().await;
            for backend_name in &failed_to_remove {
                models.remove(backend_name);
                self.metrics.modify_inference_stats(|map| {
                    map.remove(backend_name);
                });
            }
        }

        // Unload idle models. The model is no longer desired (plan-191 Task
        // 5) so the reconciler will not re-load it; the physical kill
        // happens on the tamad via the re-routed unload_model.
        for backend_name in &to_unload {
            // Capture the model name before the unload removes the mirror.
            let model_name = self
                .get_model_state(backend_name)
                .await
                .map(|s| s.model_name().to_string());
            if let Some(ref model_name) = model_name {
                if let Err(e) = crate::db::queries::clear_desired(&self.db_pool(), model_name).await
                {
                    warn!(
                        "clear_desired for idle model '{}' failed: {}",
                        model_name, e
                    );
                }
            }
            if let Err(e) = self.unload_model(backend_name).await {
                warn!("Failed to unload '{}': {}", backend_name, e);
            }
        }

        // Build return value
        let mut cleaned = Vec::new();
        cleaned.extend(failed_to_remove);
        cleaned.extend(to_unload);
        cleaned
    }
}
