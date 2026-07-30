use std::sync::Arc;
use tokio::sync::Mutex;

use crate::backends::{BackendManager, BackendType};
use crate::config::Config;
use crate::db;
use crate::db::queries::UpdateCheckRecord;
use crate::db::queries::{get_all_model_configs, get_oldest_check_time};

mod backend;
mod cache;
#[cfg(test)]
mod helpers;
mod model;

#[cfg(all(test, feature = "web-ui"))]
mod orchestration_tests;

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
    Vec<(String, BackendType, String)>,
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
    pub async fn run_check(&self, config_dir: &std::path::Path) -> anyhow::Result<()> {
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

        // Phase 1: Sync DB - fetch all items to check
        // For backends: iterate ALL installed variants (not just active ones)
        let (backends, models) = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<UpdateSyncResults> {
                let mgr = BackendManager::open(&config_dir)?;

                // Collect all unique (name, backend_type) pairs from all installed backends
                let all_backends = mgr.list_active().unwrap_or_default();
                let backend_names: Vec<String> =
                    all_backends.iter().map(|b| b.name.clone()).collect();

                // For each backend name, get ALL versions and group by variant
                let mut backend_entries: Vec<(String, BackendType, String)> = Vec::new();
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

                let open = db::open(&config_dir)?;
                let db_model_records = get_all_model_configs(&open.conn)?;
                let models: Vec<(i64, Option<String>)> = db_model_records
                    .into_iter()
                    .map(|r| (r.id, Some(r.repo_id)))
                    .collect();

                Ok((backend_entries, models))
            }
        })
        .await??;

        // Phase 2: Async network - check each backend
        for (backend_name, backend_type, gpu_variant) in &backends {
            if let Err(e) = self
                .check_backend(config_dir, backend_name, backend_type, gpu_variant)
                .await
            {
                tracing::warn!("Failed to check backend {}: {}", backend_name, e);
            }
        }

        // Phase 2: Async network - check each model
        for (model_id, repo_id) in &models {
            if let Err(e) = self
                .check_model(config_dir, *model_id, repo_id.as_deref())
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
        config_dir: &std::path::Path,
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
        let status_str = status.to_string();
        tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let item_type = item_type.to_string();
            let item_id = item_id.to_string();
            let current_version = current_version.map(String::from);
            let latest_version = latest_version.map(String::from);
            let error_message = error_message.map(String::from);
            let details_json = details_json.map(String::from);
            let status = status_str;
            move || -> anyhow::Result<()> {
                let open = db::open(&config_dir)?;
                crate::db::queries::upsert_update_check(
                    &open.conn,
                    crate::db::queries::UpdateCheckParams {
                        item_type: &item_type,
                        item_id: &item_id,
                        current_version: current_version.as_deref(),
                        latest_version: latest_version.as_deref(),
                        update_available,
                        status: &status,
                        error_message: error_message.as_deref(),
                        details_json: details_json.as_deref(),
                        checked_at: now,
                    },
                )?;
                Ok(())
            }
        })
        .await??;
        Ok(())
    }

    /// Get cached update check results.
    pub async fn get_results(
        &self,
        config_dir: &std::path::Path,
    ) -> anyhow::Result<Vec<UpdateCheckRecord>> {
        tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<Vec<UpdateCheckRecord>> {
                let repo = crate::db::repository::Repository::open(&config_dir)?;
                repo.get_all_update_checks()
            }
        })
        .await?
    }

    /// Check if enough time has passed since last check (based on interval).
    pub async fn should_check(&self, config_dir: &std::path::Path) -> anyhow::Result<bool> {
        let config_dir_for_config = config_dir.to_path_buf();
        let db_path = config_dir_for_config.join("tama.db");
        let config = tokio::task::spawn_blocking(move || Config::load_from(&db_path)).await??;

        let interval_hours = config.general.update_check_interval as i64;
        let interval_secs = interval_hours * 3600;

        let oldest = tokio::task::spawn_blocking({
            let config_dir_for_db = config_dir.to_path_buf();
            move || -> anyhow::Result<Option<i64>> {
                let open = db::open(&config_dir_for_db)?;
                get_oldest_check_time(&open.conn)
            }
        })
        .await??;

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
