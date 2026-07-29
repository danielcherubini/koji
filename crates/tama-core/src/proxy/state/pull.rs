//! Pull job state: active pull jobs, in-flight downloads, and the pull queue service.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use super::super::pull_jobs::PullJob;
use super::super::pull_queue::PullQueueService;

/// State for managing model pull operations.
#[derive(Clone, Default)]
pub(crate) struct PullState {
    /// Active pull jobs keyed by job_id.
    pub(crate) pull_jobs: Arc<RwLock<HashMap<String, PullJob>>>,
    /// Set of destination paths currently being pulled. Used to prevent
    /// concurrent pulls writing to the same temp files, which would silently
    /// corrupt the assembled output.
    pub(crate) in_flight_pulls: Arc<Mutex<HashSet<PathBuf>>>,
    /// Pull queue service for background pull processing.
    pub(crate) pull_queue: Option<Arc<PullQueueService>>,
}

impl PullState {
    pub(crate) fn new(pull_queue: Option<Arc<PullQueueService>>) -> Self {
        Self {
            pull_jobs: Arc::new(RwLock::new(HashMap::new())),
            in_flight_pulls: Arc::new(Mutex::new(HashSet::new())),
            pull_queue,
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

    /// Clear both the pull jobs map and the in-flight pulls set.
    pub(crate) async fn clear(&self) {
        self.pull_jobs.write().await.clear();
        self.in_flight_pulls.lock().await.clear();
    }
}
