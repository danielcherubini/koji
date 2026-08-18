//! Pool of per-tamad stats streams (plan-191 Task 4).
//!
//! [`TamadPool`] maintains one persistent `StreamStats` gRPC connection per
//! registered tamad, resilient to reconnects with capped exponential
//! backoff. The latest snapshot per tamad is available via
//! [`TamadHandle::latest`] for dashboard fan-out, and the live online/offline
//! status is mirrored into `tamad_registry.status`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use sqlx::PgPool;

use crate::providers::TamadConnection;
use crate::tamad::client::TamadClient;
use crate::tamad::SystemStats;

/// Default base delay for the reconnect backoff (doubles per attempt).
const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Maximum reconnect backoff delay.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// A fresh stats snapshot received from a tamad stream.
#[derive(Debug, Clone)]
pub struct LatestStats {
    /// The latest `SystemStats` received from the tamad.
    pub stats: SystemStats,
    /// When the snapshot was received.
    pub at: Instant,
}

/// The last successful `HealthCheck` result for a tamad (plan-191 Task 9):
/// the tamad's self-reported version, refreshed on every stream (re)connect.
#[derive(Debug, Clone)]
pub struct HealthState {
    /// The tamad binary's version string.
    pub version: String,
    /// When the check succeeded.
    pub checked_at: Instant,
}

/// One tamad connection in the pool: client, latest snapshot, live status,
/// and the background stream task.
pub struct TamadHandle {
    /// The registered connection record (id, name, url, protocol, token).
    pub connection: TamadConnection,
    /// One-off RPC client. Mutex because `TamadClient::ensure_channel`
    /// mutates the lazy channel cache; the stats stream task holds the
    /// lock only for the duration of the stream-open call.
    client: tokio::sync::Mutex<TamadClient>,
    latest: tokio::sync::RwLock<Option<LatestStats>>,
    health: tokio::sync::RwLock<Option<HealthState>>,
    online: tokio::sync::watch::Sender<bool>,
    cancel: tokio::sync::watch::Sender<bool>,
    task: tokio::sync::Mutex<Option<tokio::task::AbortHandle>>,
}

impl TamadHandle {
    fn new(connection: TamadConnection) -> Self {
        let (online, _) = tokio::sync::watch::channel(false);
        let (cancel, _) = tokio::sync::watch::channel(false);
        Self {
            client: tokio::sync::Mutex::new(TamadClient::new(&connection)),
            connection,
            latest: tokio::sync::RwLock::new(None),
            health: tokio::sync::RwLock::new(None),
            online,
            cancel,
            task: tokio::sync::Mutex::new(None),
        }
    }

    /// The latest stats snapshot, if the stream has delivered one.
    ///
    /// Snapshots are ~1s fresh while the tamad is up; callers that act on
    /// the data (e.g. the reconciler) must treat a snapshot older than a few
    /// seconds as stale.
    pub async fn latest(&self) -> Option<SystemStats> {
        self.latest.read().await.as_ref().map(|l| l.stats.clone())
    }

    /// The latest stats snapshot if it is at most `max_age` old.
    ///
    /// For callers that *act* on snapshot data (the reconciler): never act
    /// on stale data — a missing/old snapshot means "skip this tick".
    pub async fn latest_fresh(&self, max_age: Duration) -> Option<SystemStats> {
        let latest = self.latest.read().await;
        let l = latest.as_ref()?;
        (Instant::now().duration_since(l.at) <= max_age).then(|| l.stats.clone())
    }

    /// Whether the stats stream is currently open.
    pub async fn is_online(&self) -> bool {
        *self.online.borrow()
    }

    /// The last cached tamad version (from `HealthCheck`, refreshed on every
    /// stream (re)connect; `None` when no successful health check has been
    /// observed yet — e.g. the tamad is offline from the start).
    pub async fn version(&self) -> Option<String> {
        self.health.read().await.as_ref().map(|h| h.version.clone())
    }

    /// The full last `HealthCheck` result, if any.
    pub async fn health_state(&self) -> Option<HealthState> {
        self.health.read().await.clone()
    }

    /// Load a model on this tamad (one-off `LoadModel` RPC, plan-191
    /// Task 5). The lock is held for the duration of the call.
    pub async fn load_model(
        &self,
        req: &crate::tamad::LoadModelRequest,
    ) -> Result<crate::tamad::LoadModelResponse> {
        let mut client = self.client.lock().await;
        client.load_model(req).await
    }

    /// Unload a model on this tamad (one-off `UnloadModel` RPC, plan-191
    /// Task 5).
    pub async fn unload_model(&self, model_name: &str) -> Result<()> {
        let mut client = self.client.lock().await;
        client
            .unload_model(&crate::tamad::UnloadModelRequest {
                provider_name: String::new(),
                model_name: model_name.to_string(),
            })
            .await
    }

    /// Dispatch a model pull on this tamad (plan-191 Task 6).
    ///
    /// Returns the tamad-side job id; stream progress with
    /// [`stream_job`](Self::stream_job).
    pub async fn pull_model(&self, req: &crate::tamad::PullModelRequest) -> Result<String> {
        let mut client = self.client.lock().await;
        client.pull_model(req).await
    }

    /// Cancel a running job on this handle (plan-191 follow-up B).
    ///
    /// Returns the tamad's idempotent flag: `true` = the job was running
    /// and its runner was asked to stop; `false` = unknown or already
    /// terminal (safe to retry after reconnects).
    pub async fn cancel_job(&self, tamad_job_id: &str) -> Result<bool> {
        let mut client = self.client.lock().await;
        client.cancel_job(tamad_job_id).await
    }

    /// Dispatch a backend install on this tamad (plan-191 Task 7).
    ///
    /// Returns the tamad-side job id; stream progress with
    /// [`stream_job`](Self::stream_job).
    pub async fn install_provider(
        &self,
        req: &crate::tamad::InstallProviderRequest,
    ) -> Result<String> {
        let mut client = self.client.lock().await;
        client.install_provider(req).await
    }

    /// Dispatch a backend update on this tamad (plan-191 Task 7).
    pub async fn update_provider(
        &self,
        req: &crate::tamad::UpdateProviderRequest,
    ) -> Result<String> {
        let mut client = self.client.lock().await;
        client.update_provider(req).await
    }

    /// Remove a backend install (files + processes) on this tamad
    /// (plan-191 Task 7, synchronous).
    pub async fn remove_provider(&self, req: &crate::tamad::RemoveProviderRequest) -> Result<()> {
        let mut client = self.client.lock().await;
        client.remove_provider(req).await
    }

    /// Dispatch a benchmark on this tamad (plan-191 Task 8).
    ///
    /// Returns the tamad-side job id; stream progress with
    /// [`stream_job`](Self::stream_job).
    pub async fn run_benchmark(&self, req: &crate::tamad::RunBenchmarkRequest) -> Result<String> {
        let mut client = self.client.lock().await;
        client.run_benchmark(req).await
    }

    /// Open the job stream for a tamad job on this handle (plan-191
    /// Task 6). A fresh channel is opened per stream.
    pub async fn stream_job(
        &self,
        job_id: &str,
    ) -> Result<tonic::Streaming<crate::tamad::JobEvent>> {
        let client = self.client.lock().await;
        client.stream_job(job_id).await
    }

    /// Cancel and abort the background stream task (idempotent).
    async fn shutdown(&self) {
        self.cancel.send_replace(true);
        if let Ok(mut task) = self.task.try_lock() {
            if let Some(handle) = task.take() {
                handle.abort();
            }
        }
    }
}

/// Pool of live per-tamad stream handles, keyed by tamad id.
pub struct TamadPool {
    handles: tokio::sync::RwLock<HashMap<String, Arc<TamadHandle>>>,
    db_pool: Arc<PgPool>,
    /// Test override for the reconnect backoff base (defaults to 1s).
    backoff_base: Option<Duration>,
}

impl TamadPool {
    /// Create an empty pool backed by the shared Postgres pool.
    pub fn new(db_pool: Arc<PgPool>) -> Self {
        Self {
            handles: tokio::sync::RwLock::new(HashMap::new()),
            db_pool,
            backoff_base: None,
        }
    }

    /// Override the reconnect backoff base (for tests).
    pub fn with_backoff_base(mut self, base: Duration) -> Self {
        self.backoff_base = Some(base);
        self
    }

    /// Load all rows from `tamad_registry` and start a stream task for each.
    ///
    /// Called at proxy startup after the DB pool is ready; API mutations call
    /// [`upsert_connection`](Self::upsert_connection) /
    /// [`remove_connection`](Self::remove_connection) for individual
    /// connections instead.
    pub async fn load_all(&self) -> Result<()> {
        let tamads = crate::db::queries::list_tamads(self.db_pool.as_ref())
            .await
            .context("failed to list tamads from registry")?;
        for tamad in &tamads {
            self.upsert_connection(tamad).await.with_context(|| {
                format!("failed to start stream task for tamad '{}'", tamad.name)
            })?;
        }
        Ok(())
    }

    /// Start (or replace) the stream task for a tamad connection.
    ///
    /// If a handle already exists for the same id, the old task is cancelled
    /// and replaced — used after register/update so url/token changes take
    /// effect immediately.
    pub async fn upsert_connection(&self, conn: &TamadConnection) -> Result<()> {
        let handle = {
            let mut handles = self.handles.write().await;
            if let Some(old) = handles.remove(&conn.id) {
                old.shutdown().await;
            }
            let handle = Arc::new(TamadHandle::new(conn.clone()));
            handles.insert(conn.id.clone(), handle.clone());
            handle
        };

        let backoff = self.backoff_base.unwrap_or(DEFAULT_BACKOFF_BASE);
        let db_pool = Arc::clone(&self.db_pool);
        let task = tokio::spawn(run_stream_task(db_pool, handle.clone(), backoff));
        *handle.task.lock().await = Some(task.abort_handle());
        Ok(())
    }

    /// Cancel and drop the stream task for a tamad (used after delete).
    pub async fn remove_connection(&self, id: &str) -> Result<()> {
        if let Some(handle) = self.handles.write().await.remove(id) {
            handle.shutdown().await;
        }
        Ok(())
    }

    /// Look up a handle by tamad id.
    pub async fn get(&self, id: &str) -> Option<Arc<TamadHandle>> {
        self.handles.read().await.get(id).cloned()
    }

    /// All current handles (order unspecified).
    pub async fn list_handles(&self) -> Vec<Arc<TamadHandle>> {
        self.handles.read().await.values().cloned().collect()
    }

    /// Resolve the handle for a provider's assigned tamad, if any.
    pub async fn handle_for_provider(&self, tamad_id: Option<&str>) -> Option<Arc<TamadHandle>> {
        let tamad_id = tamad_id?;
        self.get(tamad_id).await
    }
}

/// Background stream task: keep one `StreamStats` stream open per tamad,
/// tracking the latest snapshot and the online/offline status.
///
/// On failure (connect error, stream error, or close), the task marks the
/// tamad offline and reconnects with exponential backoff (base → 2× → 4× …,
/// capped at 30s) until the `cancel` signal is set.
async fn run_stream_task(db_pool: Arc<PgPool>, handle: Arc<TamadHandle>, backoff_base: Duration) {
    let tamad_id = handle.connection.id.clone();

    // HTTP-protocol connections have no stats stream — the handle stays
    // in its initial "unknown" state and no task work remains.
    if !handle.connection.protocol.is_grpc() {
        tracing::debug!(tamad_id = %tamad_id, "tamad uses HTTP protocol; no stats stream");
        return;
    }

    let mut backoff = backoff_base;
    let mut cancel_rx = handle.cancel.subscribe();
    loop {
        if *cancel_rx.borrow_and_update() {
            break;
        }

        // The guard is dropped as soon as the stream is open — the stream
        // owns its own fresh channel (see TamadClient).
        let stream_result = {
            let client = handle.client.lock().await;
            client.stream_stats().await
        };
        let mut stream = match stream_result {
            Ok(stream) => stream,
            Err(e) => {
                tracing::debug!(tamad_id = %tamad_id, error = %e, "StreamStats connect failed");
                mark_offline(&db_pool, &handle, &tamad_id).await;
                if sleep_or_cancel(&mut cancel_rx, backoff).await {
                    break;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // Stream is open: online.
        let _ = handle.online.send_replace(true);
        update_tamad_status(&db_pool, &tamad_id, "online").await;
        tracing::info!(tamad_id = %tamad_id, "tamad stats stream connected");
        backoff = backoff_base;

        // Cache the tamad's reported version (system health endpoint +
        // dashboard host cards, plan-191 Task 9). Failures are not fatal:
        // the stream itself is live.
        let health = {
            let mut client = handle.client.lock().await;
            client.health_check_full().await
        };
        match health {
            Ok(resp) => {
                *handle.health.write().await = Some(HealthState {
                    version: resp.version,
                    checked_at: Instant::now(),
                });
            }
            Err(e) => {
                tracing::debug!(tamad_id = %tamad_id, error = %e, "HealthCheck failed");
            }
        }

        'stream: loop {
            tokio::select! {
                _ = cancel_rx.changed() => break 'stream,
                item = stream.next() => {
                    match item {
                        Some(Ok(stats)) => {
                            *handle.latest.write().await =
                                Some(LatestStats { stats, at: Instant::now() });
                        }
                        Some(Err(e)) => {
                            tracing::warn!(
                                tamad_id = %tamad_id,
                                error = %e,
                                "StreamStats error; reconnecting"
                            );
                            break 'stream;
                        }
                        None => {
                            tracing::debug!(
                                tamad_id = %tamad_id,
                                "StreamStats closed by tamad; reconnecting"
                            );
                            break 'stream;
                        }
                    }
                }
            }
        }

        // Stream ended (cancelled, errored, or closed by the tamad).
        if *cancel_rx.borrow() {
            break;
        }
        mark_offline(&db_pool, &handle, &tamad_id).await;
        if sleep_or_cancel(&mut cancel_rx, backoff).await {
            break;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Mark the handle offline and mirror it into `tamad_registry.status`.
async fn mark_offline(db_pool: &PgPool, handle: &TamadHandle, tamad_id: &str) {
    let _ = handle.online.send_replace(false);
    update_tamad_status(db_pool, tamad_id, "offline").await;
}

/// Update `tamad_registry.status` (failures are logged, never fatal).
async fn update_tamad_status(db_pool: &PgPool, tamad_id: &str, status: &str) {
    if let Err(e) = crate::db::queries::update_tamad_status(db_pool, tamad_id, status).await {
        tracing::debug!(tamad_id = %tamad_id, "failed to update tamad status: {}", e);
    }
}

/// Sleep for `duration`, waking early when the cancel signal is set.
/// Returns `true` when cancelled.
async fn sleep_or_cancel(
    cancel_rx: &mut tokio::sync::watch::Receiver<bool>,
    duration: Duration,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = cancel_rx.changed() => true,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-stubs"))]
pub mod test_support {
    //! Shared gRPC tamad stub + helpers for pool / dashboard fan-out tests
    //! (plan-191 Task 4).

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use futures_util::{Stream, StreamExt};

    use crate::providers::{Protocol, TamadConnection, TamadStatus};
    use crate::tamad::{JobEvent, LogEntry, StatsRequest, SystemStats};

    // ── Stub tamad gRPC service ──

    /// In-test `TamadService` stub: `stream_stats` fails for the first
    /// `fail_first_n` calls, succeeds for the next `succeed_until -
    /// fail_first_n` calls, and fails forever after (simulating the tamad
    /// going down).
    ///
    /// A successful stream yields two snapshots and then stays open until
    /// `down` is set — the test controls when the stream ends.
    ///
    /// `pull_model` / `install_provider` / `update_provider` record their
    /// requests and return the scripted job id (or fail with `unavailable`
    /// when the matching `*_fail` flag is set); `remove_provider` records
    /// the request; `stream_job` replays `stream_job_events` for any known
    /// job id and then holds the stream open until `down` is set (so a
    /// "tamad died mid-job" scenario is scripted by omitting a terminal
    /// event).
    #[derive(Clone)]
    pub struct StubTamad {
        pub fail_first_n: usize,
        pub succeed_until: usize,
        pub down: Arc<tokio::sync::watch::Sender<bool>>,
        pub calls: Arc<AtomicUsize>,
        pub successes: Arc<AtomicUsize>,
        /// Recorded `pull_model` requests (for assertions).
        pub pull_requests: Arc<tokio::sync::Mutex<Vec<crate::tamad::PullModelRequest>>>,
        /// Job id returned by `pull_model`.
        pub pull_job_id: String,
        /// When set, `pull_model` fails with `unavailable` (tamad offline).
        pub pull_model_fail: Arc<tokio::sync::Mutex<bool>>,
        /// Recorded `install_provider` requests (for assertions).
        pub install_requests: Arc<tokio::sync::Mutex<Vec<crate::tamad::InstallProviderRequest>>>,
        /// Job id returned by `install_provider`.
        pub install_job_id: String,
        /// When set, `install_provider` fails with `unavailable`.
        pub install_dispatch_fail: Arc<tokio::sync::Mutex<bool>>,
        /// Recorded `update_provider` requests (for assertions).
        pub update_requests: Arc<tokio::sync::Mutex<Vec<crate::tamad::UpdateProviderRequest>>>,
        /// Job id returned by `update_provider`.
        pub update_job_id: String,
        /// When set, `update_provider` fails with `unavailable`.
        pub update_dispatch_fail: Arc<tokio::sync::Mutex<bool>>,
        /// Recorded `remove_provider` requests (for assertions).
        pub remove_requests: Arc<tokio::sync::Mutex<Vec<crate::tamad::RemoveProviderRequest>>>,
        /// When set, `remove_provider` fails with `unavailable`.
        pub remove_dispatch_fail: Arc<tokio::sync::Mutex<bool>>,
        /// Scripted `StreamJob` events replayed for any known job id.
        pub stream_job_events: Arc<tokio::sync::Mutex<Vec<crate::tamad::JobEvent>>>,
        /// Scripted `StreamJob` events keyed by exact job id (bench jobs:
        /// multiple sequential runs need distinct event streams).
        pub stream_job_events_by_id:
            Arc<tokio::sync::Mutex<std::collections::HashMap<String, Vec<crate::tamad::JobEvent>>>>,
        /// Number of `stream_job` calls received (for test synchronization).
        pub stream_job_calls: Arc<AtomicUsize>,
        /// Recorded `load_model` requests (for assertions).
        pub load_requests: Arc<tokio::sync::Mutex<Vec<crate::tamad::LoadModelRequest>>>,
        /// Per-model scripted `LoadModel` delay (model → simulated duration
        /// inside the tamad-side health poll before the RPC returns).
        pub load_delays: std::collections::HashMap<String, Duration>,
        /// When set, `load_model` fails with `unavailable` (simulated
        /// load failure).
        pub load_model_fail: Arc<tokio::sync::Mutex<bool>>,
        /// Recorded `run_benchmark` requests (for assertions).
        pub bench_requests: Arc<tokio::sync::Mutex<Vec<crate::tamad::RunBenchmarkRequest>>>,
        /// Prefix of the job ids returned by `run_benchmark` (the stub
        /// appends a per-call counter: "{prefix}-1", "{prefix}-2", ...).
        pub bench_job_id: String,
        /// When set, `run_benchmark` fails with `unavailable`.
        pub bench_dispatch_fail: Arc<tokio::sync::Mutex<bool>>,
        /// GPUs included in every emitted `SystemStats` snapshot (default
        /// empty — CPU-only stub host).
        pub stats_gpus: Vec<crate::tamad::GpuInfo>,
    }

    type EmptyStream<T> =
        futures_util::stream::Iter<std::vec::IntoIter<std::result::Result<T, tonic::Status>>>;

    type BoxedStatsStream = std::pin::Pin<
        Box<dyn Stream<Item = std::result::Result<SystemStats, tonic::Status>> + Send>,
    >;

    type BoxedJobStream =
        std::pin::Pin<Box<dyn Stream<Item = std::result::Result<JobEvent, tonic::Status>> + Send>>;

    pub fn stats(cpu: f64) -> SystemStats {
        SystemStats {
            cpu_usage_percent: cpu,
            memory_total_bytes: 1024,
            memory_used_bytes: 512,
            swap_total_bytes: 0,
            swap_used_bytes: 0,
            disk_total_bytes: 0,
            disk_free_bytes: 0,
            gpus: vec![],
            processes: vec![],
        }
    }

    /// A fixed snapshot with the given GPU list (default cpu/memory values).
    pub fn stats_with_gpus(cpu: f64, gpus: Vec<crate::tamad::GpuInfo>) -> SystemStats {
        SystemStats {
            cpu_usage_percent: cpu,
            memory_total_bytes: 1024,
            memory_used_bytes: 512,
            swap_total_bytes: 0,
            swap_used_bytes: 0,
            disk_total_bytes: 0,
            disk_free_bytes: 0,
            gpus,
            processes: vec![],
        }
    }

    /// Build a scripted `JobEvent` for a pull job.
    pub fn job_event(job_id: &str, progress: i32, message: &str, status: &str) -> JobEvent {
        job_event_bytes(job_id, progress, message, status, 0, 0)
    }

    /// Build a scripted pull `JobEvent` with byte counters (the relay's
    /// progress source for tamad-hosted downloads).
    pub fn job_event_bytes(
        job_id: &str,
        progress: i32,
        message: &str,
        status: &str,
        bytes_downloaded: i64,
        total_bytes: i64,
    ) -> JobEvent {
        JobEvent {
            job_id: job_id.to_string(),
            kind: "pull".to_string(),
            progress,
            message: message.to_string(),
            status: status.to_string(),
            result_json: String::new(),
            error: String::new(),
            bytes_downloaded,
            total_bytes,
        }
    }

    /// A succeeded terminal job event with the given result JSON.
    pub fn terminal_success(job_id: &str, result_json: &str) -> JobEvent {
        let mut ev = job_event_bytes(job_id, 100, "done", "succeeded", 0, 0);
        ev.result_json = result_json.to_string();
        ev
    }

    /// A failed terminal job event with the given error.
    pub fn job_event_failed(job_id: &str, error: &str) -> JobEvent {
        let mut ev = job_event(job_id, 0, "failed", "failed");
        ev.error = error.to_string();
        ev
    }

    #[tonic::async_trait]
    impl crate::tamad::TamadService for StubTamad {
        type LogsStream = EmptyStream<LogEntry>;
        type StreamStatsStream = BoxedStatsStream;
        type StreamJobStream = BoxedJobStream;

        async fn list_providers(
            &self,
            _request: tonic::Request<crate::tamad::Empty>,
        ) -> std::result::Result<tonic::Response<crate::tamad::ListProvidersResponse>, tonic::Status>
        {
            Err(tonic::Status::unimplemented("not used in tests"))
        }
        async fn install_provider(
            &self,
            request: tonic::Request<crate::tamad::InstallProviderRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::JobIdResponse>, tonic::Status>
        {
            if *self.install_dispatch_fail.lock().await {
                return Err(tonic::Status::unavailable("simulated tamad offline"));
            }
            let req = request.into_inner();
            self.install_requests.lock().await.push(req);
            Ok(tonic::Response::new(crate::tamad::JobIdResponse {
                job_id: self.install_job_id.clone(),
            }))
        }
        async fn load_model(
            &self,
            request: tonic::Request<crate::tamad::LoadModelRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::LoadModelResponse>, tonic::Status>
        {
            // Record the attempt FIRST (even the ones that fail) so
            // assertions can count how many loads actually reached the
            // tamad.
            let req = request.into_inner();
            self.load_requests.lock().await.push(req.clone());
            if *self.load_model_fail.lock().await {
                return Err(tonic::Status::unavailable("simulated load failure"));
            }
            if let Some(delay) = self.load_delays.get(&req.model_name) {
                tokio::time::sleep(*delay).await;
            }
            Ok(tonic::Response::new(crate::tamad::LoadModelResponse {
                endpoint_url: "http://127.0.0.1:5801".to_string(),
                pid: 1234,
                status: "ready".to_string(),
            }))
        }
        async fn unload_model(
            &self,
            _request: tonic::Request<crate::tamad::UnloadModelRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::Empty>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used in tests"))
        }
        async fn update_provider(
            &self,
            request: tonic::Request<crate::tamad::UpdateProviderRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::JobIdResponse>, tonic::Status>
        {
            if *self.update_dispatch_fail.lock().await {
                return Err(tonic::Status::unavailable("simulated tamad offline"));
            }
            let req = request.into_inner();
            self.update_requests.lock().await.push(req);
            Ok(tonic::Response::new(crate::tamad::JobIdResponse {
                job_id: self.update_job_id.clone(),
            }))
        }
        async fn remove_provider(
            &self,
            request: tonic::Request<crate::tamad::RemoveProviderRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::Empty>, tonic::Status> {
            if *self.remove_dispatch_fail.lock().await {
                return Err(tonic::Status::unavailable("simulated tamad offline"));
            }
            let req = request.into_inner();
            self.remove_requests.lock().await.push(req);
            Ok(tonic::Response::new(crate::tamad::Empty {}))
        }
        async fn logs(
            &self,
            _request: tonic::Request<crate::tamad::LogsRequest>,
        ) -> std::result::Result<tonic::Response<Self::LogsStream>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used in tests"))
        }
        async fn health_check(
            &self,
            _request: tonic::Request<crate::tamad::Empty>,
        ) -> std::result::Result<tonic::Response<crate::tamad::HealthResponse>, tonic::Status>
        {
            Ok(tonic::Response::new(crate::tamad::HealthResponse {
                status: "ok".to_string(),
                version: "9.9.9-stub".to_string(),
            }))
        }
        async fn stream_stats(
            &self,
            _request: tonic::Request<StatsRequest>,
        ) -> std::result::Result<tonic::Response<Self::StreamStatsStream>, tonic::Status> {
            let calls = self.calls.fetch_add(1, Ordering::SeqCst);
            if calls < self.fail_first_n {
                return Err(tonic::Status::unavailable("simulated connect failure"));
            }
            let successes = self.successes.fetch_add(1, Ordering::SeqCst);
            if successes >= self.succeed_until {
                return Err(tonic::Status::unavailable("simulated tamad down"));
            }
            let mut down_rx = self.down.subscribe();
            let gpus_a = self.stats_gpus.clone();
            let gpus_b = self.stats_gpus.clone();
            let gpus_c = self.stats_gpus.clone();
            let stream = futures_util::stream::iter(vec![
                Ok(stats_with_gpus(1.5, gpus_a)),
                Ok(stats_with_gpus(42.5, gpus_b)),
            ])
            .chain(futures_util::stream::once(async move {
                // Hold the stream open until the test signals "down",
                // then emit a final snapshot and close.
                let _ = down_rx.changed().await;
                Ok::<SystemStats, tonic::Status>(stats_with_gpus(42.5, gpus_c))
            }));
            Ok(tonic::Response::new(Box::pin(stream)))
        }
        async fn stream_job(
            &self,
            request: tonic::Request<crate::tamad::JobRequest>,
        ) -> std::result::Result<tonic::Response<Self::StreamJobStream>, tonic::Status> {
            self.stream_job_calls.fetch_add(1, Ordering::Relaxed);
            let job_id = request.into_inner().job_id;
            let by_id_known = self
                .stream_job_events_by_id
                .lock()
                .await
                .contains_key(&job_id);
            if job_id != self.pull_job_id
                && job_id != self.install_job_id
                && job_id != self.update_job_id
                && !by_id_known
            {
                return Err(tonic::Status::not_found(format!(
                    "unknown job id '{job_id}'"
                )));
            }
            // Exact-id scripted events take precedence (bench jobs:
            // several sequential runs need distinct event streams).
            let by_id = self
                .stream_job_events_by_id
                .lock()
                .await
                .get(&job_id)
                .cloned();
            let events = if let Some(scripted) = by_id {
                scripted
            } else {
                self.stream_job_events.lock().await.clone()
            };
            let mut down_rx = self.down.subscribe();
            // Replay the scripted events, then end the stream CLEANLY
            // (EOF) once `down` is set — models the tamad disconnecting
            // mid-job. The sender task drops `tx` when `down` flips.
            let (tx, rx) = tokio::sync::mpsc::channel(events.len().max(1));
            tokio::spawn(async move {
                for ev in events {
                    if tx.send(ev).await.is_err() {
                        return;
                    }
                }
                let _ = down_rx.changed().await;
                // drop tx → receiver stream ends (clean EOF)
            });
            let stream = futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv()
                    .await
                    .map(|ev| (Ok::<JobEvent, tonic::Status>(ev), rx))
            });
            Ok(tonic::Response::new(Box::pin(stream)))
        }
        async fn restart_provider(
            &self,
            _request: tonic::Request<crate::tamad::RestartProviderRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::Empty>, tonic::Status> {
            Err(tonic::Status::unimplemented("not used in tests"))
        }
        async fn pull_model(
            &self,
            request: tonic::Request<crate::tamad::PullModelRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::JobIdResponse>, tonic::Status>
        {
            if *self.pull_model_fail.lock().await {
                return Err(tonic::Status::unavailable("simulated tamad offline"));
            }
            let req = request.into_inner();
            self.pull_requests.lock().await.push(req);
            Ok(tonic::Response::new(crate::tamad::JobIdResponse {
                job_id: self.pull_job_id.clone(),
            }))
        }
        async fn run_benchmark(
            &self,
            request: tonic::Request<crate::tamad::RunBenchmarkRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::JobIdResponse>, tonic::Status>
        {
            if *self.bench_dispatch_fail.lock().await {
                return Err(tonic::Status::unavailable("simulated tamad offline"));
            }
            let req = request.into_inner();
            let mut requests = self.bench_requests.lock().await;
            requests.push(req);
            // "{prefix}-{n}" so each sequential dispatch streams its own
            // scripted event list.
            let job_id = format!("{}-{}", self.bench_job_id, requests.len());
            Ok(tonic::Response::new(crate::tamad::JobIdResponse { job_id }))
        }

        async fn cancel_job(
            &self,
            _request: tonic::Request<crate::tamad::CancelJobRequest>,
        ) -> std::result::Result<tonic::Response<crate::tamad::CancelJobResponse>, tonic::Status>
        {
            // Idempotent no-op: the stub reports "nothing to cancel".
            Ok(tonic::Response::new(crate::tamad::CancelJobResponse {
                cancelled: false,
            }))
        }
    }

    /// Start the stub service on an ephemeral localhost port.
    ///
    /// The port is discovered by binding (then closing) a std listener; the
    /// tonic server rebinds it. "Tamad down" is simulated by the stub's
    /// fail-forever mode, not by killing the server.
    pub async fn start_stub(service: StubTamad) -> std::net::SocketAddr {
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0")
                .expect("bind ephemeral port for stub tamad");
            probe.local_addr().expect("stub tamad addr").port()
        };
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let server = tonic::transport::Server::builder()
            .add_service(crate::tamad::TamadServiceServer::new(service));
        tokio::spawn(async move {
            if let Err(e) = server.serve(addr).await {
                tracing::debug!("stub tamad server stopped: {}", e);
            }
        });
        // Wait for the server to actually accept connections (the serve
        // task started above; connect-refused races otherwise).
        for _ in 0..100 {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                return addr;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        addr
    }

    pub fn grpc_conn(id: &str, name: &str, url: &str) -> TamadConnection {
        TamadConnection {
            id: id.to_string(),
            name: name.to_string(),
            url: url.to_string(),
            protocol: Protocol::Grpc,
            token: Some("secret".to_string()),
            status: TamadStatus::Unknown,
        }
    }

    /// Minimal `StubTamad` for tests that only need a stats stream (new
    /// tests prefer this over repeating the full literal).
    pub fn stub_default() -> StubTamad {
        let (down_tx, _) = tokio::sync::watch::channel(false);
        StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(down_tx),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-stub".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        }
    }

    /// Poll `f` (every 20ms) until it returns true or the 10s deadline hits.
    pub async fn wait_for<F, Fut>(mut f: F) -> bool
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if f().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::test_support::*;
    use super::*;
    use crate::db::queries::{get_tamad, insert_tamad};
    use crate::providers::{Protocol, TamadStatus};
    use crate::testing::postgres::with_schema;

    /// Full lifecycle: stream delivers snapshots (latest updates, online
    /// flips, DB status "online"); when the tamad goes down the handle
    /// flips offline (DB status "offline") and keeps the last snapshot;
    /// `remove_connection` drops the handle.
    #[tokio::test]
    async fn test_stream_lifecycle_online_offline() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        // Tamad that succeeds exactly once (two snapshots), then "goes down".
        let (down_tx, _) = tokio::sync::watch::channel(false);
        let down = Arc::new(down_tx);
        let stub = StubTamad {
            fail_first_n: 0,
            succeed_until: 1,
            down: Arc::clone(&down),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-stub".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        };
        let addr = start_stub(stub.clone()).await;
        let url = format!("grpc://{addr}");

        insert_tamad(
            &db_pool,
            "uuid-lifecycle",
            "stub",
            &url,
            "grpc",
            Some("secret"),
        )
        .await
        .unwrap();

        let pool = TamadPool::new(db_pool.clone()).with_backoff_base(Duration::from_millis(50));
        let conn = grpc_conn("uuid-lifecycle", "stub", &url);
        pool.upsert_connection(&conn).await.unwrap();

        let handle = pool.get("uuid-lifecycle").await.expect("handle registered");
        assert!(
            wait_for(|| async { handle.is_online().await }).await,
            "handle should come online"
        );
        let latest = handle.latest().await.expect("snapshot received");
        assert_eq!(
            latest.cpu_usage_percent, 42.5,
            "latest should be last snapshot"
        );

        // Poll the DB: the watch flag flips before the status write lands.
        assert!(
            wait_for(|| async {
                get_tamad(&db_pool, "uuid-lifecycle")
                    .await
                    .unwrap()
                    .unwrap()
                    .status
                    .is_online()
            })
            .await,
            "DB status should be online"
        );

        // Tamad "goes down" — the open stream ends and reconnects fail.
        down.send_replace(true);
        assert!(
            wait_for(|| async { !handle.is_online().await }).await,
            "handle should go offline after the stream ends"
        );
        assert!(
            wait_for(|| async {
                get_tamad(&db_pool, "uuid-lifecycle")
                    .await
                    .unwrap()
                    .unwrap()
                    .status
                    .is_offline()
            })
            .await,
            "DB status should be offline"
        );

        // The last snapshot is retained after the stream ends.
        let latest = handle.latest().await.expect("last snapshot retained");
        assert_eq!(latest.cpu_usage_percent, 42.5);

        // remove drops the handle and stops the task.
        pool.remove_connection("uuid-lifecycle").await.unwrap();
        assert!(pool.get("uuid-lifecycle").await.is_none());
        assert!(pool.list_handles().await.is_empty());

        guard.finish().await;
    }

    /// Connect failures are retried with backoff until the tamad comes up.
    #[tokio::test]
    async fn test_reconnect_after_failures() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        let (keep_down_open, _) = tokio::sync::watch::channel(false);
        let stub = StubTamad {
            fail_first_n: 3,
            succeed_until: usize::MAX,
            down: Arc::new(keep_down_open),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-stub".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        };
        let addr = start_stub(stub.clone()).await;
        let url = format!("grpc://{addr}");

        let pool = TamadPool::new(db_pool.clone()).with_backoff_base(Duration::from_millis(20));
        let conn = grpc_conn("uuid-reconnect", "stub", &url);
        pool.upsert_connection(&conn).await.unwrap();
        let handle = pool.get("uuid-reconnect").await.unwrap();

        assert!(
            wait_for(|| async { handle.is_online().await }).await,
            "handle should reconnect after failures"
        );
        assert!(
            stub.calls.load(Ordering::SeqCst) >= 4,
            "3 failures + 1 success expected, got {}",
            stub.calls.load(Ordering::SeqCst)
        );
        let latest = handle.latest().await.expect("snapshot received");
        assert_eq!(latest.cpu_usage_percent, 42.5);

        guard.finish().await;
    }

    /// `load_all` starts a stream task for every row in `tamad_registry`.
    #[tokio::test]
    async fn test_load_all_starts_streams() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        let (keep_down_open, _) = tokio::sync::watch::channel(false);
        let stub = StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(keep_down_open),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-stub".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        };
        let addr = start_stub(stub).await;
        let url = format!("grpc://{addr}");

        insert_tamad(
            &db_pool,
            "uuid-load-a",
            "load-a",
            &url,
            "grpc",
            Some("secret"),
        )
        .await
        .unwrap();
        insert_tamad(
            &db_pool,
            "uuid-load-b",
            "load-b",
            &url,
            "grpc",
            Some("secret"),
        )
        .await
        .unwrap();

        let pool = TamadPool::new(db_pool.clone()).with_backoff_base(Duration::from_millis(20));
        pool.load_all().await.unwrap();

        let handles = pool.list_handles().await;
        assert_eq!(handles.len(), 2, "one handle per registry row");

        let handle_a = pool.get("uuid-load-a").await.expect("handle a");
        assert!(
            wait_for(|| async { handle_a.is_online().await }).await,
            "loaded tamad should come online"
        );

        // handle_for_provider resolves by id; None → None.
        let h = pool
            .handle_for_provider(Some("uuid-load-b"))
            .await
            .expect("handle b via provider");
        assert_eq!(h.connection.id, "uuid-load-b");
        assert!(pool.handle_for_provider(None).await.is_none());

        guard.finish().await;
    }

    /// After the stats stream opens, the pool caches the tamad's
    /// `HealthCheck` response (the tamad's version) so the system health
    /// endpoint and dashboard host cards can report it (plan-191 Task 9).
    #[tokio::test]
    async fn test_health_check_cached_after_stream_open() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        let addr = start_stub(test_support::stub_default()).await;
        let url = format!("grpc://{addr}");

        let pool = TamadPool::new(db_pool.clone()).with_backoff_base(Duration::from_millis(20));
        let conn = grpc_conn("uuid-health", "host-h", &url);
        pool.upsert_connection(&conn).await.unwrap();

        let handle = pool.get("uuid-health").await.expect("handle registered");
        assert!(
            wait_for(|| async { handle.is_online().await }).await,
            "handle should come online"
        );
        assert!(
            wait_for(|| async { handle.version().await.is_some() }).await,
            "health check should be cached after the stream opens"
        );
        assert_eq!(
            handle.version().await.as_deref(),
            Some("9.9.9-stub"),
            "cached version must be the tamad's reported version"
        );

        guard.finish().await;
    }

    /// An upsert with the same id replaces the existing handle/task.
    #[tokio::test]
    async fn test_upsert_replaces_existing_handle() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        let pool = TamadPool::new(db_pool.clone()).with_backoff_base(Duration::from_millis(10));

        let conn1 = grpc_conn("uuid-replace", "old-name", "grpc://127.0.0.1:1");
        pool.upsert_connection(&conn1).await.unwrap();

        let mut conn2 = conn1.clone();
        conn2.name = "new-name".to_string();
        conn2.url = "grpc://127.0.0.1:2".to_string();
        pool.upsert_connection(&conn2).await.unwrap();

        assert_eq!(pool.list_handles().await.len(), 1, "no duplicate handles");
        let handle = pool.get("uuid-replace").await.unwrap();
        assert_eq!(handle.connection.name, "new-name");
        assert_eq!(handle.connection.url, "grpc://127.0.0.1:2");

        pool.remove_connection("uuid-replace").await.unwrap();
        assert!(pool.get("uuid-replace").await.is_none());

        guard.finish().await;
    }

    /// HTTP-protocol connections have no stats stream: the handle exists but
    /// stays "unknown" (never online, no snapshot, DB status unchanged).
    #[tokio::test]
    async fn test_http_connection_stays_unknown() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        insert_tamad(
            &db_pool,
            "uuid-http",
            "http-tamad",
            "http://127.0.0.1:9",
            "http",
            None,
        )
        .await
        .unwrap();

        let pool = TamadPool::new(db_pool.clone());
        let conn = TamadConnection {
            id: "uuid-http".to_string(),
            name: "http-tamad".to_string(),
            url: "http://127.0.0.1:9".to_string(),
            protocol: Protocol::Http,
            token: None,
            status: TamadStatus::Unknown,
        };
        pool.upsert_connection(&conn).await.unwrap();

        // Give the (immediately-exiting) task a tick to prove it does nothing.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let handle = pool.get("uuid-http").await.expect("handle registered");
        assert!(!handle.is_online().await, "HTTP tamad never goes online");
        assert!(handle.latest().await.is_none(), "no snapshots for HTTP");

        let row = get_tamad(&db_pool, "uuid-http").await.unwrap().unwrap();
        assert!(row.status.is_unknown(), "DB status stays unknown");

        guard.finish().await;
    }
}
