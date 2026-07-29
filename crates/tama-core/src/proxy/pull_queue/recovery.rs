use std::sync::Arc;

use anyhow::Result;

use super::service::PullQueueService;

impl PullQueueService {
    /// Perform startup recovery: re-queue stale running items so they get retried.
    ///
    /// Clears started_at so the pull restarts fresh (hf-hub resumes if the
    /// partial file exists on disk, otherwise it pulls from scratch).
    pub fn on_startup_recovery(&self) -> Result<()> {
        // Mark stale running items as queued by updating their status.
        // ModelManager doesn't have a dedicated method for this, so we use the
        // raw connection for the SQL update.
        self.model_mgr.lock().unwrap().conn().execute(
            "UPDATE pull_queue SET status = 'queued', started_at = NULL, completed_at = NULL
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
            "UPDATE pull_queue SET status = 'running', started_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE job_id = ?1 AND status = 'queued'",
            [job_id],
        )?;
        Ok(rows > 0)
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
    let item = match svc.get_queue_item(&job_id) {
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
                let jobs = state.pull.pull_jobs.read().await;
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
                    "Pull task died before registering in pull_jobs — re-queuing"
                );
                if let Err(e) = svc.model_mgr.lock().unwrap().conn().execute(
                    "UPDATE pull_queue SET status = 'queued', started_at = NULL WHERE job_id = ?1",
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
