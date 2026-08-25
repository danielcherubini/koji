//! Idle-timeout unload (plan-191 Task 10 slim-down).
//!
//! The proxy no longer inspects processes (ADR-0010): dead/back-crashed
//! detection and restarts live on the tamad (its restart budget is the bound
//! — `budget_exhausted`). What remains here is proxy-side bookkeeping:
//! unloading *idle* Ready models, decided from the live wire rows plus the
//! proxy-owned LRU access-time map (the mirror is gone, plan-193 T5c).

use std::time::{Duration, Instant};
use tracing::warn;

use super::ProxyState;

impl ProxyState {
    /// Unload backends that have been idle longer than the configured
    /// timeout (gated on `auto_unload`).
    ///
    /// Note: the idle decision uses the proxy-owned `last_accessed` map
    /// (request recency) — rows only carry the per-frame `last_seen_ms`.
    pub async fn check_idle_timeouts(&self) -> Vec<String> {
        let now = Instant::now();
        let mut to_unload = Vec::new();

        let (auto_unload, idle_timeout_secs) = {
            let cfg = self.config.read().await;
            (cfg.proxy.auto_unload, cfg.proxy.idle_timeout_secs)
        };
        let idle_timeout = Duration::from_secs(idle_timeout_secs);

        // Loaded inference models come from the live wire rows (plan-193
        // T4); failed/unloading models are not present in the eligible frame.
        let model_configs = self.registry.model_configs.read().await;
        let rows = crate::proxy::live_rows(self.tamad_pool().as_ref()).await;
        for r in rows.all().iter().filter(|r| r.status == "ready") {
            // Skip non-inference backends (TTS, compaction) — separate
            // lifecycle (they are not auto-unloaded while idle).
            let backend = model_configs
                .get(&r.key)
                .map(|c| c.backend.as_str())
                .unwrap_or("");
            if backend.starts_with("tts_") || backend == "compaction" {
                continue;
            }

            // Ready model — check idle timeout via the proxy-owned access map.
            if let Some(last) = self.registry.last_accessed_time(&r.key).await {
                let idle_duration = now.saturating_duration_since(last);
                if auto_unload && idle_duration > idle_timeout {
                    warn!(
                        "Backend '{}' idle for {}s (timeout: {}s)",
                        r.key,
                        idle_duration.as_secs(),
                        idle_timeout_secs
                    );
                    to_unload.push(r.key.clone());
                }
            }
        }
        drop(model_configs);

        // Unload idle models; the physical kill happens on the tamad via
        // the re-routed unload_model (the live row presence gate guards it).
        for backend_name in &to_unload {
            if let Err(e) = self.unload_model(backend_name).await {
                warn!("Failed to unload '{}': {}", backend_name, e);
            }
        }

        to_unload
    }
}
