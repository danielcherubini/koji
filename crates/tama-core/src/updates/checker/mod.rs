use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::db::queries::UpdateCheckRecord;
use crate::db::queries::{get_all_model_configs, get_oldest_check_time};
use crate::installations::{InstallationManager, InstallationType};

mod backend;
mod cache;
#[cfg(test)]
mod helpers;
mod model;

#[cfg(test)]
mod tests;

pub use cache::*;
#[cfg(test)]
pub use helpers::*;

#[cfg(feature = "web-ui")]
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "PascalCase")]
pub enum UpdateEvent {
    CheckStarted {
        item_type: String,
        item_id: String,
        variant: Option<String>,
    },
    CheckCompleted {
        item_type: String,
        item_id: String,
        variant: Option<String>,
        dto: serde_json::Value,
    },
    CheckError {
        item_type: String,
        item_id: String,
        variant: Option<String>,
        error: String,
    },
    CheckSkipped {
        item_type: String,
        reason: String,
    },
}

#[cfg(feature = "web-ui")]
impl UpdateEvent {
    /// Serialize into an SSE event: the `event:` name is the variant name and
    /// the JSON data is the internally-tagged payload (includes the `"event"` key).
    pub fn to_sse_event(&self) -> anyhow::Result<axum::response::sse::Event> {
        let value = serde_json::to_value(self)?;
        let name = value
            .get("event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let event = axum::response::sse::Event::default()
            .event(name)
            .json_data(&value)?;
        Ok(event)
    }
}

/// Shared state for the update checker. Uses Arc<Mutex<()>> as a binary semaphore
/// to ensure that only one update check run occurs at any given time across the system.
/// Locking this guard serializes checks without needing to protect specific shared data.
#[derive(Clone)]
pub struct UpdateChecker {
    /// Mutex used as a synchronization primitive to prevent concurrent check runs.
    lock: Arc<Mutex<()>>,
    /// In-memory LRU cache for remote GGUF listings.
    gguf_listing_cache: GgufListingCache,
    /// Broadcast sender for update events (web UI feature).
    #[cfg(feature = "web-ui")]
    pub update_events_tx: Option<tokio::sync::broadcast::Sender<UpdateEvent>>,
}

impl std::fmt::Debug for UpdateChecker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateChecker").finish_non_exhaustive()
    }
}

/// Results from an initial sync of backends and models to check for updates.
pub type UpdateSyncResults = (
    Vec<(String, InstallationType, String)>,
    Vec<(i64, Option<String>)>,
);

impl UpdateChecker {
    pub fn new() -> Self {
        Self {
            lock: Arc::new(Mutex::new(())),
            gguf_listing_cache: GgufListingCache::new(),
            #[cfg(feature = "web-ui")]
            update_events_tx: None,
        }
    }

    /// Set the broadcast sender for update events.
    #[cfg(feature = "web-ui")]
    pub fn set_update_events_tx(&mut self, tx: tokio::sync::broadcast::Sender<UpdateEvent>) {
        self.update_events_tx = Some(tx);
    }

    /// Test hook: pre-populate the GGUF listing cache (plan-190 Task 4 —
    /// moved from the in-file orchestration tests that could touch the
    /// private `gguf_listing_cache` field).
    #[doc(hidden)]
    pub async fn seed_gguf_listing_cache(
        &self,
        repo_id: String,
        commit_sha: String,
        files: Vec<crate::models::pull::RemoteGguf>,
        now: Option<i64>,
    ) {
        self.gguf_listing_cache
            .insert(repo_id, commit_sha, files, now)
            .await
    }

    /// Test hook: acquire the run lock to simulate a concurrent check in
    /// progress (plan-190 Task 4).
    #[doc(hidden)]
    pub fn try_hold_run_lock(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.lock.try_lock().ok()
    }

    /// Emit an update event (non-blocking, fire-and-forget).
    #[cfg(feature = "web-ui")]
    fn emit(&self, event: UpdateEvent) {
        if let Some(ref tx) = self.update_events_tx {
            if let Err(e) = tx.send(event) {
                tracing::trace!("Dropped update event: {}", e);
            }
        }
    }

    /// Run a full update check for all backends and models.
    /// Returns immediately if another check is already in progress.
    pub async fn run_check(
        &self,
        config_dir: &std::path::Path,
        pool: &sqlx::PgPool,
    ) -> anyhow::Result<()> {
        // Try to acquire the lock
        let _guard = match self.lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                #[cfg(feature = "web-ui")]
                self.emit(UpdateEvent::CheckSkipped {
                    item_type: "all".to_string(),
                    reason: "Update check already in progress".to_string(),
                });
                tracing::info!("Update check already in progress, skipping");
                return Ok(());
            }
        };

        tracing::info!("Starting update check for all items");

        // Phase 1a: fetch all models to check from Postgres (plan-190 Task 5).
        let models: Vec<(i64, Option<String>)> = get_all_model_configs(pool)
            .await?
            .into_iter()
            .map(|r| (r.id, Some(r.repo_id)))
            .collect();

        // Phase 1b: sync DB - fetch all backends to check.
        // For backends: iterate ALL installed variants (not just active ones)
        let backends = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<Vec<(String, InstallationType, String)>> {
                let mgr = InstallationManager::open(&config_dir)?;

                // Collect all unique (name, backend_type) pairs from all installed backends
                let all_backends = mgr.list_active().unwrap_or_default();
                let backend_names: Vec<String> =
                    all_backends.iter().map(|b| b.name.clone()).collect();

                // For each backend name, get ALL versions and group by variant
                let mut backend_entries: Vec<(String, InstallationType, String)> = Vec::new();
                for name in &backend_names {
                    if let Ok(Some(versions)) = mgr.list_versions(name, None) {
                        // Collect unique variants for this backend
                        let mut variants: Vec<String> =
                            versions.iter().map(|v| v.gpu_variant.clone()).collect();
                        variants.sort();
                        variants.dedup();

                        for variant in variants {
                            // Get the backend type from the first version with this variant
                            if let Some(info) = versions.iter().find(|v| v.gpu_variant == variant) {
                                backend_entries.push((
                                    name.clone(),
                                    info.backend_type.clone(),
                                    variant.clone(),
                                ));
                            }
                        }
                    }
                }

                Ok(backend_entries)
            }
        })
        .await??;

        // Phase 2: Async network - check each backend
        for (backend_name, backend_type, gpu_variant) in &backends {
            if let Err(e) = self
                .check_backend(config_dir, pool, backend_name, backend_type, gpu_variant)
                .await
            {
                tracing::warn!("Failed to check backend {}: {}", backend_name, e);
            }
        }

        // Phase 2: Async network - check each model
        for (model_id, repo_id) in &models {
            if let Err(e) = self
                .check_model(config_dir, pool, *model_id, repo_id.as_deref())
                .await
            {
                tracing::warn!("Failed to check model {}: {}", model_id, e);
            }
        }

        tracing::info!("Update check complete");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_check_result(
        &self,
        pool: &sqlx::PgPool,
        item_type: &str,
        item_id: &str,
        current_version: Option<&str>,
        latest_version: Option<&str>,
        update_available: bool,
        status: &str,
        error_message: Option<&str>,
        details_json: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        crate::db::queries::upsert_update_check(
            pool,
            crate::db::queries::UpdateCheckParams {
                item_type,
                item_id,
                current_version,
                latest_version,
                update_available,
                status,
                error_message,
                details_json,
                checked_at: now,
            },
        )
        .await
    }

    /// Get cached update check results.
    pub async fn get_results(&self, pool: &sqlx::PgPool) -> anyhow::Result<Vec<UpdateCheckRecord>> {
        crate::db::queries::get_all_update_checks(pool).await
    }

    /// Check if enough time has passed since last check (based on interval).
    ///
    /// Both the interval (Postgres-backed global config, plan-190 Task 3)
    /// and the oldest-check-time lookup (plan-190 Task 4) are Postgres-based.
    pub async fn should_check(&self, pool: &sqlx::PgPool) -> anyhow::Result<bool> {
        let config = Config::load_from_pool(pool).await?;

        let interval_hours = config.general.update_check_interval as i64;
        let interval_secs = interval_hours * 3600;

        let oldest = get_oldest_check_time(pool).await?;

        let now = chrono::Utc::now().timestamp();
        match oldest {
            Some(ts) => Ok(now - ts >= interval_secs),
            None => Ok(true),
        }
    }
}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}
