use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;

use super::pull_queue::PullQueueService;
use super::state::repo_pull::{RepoPullError, RepoPullStart, RepoPullStatusDto};
use super::state::{MetricsState, PullState, RegistryState};

/// Cache entry for discovered GPU devices: (discovered_at, devices).
type GpuDeviceCacheEntry = (Instant, Vec<crate::gpu::GpuDeviceInfo>);

/// State for a model backend lifecycle.
#[derive(Debug, Clone)]
pub enum BackendState {
    /// Backend is starting up (placeholder during initialization)
    Starting {
        model_name: String,
        backend: String,
        backend_url: String,
        backend_pid: u32,
        last_accessed: Instant,
        start_time: Instant,
        consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
        failure_timestamp: Option<std::time::SystemTime>,
        is_docker: bool,
    },
    /// Backend is ready and accepting traffic
    Ready {
        model_name: String,
        backend: String,
        backend_pid: u32,
        backend_url: String,
        load_time: std::time::SystemTime,
        last_accessed: Instant,
        consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
        failure_timestamp: Option<std::time::SystemTime>,
        restart_count: u32,
        is_docker: bool,
    },
    /// Backend failed to start
    Failed {
        model_name: String,
        backend: String,
        error: String,
    },
    /// Backend is in the process of being unloaded (holding lock during SIGTERM)
    Unloading {
        model_name: String,
        backend: String,
        backend_pid: u32,
        backend_url: String,
        last_accessed: Instant,
        consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
        failure_timestamp: Option<std::time::SystemTime>,
        restart_count: u32,
        is_docker: bool,
    },
}

impl Default for BackendState {
    fn default() -> Self {
        Self::Failed {
            model_name: String::new(),
            backend: String::new(),
            error: String::new(),
        }
    }
}

impl BackendState {
    pub fn model_name(&self) -> &str {
        match self {
            BackendState::Starting { model_name, .. } => model_name,
            BackendState::Ready { model_name, .. } => model_name,
            BackendState::Failed { model_name, .. } => model_name,
            BackendState::Unloading { model_name, .. } => model_name,
        }
    }

    pub fn backend(&self) -> &str {
        match self {
            BackendState::Starting { backend, .. } => backend,
            BackendState::Ready { backend, .. } => backend,
            BackendState::Failed { backend, .. } => backend,
            BackendState::Unloading { backend, .. } => backend,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, BackendState::Ready { .. })
    }

    pub fn backend_url(&self) -> Option<&str> {
        match self {
            BackendState::Ready { backend_url, .. } => Some(backend_url),
            BackendState::Unloading { .. } => None,
            _ => None,
        }
    }

    pub fn backend_pid(&self) -> Option<u32> {
        match self {
            BackendState::Starting { backend_pid, .. } => Some(*backend_pid),
            BackendState::Ready { backend_pid, .. } => Some(*backend_pid),
            BackendState::Unloading { backend_pid, .. } => Some(*backend_pid),
            _ => None,
        }
    }

    /// Returns true if this backend is running inside a Docker container.
    pub fn is_docker(&self) -> bool {
        match self {
            BackendState::Starting { is_docker, .. }
            | BackendState::Ready { is_docker, .. }
            | BackendState::Unloading { is_docker, .. } => *is_docker,
            BackendState::Failed { .. } => false,
        }
    }

    /// Returns true if this is a TTS backend (identified by backend name prefix).
    /// TTS backends are stored with names like "tts_kokoro" and have their own
    /// lifecycle management separate from LLM models.
    pub fn is_tts_backend(&self) -> bool {
        self.backend().starts_with("tts_")
    }

    /// Returns true if this is a non-inference backend (TTS or compaction).
    /// Non-inference backends are excluded from idle timeout checks and LRU eviction.
    pub fn is_non_inference_backend(&self) -> bool {
        self.backend().starts_with("tts_") || self.backend() == "compaction"
    }

    pub fn consecutive_failures(&self) -> Option<&Arc<std::sync::atomic::AtomicU32>> {
        match self {
            BackendState::Starting {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            BackendState::Ready {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            BackendState::Failed { .. } => None,
            BackendState::Unloading {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
        }
    }

    pub fn load_time(&self) -> Option<std::time::SystemTime> {
        match self {
            BackendState::Ready { load_time, .. } => Some(*load_time),
            BackendState::Unloading { .. } => None,
            _ => None,
        }
    }

    pub fn last_accessed(&self) -> Option<Instant> {
        match self {
            BackendState::Ready { last_accessed, .. } => Some(*last_accessed),
            BackendState::Starting { last_accessed, .. } => Some(*last_accessed),
            BackendState::Failed { .. } => None,
            BackendState::Unloading { last_accessed, .. } => Some(*last_accessed),
        }
    }

    /// Get the restart count for this model (only set on Ready/Unloading states).
    pub fn restart_count(&self) -> Option<u32> {
        match self {
            BackendState::Ready { restart_count, .. } => Some(*restart_count),
            BackendState::Unloading { restart_count, .. } => Some(*restart_count),
            _ => None,
        }
    }

    /// Get the start time for Starting state models.
    pub fn start_time(&self) -> Option<Instant> {
        match self {
            BackendState::Starting { start_time, .. } => Some(*start_time),
            _ => None,
        }
    }

    /// Check if the backend has failed and the cooldown has elapsed.
    pub fn can_reload(&self, cooldown_seconds: u64) -> bool {
        match self {
            BackendState::Failed { .. } => false,
            BackendState::Unloading { .. } => false,
            BackendState::Starting {
                failure_timestamp, ..
            }
            | BackendState::Ready {
                failure_timestamp, ..
            } => failure_timestamp
                .map(|ts| {
                    std::time::SystemTime::now()
                        .duration_since(ts)
                        .map(|d| d.as_secs() >= cooldown_seconds)
                        .unwrap_or(false)
                })
                .unwrap_or(true),
        }
    }
}

/// Metrics for the proxy server.
#[derive(Debug, Default)]
pub struct ProxyMetrics {
    pub total_requests: std::sync::atomic::AtomicU64,
    pub successful_requests: std::sync::atomic::AtomicU64,
    pub failed_requests: std::sync::atomic::AtomicU64,
    pub models_loaded: std::sync::atomic::AtomicU64,
    pub models_unloaded: std::sync::atomic::AtomicU64,
}

/// Latest inference timing stats extracted from llama_cpp response `timings` object.
///
/// Stored behind a `watch` channel in `ProxyState`. Updated on each non-streaming
/// response that includes a `timings` field. Fields are `Option<f32>` — `None` when
/// the value cannot be computed (e.g. division by zero) or has not been observed yet.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LatestInferenceStats {
    /// Token generation speed (predicted_per_second from timings)
    pub tps: Option<f32>,
    /// Prompt processing speed in tokens per second (prompt_per_second from timings)
    pub prompt_tps: Option<f32>,
    /// Cache hit rate percentage (cache_n / prompt_n * 100), None if prompt_n == 0
    pub cache_hit_pct: Option<f32>,
    /// Speculative decoding acceptance rate (draft_n_accepted / draft_n * 100), None if draft_n == 0
    pub spec_accept_pct: Option<f32>,
    /// True if draft_n > 0 has ever been observed (spec decoding is active on this backend)
    pub spec_decoding_active: bool,
    /// Unix ms timestamp of the last update
    pub last_updated_ms: i64,
}

/// Manages proxy state and model lifecycle.
///
/// Composed from three domain sub-structs: `registry` (models/configs/aliases),
/// `metrics` (counters/channels), `pull` (jobs/downloads). Remaining fields
/// are standalone configuration or service handles.
impl Clone for ProxyState {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            metrics: self.metrics.clone(),
            pull: self.pull.clone(),
            config: Arc::clone(&self.config),
            client: self.client.clone(),
            db_dir: self.db_dir.clone(),
            config_write_semaphore: Arc::clone(&self.config_write_semaphore),
            backend_logs: self.backend_logs.clone(),
            gpu_devices_cache: Arc::clone(&self.gpu_devices_cache),
            model_tasks: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            cookie_key: self.cookie_key.clone(),
            langfuse_client: Arc::clone(&self.langfuse_client),
            remote_forwarder: self.remote_forwarder.clone(),
            tamad_clients: Arc::clone(&self.tamad_clients),
            db_pool: self.db_pool.clone(),
        }
    }
}

pub struct ProxyState {
    /// Model registry: loaded backends, model configs, and alias caches.
    pub(crate) registry: RegistryState,
    /// Metrics and channel handles: counters, system metrics, inference stats.
    pub(crate) metrics: MetricsState,
    /// Pull job state: active jobs, in-flight downloads, queue service.
    pub(crate) pull: PullState,
    pub(crate) config: Arc<tokio::sync::RwLock<crate::config::Config>>,
    pub(crate) client: reqwest::Client,
    pub(crate) db_dir: Option<std::path::PathBuf>,
    /// Semaphore controlling concurrent post-pull config writes.
    /// Replaces the old global CONFIG_WRITE_LOCK to allow controlled
    /// parallelism (default capacity=4) instead of full serialization.
    pub(crate) config_write_semaphore: Arc<tokio::sync::Semaphore>,
    /// Backend log stream manager — broadcasts backend stdout/stderr via SSE.
    pub(crate) backend_logs: crate::installations::log_stream::BackendLogManager,
    /// Cache for discovered GPU devices, keyed by backend name.
    /// Value is (discovered_at_instant, list_of_devices).
    #[allow(clippy::type_complexity)]
    pub(crate) gpu_devices_cache: Arc<tokio::sync::RwLock<HashMap<String, GpuDeviceCacheEntry>>>,
    /// Per-model JoinSets tracking spawned tasks (stdout/stderr readers, reaper).
    /// Used for clean cancellation on unload.
    pub(crate) model_tasks: tokio::sync::RwLock<HashMap<String, JoinSet<()>>>,
    /// Signing key for session cookies (OAuth2 OIDC login).
    pub(crate) cookie_key: cookie::Key,
    /// Langfuse observability client, initialized from config at startup.
    /// Wrapped in RwLock so it can be refreshed when config is updated via PATCH.
    pub(crate) langfuse_client:
        Arc<tokio::sync::RwLock<Option<Arc<crate::proxy::forward::langfuse::LangfuseClient>>>>,
    /// HTTP forwarder for remote OpenAI-compatible providers.
    pub(crate) remote_forwarder: crate::proxy::remote::RemoteForwarder,
    /// Pool of tamad clients, keyed by tamad ID.
    /// Uses Arc<RwLock> so mutable access (lazy client creation) works across clones.
    pub(crate) tamad_clients:
        Arc<tokio::sync::RwLock<HashMap<String, crate::tamad::client::TamadClient>>>,
    /// Postgres pool (plan-190). `None` until Postgres becomes required
    /// (Task 9); `main.rs` is the single owner of the pool and hands the
    /// same `Arc<PgPool>` to both `ProxyState` and `WebState`.
    pub(crate) db_pool: Option<Arc<sqlx::PgPool>>,
}

impl ProxyState {
    /// The Postgres pool, if Postgres is enabled (plan-190). `None` during
    /// the SQLite-only migration stages.
    pub fn db_pool(&self) -> Option<Arc<sqlx::PgPool>> {
        self.db_pool.clone()
    }

    /// Start a whole-repo `hf` CLI pull.
    ///
    /// `model_id` is the pre-created stub row (None = no DB update on
    /// completion). Takes `&Arc<Self>` so the spawned wait-loop can clone the
    /// state and outlive the caller.
    pub async fn start_repo_pull(
        self: &Arc<Self>,
        repo_id: &str,
        model_id: Option<i64>,
    ) -> Result<RepoPullStart, RepoPullError> {
        super::state::repo_pull::start_repo_pull(self, repo_id, model_id).await
    }

    /// Live status snapshot of a whole-repo pull job, or `None` if the job id
    /// is unknown.
    ///
    /// `bytes_done` is computed here (inside tama-core) via `scan_dir_bytes`,
    /// wrapped in `spawn_blocking` so the recursive fs walk doesn't block a
    /// web worker thread.
    pub async fn get_repo_pull_status(&self, job_id: &str) -> Option<RepoPullStatusDto> {
        let job = self.pull.get_repo_pull(job_id).await?;
        let dest = job.dest.clone();
        let bytes_done =
            tokio::task::spawn_blocking(move || super::state::repo_pull::scan_dir_bytes(&dest))
                .await
                .unwrap_or(0);
        Some(RepoPullStatusDto {
            job_id: job.job_id.clone(),
            status: job.status.to_string(),
            bytes_done,
            total_bytes: job.total_bytes,
            error: job.error.clone(),
            context_length: job.context_length,
        })
    }

    /// Cancel + kill a running whole-repo pull job.
    ///
    /// Err message is user-facing: "not found" / "already finished".
    pub async fn cancel_repo_pull(&self, job_id: &str) -> Result<(), String> {
        self.pull.cancel_repo_pull(job_id).await
    }

    /// Gracefully shut down the proxy state.
    ///
    /// This method is called during a hard restart to clean up resources:
    /// - Closes the metrics broadcast channel to stop metrics streaming
    /// - Clears all loaded models from the models map
    /// - Clears active pull jobs
    /// - Clears in-flight pulls
    pub async fn shutdown(&self) {
        // Close the metrics broadcast channel to stop the metrics stream
        let _ = self
            .metrics
            .metrics_tx
            .send(crate::gpu::MetricsSnapshot::default());

        // Clear all loaded models
        let mut models = self.registry.models.write().await;
        models.clear();

        // Abort all per-model task JoinSets (stdout/stderr readers, reapers)
        let mut all_tasks = self.model_tasks.write().await;
        for (_backend, mut tasks) in all_tasks.drain() {
            tasks.abort_all();
        }

        // Clear inference stats
        self.metrics.clear_inference_stats();

        // Clear pull jobs and in-flight pulls
        self.pull.clear().await;
    }

    /// Returns a reference to the HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Returns a reference to the database directory.
    pub fn db_dir(&self) -> &Option<std::path::PathBuf> {
        &self.db_dir
    }

    /// Returns a reference to the pull queue service.
    pub fn pull_queue(&self) -> &Option<Arc<PullQueueService>> {
        &self.pull.pull_queue
    }

    /// Sets the pull queue service. Used by tests in other workspace crates.
    #[allow(dead_code)]
    pub fn set_pull_queue(&mut self, queue: Option<Arc<PullQueueService>>) {
        self.pull.pull_queue = queue;
    }

    /// Returns a reference to the backend log stream manager.
    pub fn backend_logs(&self) -> &crate::installations::log_stream::BackendLogManager {
        &self.backend_logs
    }

    /// Perform a health check against a tamad instance.
    ///
    /// Looks up the tamad in the client pool, creating it lazily from the DB
    /// if not yet cached. Returns `true` if the tamad reports status "ok".
    /// Connection errors (network unreachable, refused, etc.) propagate as `Err`.
    pub async fn tamad_health_check(&self, tamad_id: &str) -> anyhow::Result<bool> {
        let mut clients = self.tamad_clients.write().await;

        // Fast path: client already cached
        if let Some(client) = clients.get_mut(tamad_id) {
            return client.health_check().await;
        }

        // Slow path: load from DB and create client
        let pool = self
            .db_pool()
            .with_context(|| "Database not available for tamad lookup")?;
        let tamad_record = crate::db::queries::get_tamad(pool.as_ref(), tamad_id)
            .await
            .with_context(|| "Failed to look up tamad in database")?
            .ok_or_else(|| anyhow::anyhow!("tamad '{}' not found in registry", tamad_id))?;

        let client = crate::tamad::client::TamadClient::new(&tamad_record);
        clients.insert(tamad_id.to_string(), client);

        clients
            .get_mut(tamad_id)
            .ok_or_else(|| anyhow::anyhow!("Failed to get newly inserted tamad client"))?
            .health_check()
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the get_repo_pull_status delegate builds a DTO with a
    /// computed bytes_done (recursive file sizes under dest) and that an
    /// unknown job id yields None.
    #[tokio::test]
    async fn test_get_repo_pull_status_dto() {
        let state = Arc::new(ProxyState::new(
            crate::config::Config::default(),
            None,
            None,
        ));
        let dest = tempfile::tempdir().unwrap();
        std::fs::write(dest.path().join("a.bin"), vec![0u8; 100]).unwrap();
        let nested = dest.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("b.bin"), vec![1u8; 50]).unwrap();

        let child_arc: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        state
            .pull
            .upsert_repo_pull(crate::proxy::state::RepoPullJob {
                job_id: "job-dto".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: Some(7),
                dest: dest.path().to_path_buf(),
                total_bytes: Some(300),
                status: crate::proxy::state::RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                child: child_arc,
            })
            .await;

        let dto = state
            .get_repo_pull_status("job-dto")
            .await
            .expect("running job should have a DTO");
        assert_eq!(dto.job_id, "job-dto");
        assert_eq!(dto.status, "running");
        assert_eq!(dto.bytes_done, 150);
        assert_eq!(dto.total_bytes, Some(300));
        assert!(dto.error.is_none());
        assert!(dto.context_length.is_none());

        assert!(state.get_repo_pull_status("missing").await.is_none());
    }

    /// Test that the cancel_repo_pull delegate surfaces user-facing errors
    /// ("not found" / "already finished") and flags a running job.
    #[tokio::test]
    async fn test_cancel_repo_pull_delegate() {
        let state = Arc::new(ProxyState::new(
            crate::config::Config::default(),
            None,
            None,
        ));

        assert_eq!(
            state.cancel_repo_pull("missing").await,
            Err("not found".to_string())
        );

        let child_arc: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        state
            .pull
            .upsert_repo_pull(crate::proxy::state::RepoPullJob {
                job_id: "job-cancel-dto".to_string(),
                repo_id: "owner/repo".to_string(),
                model_id: None,
                dest: std::path::PathBuf::from("/tmp/models/owner/repo"),
                total_bytes: None,
                status: crate::proxy::state::RepoPullStatus::Running,
                error: None,
                cancel_requested: false,
                context_length: None,
                stderr_tail: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                child: child_arc,
            })
            .await;

        state
            .cancel_repo_pull("job-cancel-dto")
            .await
            .expect("first cancel should succeed");
        assert_eq!(
            state.cancel_repo_pull("job-cancel-dto").await,
            Err("already finished".to_string())
        );

        // The DTO reflects the cancellation (status is terminal, no error).
        let dto = state
            .get_repo_pull_status("job-cancel-dto")
            .await
            .expect("job should still be queryable");
        assert_eq!(dto.status, "cancelled");
        assert!(dto.error.is_none());
    }

    /// Test that the start_repo_pull delegate (Arc receiver, public boundary)
    /// validates the repo id before any other work.
    #[tokio::test]
    async fn test_start_repo_pull_delegate_invalid_id() {
        let state = Arc::new(ProxyState::new(
            crate::config::Config::default(),
            None,
            None,
        ));
        let err = state
            .start_repo_pull("a/b\\c", None)
            .await
            .expect_err("invalid repo id must be rejected");
        assert!(
            matches!(err, crate::proxy::RepoPullError::InvalidRepoId(_)),
            "expected InvalidRepoId, got: {err:?}"
        );
    }

    /// Verify the public surface exposes service handles and sub-struct
    /// composition — not lock guards.
    #[test]
    fn test_proxy_state_public_surface() {
        let state = ProxyState::new(crate::config::Config::default(), None, None);
        let _: &reqwest::Client = state.client();
        let _: &Option<std::path::PathBuf> = state.db_dir();
        let _: &Option<Arc<PullQueueService>> = state.pull_queue();
        let _: &crate::installations::log_stream::BackendLogManager = state.backend_logs();
        // Sub-structs are composed and independently cloneable.
        let _registry = state.registry.clone();
        let _metrics = state.metrics.clone();
        let _pull = state.pull.clone();
    }

    #[test]
    fn test_latest_inference_stats_default() {
        let stats = LatestInferenceStats::default();
        assert!(stats.tps.is_none());
        assert!(stats.prompt_tps.is_none());
        assert!(stats.cache_hit_pct.is_none());
        assert!(stats.spec_accept_pct.is_none());
        assert!(!stats.spec_decoding_active);
        assert_eq!(stats.last_updated_ms, 0);
    }

    #[test]
    fn test_latest_inference_stats_clone_copy() {
        let stats = LatestInferenceStats {
            tps: Some(50.0),
            prompt_tps: Some(200.0),
            cache_hit_pct: Some(85.5),
            spec_accept_pct: Some(90.0),
            spec_decoding_active: true,
            last_updated_ms: 1234567890,
        };
        // Test Copy
        let stats2: LatestInferenceStats = stats;
        assert_eq!(stats2.tps, Some(50.0));
        assert!(stats2.spec_decoding_active);
        // Original is still usable after copy
        assert_eq!(stats.tps, Some(50.0));
        // Test Clone
        let stats3 = stats;
        assert_eq!(stats3.prompt_tps, Some(200.0));
    }

    #[test]
    fn test_latest_inference_stats_serialization() {
        let stats = LatestInferenceStats {
            tps: Some(50.0),
            prompt_tps: Some(200.0),
            cache_hit_pct: Some(85.5),
            spec_accept_pct: Some(90.0),
            spec_decoding_active: true,
            last_updated_ms: 1700000000000,
        };

        let json = serde_json::to_string(&stats).expect("serialization failed");
        let value: serde_json::Value = serde_json::from_str(&json).expect("deserialization failed");

        // All 6 fields must be present
        assert!(value.get("tps").is_some(), "missing field: tps");
        assert!(
            value.get("prompt_tps").is_some(),
            "missing field: prompt_tps"
        );
        assert!(
            value.get("cache_hit_pct").is_some(),
            "missing field: cache_hit_pct"
        );
        assert!(
            value.get("spec_accept_pct").is_some(),
            "missing field: spec_accept_pct"
        );
        assert!(
            value.get("spec_decoding_active").is_some(),
            "missing field: spec_decoding_active"
        );
        assert!(
            value.get("last_updated_ms").is_some(),
            "missing field: last_updated_ms"
        );

        // Correct types: f32 -> number, bool -> bool, i64 -> number
        assert_eq!(value["tps"], serde_json::json!(50.0));
        assert_eq!(value["prompt_tps"], serde_json::json!(200.0));
        assert_eq!(value["cache_hit_pct"], serde_json::json!(85.5));
        assert_eq!(value["spec_accept_pct"], serde_json::json!(90.0));
        assert_eq!(value["spec_decoding_active"], serde_json::json!(true));
        assert_eq!(
            value["last_updated_ms"],
            serde_json::json!(1700000000000_i64)
        );

        // Test with None values (not yet observed)
        let empty = LatestInferenceStats::default();
        let json_empty = serde_json::to_string(&empty).expect("serialization failed");
        let value_empty: serde_json::Value =
            serde_json::from_str(&json_empty).expect("deserialization failed");
        assert!(value_empty["tps"].is_null());
        assert!(value_empty["prompt_tps"].is_null());
        assert!(value_empty["cache_hit_pct"].is_null());
        assert!(value_empty["spec_accept_pct"].is_null());
        assert_eq!(
            value_empty["spec_decoding_active"],
            serde_json::json!(false)
        );
        assert_eq!(value_empty["last_updated_ms"], serde_json::json!(0_i64));
    }

    #[test]
    fn test_inference_stats_watch_round_trip() {
        let (tx, mut rx) =
            tokio::sync::watch::channel::<HashMap<String, LatestInferenceStats>>(HashMap::new());
        // Initial value is empty
        assert!(rx.borrow_and_update().is_empty());
        // Send stats for a backend
        let mut map = HashMap::new();
        map.insert(
            "backend-a".to_string(),
            LatestInferenceStats {
                tps: Some(42.0),
                prompt_tps: Some(100.0),
                cache_hit_pct: Some(75.0),
                spec_accept_pct: Some(80.0),
                spec_decoding_active: true,
                last_updated_ms: 999,
            },
        );
        tx.send_replace(map);
        // Verify
        let received = rx.borrow_and_update();
        assert_eq!(received.len(), 1);
        let stats = received.get("backend-a").unwrap();
        assert_eq!(stats.tps, Some(42.0));
        assert_eq!(stats.cache_hit_pct, Some(75.0));
        assert!(stats.spec_decoding_active);
        assert_eq!(stats.last_updated_ms, 999);
    }

    #[test]
    fn test_inference_stats_per_backend_isolation() {
        let (tx, mut rx) =
            tokio::sync::watch::channel::<HashMap<String, LatestInferenceStats>>(HashMap::new());

        // Insert stats for backend-a
        let mut map = HashMap::new();
        map.insert(
            "backend-a".to_string(),
            LatestInferenceStats {
                tps: Some(50.0),
                prompt_tps: Some(200.0),
                cache_hit_pct: Some(85.0),
                spec_accept_pct: Some(90.0),
                spec_decoding_active: true,
                last_updated_ms: 1000,
            },
        );
        tx.send_replace(map);

        // Insert stats for backend-b
        let mut map2 = rx.borrow_and_update().clone();
        map2.insert(
            "backend-b".to_string(),
            LatestInferenceStats {
                tps: Some(30.0),
                prompt_tps: Some(100.0),
                cache_hit_pct: Some(50.0),
                spec_accept_pct: None,
                spec_decoding_active: false,
                last_updated_ms: 2000,
            },
        );
        tx.send_replace(map2);

        // Verify both backends have independent stats
        let received = rx.borrow_and_update();
        assert_eq!(received.len(), 2);

        let a = received.get("backend-a").unwrap();
        assert_eq!(a.tps, Some(50.0));
        assert!(a.spec_decoding_active);

        let b = received.get("backend-b").unwrap();
        assert_eq!(b.tps, Some(30.0));
        assert!(!b.spec_decoding_active);
        assert!(b.spec_accept_pct.is_none());
    }
}
