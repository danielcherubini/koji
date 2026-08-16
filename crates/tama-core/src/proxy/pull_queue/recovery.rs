use std::sync::Arc;

use anyhow::Result;
use sqlx::types::time::OffsetDateTime;
use sqlx::PgPool;

use super::service::PullQueueService;

/// Re-queue stale running items so they get retried on startup.
///
/// Clears started_at so the pull restarts fresh (hf-hub resumes if the
/// partial file exists on disk, otherwise it pulls from scratch).
pub(crate) async fn requeue_stale_running(pool: &PgPool) -> Result<()> {
    sqlx::query(
        "UPDATE pull_queue SET status = 'queued', started_at = NULL, completed_at = NULL
         WHERE status = 'running' AND (started_at IS NULL OR
           EXTRACT(EPOCH FROM (now() - started_at)) > 3600)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Atomically claim a queued item as running.
///
/// Returns `true` if a row was affected (item was queued, now running),
/// `false` if no row matched (item already started by someone else).
pub(crate) async fn claim_queued_item(pool: &PgPool, job_id: &str) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE pull_queue SET status = 'running', started_at = now()
         WHERE job_id = $1 AND status = 'queued'",
    )
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Re-queue a running item whose pull task died without registering.
pub(crate) async fn requeue_dead_task(pool: &PgPool, job_id: &str) -> Result<()> {
    sqlx::query("UPDATE pull_queue SET status = 'queued', started_at = NULL WHERE job_id = $1")
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(())
}

impl PullQueueService {
    /// Perform startup recovery: re-queue stale running items so they get retried.
    ///
    /// Clears started_at so the pull restarts fresh (hf-hub resumes if the
    /// partial file exists on disk, otherwise it pulls from scratch).
    pub async fn on_startup_recovery(&self) -> Result<()> {
        requeue_stale_running(self.pool.as_ref()).await
    }

    /// Atomically claim a queued item as running.
    ///
    /// Returns `true` if the item was claimed (was queued, now running),
    /// `false` if it was already started by someone else.
    pub async fn try_mark_running(&self, job_id: &str) -> Result<bool> {
        claim_queued_item(self.pool.as_ref(), job_id).await
    }
}

/// Start a pull from the queue.
///
/// This is the ONLY code path that transitions items from `queued` → `running`.
/// Reads the queued item from DB, constructs a QuantPullSpec, and calls
/// the real pull implementation from pull.rs.
async fn start_pull_from_queue(
    state: Arc<crate::proxy::ProxyState>,
    svc: Arc<PullQueueService>,
    job_id: String,
) {
    // Read the queue item from DB to get details
    let item = match svc.get_queue_item(&job_id).await {
        Ok(Some(item)) => item,
        _ => return,
    };

    // Construct QuantPullSpec from DB data
    let spec = crate::proxy::tama_handlers::QuantPullSpec {
        filename: item.filename.clone(),
        quant: item.quant.clone(),
        context_length: item.context_length,
    };

    // Delegate to the real pull implementation in pull.rs.
    // Note: the caller (queue_processor_loop) already spawned a task,
    // so we call directly without another spawn.
    crate::proxy::tama_handlers::start_pull_from_queue(
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
pub(crate) async fn queue_processor_loop(state: Arc<crate::proxy::ProxyState>) {
    let svc = state
        .pull_queue()
        .as_ref()
        .expect("pull_queue must be configured");

    // Startup recovery: mark stale running items as queued
    if let Err(e) = svc.on_startup_recovery().await {
        tracing::error!(error=%e, "Startup recovery failed");
    }

    let poll_interval = std::cmp::max(svc.poll_interval_secs, 1);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;

        // Check if anything is currently running (only one at a time in sequential mode)
        let active = match svc.get_active_items().await {
            Ok(items) => items,
            Err(e) => {
                tracing::error!(error=%e, "Failed to check active pulls");
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
            let current = match svc.get_queue_item(&item.job_id).await {
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
                let jobs = state.pull.pull_jobs.read().await;
                jobs.contains_key(&item.job_id)
            };
            if !is_alive {
                // Task hasn't registered yet — could be a race condition after spawn,
                // or the task may have crashed before registering. Wait one more poll
                // cycle to give it time, unless started_at is very old (stale).
                if let Some(st) = item.started_at {
                    let now_utc = OffsetDateTime::now_utc();
                    let age = std::time::Duration::from_secs(
                        (now_utc - st).whole_seconds().max(0) as u64
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
                    "Pull task died before registering in pull_jobs — re-queuing"
                );
                if let Err(e) = requeue_dead_task(svc.pool.as_ref(), &item.job_id).await {
                    tracing::error!(error=%e, job_id=%item.job_id, "Failed to re-queue dead task");
                }
                continue;
            }
            // Task is alive or just needs more time. Don't re-queue yet.
            continue;
        }

        // Try to dequeue the next queued item
        let Some(item) = (match svc.dequeue().await {
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
        let was_queued = match svc.try_mark_running(&item.job_id).await {
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
            let _ = svc
                .update_status(&item.job_id, "running", 0, None, None, None)
                .await;
            // Spawn the actual pull (delegated to a separate async function)
            let job_id = item.job_id.clone();
            let state_clone = Arc::clone(&state);
            let svc_clone = Arc::clone(svc);
            tokio::spawn(async move {
                start_pull_from_queue(state_clone, svc_clone, job_id).await;
            });
        }
    }
}
