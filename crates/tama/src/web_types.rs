//! Web UI types for tama.
//!
//! These types are defined in the tama crate (not tama-core) to keep tama-core
//! free of web-specific concepts. The tama crate owns all web UI state management.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tama_core::gpu::BuildPrerequisites;
use tama_core::installations::InstallationType;
use tama_core::updates::UpdateChecker;
use tokio::sync::{broadcast, Mutex, RwLock};

// ── Job types ────────────────────────────────────────────────────────────────

pub type JobId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Install,
    Update,
    Restore,
    Benchmark,
}

#[derive(Debug, Clone)]
pub enum JobEvent {
    Log(String),
    Status(JobStatus),
    /// Structured result payload for the job (currently: benchmark results JSON).
    Result(String),
}

pub struct JobState {
    pub status: JobStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

impl std::fmt::Debug for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobState")
            .field("status", &self.status)
            .field("started_at", &self.started_at)
            .field("finished_at", &self.finished_at)
            .field("error", &self.error)
            .finish()
    }
}

pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    /// Backend type as a string (e.g., "llama_cpp", "llama_server").
    /// Converted to InstallationType when needed by tama-core.
    pub backend_type: Option<String>,
    pub state: RwLock<JobState>,
    pub log_head: RwLock<VecDeque<String>>,
    pub log_tail: RwLock<VecDeque<String>>,
    pub log_dropped: AtomicU64,
    pub log_tx: broadcast::Sender<JobEvent>,
    /// Benchmark results JSON (set when benchmark completes)
    pub benchmark_results: RwLock<Option<String>>,
    pub child_pids: RwLock<Vec<u32>>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("backend_type", &self.backend_type)
            .field("state", &self.state.try_read().ok())
            .field("log_head", &self.log_head.try_read().ok())
            .field("log_tail", &self.log_tail.try_read().ok())
            .field(
                "log_dropped",
                &self.log_dropped.load(std::sync::atomic::Ordering::Relaxed),
            )
            .field("benchmark_results", &self.benchmark_results.try_read().ok())
            .field("child_pids", &self.child_pids.try_read().ok())
            .finish()
    }
}

/// Maximum number of log lines to retain in the head buffer (oldest 100 lines).
pub const LOG_HEAD_CAP: usize = 100;
/// Maximum number of recent log lines retained after the head is full.
pub const LOG_TAIL_CAP: usize = 400;
/// Broadcast channel capacity for live log delivery.
pub const LOG_BROADCAST_CAP: usize = 1024;
pub const RETAINED_FINISHED_JOBS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("another backend job is already running")]
    AlreadyRunning(JobId),
    #[error("job not found")]
    NotFound,
}

#[derive(Clone, Debug)]
pub struct JobManager {
    jobs: Arc<RwLock<HashMap<JobId, Arc<Job>>>>,
    finished_order: Arc<Mutex<VecDeque<JobId>>>,
    active: Arc<Mutex<Option<JobId>>>,
}

impl JobManager {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            finished_order: Arc::new(Mutex::new(VecDeque::new())),
            active: Arc::new(Mutex::new(None)),
        }
    }

    /// Reserve an active slot, return a fresh Job. Returns AlreadyRunning if one is active.
    pub async fn submit(
        &self,
        kind: JobKind,
        backend_type: Option<InstallationType>,
    ) -> Result<Arc<Job>, JobError> {
        let job_id = format!("j_{}", uuid::Uuid::new_v4().simple());

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let job = Arc::new(Job {
            id: job_id.clone(),
            kind,
            backend_type: backend_type.map(|bt| bt.to_string()),
            state: RwLock::new(JobState {
                status: JobStatus::Running,
                started_at: now,
                finished_at: None,
                error: None,
            }),
            log_head: RwLock::new(VecDeque::new()),
            log_tail: RwLock::new(VecDeque::new()),
            log_dropped: AtomicU64::new(0),
            log_tx: broadcast::channel(LOG_BROADCAST_CAP).0,
            child_pids: RwLock::new(Vec::new()),
            benchmark_results: RwLock::new(None),
        });

        let mut active = self.active.lock().await;
        if let Some(ref id) = *active {
            return Err(JobError::AlreadyRunning(id.clone()));
        }
        *active = Some(job_id.clone());
        drop(active);

        self.jobs.write().await.insert(job_id.clone(), job.clone());

        Ok(job)
    }

    pub async fn get(&self, id: &JobId) -> Option<Arc<Job>> {
        self.jobs.read().await.get(id).cloned()
    }

    pub async fn active(&self) -> Option<Arc<Job>> {
        let active_id = self.active.lock().await.clone();
        if let Some(id) = active_id {
            self.jobs.read().await.get(&id).cloned()
        } else {
            None
        }
    }

    /// Append a log line to the job.
    pub async fn append_log(&self, job: &Job, line: String) {
        if line.contains("pid=") {
            if let Some(start) = line.find("pid=") {
                let pid_str = &line[start + 4..];
                let end = pid_str
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(pid_str.len());
                if let Ok(pid) = pid_str[..end].parse::<u32>() {
                    self.register_child(job, pid).await;
                }
            }
        }

        let mut head = job.log_head.write().await;

        if head.len() < LOG_HEAD_CAP {
            head.push_back(line.clone());
            drop(head);
            let _ = job.log_tx.send(JobEvent::Log(line.clone()));
            return;
        }

        drop(head);

        let mut tail = job.log_tail.write().await;
        if tail.len() < LOG_TAIL_CAP {
            tail.push_back(line.clone());
        } else {
            tail.pop_front();
            tail.push_back(line.clone());
            job.log_dropped.fetch_add(1, Ordering::Relaxed);
        }
        drop(tail);

        let _ = job.log_tx.send(JobEvent::Log(line));
    }

    /// Register a child process PID for this job.
    pub async fn register_child(&self, job: &Job, pid: u32) {
        let mut pids = job.child_pids.write().await;
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }

    /// Kill all child processes for a job.
    pub async fn kill_children(&self, job: &Job) {
        let pids = job.child_pids.read().await;
        if pids.is_empty() {
            return;
        }

        {
            let mut sigterm_futures = Vec::new();
            for &pid in pids.iter() {
                sigterm_futures.push(tokio::task::spawn_blocking(move || {
                    let _ = std::process::Command::new("kill")
                        .arg("-SIGTERM")
                        .arg(pid.to_string())
                        .status();
                }));
            }

            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                futures_util::future::join_all(sigterm_futures),
            )
            .await;

            for &pid in pids.iter() {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = std::process::Command::new("kill")
                        .arg("-SIGKILL")
                        .arg(pid.to_string())
                        .status();
                    #[cfg(unix)]
                    {
                        let _ =
                            nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid as i32), None);
                    }
                })
                .await;
            }
        }

        tracing::info!("Killed {} child process(es) for job {}", pids.len(), job.id);
    }

    /// Mark the job terminal, broadcast the status event, release the active slot,
    /// and FIFO-evict finished jobs beyond RETAINED_FINISHED_JOBS.
    pub async fn finish(&self, job: &Job, status: JobStatus, error: Option<String>) {
        {
            let mut state = job.state.write().await;
            state.status = status;
            state.finished_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
            );
            state.error = error;
        }

        let _ = job.log_tx.send(JobEvent::Status(status));

        *self.active.lock().await = None;

        let mut finished_order = self.finished_order.lock().await;
        finished_order.push_back(job.id.clone());

        while finished_order.len() > RETAINED_FINISHED_JOBS {
            if let Some(evict_id) = finished_order.pop_front() {
                self.jobs.write().await.remove(&evict_id);
            }
        }
    }
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Capabilities types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilitiesDto {
    pub os: String,
    pub arch: String,
    pub git_available: bool,
    pub cmake_available: bool,
    pub compiler_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_cuda_version: Option<String>,
    pub supported_cuda_versions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CapabilitiesCache {
    inner: Arc<tokio::sync::Mutex<Option<(std::time::Instant, CapabilitiesDto)>>>,
}

impl CapabilitiesCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    pub async fn get_or_compute(
        &self,
        detect_prereqs: fn() -> BuildPrerequisites,
        detect_cuda: fn() -> Option<String>,
    ) -> anyhow::Result<CapabilitiesDto> {
        use std::time::Duration;

        let now = std::time::Instant::now();
        let mut guard = self.inner.lock().await;

        if let Some((cached_at, cached)) = &*guard {
            if now.duration_since(*cached_at) < Duration::from_secs(5) {
                return Ok(cached.clone());
            }
        }

        let result = tokio::task::spawn_blocking(move || {
            let caps = detect_prereqs();
            let cuda = detect_cuda();
            CapabilitiesDto {
                os: caps.os,
                arch: caps.arch,
                git_available: caps.git_available,
                cmake_available: caps.cmake_available,
                compiler_available: caps.compiler_available,
                detected_cuda_version: cuda,
                supported_cuda_versions: vec![
                    "11.1".to_string(),
                    "12.4".to_string(),
                    "13.1".to_string(),
                ],
            }
        })
        .await;

        let caps = match result {
            Ok(c) => c,
            Err(e) => {
                return Err(anyhow::anyhow!("Failed to detect capabilities: {}", e));
            }
        };

        *guard = Some((now, caps.clone()));
        Ok(caps)
    }
}

impl Default for CapabilitiesCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Upload types ─────────────────────────────────────────────────────────────

/// Temporary upload entry for restore archives.
#[derive(Clone, Debug)]
pub struct UploadEntry {
    pub path: std::path::PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ── WebState ─────────────────────────────────────────────────────────────────

/// Web UI state.
///
/// Contains only the fields needed by the web control plane (SSR + CSR).
/// Defined in tama crate to avoid circular dependency with tama-core.
#[derive(Debug, Clone)]
pub struct WebState {
    /// Job manager for backend install/update/restore/benchmark operations.
    pub jobs: Option<Arc<JobManager>>,
    /// Cache for backend capabilities.
    pub capabilities: Option<Arc<CapabilitiesCache>>,
    /// Shared update checker to prevent concurrent runs across requests.
    pub update_checker: Arc<UpdateChecker>,
    /// The version of the running tama binary (passed from the CLI at startup).
    pub binary_version: String,
    /// Broadcast sender for self-update progress messages.
    /// `None` when no update is in progress.
    pub update_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>>,
    /// Temporary upload storage for restore archives.
    pub upload_lock: Arc<tokio::sync::RwLock<std::collections::HashMap<String, UploadEntry>>>,
    pub repository: Option<std::sync::Arc<std::sync::Mutex<tama_core::db::repository::Repository>>>,
}

// Compile-time check: WebState must be Clone + Send + Sync + 'static for the
// axum Extension extractor to work.
const _: fn() = || {
    fn assert_clone_send_sync<T: Clone + Send + Sync + 'static>() {}
    assert_clone_send_sync::<WebState>();
};
