use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;

use super::pull_jobs::PullJob;
use super::pull_queue::PullQueueService;

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

    /// Check if the server has failed and the cooldown has elapsed.
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
/// TODO(2026-06-27): Consider splitting into sub-structs (ModelRegistry, MetricsCollector, DownloadManager)
/// to reduce the 20+ public fields and improve cohesion.
/// See docs/plans/README.md Code Quality Backlog.
impl Clone for ProxyState {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
            model_configs: Arc::clone(&self.model_configs),
            aliases: Arc::clone(&self.aliases),
            models: Arc::clone(&self.models),
            client: self.client.clone(),
            metrics: Arc::clone(&self.metrics),
            db_dir: self.db_dir.clone(),
            pull_jobs: Arc::clone(&self.pull_jobs),
            system_metrics: Arc::clone(&self.system_metrics),
            in_flight_pulls: Arc::clone(&self.in_flight_pulls),
            metrics_tx: self.metrics_tx.clone(),
            pull_queue: self.pull_queue.clone(),
            config_write_semaphore: Arc::clone(&self.config_write_semaphore),
            backend_logs: self.backend_logs.clone(),
            inference_stats: self.inference_stats.clone(),
            gpu_devices_cache: Arc::clone(&self.gpu_devices_cache),
            model_tasks: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            cookie_key: cookie::Key::generate(),
        }
    }
}

pub struct ProxyState {
    pub(crate) config: Arc<tokio::sync::RwLock<crate::config::Config>>,
    pub(crate) model_configs:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::config::ModelConfig>>>,
    /// alias_name → resolved model name (api_name or repo_id)
    /// Only enabled aliases are cached. Populated from DB on init and reload.
    pub(crate) aliases: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub(crate) models: Arc<tokio::sync::RwLock<std::collections::HashMap<String, BackendState>>>,
    pub(crate) client: reqwest::Client,
    pub(crate) metrics: Arc<ProxyMetrics>,
    pub(crate) db_dir: Option<std::path::PathBuf>,
    pub(crate) pull_jobs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>>,
    pub(crate) system_metrics: Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>>,
    /// Set of destination paths currently being pulled. Used to prevent
    /// concurrent pulls writing to the same temp files, which would silently
    /// corrupt the assembled output.
    pub(crate) in_flight_pulls:
        Arc<tokio::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>>,
    pub(crate) metrics_tx: tokio::sync::broadcast::Sender<crate::gpu::MetricsSnapshot>,
    pub(crate) pull_queue: Option<Arc<PullQueueService>>,
    /// Semaphore controlling concurrent post-pull config writes.
    /// Replaces the old global CONFIG_WRITE_LOCK to allow controlled
    /// parallelism (default capacity=4) instead of full serialization.
    pub(crate) config_write_semaphore: Arc<tokio::sync::Semaphore>,
    /// Backend log stream manager — broadcasts backend stdout/stderr via SSE.
    pub(crate) backend_logs: crate::backends::log_stream::BackendLogManager,
    /// Watch channel for per-backend inference stats. Keyed by backend_name.
    /// Single-producer (intercept handler), multi-consumer (metrics task).
    pub(crate) inference_stats: tokio::sync::watch::Sender<HashMap<String, LatestInferenceStats>>,
    /// Cache for discovered GPU devices, keyed by backend name.
    /// Value is (discovered_at_instant, list_of_devices).
    #[allow(clippy::type_complexity)]
    pub(crate) gpu_devices_cache: Arc<tokio::sync::RwLock<HashMap<String, GpuDeviceCacheEntry>>>,
    /// Per-model JoinSets tracking spawned tasks (stdout/stderr readers, reaper).
    /// Used for clean cancellation on unload.
    pub(crate) model_tasks: tokio::sync::RwLock<HashMap<String, JoinSet<()>>>,
    /// Signing key for session cookies (OAuth2 OIDC login).
    pub(crate) cookie_key: cookie::Key,
}

impl ProxyState {
    /// Open a DB connection for a quick sync operation.
    /// Returns None if db_dir is not configured (e.g., in tests).
    pub fn open_db(&self) -> Option<rusqlite::Connection> {
        self.db_dir
            .as_ref()
            .and_then(|dir| crate::db::open(dir).ok().map(|r| r.conn))
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
        let _ = self.metrics_tx.send(crate::gpu::MetricsSnapshot::default());

        // Clear all loaded models
        let mut models = self.models.write().await;
        models.clear();

        // Abort all per-model task JoinSets (stdout/stderr readers, reapers)
        let mut all_tasks = self.model_tasks.write().await;
        for (_backend, mut tasks) in all_tasks.drain() {
            tasks.abort_all();
        }

        // Clear active pull jobs
        let mut pull_jobs = self.pull_jobs.write().await;
        pull_jobs.clear();

        // Clear in-flight pulls
        let mut in_flight = self.in_flight_pulls.lock().await;
        in_flight.clear();

        // Clear inference stats
        let _ = self.inference_stats.send_replace(HashMap::new());
    }

    // ── Read-only accessors for commonly-accessed fields ──

    /// Returns a reference to the config RwLock.
    pub fn config(&self) -> &Arc<tokio::sync::RwLock<crate::config::Config>> {
        &self.config
    }

    /// Returns a reference to the model configs RwLock.
    pub fn model_configs(
        &self,
    ) -> &Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::config::ModelConfig>>>
    {
        &self.model_configs
    }

    /// Returns a reference to the aliases RwLock.
    pub fn aliases(&self) -> &Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>> {
        &self.aliases
    }

    /// Returns a reference to the models RwLock.
    pub fn models(
        &self,
    ) -> &Arc<tokio::sync::RwLock<std::collections::HashMap<String, BackendState>>> {
        &self.models
    }

    /// Returns a reference to the HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Returns a reference to the metrics.
    pub fn metrics(&self) -> &Arc<ProxyMetrics> {
        &self.metrics
    }

    /// Returns a reference to the database directory.
    pub fn db_dir(&self) -> &Option<std::path::PathBuf> {
        &self.db_dir
    }

    /// Returns a reference to the pull jobs RwLock.
    pub fn pull_jobs(
        &self,
    ) -> &Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>> {
        &self.pull_jobs
    }

    /// Returns a reference to the system metrics RwLock.
    pub fn system_metrics(&self) -> &Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>> {
        &self.system_metrics
    }

    /// Returns a reference to the in-flight pulls Mutex.
    pub fn in_flight_pulls(
        &self,
    ) -> &Arc<tokio::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>> {
        &self.in_flight_pulls
    }

    /// Returns a reference to the metrics broadcast sender.
    pub fn metrics_tx(&self) -> &tokio::sync::broadcast::Sender<crate::gpu::MetricsSnapshot> {
        &self.metrics_tx
    }

    /// Returns a reference to the pull queue service.
    pub fn pull_queue(&self) -> &Option<Arc<PullQueueService>> {
        &self.pull_queue
    }

    /// Sets the pull queue service. Used by tests.
    pub fn set_pull_queue(&mut self, queue: Option<Arc<PullQueueService>>) {
        self.pull_queue = queue;
    }

    /// Returns a reference to the config write semaphore.
    pub fn config_write_semaphore(&self) -> &Arc<tokio::sync::Semaphore> {
        &self.config_write_semaphore
    }

    /// Returns a reference to the backend log stream manager.
    pub fn backend_logs(&self) -> &crate::backends::log_stream::BackendLogManager {
        &self.backend_logs
    }

    /// Returns a reference to the inference stats watch sender.
    pub fn inference_stats(
        &self,
    ) -> &tokio::sync::watch::Sender<HashMap<String, LatestInferenceStats>> {
        &self.inference_stats
    }

    /// Returns a reference to the GPU devices cache RwLock.
    pub fn gpu_devices_cache(
        &self,
    ) -> &Arc<tokio::sync::RwLock<HashMap<String, GpuDeviceCacheEntry>>> {
        &self.gpu_devices_cache
    }

    /// Returns a reference to the model tasks RwLock.
    pub fn model_tasks(&self) -> &tokio::sync::RwLock<HashMap<String, JoinSet<()>>> {
        &self.model_tasks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that ProxyState exposes accessor methods for commonly-accessed fields.
    #[test]
    fn test_proxy_state_accessors_exist() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);

        // Core field accessors return correct types
        let _: &Arc<tokio::sync::RwLock<crate::config::Config>> = state.config();
        let _: &Arc<
            tokio::sync::RwLock<std::collections::HashMap<String, crate::config::ModelConfig>>,
        > = state.model_configs();
        let _: &Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>> =
            state.aliases();
        let _: &Arc<tokio::sync::RwLock<std::collections::HashMap<String, BackendState>>> =
            state.models();
        let _: &reqwest::Client = state.client();
        let _: &Arc<ProxyMetrics> = state.metrics();
        let _: &Option<std::path::PathBuf> = state.db_dir();
        let _: &Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>> =
            state.pull_jobs();
        let _: &Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>> = state.system_metrics();
        let _: &Arc<tokio::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>> =
            state.in_flight_pulls();
        let _: &tokio::sync::broadcast::Sender<crate::gpu::MetricsSnapshot> = state.metrics_tx();
        let _: &Option<Arc<PullQueueService>> = state.pull_queue();
        let _: &Arc<tokio::sync::Semaphore> = state.config_write_semaphore();
        let _: &crate::backends::log_stream::BackendLogManager = state.backend_logs();
        let _: &tokio::sync::watch::Sender<HashMap<String, LatestInferenceStats>> =
            state.inference_stats();
        let _: &Arc<tokio::sync::RwLock<HashMap<String, GpuDeviceCacheEntry>>> =
            state.gpu_devices_cache();
        let _: &tokio::sync::RwLock<HashMap<String, JoinSet<()>>> = state.model_tasks();
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
        // Send stats for a server
        let mut map = HashMap::new();
        map.insert(
            "server-a".to_string(),
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
        let stats = received.get("server-a").unwrap();
        assert_eq!(stats.tps, Some(42.0));
        assert_eq!(stats.cache_hit_pct, Some(75.0));
        assert!(stats.spec_decoding_active);
        assert_eq!(stats.last_updated_ms, 999);
    }

    #[test]
    fn test_inference_stats_per_server_isolation() {
        let (tx, mut rx) =
            tokio::sync::watch::channel::<HashMap<String, LatestInferenceStats>>(HashMap::new());

        // Insert stats for server-a
        let mut map = HashMap::new();
        map.insert(
            "server-a".to_string(),
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

        // Insert stats for server-b
        let mut map2 = rx.borrow_and_update().clone();
        map2.insert(
            "server-b".to_string(),
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

        // Verify both servers have independent stats
        let received = rx.borrow_and_update();
        assert_eq!(received.len(), 2);

        let a = received.get("server-a").unwrap();
        assert_eq!(a.tps, Some(50.0));
        assert!(a.spec_decoding_active);

        let b = received.get("server-b").unwrap();
        assert_eq!(b.tps, Some(30.0));
        assert!(!b.spec_decoding_active);
        assert!(b.spec_accept_pct.is_none());
    }
}
