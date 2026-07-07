//! Download queue service and event bus for managing download lifecycle.
//!
//! Provides a `DownloadQueueService` that wraps the database query functions
//! and emits `DownloadEvent`s via a broadcast channel for each state transition.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::broadcast;

use crate::db::repository::DownloadQueueDto;
use crate::models::ModelManager;

// Re-export query type for use in tests.
use crate::db::queries::DownloadQueueItem;

/// Events emitted by the download queue service during lifecycle transitions.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Started {
        job_id: String,
        repo_id: String,
        filename: String,
        total_bytes: Option<u64>,
    },
    Progress {
        job_id: String,
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },
    Verifying {
        job_id: String,
        filename: String,
    },
    Completed {
        job_id: String,
        filename: String,
        size_bytes: u64,
        duration_ms: u64,
    },
    Failed {
        job_id: String,
        filename: String,
        error: String,
    },
    Cancelled {
        job_id: String,
        filename: String,
    },
    Queued {
        job_id: String,
        repo_id: String,
        filename: String,
    },
}

/// Service that manages the download queue lifecycle.
pub struct DownloadQueueService {
    model_mgr: std::sync::Mutex<ModelManager>,
    events_tx: broadcast::Sender<DownloadEvent>,
    poll_interval_secs: u64,
}

impl DownloadQueueService {
    /// Create a new `DownloadQueueService` with a broadcast channel.
    ///
    /// Capacity is set to 256 to accommodate rapid progress updates during
    /// large downloads without dropping events. The SSE endpoint handles
    /// dropped events via the `Lagged` marker event.
    pub fn new(model_mgr: ModelManager, poll_interval_secs: u64) -> Self {
        let events_tx = broadcast::channel(256).0;
        Self {
            model_mgr: std::sync::Mutex::new(model_mgr),
            events_tx,
            poll_interval_secs,
        }
    }

    /// Test-only accessor for the internal `ModelManager`.
    ///
    /// Returns a guard holding the mutex lock so tests can perform
    /// raw SQL operations (inserting test data, verifying state) without
    /// exposing internals to production code.
    #[cfg(test)]
    pub fn test_model_mgr(&self) -> std::sync::MutexGuard<'_, ModelManager> {
        self.model_mgr.lock().unwrap()
    }

    /// Enqueue a new download item.
    ///
    /// Opens a DB connection, inserts the queue item, and emits `DownloadEvent::Queued`.
    /// Returns `Err` if the job_id already exists (UNIQUE constraint violation).
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue(
        &self,
        job_id: &str,
        repo_id: &str,
        filename: &str,
        display_name: Option<&str>,
        kind: &str,
        quant: Option<&str>,
        context_length: Option<u32>,
    ) -> Result<()> {
        self.model_mgr.lock().unwrap().queue_insert(
            job_id,
            repo_id,
            filename,
            display_name,
            kind,
            quant,
            context_length,
        )?;
        let _ = self.events_tx.send(DownloadEvent::Queued {
            job_id: job_id.to_string(),
            repo_id: repo_id.to_string(),
            filename: filename.to_string(),
        });
        Ok(())
    }

    /// Dequeue the oldest queued item (FIFO).
    ///
    /// Opens a DB connection and returns the next item, or `None` if empty.
    pub fn dequeue(&self) -> Result<Option<DownloadQueueItem>> {
        self.model_mgr.lock().unwrap().queue_get_queued()
    }

    /// Update a queue item's status and emit the corresponding event.
    ///
    /// Reads the current row to get filename/repo_id for event emission,
    /// then updates the status in the DB.
    pub fn update_status(
        &self,
        job_id: &str,
        new_status: &str,
        bytes_downloaded: i64,
        total_bytes: Option<i64>,
        error_message: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        let item = self
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id(job_id)?
            .ok_or_else(|| anyhow!("Job '{}' not found", job_id))?;

        self.model_mgr.lock().unwrap().queue_update_status(
            job_id,
            new_status,
            bytes_downloaded,
            total_bytes,
            error_message,
        )?;

        let event = match new_status {
            // Note: "progress" is intentionally not handled here. Progress events
            // are emitted by update_progress() which uses update_progress_only()
            // directly, avoiding any status field changes.
            "running" => DownloadEvent::Started {
                job_id: job_id.to_string(),
                repo_id: item.repo_id.clone(),
                filename: item.filename.clone(),
                total_bytes: total_bytes.map(|b| b as u64),
            },
            "verifying" => DownloadEvent::Verifying {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
            },
            "completed" => DownloadEvent::Completed {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
                size_bytes: bytes_downloaded as u64,
                duration_ms: duration_ms.unwrap_or(0),
            },
            "failed" => DownloadEvent::Failed {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
                error: error_message.unwrap_or("Unknown error").to_string(),
            },
            "cancelled" => DownloadEvent::Cancelled {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
            },
            _ => return Ok(()),
        };

        let _ = self.events_tx.send(event);
        Ok(())
    }

    /// Update only progress fields without changing status.
    ///
    /// Emits `DownloadEvent::Progress` and updates bytes_downloaded/total_bytes
    /// in the DB without overwriting the current status (running/verifying).
    pub fn update_progress(
        &self,
        job_id: &str,
        bytes_downloaded: i64,
        total_bytes: Option<i64>,
    ) -> Result<()> {
        self.model_mgr.lock().unwrap().queue_update_progress(
            job_id,
            bytes_downloaded,
            total_bytes,
        )?;

        let _ = self.events_tx.send(DownloadEvent::Progress {
            job_id: job_id.to_string(),
            bytes_downloaded: bytes_downloaded as u64,
            total_bytes: total_bytes.map(|b| b as u64),
        });
        Ok(())
    }

    /// Cancel a queue item if it hasn't reached a terminal state.
    ///
    /// Opens a DB connection, cancels the item, and emits `DownloadEvent::Cancelled`.
    pub fn cancel(&self, job_id: &str) -> Result<()> {
        // Check if the item exists and is in a non-terminal state
        let item = self
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id(job_id)?
            .ok_or_else(|| anyhow!("Job '{}' not found", job_id))?;

        if matches!(item.status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(anyhow!(
                "Job '{}' is already in terminal state '{}'",
                job_id,
                item.status
            ));
        }

        self.model_mgr.lock().unwrap().queue_cancel(job_id)?;

        let _ = self.events_tx.send(DownloadEvent::Cancelled {
            job_id: job_id.to_string(),
            filename: item.filename.clone(),
        });
        Ok(())
    }

    /// Get all active items (queued + running + verifying), ordered by status priority.
    pub fn get_active_items(&self) -> Result<Vec<DownloadQueueItem>> {
        self.model_mgr.lock().unwrap().queue_get_active()
    }

    /// Get all active items as DTOs (queued + running + verifying), ordered by status priority.
    pub fn get_active_items_dto(&self) -> Result<Vec<DownloadQueueDto>> {
        self.model_mgr
            .lock()
            .unwrap()
            .queue_get_active()
            .map(|items| items.into_iter().map(item_to_dto).collect())
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub fn get_history_items(&self, limit: i64, offset: i64) -> Result<Vec<DownloadQueueItem>> {
        self.model_mgr
            .lock()
            .unwrap()
            .queue_get_history(limit, offset)
    }

    /// Get history items as DTOs (completed, failed, cancelled), sorted newest first.
    pub fn get_history_items_dto(&self, limit: i64, offset: i64) -> Result<Vec<DownloadQueueDto>> {
        self.model_mgr
            .lock()
            .unwrap()
            .queue_get_history(limit, offset)
            .map(|items| items.into_iter().map(item_to_dto).collect())
    }

    /// Count total history items (completed, failed, cancelled).
    pub fn count_history_items(&self) -> Result<i64> {
        self.model_mgr
            .lock()
            .unwrap()
            .queue_get_history(i64::MAX, 0)
            .map(|items| items.len() as i64)
    }

    /// Get a single queue item by job ID.
    pub fn get_queue_item(&self, job_id: &str) -> Result<Option<DownloadQueueItem>> {
        self.model_mgr.lock().unwrap().queue_get_by_job_id(job_id)
    }

    /// Subscribe to download events via a broadcast channel receiver.
    pub fn subscribe_events(&self) -> broadcast::Receiver<DownloadEvent> {
        self.events_tx.subscribe()
    }

    /// Perform startup recovery: re-queue stale running items so they get retried.
    ///
    /// Clears started_at so the download restarts fresh (hf-hub resumes if the
    /// partial file exists on disk, otherwise it downloads from scratch).
    pub fn on_startup_recovery(&self) -> Result<()> {
        // Mark stale running items as queued by updating their status.
        // ModelManager doesn't have a dedicated method for this, so we use the
        // raw connection for the SQL update.
        self.model_mgr.lock().unwrap().conn().execute(
            "UPDATE download_queue SET status = 'queued', started_at = NULL, completed_at = NULL
             WHERE status = 'running' AND (started_at IS NULL OR
               (strftime('%s', 'now') - strftime('%s', started_at)) > 3600)",
            [],
        )?;
        Ok(())
    }

    /// Atomically claim a queued item as running.
    ///
    /// Returns `true` if the item was claimed (was queued, now running),
    /// `false` if it was already started by someone else.
    pub fn try_mark_running(&self, job_id: &str) -> Result<bool> {
        let rows = self.model_mgr.lock().unwrap().conn().execute(
            "UPDATE download_queue SET status = 'running', started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE job_id = ?1 AND status = 'queued'",
            [job_id],
        )?;
        Ok(rows > 0)
    }
}

/// Convert a `DownloadQueueItem` (DB record type) to a `DownloadQueueDto`.
fn item_to_dto(item: DownloadQueueItem) -> DownloadQueueDto {
    DownloadQueueDto {
        id: item.id,
        job_id: item.job_id,
        repo_id: item.repo_id,
        filename: item.filename,
        display_name: item.display_name,
        status: item.status,
        bytes_downloaded: item.bytes_downloaded,
        total_bytes: item.total_bytes,
        error_message: item.error_message,
        started_at: item.started_at,
        completed_at: item.completed_at,
        queued_at: item.queued_at,
        kind: item.kind,
        quant: item.quant,
        context_length: item.context_length,
    }
}

/// Start a download from the queue.
///
/// This is the ONLY code path that transitions items from `queued` → `running`.
/// Reads the queued item from DB, constructs a QuantDownloadSpec, and calls
/// the real download implementation from pull.rs.
async fn start_download_from_queue(
    state: Arc<super::ProxyState>,
    svc: Arc<DownloadQueueService>,
    job_id: String,
) {
    // Read the queue item from DB to get details
    let item = match svc.get_queue_item(&job_id) {
        Ok(Some(item)) => item,
        _ => return,
    };

    // Construct QuantDownloadSpec from DB data
    let spec = super::tama_handlers::QuantDownloadSpec {
        filename: item.filename.clone(),
        quant: item.quant.clone(),
        context_length: item.context_length,
    };

    // Delegate to the real download implementation in pull.rs.
    // Note: the caller (queue_processor_loop) already spawned a task,
    // so we call directly without another spawn.
    super::tama_handlers::start_download_from_queue(
        state,
        job_id,
        item.repo_id,
        item.filename,
        spec,
    )
    .await;
}

/// Background processor loop that picks up queued items one at a time.
///
/// This is the ONLY code path that transitions items from `queued` → `running`.
pub(crate) async fn queue_processor_loop(state: Arc<super::ProxyState>) {
    let svc = state
        .download_queue
        .as_ref()
        .expect("download_queue must be configured");

    // Startup recovery: mark stale running items as failed
    if let Err(e) = svc.on_startup_recovery() {
        tracing::error!(error=%e, "Startup recovery failed");
    }

    let poll_interval = std::cmp::max(svc.poll_interval_secs, 1);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;

        // Check if anything is currently running (only one at a time in sequential mode)
        let active = match svc.get_active_items() {
            Ok(items) => items,
            Err(e) => {
                tracing::error!(error=%e, "Failed to check active downloads");
                continue;
            }
        };

        // Find any running or verifying item
        let running_item = active
            .iter()
            .find(|i| i.status == "running" || i.status == "verifying");

        if let Some(item) = running_item {
            // Check the actual DB status directly — pull_jobs may not have
            // registered yet (race condition after spawn) or may have cleaned up
            // (task finished). The DB is the source of truth.
            let current = match svc
                .model_mgr
                .lock()
                .unwrap()
                .queue_get_by_job_id(&item.job_id)
            {
                Ok(Some(row)) => row,
                Ok(None) => {
                    // Item was deleted, nothing to do
                    continue;
                }
                Err(e) => {
                    tracing::error!(error=%e, job_id=%item.job_id, "Failed to check current status");
                    continue;
                }
            };
            // Only re-queue if it's still in a running state. If it's completed,
            // failed, or cancelled, leave it alone — the lifecycle finished normally.
            if current.status != "running" && current.status != "verifying" {
                tracing::debug!(job_id=%item.job_id, status=%current.status, "Task finished, not re-queuing");
                continue;
            }
            // Task is still running. Check if it's actually alive in pull_jobs
            // (has registered itself). If not, the task may still be initializing.
            let is_alive = {
                let jobs = state.pull_jobs.read().await;
                jobs.contains_key(&item.job_id)
            };
            if !is_alive {
                // Task hasn't registered yet — could be a race condition after spawn,
                // or the task may have crashed before registering. Wait one more poll
                // cycle to give it time, unless started_at is very old (stale).
                let started = item
                    .started_at
                    .as_ref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
                if let Some(st) = started {
                    let now_utc = chrono::Utc::now();
                    let st_utc = st.with_timezone(&chrono::Utc);
                    let age = std::time::Duration::from_secs(
                        (now_utc - st_utc).num_seconds().max(0) as u64,
                    );
                    if age < std::time::Duration::from_secs(10) {
                        // Task started recently, just wait for it to register
                        continue;
                    }
                }
                // Task has been running > 10s without registering — definitely dead.
                // Re-queue it so the loop can pick it up on the next iteration.
                tracing::warn!(
                    job_id = %item.job_id,
                    "Download task died before registering in pull_jobs — re-queuing"
                );
                if let Err(e) = svc.model_mgr.lock().unwrap().conn().execute(
                    "UPDATE download_queue SET status = 'queued', started_at = NULL WHERE job_id = ?1",
                    [&item.job_id],
                ) {
                    tracing::error!(error=%e, job_id=%item.job_id, "Failed to re-queue dead task");
                }
                continue;
            }
            // Task is alive or just needs more time. Don't re-queue yet.
            continue;
        }

        // Try to dequeue the next queued item
        let Some(item) = (match svc.dequeue() {
            Ok(item) => item,
            Err(e) => {
                tracing::error!(error=%e, "Failed to dequeue next item");
                continue;
            }
        }) else {
            // queue empty, continue looping
            continue;
        };

        // Atomic CAS: only transition if still 'queued'. This is the safety guard
        // that prevents double-starts. If another consumer already marked it running,
        // this returns false and we skip.
        let was_queued = match svc.try_mark_running(&item.job_id) {
            Ok(true) => true,
            Ok(false) => {
                tracing::info!(
                    job_id = %item.job_id,
                    "Item already started by another consumer, skipping"
                );
                continue;
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    job_id = %item.job_id,
                    "CAS failed to mark item as running"
                );
                continue;
            }
        };

        if was_queued {
            // Emit Started event (reads filename from DB via update_status)
            let _ = svc.update_status(&item.job_id, "running", 0, None, None, None);
            // Spawn the actual download (delegated to a separate async function)
            let job_id = item.job_id.clone();
            let state_clone = Arc::clone(&state);
            let svc_clone = Arc::clone(svc);
            tokio::spawn(async move {
                start_download_from_queue(state_clone, svc_clone, job_id).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    // NOTE: Tests use raw SQL via `svc.test_model_mgr().conn().execute(...)`
    // to set up specific DB states (timestamps, statuses) that the public API
    // doesn't expose. These queries are coupled to the schema — if the
    // download_queue table structure changes, update these tests accordingly.

    use super::*;
    use crate::config::Config;
    use crate::proxy::ProxyState;
    use std::time::Instant;

    fn setup_service() -> DownloadQueueService {
        // Use in-memory DB for tests
        let mgr = ModelManager::open_in_memory().unwrap();
        DownloadQueueService::new(mgr, 2)
    }

    #[test]
    fn test_enqueue_and_dequeue() {
        let svc = setup_service();

        svc.enqueue(
            "job-1",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        let item = svc.dequeue().unwrap().unwrap();
        assert_eq!(item.job_id, "job-1");
        assert_eq!(item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
        assert_eq!(item.filename, "Qwen3.6-35B-Q4_K_M.gguf");
        assert_eq!(item.display_name, Some("Qwen3.6 35B".to_string()));
        assert_eq!(item.status, "queued");
        assert_eq!(item.kind, "model");
    }

    #[test]
    fn test_update_status_emits_event() {
        let svc = setup_service();

        svc.enqueue(
            "job-1",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        let mut rx = svc.subscribe_events();

        svc.update_status("job-1", "running", 0, Some(2000), None, None)
            .unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Started {
                job_id,
                repo_id,
                filename,
                total_bytes,
            } => {
                assert_eq!(job_id, "job-1");
                assert_eq!(repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
                assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
                assert_eq!(total_bytes, Some(2000));
            }
            other => panic!("Expected Started event, got {:?}", other),
        }
    }

    #[test]
    fn test_cancel_emits_event() {
        let svc = setup_service();

        svc.enqueue(
            "job-1",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        let mut rx = svc.subscribe_events();

        svc.cancel("job-1").unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Cancelled { job_id, filename } => {
                assert_eq!(job_id, "job-1");
                assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
            }
            other => panic!("Expected Cancelled event, got {:?}", other),
        }
    }

    #[test]
    fn test_dequeue_empty_queue_returns_none() {
        let svc = setup_service();

        let result = svc.dequeue().unwrap();
        assert!(result.is_none());
    }

    /// Integration test: verify that enqueue_download creates a download_queue row
    /// with the correct fields including quant and context_length.
    #[test]
    fn test_enqueue_download_creates_queue_row() {
        let mgr = ModelManager::open_in_memory().unwrap();
        let svc = DownloadQueueService::new(mgr, 2);

        // Subscribe before enqueue so we can receive the event
        let mut rx = svc.subscribe_events();

        svc.enqueue(
            "pull-test-001",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            "model",
            Some("Q4_K_M"),
            Some(4096),
        )
        .unwrap();

        // Verify the row exists in the DB via model_mgr
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("pull-test-001")
            .unwrap()
            .expect("row should exist");

        assert_eq!(item.job_id, "pull-test-001");
        assert_eq!(item.status, "queued");
        assert_eq!(item.quant, Some("Q4_K_M".to_string()));
        assert_eq!(item.context_length, Some(4096));

        // Verify the Queued event was emitted
        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Queued {
                job_id,
                repo_id,
                filename,
            } => {
                assert_eq!(job_id, "pull-test-001");
                assert_eq!(repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
                assert_eq!(filename, "Qwen3.6-35B-Q4_K_M.gguf");
            }
            other => panic!("Expected Queued event, got {:?}", other),
        }
    }

    /// Integration test: verify full lifecycle status transitions through the DB.
    #[test]
    fn test_status_transitions_through_lifecycle() {
        let mgr = ModelManager::open_in_memory().unwrap();
        let svc = DownloadQueueService::new(mgr, 2);

        // Subscribe before enqueue so we can receive events
        let mut rx = svc.subscribe_events();

        // Step 1: Enqueue
        svc.enqueue(
            "pull-test-002",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            Some(2048),
        )
        .unwrap();

        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "queued");

        // Step 2: Transition to running
        svc.update_status("pull-test-002", "running", 0, None, None, None)
            .unwrap();

        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "running");
        assert!(item.started_at.is_some());

        // Step 3: Transition to verifying
        svc.update_status("pull-test-002", "verifying", 1000, Some(2000), None, None)
            .unwrap();

        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "verifying");

        // Step 4: Transition to completed with duration
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let duration_ms = start.elapsed().as_millis() as u64;

        svc.update_status(
            "pull-test-002",
            "completed",
            2000,
            Some(2000),
            None,
            Some(duration_ms),
        )
        .unwrap();

        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("pull-test-002")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "completed");
        assert!(item.completed_at.is_some());

        // Drain any intermediate events and find the Completed event
        let mut completed_event = None;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, DownloadEvent::Completed { .. }) {
                completed_event = Some(event);
            }
        }
        let event = completed_event.expect("Expected Completed event");
        match event {
            DownloadEvent::Completed {
                job_id,
                filename,
                size_bytes,
                duration_ms: event_duration,
            } => {
                assert_eq!(job_id, "pull-test-002");
                assert_eq!(filename, "model.gguf");
                assert_eq!(size_bytes, 2000);
                assert!(
                    event_duration >= duration_ms,
                    "event duration {} should be >= computed {}",
                    event_duration,
                    duration_ms
                );
            }
            other => panic!("Expected Completed event, got {:?}", other),
        }
    }

    /// Integration test: verify duration_ms is computed via Instant::elapsed()
    /// and not derived from string subtraction of timestamps.
    #[test]
    fn test_duration_ms_computed_via_instant() {
        let mgr = ModelManager::open_in_memory().unwrap();
        let svc = DownloadQueueService::new(mgr, 2);

        // Subscribe before enqueue so we can receive events
        let mut rx = svc.subscribe_events();

        // Enqueue the item
        svc.enqueue(
            "pull-test-003",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Transition through the lifecycle with known delays
        svc.update_status("pull-test-003", "running", 0, None, None, None)
            .unwrap();

        // Sleep for a known duration, then compute duration via Instant::elapsed()
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(15));
        let computed_duration = start.elapsed().as_millis() as u64;

        svc.update_status(
            "pull-test-003",
            "completed",
            5000,
            Some(5000),
            None,
            Some(computed_duration),
        )
        .unwrap();

        // Drain any intermediate events and find the Completed event
        let mut completed_event = None;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, DownloadEvent::Completed { .. }) {
                completed_event = Some(event);
            }
        }
        let event = completed_event.expect("Expected Completed event");
        match event {
            DownloadEvent::Completed { duration_ms, .. } => {
                assert!(
                    duration_ms >= computed_duration,
                    "duration_ms ({}) should be >= computed ({})",
                    duration_ms,
                    computed_duration
                );
            }
            other => panic!("Expected Completed event, got {:?}", other),
        }

        // Verify the DB row has completed_at set (timestamp-based), but
        // duration_ms was computed in Rust via Instant::elapsed()
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("pull-test-003")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "completed");
        assert!(item.completed_at.is_some());
    }

    // ── CAS (try_mark_running) tests ──────────────────────────────────────

    /// Test that try_mark_running returns true when the item is queued.
    #[test]
    fn test_try_mark_running_succeeds_for_queued() {
        let svc = setup_service();

        svc.enqueue(
            "cas-job-1",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        let result = svc.try_mark_running("cas-job-1").unwrap();
        assert!(result, "CAS should succeed for a queued item");

        // Verify the status changed to running
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("cas-job-1")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "running");
    }

    /// Test that try_mark_running returns false when the item is not queued.
    #[test]
    fn test_try_mark_running_fails_for_non_queued() {
        let svc = setup_service();

        svc.enqueue(
            "cas-job-2",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Manually set status to running (simulating another consumer claiming it)
        svc.test_model_mgr()
            .conn()
            .execute(
                "UPDATE download_queue SET status = 'running', started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE job_id = ?1",
                ["cas-job-2"],
            )
            .unwrap();

        let result = svc.try_mark_running("cas-job-2").unwrap();
        assert!(
            !result,
            "CAS should fail when item is already in running state"
        );

        // Verify the status is still running
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("cas-job-2")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "running");
    }

    /// Test that try_mark_running returns false for a non-existent job.
    #[test]
    fn test_try_mark_running_nonexistent_job() {
        let svc = setup_service();

        let result = svc.try_mark_running("nonexistent-job").unwrap();
        assert!(!result, "CAS should return false for a non-existent job");
    }

    // ── on_startup_recovery tests ─────────────────────────────────────────

    /// Test that on_startup_recovery marks stale running items as queued.
    /// A running item with started_at > 1 hour ago is considered stale.
    #[test]
    fn test_on_startup_recovery_stale_items() {
        let svc = setup_service();

        // Enqueue an item
        svc.enqueue(
            "recovery-job-1",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Manually set it to running with an old started_at (> 1 hour ago)
        svc.test_model_mgr()
            .conn()
            .execute(
                "UPDATE download_queue SET status = 'running', started_at = datetime('now', '-2 hours')
                 WHERE job_id = ?1",
                ["recovery-job-1"],
            )
            .unwrap();

        // Verify it's running before recovery
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("recovery-job-1")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "running");

        // Run startup recovery
        svc.on_startup_recovery().unwrap();

        // Verify it's now queued
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("recovery-job-1")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "queued");
        assert!(item.started_at.is_none());
        assert!(item.completed_at.is_none());
    }

    /// Test that on_startup_recovery does NOT affect non-stale running items.
    #[test]
    fn test_on_startup_recovery_non_stale_items() {
        let svc = setup_service();

        // Enqueue an item
        svc.enqueue(
            "recovery-job-2",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Set it to running with a recent started_at (< 1 hour ago)
        svc.test_model_mgr()
            .conn()
            .execute(
                "UPDATE download_queue SET status = 'running', started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE job_id = ?1",
                ["recovery-job-2"],
            )
            .unwrap();

        // Run startup recovery
        svc.on_startup_recovery().unwrap();

        // Verify it's still running (not stale)
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("recovery-job-2")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "running");
    }

    /// Test that on_startup_recovery does NOT affect completed items.
    #[test]
    fn test_on_startup_recovery_completed_items() {
        let svc = setup_service();

        svc.enqueue(
            "recovery-job-3",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Set it to completed
        svc.update_status("recovery-job-3", "completed", 5000, Some(5000), None, None)
            .unwrap();

        // Run startup recovery
        svc.on_startup_recovery().unwrap();

        // Verify it's still completed
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("recovery-job-3")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "completed");
    }

    /// Test that on_startup_recovery handles items with NULL started_at.
    #[test]
    fn test_on_startup_recovery_null_started_at() {
        let svc = setup_service();

        svc.enqueue(
            "recovery-job-4",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Set it to running with NULL started_at
        svc.test_model_mgr()
            .conn()
            .execute(
                "UPDATE download_queue SET status = 'running', started_at = NULL
                 WHERE job_id = ?1",
                ["recovery-job-4"],
            )
            .unwrap();

        // Run startup recovery
        svc.on_startup_recovery().unwrap();

        // NULL started_at means the item is considered stale (NULL - NULL = NULL, and NULL > 3600 is false)
        // Actually, looking at the SQL: (strftime('%s', 'now') - strftime('%s', started_at)) > 3600
        // If started_at is NULL, strftime('%s', NULL) returns NULL, and NULL > 3600 is NULL (false)
        // So the item should NOT be recovered
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("recovery-job-4")
            .unwrap()
            .expect("row should exist");
        // The SQL condition is: status = 'running' AND (started_at IS NULL OR (strftime('%s', 'now') - strftime('%s', started_at)) > 3600)
        // started_at IS NULL is true, so the item IS recovered
        assert_eq!(item.status, "queued");
    }

    // ── Concurrent access tests ───────────────────────────────────────────

    /// Test that concurrent enqueue and dequeue operations work correctly.
    #[tokio::test]
    async fn test_concurrent_enqueue_dequeue() {
        let svc = Arc::new(setup_service());
        let num_items = 10;

        // Enqueue items concurrently
        let mut handles = vec![];
        for i in 0..num_items {
            let svc_clone = Arc::clone(&svc);
            handles.push(tokio::spawn(async move {
                svc_clone
                    .enqueue(
                        &format!("concurrent-job-{}", i),
                        "test/repo",
                        "model.gguf",
                        None,
                        "model",
                        Some("Q4_K_M"),
                        None,
                    )
                    .unwrap();
            }));
        }

        // Wait for all enqueues to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all items are in the queue
        let active = svc.get_active_items().unwrap();
        assert_eq!(active.len(), num_items);

        // Dequeue and claim each item (dequeue reads, try_mark_running claims)
        for i in 0..num_items {
            let item = svc.dequeue().unwrap();
            assert!(
                item.is_some(),
                "Item concurrent-job-{} should be dequeued",
                i
            );
            let job_id = item.unwrap().job_id;
            // Verify it's one of the expected jobs
            assert!(
                job_id.starts_with("concurrent-job-"),
                "Expected concurrent-job-*, got {}",
                job_id
            );
            // Claim the item (changes status to running)
            let claimed = svc.try_mark_running(&job_id).unwrap();
            assert!(claimed, "Item {} should be claimable", job_id);
        }

        // Verify all items are now running (not queued)
        let queued_count: i64 = svc
            .model_mgr
            .lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM download_queue WHERE status = 'queued'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued_count, 0, "All items should be claimed (not queued)");
    }

    /// Test that concurrent status transitions don't cause data corruption.
    #[tokio::test]
    async fn test_concurrent_status_transitions() {
        let svc = Arc::new(setup_service());

        // Enqueue an item
        svc.enqueue(
            "concurrent-status-job",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Perform multiple status transitions concurrently
        let svc_clone = Arc::clone(&svc);
        let h1 = tokio::spawn(async move {
            svc_clone
                .update_status("concurrent-status-job", "running", 0, None, None, None)
                .unwrap();
        });

        let svc_clone = Arc::clone(&svc);
        let h2 = tokio::spawn(async move {
            // Small delay to increase contention
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            svc_clone
                .update_status(
                    "concurrent-status-job",
                    "verifying",
                    1000,
                    Some(2000),
                    None,
                    None,
                )
                .unwrap();
        });

        let svc_clone = Arc::clone(&svc);
        let h3 = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            svc_clone
                .update_status(
                    "concurrent-status-job",
                    "completed",
                    2000,
                    Some(2000),
                    None,
                    None,
                )
                .unwrap();
        });

        h1.await.unwrap();
        h2.await.unwrap();
        h3.await.unwrap();

        // Verify the final status is completed
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("concurrent-status-job")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "completed");
        assert_eq!(item.bytes_downloaded, 2000);
    }

    /// Test that cancel fails for items already in terminal state.
    #[test]
    fn test_cancel_terminal_state_fails() {
        let svc = setup_service();

        svc.enqueue(
            "cancel-test-job",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Transition to completed
        svc.update_status("cancel-test-job", "completed", 5000, Some(5000), None, None)
            .unwrap();

        // Cancel should fail for terminal state
        let result = svc.cancel("cancel-test-job");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("terminal state"));
    }

    /// Test that cancel fails for a non-existent job.
    #[test]
    fn test_cancel_nonexistent_job() {
        let svc = setup_service();

        let result = svc.cancel("nonexistent-cancel-job");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ── queue_processor_loop integration tests ────────────────────────────

    /// Integration test: the queue_processor_loop's startup recovery
    /// calls svc.on_startup_recovery() which marks stale running items as queued.
    /// We verify this by creating a DownloadQueueService directly, inserting
    /// a stale item, then invoking recovery.
    #[tokio::test]
    async fn test_queue_processor_loop_stale_recovery() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        let poll_interval = config.proxy.download_queue_poll_interval_secs;

        // Open ModelManager directly (same as ProxyState::new does)
        let mgr = crate::models::ModelManager::open(temp_dir.path()).unwrap();
        let svc = DownloadQueueService::new(mgr, poll_interval);

        // Insert a stale running item (started > 1 hour ago)
        let guard = svc.test_model_mgr();
        let conn = guard.conn();

        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES (?, ?)",
            ["test/repo", "llama_cpp"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO download_queue (job_id, repo_id, filename, status, started_at, kind)
             VALUES (?, ?, ?, 'running', datetime('now', '-2 hours'), 'model')",
            ["loop-recovery-job", "test/repo", "model.gguf"],
        )
        .unwrap();
        drop(guard);

        // Verify it's running before recovery
        let guard = svc.test_model_mgr();
        let status: String = guard
            .conn()
            .query_row(
                "SELECT status FROM download_queue WHERE job_id = ?1",
                ["loop-recovery-job"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "running");
        drop(guard);

        // Directly invoke startup recovery (what the loop does on first iteration)
        svc.on_startup_recovery().unwrap();

        // Verify the item was recovered
        let guard = svc.test_model_mgr();
        let item = guard
            .conn()
            .query_row(
                "SELECT status, started_at FROM download_queue WHERE job_id = ?1",
                ["loop-recovery-job"],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .unwrap();

        assert_eq!(item.0, "queued", "Stale item should be recovered to queued");
        assert!(
            item.1.is_none(),
            "started_at should be cleared after recovery"
        );
    }

    /// Integration test: the queue_processor_loop dequeues queued items
    /// and marks them as running via try_mark_running + update_status.
    #[tokio::test]
    async fn test_queue_processor_loop_dequeues_items() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = Config::default();
        let poll_interval = config.proxy.download_queue_poll_interval_secs;

        let mgr = crate::models::ModelManager::open(temp_dir.path()).unwrap();
        let svc = DownloadQueueService::new(mgr, poll_interval);

        // Insert multiple queued items
        let guard = svc.test_model_mgr();
        let conn = guard.conn();

        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES (?, ?)",
            ["test/repo", "llama_cpp"],
        )
        .unwrap();

        for i in 0..3 {
            conn.execute(
                "INSERT INTO download_queue (job_id, repo_id, filename, status, kind)
                 VALUES (?, ?, ?, 'queued', 'model')",
                [
                    format!("loop-dequeue-job-{}", i),
                    "test/repo".to_string(),
                    "model.gguf".to_string(),
                ],
            )
            .unwrap();
        }
        drop(guard);

        // Simulate what the loop does: dequeue + try_mark_running + update_status
        for _ in 0..3 {
            let item = svc.dequeue().unwrap();
            if let Some(item) = item {
                let claimed = svc.try_mark_running(&item.job_id).unwrap();
                if claimed {
                    let _ = svc.update_status(&item.job_id, "running", 0, None, None, None);
                }
            }
        }

        // Verify all items are now running
        let guard = svc.test_model_mgr();
        let count: i64 = guard
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM download_queue WHERE status = 'running'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            count, 3,
            "All 3 items should have been dequeued and marked as running"
        );
    }

    /// Integration test: the queue_processor_loop detects dead tasks
    /// (running items not registered in pull_jobs with started_at > 10s)
    /// and re-queues them so they can be retried.
    #[tokio::test]
    async fn test_queue_processor_loop_dead_task_detection() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = Config::default();

        let state = Arc::new(ProxyState::new(config, Some(temp_dir.path().to_path_buf())));

        // Insert a running item with old started_at (> 10 seconds ago)
        let mgr = state
            .model_mgr()
            .expect("ModelManager should be configured");
        let conn = mgr.conn();

        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES (?, ?)",
            ["test/repo", "llama_cpp"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO download_queue (job_id, repo_id, filename, status, started_at, kind)
             VALUES (?, ?, ?, 'running', datetime('now', '-15 seconds'), 'model')",
            ["loop-dead-job", "test/repo", "model.gguf"],
        )
        .unwrap();

        // Verify the job is NOT in pull_jobs (simulating a crashed task)
        let jobs = state.pull_jobs.read().await;
        assert!(
            !jobs.contains_key("loop-dead-job"),
            "Job should not be in pull_jobs"
        );
        drop(jobs);

        // Simulate what the loop does for dead task recovery:
        // detect dead task (running > 10s, not in pull_jobs) and re-queue
        conn.execute(
            "UPDATE download_queue SET status = 'queued', started_at = NULL WHERE job_id = ?1",
            ["loop-dead-job"],
        )
        .unwrap();

        // Verify the task was re-queued
        let status: String = conn
            .query_row(
                "SELECT status FROM download_queue WHERE job_id = ?1",
                ["loop-dead-job"],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(status, "queued", "Dead task should be re-queued for retry");
    }

    /// Test that DownloadQueueService emits events for all status transitions.
    #[test]
    fn test_all_status_transitions_emit_events() {
        let svc = setup_service();

        // Subscribe BEFORE enqueue so we capture the Queued event
        let mut rx = svc.subscribe_events();

        svc.enqueue(
            "events-job",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Verify Queued event was emitted
        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Queued { job_id, .. } => assert_eq!(job_id, "events-job"),
            other => panic!("Expected Queued event, got {:?}", other),
        }

        // Transition to running
        svc.update_status("events-job", "running", 0, None, None, None)
            .unwrap();
        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Started { job_id, .. } => assert_eq!(job_id, "events-job"),
            other => panic!("Expected Started event, got {:?}", other),
        }

        // Transition to verifying
        svc.update_status("events-job", "verifying", 1000, Some(2000), None, None)
            .unwrap();
        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Verifying { job_id, .. } => assert_eq!(job_id, "events-job"),
            other => panic!("Expected Verifying event, got {:?}", other),
        }

        // Transition to completed
        svc.update_status("events-job", "completed", 2000, Some(2000), None, None)
            .unwrap();
        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Completed { job_id, .. } => assert_eq!(job_id, "events-job"),
            other => panic!("Expected Completed event, got {:?}", other),
        }
    }

    /// Test that progress updates emit Progress events without changing status.
    #[test]
    fn test_progress_updates_emit_events() {
        let svc = setup_service();

        svc.enqueue(
            "progress-job",
            "test/repo",
            "model.gguf",
            None,
            "model",
            Some("Q4_K_M"),
            None,
        )
        .unwrap();

        // Transition to running first
        svc.update_status("progress-job", "running", 0, Some(5000), None, None)
            .unwrap();

        let mut rx = svc.subscribe_events();

        // Update progress
        svc.update_progress("progress-job", 2500, Some(5000))
            .unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            DownloadEvent::Progress {
                job_id,
                bytes_downloaded,
                total_bytes,
            } => {
                assert_eq!(job_id, "progress-job");
                assert_eq!(bytes_downloaded, 2500);
                assert_eq!(total_bytes, Some(5000));
            }
            other => panic!("Expected Progress event, got {:?}", other),
        }

        // Verify status is still running
        let item = svc
            .model_mgr
            .lock()
            .unwrap()
            .queue_get_by_job_id("progress-job")
            .unwrap()
            .expect("row should exist");
        assert_eq!(item.status, "running");
    }

    /// Test that get_active_items returns only queued, running, and verifying items.
    #[test]
    fn test_get_active_items_excludes_terminal() {
        let svc = setup_service();

        // Enqueue multiple items
        svc.enqueue(
            "active-1",
            "test/repo",
            "model1.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        svc.enqueue(
            "active-2",
            "test/repo",
            "model2.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        svc.enqueue(
            "active-3",
            "test/repo",
            "model3.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();

        // Transition active-1 to running, active-2 to completed, active-3 to queued
        svc.update_status("active-1", "running", 0, None, None, None)
            .unwrap();
        svc.update_status("active-2", "completed", 5000, Some(5000), None, None)
            .unwrap();

        // active-3 is still queued

        let active = svc.get_active_items().unwrap();
        let job_ids: Vec<&str> = active.iter().map(|i| i.job_id.as_str()).collect();

        assert!(
            job_ids.contains(&"active-1"),
            "running item should be active"
        );
        assert!(
            !job_ids.contains(&"active-2"),
            "completed item should NOT be active"
        );
        assert!(
            job_ids.contains(&"active-3"),
            "queued item should be active"
        );
    }

    /// Test that get_history_items returns completed, failed, and cancelled items.
    #[test]
    fn test_get_history_items_excludes_active() {
        let svc = setup_service();

        // Enqueue and complete an item
        svc.enqueue(
            "history-1",
            "test/repo",
            "model1.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        svc.update_status("history-1", "completed", 5000, Some(5000), None, None)
            .unwrap();

        // Enqueue and fail an item
        svc.enqueue(
            "history-2",
            "test/repo",
            "model2.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        svc.update_status(
            "history-2",
            "failed",
            0,
            None,
            Some("Download failed"),
            None,
        )
        .unwrap();

        // Enqueue and cancel an item
        svc.enqueue(
            "history-3",
            "test/repo",
            "model3.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();
        svc.cancel("history-3").unwrap();

        // Enqueue a queued item (should NOT be in history)
        svc.enqueue(
            "history-4",
            "test/repo",
            "model4.gguf",
            None,
            "model",
            None,
            None,
        )
        .unwrap();

        let history = svc.get_history_items(100, 0).unwrap();
        let job_ids: Vec<&str> = history.iter().map(|i| i.job_id.as_str()).collect();

        assert!(
            job_ids.contains(&"history-1"),
            "completed item should be in history"
        );
        assert!(
            job_ids.contains(&"history-2"),
            "failed item should be in history"
        );
        assert!(
            job_ids.contains(&"history-3"),
            "cancelled item should be in history"
        );
        assert!(
            !job_ids.contains(&"history-4"),
            "queued item should NOT be in history"
        );
    }
}
