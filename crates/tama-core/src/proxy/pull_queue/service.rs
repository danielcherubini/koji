use anyhow::{anyhow, Result};
use tokio::sync::broadcast;

use crate::models::ModelManager;

use super::events::PullEvent;

/// Service that manages the pull queue lifecycle.
pub struct PullQueueService {
    pub(super) model_mgr: std::sync::Mutex<ModelManager>,
    events_tx: broadcast::Sender<PullEvent>,
    pub(super) poll_interval_secs: u64,
}

impl PullQueueService {
    /// Create a new `PullQueueService` with a broadcast channel.
    ///
    /// Capacity is set to 256 to accommodate rapid progress updates during
    /// large pulls without dropping events. The SSE endpoint handles
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

    /// Enqueue a new pull item.
    ///
    /// Opens a DB connection, inserts the queue item, and emits `PullEvent::Queued`.
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
        let _ = self.events_tx.send(PullEvent::Queued {
            job_id: job_id.to_string(),
            repo_id: repo_id.to_string(),
            filename: filename.to_string(),
        });
        Ok(())
    }

    /// Dequeue the oldest queued item (FIFO).
    ///
    /// Opens a DB connection and returns the next item, or `None` if empty.
    pub fn dequeue(&self) -> Result<Option<crate::db::queries::PullQueueItem>> {
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
        bytes_pulled: i64,
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
            bytes_pulled,
            total_bytes,
            error_message,
        )?;

        let event = match new_status {
            // Note: "progress" is intentionally not handled here. Progress events
            // are emitted by update_progress() which uses update_progress_only()
            // directly, avoiding any status field changes.
            "running" => PullEvent::Started {
                job_id: job_id.to_string(),
                repo_id: item.repo_id.clone(),
                filename: item.filename.clone(),
                total_bytes: total_bytes.map(|b| b as u64),
            },
            "verifying" => PullEvent::Verifying {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
            },
            "completed" => PullEvent::Completed {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
                size_bytes: bytes_pulled as u64,
                duration_ms: duration_ms.unwrap_or(0),
            },
            "failed" => PullEvent::Failed {
                job_id: job_id.to_string(),
                filename: item.filename.clone(),
                error: error_message.unwrap_or("Unknown error").to_string(),
            },
            "cancelled" => PullEvent::Cancelled {
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
    /// Emits `PullEvent::Progress` and updates bytes_pulled/total_bytes
    /// in the DB without overwriting the current status (running/verifying).
    pub fn update_progress(
        &self,
        job_id: &str,
        bytes_pulled: i64,
        total_bytes: Option<i64>,
    ) -> Result<()> {
        self.model_mgr
            .lock()
            .unwrap()
            .queue_update_progress(job_id, bytes_pulled, total_bytes)?;

        let _ = self.events_tx.send(PullEvent::Progress {
            job_id: job_id.to_string(),
            bytes_pulled: bytes_pulled as u64,
            total_bytes: total_bytes.map(|b| b as u64),
        });
        Ok(())
    }

    /// Cancel a queue item if it hasn't reached a terminal state.
    ///
    /// Opens a DB connection, cancels the item, and emits `PullEvent::Cancelled`.
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

        let _ = self.events_tx.send(PullEvent::Cancelled {
            job_id: job_id.to_string(),
            filename: item.filename.clone(),
        });
        Ok(())
    }

    /// Get all active items (queued + running + verifying), ordered by status priority.
    pub fn get_active_items(&self) -> Result<Vec<crate::db::queries::PullQueueItem>> {
        self.model_mgr.lock().unwrap().queue_get_active()
    }

    /// Get all active items (queued + running + verifying), ordered by status priority.
    pub fn get_active_items_dto(&self) -> Result<Vec<crate::db::queries::PullQueueItem>> {
        self.model_mgr.lock().unwrap().queue_get_active()
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub fn get_history_items(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<crate::db::queries::PullQueueItem>> {
        self.model_mgr
            .lock()
            .unwrap()
            .queue_get_history(limit, offset)
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub fn get_history_items_dto(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<crate::db::queries::PullQueueItem>> {
        self.model_mgr
            .lock()
            .unwrap()
            .queue_get_history(limit, offset)
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
    pub fn get_queue_item(
        &self,
        job_id: &str,
    ) -> Result<Option<crate::db::queries::PullQueueItem>> {
        self.model_mgr.lock().unwrap().queue_get_by_job_id(job_id)
    }

    /// Subscribe to pull events via a broadcast channel receiver.
    pub fn subscribe_events(&self) -> broadcast::Receiver<PullEvent> {
        self.events_tx.subscribe()
    }
}
