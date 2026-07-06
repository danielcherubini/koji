use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::task::JoinSet;

use super::download_queue::DownloadQueueService;
use super::pull_jobs::PullJob;

/// Cache entry for discovered GPU devices: (discovered_at, devices).
type GpuDeviceCacheEntry = (Instant, Vec<crate::gpu::GpuDeviceInfo>);

/// State for a model backend.
#[derive(Debug, Clone)]
pub enum ModelState {
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

impl Default for ModelState {
    fn default() -> Self {
        Self::Failed {
            model_name: String::new(),
            backend: String::new(),
            error: String::new(),
        }
    }
}

impl ModelState {
    pub fn model_name(&self) -> &str {
        match self {
            ModelState::Starting { model_name, .. } => model_name,
            ModelState::Ready { model_name, .. } => model_name,
            ModelState::Failed { model_name, .. } => model_name,
            ModelState::Unloading { model_name, .. } => model_name,
        }
    }

    pub fn backend(&self) -> &str {
        match self {
            ModelState::Starting { backend, .. } => backend,
            ModelState::Ready { backend, .. } => backend,
            ModelState::Failed { backend, .. } => backend,
            ModelState::Unloading { backend, .. } => backend,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, ModelState::Ready { .. })
    }

    pub fn backend_url(&self) -> Option<&str> {
        match self {
            ModelState::Ready { backend_url, .. } => Some(backend_url),
            ModelState::Unloading { .. } => None,
            _ => None,
        }
    }

    pub fn backend_pid(&self) -> Option<u32> {
        match self {
            ModelState::Starting { backend_pid, .. } => Some(*backend_pid),
            ModelState::Ready { backend_pid, .. } => Some(*backend_pid),
            ModelState::Unloading { backend_pid, .. } => Some(*backend_pid),
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
            ModelState::Starting {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            ModelState::Ready {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            ModelState::Failed { .. } => None,
            ModelState::Unloading {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
        }
    }

    pub fn load_time(&self) -> Option<std::time::SystemTime> {
        match self {
            ModelState::Ready { load_time, .. } => Some(*load_time),
            ModelState::Unloading { .. } => None,
            _ => None,
        }
    }

    pub fn last_accessed(&self) -> Option<Instant> {
        match self {
            ModelState::Ready { last_accessed, .. } => Some(*last_accessed),
            ModelState::Starting { last_accessed, .. } => Some(*last_accessed),
            ModelState::Failed { .. } => None,
            ModelState::Unloading { last_accessed, .. } => Some(*last_accessed),
        }
    }

    /// Get the restart count for this model (only set on Ready/Unloading states).
    pub fn restart_count(&self) -> Option<u32> {
        match self {
            ModelState::Ready { restart_count, .. } => Some(*restart_count),
            ModelState::Unloading { restart_count, .. } => Some(*restart_count),
            _ => None,
        }
    }

    /// Get the start time for Starting state models.
    pub fn start_time(&self) -> Option<Instant> {
        match self {
            ModelState::Starting { start_time, .. } => Some(*start_time),
            _ => None,
        }
    }

    /// Check if the server has failed and the cooldown has elapsed.
    pub fn can_reload(&self, cooldown_seconds: u64) -> bool {
        match self {
            ModelState::Failed { .. } => false,
            ModelState::Unloading { .. } => false,
            ModelState::Starting {
                failure_timestamp, ..
            }
            | ModelState::Ready {
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
            in_flight_downloads: Arc::clone(&self.in_flight_downloads),
            metrics_tx: self.metrics_tx.clone(),
            download_queue: self.download_queue.clone(),
            config_write_semaphore: Arc::clone(&self.config_write_semaphore),
            backend_logs: self.backend_logs.clone(),
            inference_stats: self.inference_stats.clone(),
            gpu_devices_cache: Arc::clone(&self.gpu_devices_cache),
            model_tasks: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "web-ui")]
            web_jobs: self.web_jobs.clone(),
            #[cfg(feature = "web-ui")]
            web_capabilities: self.web_capabilities.clone(),
            #[cfg(feature = "web-ui")]
            web_update_checker: Arc::clone(&self.web_update_checker),
            #[cfg(feature = "web-ui")]
            web_binary_version: self.web_binary_version.clone(),
            #[cfg(feature = "web-ui")]
            web_update_tx: Arc::clone(&self.web_update_tx),
            #[cfg(feature = "web-ui")]
            web_upload_lock: Arc::clone(&self.web_upload_lock),
        }
    }
}

pub struct ProxyState {
    pub config: Arc<tokio::sync::RwLock<crate::config::Config>>,
    pub model_configs:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::config::ModelConfig>>>,
    /// alias_name → resolved model name (api_name or repo_id)
    /// Only enabled aliases are cached. Populated from DB on init and reload.
    pub aliases: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    pub models: Arc<tokio::sync::RwLock<std::collections::HashMap<String, ModelState>>>,
    pub client: reqwest::Client,
    pub metrics: Arc<ProxyMetrics>,
    pub db_dir: Option<std::path::PathBuf>,
    pub pull_jobs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>>,
    pub system_metrics: Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>>,
    /// Set of destination paths currently being downloaded. Used to prevent
    /// concurrent downloads writing to the same temp files, which would silently
    /// corrupt the assembled output.
    pub in_flight_downloads: Arc<tokio::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>>,
    pub metrics_tx: tokio::sync::broadcast::Sender<crate::gpu::MetricsSnapshot>,
    pub download_queue: Option<Arc<DownloadQueueService>>,
    /// Semaphore controlling concurrent post-pull config writes.
    /// Replaces the old global CONFIG_WRITE_LOCK to allow controlled
    /// parallelism (default capacity=4) instead of full serialization.
    pub config_write_semaphore: Arc<tokio::sync::Semaphore>,
    /// Backend log stream manager — broadcasts backend stdout/stderr via SSE.
    pub backend_logs: crate::backends::log_stream::BackendLogManager,
    /// Watch channel for per-backend inference stats. Keyed by backend_name.
    /// Single-producer (intercept handler), multi-consumer (metrics task).
    pub inference_stats: tokio::sync::watch::Sender<HashMap<String, LatestInferenceStats>>,
    /// Cache for discovered GPU devices, keyed by backend name.
    /// Value is (discovered_at_instant, list_of_devices).
    #[allow(clippy::type_complexity)]
    pub gpu_devices_cache: Arc<tokio::sync::RwLock<HashMap<String, GpuDeviceCacheEntry>>>,
    /// Per-model JoinSets tracking spawned tasks (stdout/stderr readers, reaper).
    /// Used for clean cancellation on unload.
    pub model_tasks: tokio::sync::RwLock<HashMap<String, JoinSet<()>>>,

    // ── Web UI fields (only present when `web-ui` feature is enabled) ──
    /// Job manager for backend install/update/restore/benchmark operations.
    #[cfg(feature = "web-ui")]
    pub web_jobs: Option<Arc<crate::web_types::JobManager>>,
    /// Cache for backend capabilities.
    #[cfg(feature = "web-ui")]
    pub web_capabilities: Option<Arc<crate::web_types::CapabilitiesCache>>,
    /// Shared update checker to prevent concurrent runs across requests.
    #[cfg(feature = "web-ui")]
    pub web_update_checker: Arc<crate::updates::UpdateChecker>,
    /// The version of the running tama binary (passed from the CLI at startup).
    #[cfg(feature = "web-ui")]
    pub web_binary_version: String,
    /// Broadcast sender for self-update progress messages.
    /// `None` when no update is in progress.
    #[cfg(feature = "web-ui")]
    pub web_update_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>>,
    /// Temporary upload storage for restore archives.
    #[cfg(feature = "web-ui")]
    pub web_upload_lock:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, crate::web_types::UploadEntry>>>,
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
    /// - Clears in-flight downloads
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

        // Clear in-flight downloads
        let mut in_flight = self.in_flight_downloads.lock().await;
        in_flight.clear();

        // Clear inference stats
        let _ = self.inference_stats.send_replace(HashMap::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
