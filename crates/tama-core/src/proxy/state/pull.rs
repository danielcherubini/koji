//! Pull job state: active pull jobs, in-flight downloads, and the pull queue service.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use super::super::pull_jobs::PullJob;
use super::super::pull_queue::PullQueueService;
use super::{RepoPullJob, RepoPullStatus};

/// State for managing model pull operations.
#[derive(Clone)]
pub(crate) struct PullState {
    /// Active pull jobs keyed by job_id.
    pub(crate) pull_jobs: Arc<RwLock<HashMap<String, PullJob>>>,
    /// Set of destination paths currently being pulled. Used to prevent
    /// concurrent pulls writing to the same temp files, which would silently
    /// corrupt the assembled output.
    pub(crate) in_flight_pulls: Arc<Mutex<HashSet<PathBuf>>>,
    /// Pull queue service for background pull processing.
    pub(crate) pull_queue: Option<Arc<PullQueueService>>,
    /// Whole-repo `hf` CLI pull jobs, keyed by job_id.
    pub(crate) repo_pulls: Arc<Mutex<HashMap<String, RepoPullJob>>>,
}

impl PullState {
    pub(crate) fn new(pull_queue: Option<Arc<PullQueueService>>) -> Self {
        Self {
            pull_jobs: Arc::new(RwLock::new(HashMap::new())),
            in_flight_pulls: Arc::new(Mutex::new(HashSet::new())),
            pull_queue,
            repo_pulls: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get a pull job by ID, if it exists.
    pub(crate) async fn get_pull_job(&self, job_id: &str) -> Option<PullJob> {
        self.pull_jobs.read().await.get(job_id).cloned()
    }

    /// Insert or update a pull job in the map.
    pub(crate) async fn upsert_pull_job(&self, job_id: String, job: PullJob) {
        self.pull_jobs.write().await.insert(job_id, job);
    }

    /// List all active pull jobs as a Vec.
    #[allow(dead_code)]
    pub(crate) fn list_pull_jobs(&self) -> Vec<PullJob> {
        self.pull_jobs
            .try_read()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear the pull jobs map and the in-flight pulls set, and drop the
    /// whole-repo pull job entries (relayed tamad jobs — no local children
    /// to kill; a relayed download on the pull host keeps running and the
    /// relay converges to its terminal state or times out).
    pub(crate) async fn clear(&self) {
        self.pull_jobs.write().await.clear();
        self.in_flight_pulls.lock().await.clear();
        self.repo_pulls.lock().await.clear();
    }

    /// Insert or update a whole-repo pull job.
    pub(crate) async fn upsert_repo_pull(&self, job: RepoPullJob) {
        self.repo_pulls.lock().await.insert(job.job_id.clone(), job);
    }

    /// Get a clone of a whole-repo pull job by id.
    ///
    /// The `child` and `stderr_tail` fields are shared `Arc`s, so the clone
    /// keeps operating on the same underlying handles.
    pub(crate) async fn get_repo_pull(&self, job_id: &str) -> Option<RepoPullJob> {
        self.repo_pulls.lock().await.get(job_id).cloned()
    }

    /// Run `f` over the mutable in-flight job under a brief job-map lock
    /// hold (no `.await` inside — the concurrency model above).
    pub(crate) async fn with_repo_pull<R>(
        &self,
        job_id: &str,
        f: impl FnOnce(&mut RepoPullJob) -> R,
    ) -> Option<R> {
        let mut map = self.repo_pulls.lock().await;
        let job = map.get_mut(job_id)?;
        Some(f(job))
    }

    /// Whether any whole-repo pull for `repo_id` is currently running.
    pub(crate) async fn repo_pull_running_for(&self, repo_id: &str) -> bool {
        self.repo_pulls
            .lock()
            .await
            .values()
            .any(|job| job.repo_id == repo_id && job.status == RepoPullStatus::Running)
    }

    /// Cancel a running whole-repo pull job.
    ///
    /// Validates and flags the job under a brief job-map lock hold (no
    /// `.await` inside). The `CancelJob` dispatch to the pull host happens
    /// in the public `ProxyState::cancel_repo_pull` wrapper BEFORE this
    /// flag is set; the relay converges the in-memory state when the tamad
    /// sends its terminal `cancelled` event.
    ///
    /// Returns `Err("not found")` for unknown ids and `Err("already finished")`
    /// for jobs in a terminal state.
    pub(crate) async fn cancel_repo_pull(&self, job_id: &str) -> Result<(), String> {
        // Brief lock hold: validate state and flag cancellation (no
        // `.await` inside the guard).
        let mut map = self.repo_pulls.lock().await;
        let job = map.get_mut(job_id).ok_or_else(|| "not found".to_string())?;
        if job.status != RepoPullStatus::Running {
            return Err("already finished".to_string());
        }
        // Flag BEFORE the host RPC (see the public wrapper) so the relay's
        // final status decision can distinguish "killed by user" from
        // "crashed".
        job.cancel_requested = true;
        job.status = RepoPullStatus::Cancelled;

        Ok(())
    }
}
