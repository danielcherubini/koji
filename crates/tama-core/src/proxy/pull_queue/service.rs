use std::sync::Arc;

use anyhow::{anyhow, Result};
use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::db::queries::PullQueueItem;

use super::events::PullEvent;

/// Service that manages the pull queue lifecycle.
pub struct PullQueueService {
    pub(super) pool: Arc<PgPool>,
    events_tx: broadcast::Sender<PullEvent>,
    pub(super) poll_interval_secs: u64,
}

impl PullQueueService {
    /// Create a new `PullQueueService` with a broadcast channel.
    ///
    /// Capacity is set to 256 to accommodate rapid progress updates during
    /// large pulls without dropping events. The SSE endpoint handles
    /// dropped events via the `Lagged` marker event.
    pub fn new(pool: Arc<PgPool>, poll_interval_secs: u64) -> Self {
        let events_tx = broadcast::channel(256).0;
        Self {
            pool,
            events_tx,
            poll_interval_secs,
        }
    }

    /// Enqueue a new pull item.
    ///
    /// Inserts the queue item and emits `PullEvent::Queued`.
    /// Returns `Err` if the job_id already exists (UNIQUE constraint violation).
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue(
        &self,
        job_id: &str,
        repo_id: &str,
        filename: &str,
        display_name: Option<&str>,
        kind: &str,
        quant: Option<&str>,
        context_length: Option<u32>,
    ) -> Result<()> {
        crate::db::queries::insert_queue_item(
            self.pool.as_ref(),
            job_id,
            repo_id,
            filename,
            display_name,
            kind,
            quant,
            context_length,
        )
        .await?;
        let _ = self.events_tx.send(PullEvent::Queued {
            job_id: job_id.to_string(),
            repo_id: repo_id.to_string(),
            filename: filename.to_string(),
        });
        Ok(())
    }

    /// Dequeue the oldest queued item (FIFO).
    ///
    /// Returns the next item, or `None` if empty.
    pub async fn dequeue(&self) -> Result<Option<PullQueueItem>> {
        crate::db::queries::get_queued_item(self.pool.as_ref()).await
    }

    /// Update a queue item's status and emit the corresponding event.
    ///
    /// Reads the current row to get filename/repo_id for event emission,
    /// then updates the status in the DB.
    pub async fn update_status(
        &self,
        job_id: &str,
        new_status: &str,
        bytes_pulled: i64,
        total_bytes: Option<i64>,
        error_message: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        let item = crate::db::queries::get_item_by_job_id(self.pool.as_ref(), job_id)
            .await?
            .ok_or_else(|| anyhow!("Job '{}' not found", job_id))?;

        crate::db::queries::update_queue_status(
            self.pool.as_ref(),
            job_id,
            new_status,
            bytes_pulled,
            total_bytes,
            error_message,
        )
        .await?;

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
    pub async fn update_progress(
        &self,
        job_id: &str,
        bytes_pulled: i64,
        total_bytes: Option<i64>,
    ) -> Result<()> {
        crate::db::queries::update_progress_only(
            self.pool.as_ref(),
            job_id,
            bytes_pulled,
            total_bytes,
        )
        .await?;

        let _ = self.events_tx.send(PullEvent::Progress {
            job_id: job_id.to_string(),
            bytes_pulled: bytes_pulled as u64,
            total_bytes: total_bytes.map(|b| b as u64),
        });
        Ok(())
    }

    /// Cancel a queue item if it hasn't reached a terminal state.
    ///
    /// Cancels the item and emits `PullEvent::Cancelled`.
    pub async fn cancel(&self, job_id: &str) -> Result<()> {
        // Check if the item exists and is in a non-terminal state
        let item = crate::db::queries::get_item_by_job_id(self.pool.as_ref(), job_id)
            .await?
            .ok_or_else(|| anyhow!("Job '{}' not found", job_id))?;

        if matches!(item.status.as_str(), "completed" | "failed" | "cancelled") {
            return Err(anyhow!(
                "Job '{}' is already in terminal state '{}'",
                job_id,
                item.status
            ));
        }

        crate::db::queries::cancel_queue_item(self.pool.as_ref(), job_id).await?;

        let _ = self.events_tx.send(PullEvent::Cancelled {
            job_id: job_id.to_string(),
            filename: item.filename.clone(),
        });
        Ok(())
    }

    /// Get all active items (queued + running + verifying), ordered by status priority.
    pub async fn get_active_items(&self) -> Result<Vec<PullQueueItem>> {
        crate::db::queries::get_active_items(self.pool.as_ref()).await
    }

    /// Get all active items (queued + running + verifying), ordered by status priority.
    pub async fn get_active_items_dto(&self) -> Result<Vec<PullQueueItem>> {
        crate::db::queries::get_active_items(self.pool.as_ref()).await
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub async fn get_history_items(&self, limit: i64, offset: i64) -> Result<Vec<PullQueueItem>> {
        crate::db::queries::get_history_items(self.pool.as_ref(), limit, offset).await
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub async fn get_history_items_dto(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PullQueueItem>> {
        crate::db::queries::get_history_items(self.pool.as_ref(), limit, offset).await
    }

    /// Count total history items (completed, failed, cancelled).
    pub async fn count_history_items(&self) -> Result<i64> {
        crate::db::queries::count_history_items(self.pool.as_ref()).await
    }

    /// Get a single queue item by job ID.
    pub async fn get_queue_item(&self, job_id: &str) -> Result<Option<PullQueueItem>> {
        crate::db::queries::get_item_by_job_id(self.pool.as_ref(), job_id).await
    }

    /// Subscribe to pull events via a broadcast channel receiver.
    pub fn subscribe_events(&self) -> broadcast::Receiver<PullEvent> {
        self.events_tx.subscribe()
    }
}
