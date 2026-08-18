use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::Router;
use futures_util::StreamExt;
use tonic::transport::Server as TonicServer;
use tracing::info;

use crate::installs::{self, TamadInstaller};
use crate::jobs::JobRegistry;
use crate::lifecycle::TamadLifecycle;
use crate::process_table::ProcessTable;
use crate::state::TamadState;
use crate::stats::StatsCollector;

use tama_core::tamad::tamad_service::Empty as GrpcEmpty;
use tama_core::tamad::tamad_service::HealthResponse;
use tama_core::tamad::tamad_service::InstallProviderRequest;
use tama_core::tamad::tamad_service::JobEvent;
use tama_core::tamad::tamad_service::JobIdResponse;
use tama_core::tamad::tamad_service::JobRequest;
use tama_core::tamad::tamad_service::ListProvidersResponse;
use tama_core::tamad::tamad_service::LoadModelRequest as GrpcLoadModelRequest;
use tama_core::tamad::tamad_service::LoadModelResponse as GrpcLoadModelResponse;
use tama_core::tamad::tamad_service::LogEntry;
use tama_core::tamad::tamad_service::LogsRequest;
use tama_core::tamad::tamad_service::PullModelRequest;
use tama_core::tamad::tamad_service::RemoveProviderRequest;
use tama_core::tamad::tamad_service::RestartProviderRequest;
use tama_core::tamad::tamad_service::RunBenchmarkRequest;
use tama_core::tamad::tamad_service::StatsRequest;
use tama_core::tamad::tamad_service::SystemStats;
use tama_core::tamad::tamad_service::UnloadModelRequest;
use tama_core::tamad::tamad_service::UpdateProviderRequest;
use tama_core::tamad::TamadService;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether a `JobEvent` carries a terminal status (the wire enum is
/// `"running" | "succeeded" | "failed"`).
fn is_terminal_event(ev: &JobEvent) -> bool {
    matches!(ev.status.as_str(), "succeeded" | "failed")
}

/// Compare two byte slices in (near-)constant time.
///
/// XOR-accumulates the per-byte difference instead of bailing out on the
/// first mismatch, so wall-clock time does not reveal how many leading
/// bytes matched. Different lengths short-circuit — the token length is
/// fixed by the deployment configuration, not secret.
fn const_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (l, r) in left.iter().zip(right.iter()) {
        diff |= l ^ r;
    }
    diff == 0
}

/// Verify the `authorization` metadata is exactly `Bearer {expected}`.
///
/// The compare is constant-time ([`const_time_eq`]) — a plain `==` bails
/// out on the first differing byte and would leak the matched prefix via
/// timing. Every gRPC RPC requires this; only the HTTP `/health`
/// liveness probe is unauthenticated.
#[allow(clippy::result_large_err)] // tonic::Status is a large error type
fn check_auth<M>(
    request: &tonic::Request<M>,
    expected: &str,
) -> std::result::Result<(), tonic::Status> {
    let provided = request
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected_header = format!("Bearer {expected}");
    if const_time_eq(provided.as_bytes(), expected_header.as_bytes()) {
        Ok(())
    } else {
        Err(tonic::Status::unauthenticated(
            "missing or invalid authorization",
        ))
    }
}

pub struct TamadServiceImpl {
    expected_token: String,
    /// Runtime state (models dir for disk sampling, etc.); the lifecycle
    /// also reads it for path remapping.
    #[allow(dead_code)] // read by the lifecycle via its own Arc clone
    state: Arc<TamadState>,
    /// In-memory table of spawned backend processes.
    table: Arc<ProcessTable>,
    /// Stateful host-stats collector; the mutex lets every stream_stats
    /// stream (one per connected proxy) share the single persistent
    /// `sysinfo::System`.
    collector: Arc<tokio::sync::Mutex<StatsCollector>>,
    /// Spawn/health/unload/restart over the process table.
    /// `Arc` so `main`'s SIGTERM handler shares the same lifecycle.
    lifecycle: Arc<TamadLifecycle>,
    /// Executor for install/update jobs (plan-191 Task 7). Production is
    /// [`TamadInstaller`]; tests inject a marker-file stub.
    installer: Arc<dyn installs::Installer>,
    /// Executor for benchmark jobs (plan-191 Task 8). Production is
    /// [`crate::bench::TamadBenchExecutor`]; tests inject a scripted stub.
    bench_executor: Arc<dyn crate::bench::BenchExecutor>,
    /// In-memory job registry (plan-191 Task 6) behind `PullModel`/
    /// `StreamJob`; later tasks add install/update/benchmark jobs here.
    jobs: Arc<JobRegistry>,
}

impl TamadServiceImpl {
    /// Create the service implementation for the given expected bearer token.
    ///
    /// The lifecycle is injected (shared with `main`'s shutdown handler so a
    /// SIGTERM to tamad kills every loaded backend — plan-191 follow-up A).
    pub fn new(
        token: String,
        state: Arc<TamadState>,
        table: Arc<ProcessTable>,
        lifecycle: Arc<TamadLifecycle>,
    ) -> Self {
        Self {
            expected_token: token,
            collector: Arc::new(tokio::sync::Mutex::new(StatsCollector::new(Arc::clone(
                &state,
            )))),
            lifecycle,
            installer: Arc::new(TamadInstaller),
            bench_executor: Arc::new(crate::bench::TamadBenchExecutor),
            jobs: JobRegistry::new(),
            state,
            table,
        }
    }

    /// Replace the install/update executor (tests inject a stub that writes
    /// a marker file instead of touching the network).
    #[allow(dead_code)]
    pub fn with_installer(self, installer: Arc<dyn installs::Installer>) -> Self {
        Self { installer, ..self }
    }

    /// Replace the benchmark executor (tests inject a scripted stub).
    #[allow(dead_code)]
    pub fn with_bench_executor(self, executor: Arc<dyn crate::bench::BenchExecutor>) -> Self {
        Self {
            bench_executor: executor,
            ..self
        }
    }

    /// The shared job registry (exposed to tests; the gRPC surface is
    /// `PullModel` + `StreamJob`).
    #[cfg(test)]
    pub fn jobs(&self) -> &Arc<JobRegistry> {
        &self.jobs
    }
}

#[async_trait]
impl TamadService for TamadServiceImpl {
    async fn list_providers(
        &self,
        request: tonic::Request<GrpcEmpty>,
    ) -> std::result::Result<tonic::Response<ListProvidersResponse>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        Ok(tonic::Response::new(ListProvidersResponse {
            providers: self.lifecycle.list().await,
        }))
    }

    async fn install_provider(
        &self,
        request: tonic::Request<InstallProviderRequest>,
    ) -> std::result::Result<tonic::Response<JobIdResponse>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let req = request.into_inner();

        // Validate up front for a fast, actionable error; the runner
        // re-validates (the spec builder is the single source of truth).
        installs::spec_from_install(&req, &self.state.install_dir())
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        tracing::info!(
            engine = %req.engine,
            version = %req.version,
            variant = %req.gpu_variant,
            "install job starting"
        );
        let state = Arc::clone(&self.state);
        let installer = Arc::clone(&self.installer);
        let job_id = self
            .jobs
            .start("install", move |handle| {
                Box::pin(async move {
                    installs::run_install_with(&req, &state, handle, &*installer).await
                })
            })
            .await;
        Ok(tonic::Response::new(JobIdResponse { job_id }))
    }

    async fn load_model(
        &self,
        request: tonic::Request<GrpcLoadModelRequest>,
    ) -> std::result::Result<tonic::Response<GrpcLoadModelResponse>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let req = request.into_inner();

        // Idempotent: a live process for this model already exists →
        // return the existing response (the reconciler re-issues loads).
        // A "failed" entry (crashed child, possibly a zombie that still
        // answers kill(pid,0)) must NOT be treated as alive.
        if let Some(entry) = self.table.get(&req.model_name).await {
            if entry.status != "failed" && crate::process::is_process_alive(entry.pid) {
                return Ok(tonic::Response::new(GrpcLoadModelResponse {
                    endpoint_url: entry.endpoint_url,
                    pid: entry.pid as i32,
                    status: entry.status,
                }));
            }
            // Dead/failed entry — replace it with a fresh load.
        }

        match self.lifecycle.load(&req).await {
            Ok(resp) => Ok(tonic::Response::new(resp)),
            Err(e) => {
                tracing::error!(model = %req.model_name, error = %e, "LoadModel failed");
                Err(tonic::Status::internal(format!(
                    "failed to load model '{}': {}",
                    req.model_name, e
                )))
            }
        }
    }

    async fn unload_model(
        &self,
        request: tonic::Request<UnloadModelRequest>,
    ) -> std::result::Result<tonic::Response<GrpcEmpty>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let model_name = request.into_inner().model_name;
        match self.lifecycle.unload(&model_name).await {
            Ok(()) => Ok(tonic::Response::new(GrpcEmpty {})),
            Err(e) if e.to_string().contains("not loaded on this tamad") => {
                Err(tonic::Status::not_found(format!(
                    "model '{}' is not loaded on this tamad",
                    model_name
                )))
            }
            Err(e) => Err(tonic::Status::internal(format!(
                "failed to unload model '{}': {}",
                model_name, e
            ))),
        }
    }

    async fn update_provider(
        &self,
        request: tonic::Request<UpdateProviderRequest>,
    ) -> std::result::Result<tonic::Response<JobIdResponse>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let req = request.into_inner();

        installs::spec_from_update(&req, &self.state.install_dir())
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        tracing::info!(
            engine = %req.engine,
            version = %req.version,
            variant = %req.gpu_variant,
            "update job starting"
        );
        let state = Arc::clone(&self.state);
        let installer = Arc::clone(&self.installer);
        let job_id = self
            .jobs
            .start("update", move |handle| {
                Box::pin(async move {
                    installs::run_update_with(&req, &state, handle, &*installer).await
                })
            })
            .await;
        Ok(tonic::Response::new(JobIdResponse { job_id }))
    }

    async fn remove_provider(
        &self,
        request: tonic::Request<RemoveProviderRequest>,
    ) -> std::result::Result<tonic::Response<GrpcEmpty>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let req = request.into_inner();

        // 1) Kill any backend processes owned by this backend (idempotent).
        let backend_names = vec![req.name.clone(), req.engine.clone()]
            .into_iter()
            .filter(|n| !n.trim().is_empty())
            .collect::<Vec<_>>();
        let unloaded =
            installs::kill_backend_processes(&self.table, &self.lifecycle, &backend_names).await;
        if !unloaded.is_empty() {
            tracing::info!(
                backend = %req.name,
                ?unloaded,
                "killed backend processes before removal"
            );
        }

        // 2) Delete the versioned install directories (idempotent).
        let engine = if req.engine.trim().is_empty() {
            req.name.trim()
        } else {
            req.engine.trim()
        };
        if !engine.is_empty() {
            installs::remove_install_dirs(
                &self.state.install_dir(),
                engine,
                Some(req.gpu_variant.as_str()),
                Some(req.version.as_str()),
            )
            .map_err(|e| tonic::Status::internal(format!("failed to remove install dirs: {e}")))?;
        }

        tracing::info!(backend = %req.name, "provider removed from this host");
        Ok(tonic::Response::new(GrpcEmpty {}))
    }

    type LogsStream = tokio_stream::Iter<std::vec::IntoIter<Result<LogEntry, tonic::Status>>>;

    async fn logs(
        &self,
        request: tonic::Request<LogsRequest>,
    ) -> std::result::Result<tonic::Response<Self::LogsStream>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        Err(tonic::Status::unimplemented("not implemented"))
    }

    async fn health_check(
        &self,
        request: tonic::Request<GrpcEmpty>,
    ) -> std::result::Result<tonic::Response<HealthResponse>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        Ok(tonic::Response::new(HealthResponse {
            status: "ok".to_string(),
            version: VERSION.to_string(),
        }))
    }

    type StreamStatsStream = Pin<
        Box<
            dyn tokio_stream::Stream<Item = std::result::Result<SystemStats, tonic::Status>> + Send,
        >,
    >;

    /// Server-streaming host stats at ~1s cadence: CPU/RAM/swap/disk +
    /// per-GPU info + the full process-table snapshot. Runs for the life
    /// of the connection — the proxy's reconnect logic (Task 4) handles
    /// re-establishment.
    async fn stream_stats(
        &self,
        request: tonic::Request<StatsRequest>,
    ) -> std::result::Result<tonic::Response<Self::StreamStatsStream>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let table = Arc::clone(&self.table);
        let collector = Arc::clone(&self.collector);

        let stream = async_stream::stream! {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let processes = table.snapshot().await;
                        let collector = Arc::clone(&collector);
                        // tick() is blocking (GPU detection shells out) —
                        // keep it off the async runtime.
                        match tokio::task::spawn_blocking(move || {
                            let mut c = collector.blocking_lock();
                            c.tick(processes)
                        })
                        .await
                        {
                            Ok(stats) => yield Ok(stats),
                            Err(e) => yield Err(tonic::Status::internal(format!(
                                "stats collection failed: {e}"
                            ))),
                        }
                    }
                }
            }
        }
        .boxed();

        Ok(tonic::Response::new(stream))
    }

    type StreamJobStream = Pin<
        Box<dyn tokio_stream::Stream<Item = std::result::Result<JobEvent, tonic::Status>> + Send>,
    >;

    /// Server-streaming job events for one job (plan-191 Task 6).
    ///
    /// Unknown job id → `not_found`. The stream ends right after the
    /// terminal event is emitted. A stream that ends BEFORE a terminal
    /// event (transport error, lag, or the tamad dying) means the job is
    /// broken — the proxy relay treats that as a failure.
    async fn stream_job(
        &self,
        request: tonic::Request<JobRequest>,
    ) -> std::result::Result<tonic::Response<Self::StreamJobStream>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let job_id = request.into_inner().job_id;
        let rx = self
            .jobs
            .subscribe(&job_id)
            .ok_or_else(|| tonic::Status::not_found(format!("unknown job id '{job_id}'")))?;

        // Late joiner: the job may already be terminal (e.g. the proxy
        // opened the stream after the runner finished). Replay the terminal
        // state and end immediately.
        if let Some(job) = self.jobs.get(&job_id) {
            if job.is_terminal() {
                let event = job.to_event();
                let stream =
                    futures_util::stream::once(async move { Ok::<JobEvent, tonic::Status>(event) })
                        .boxed();
                return Ok(tonic::Response::new(stream));
            }
        }

        // Wrap the broadcast receiver in a stream. The capacity is generous
        // (256) so `Lagged` is effectively impossible for low-rate job
        // progress; if it happens, surface it as an error. A closed channel
        // simply ends the stream. A stream that ends BEFORE a terminal event
        // (error or close) means the job is broken — the proxy relay treats
        // that as a failure.
        let stream = {
            let stream_job_id = job_id.clone();
            // Wrap the broadcast receiver in a stream. The capacity is
            // generous (256) so `Lagged` is effectively impossible for
            // low-rate job progress; if it happens, surface it as an error
            // so the proxy relay can mark the job failed. A closed channel
            // simply ends the stream. A stream that ends BEFORE a terminal
            // event (error or close) means the job is broken — the proxy
            // relay treats that as a failure.
            async_stream::stream! {
                let broadcast = tokio_stream::wrappers::BroadcastStream::new(rx)
                    .map(move |res| -> Option<std::result::Result<JobEvent, tonic::Status>> {
                        match res {
                            Ok(ev) if ev.job_id != stream_job_id => {
                                None // shared channel — filter by id
                            }
                            Ok(ev) => Some(Ok(ev)),
                            Err(
                                tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(
                                    n,
                                ),
                            ) => Some(Err(tonic::Status::internal(
                                format!("job stream lagged by {n} events"),
                            ))),
                        }
                    })
                    .filter_map(|maybe| async move { maybe });
                futures_util::pin_mut!(broadcast);

                // Yield every event for this job; end the stream right after
                // the terminal event is emitted.
                while let Some(item) = broadcast.next().await {
                    let terminal = matches!(&item, Ok(ev) if is_terminal_event(ev));
                    yield item;
                    if terminal {
                        break;
                    }
                }
            }
            .boxed()
        };

        Ok(tonic::Response::new(stream))
    }

    async fn restart_provider(
        &self,
        request: tonic::Request<RestartProviderRequest>,
    ) -> std::result::Result<tonic::Response<GrpcEmpty>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let model_name = request.into_inner().model_name;
        match self.lifecycle.restart(&model_name).await {
            Ok(_) => Ok(tonic::Response::new(GrpcEmpty {})),
            Err(e) if e.to_string().contains("not loaded on this tamad") => {
                Err(tonic::Status::not_found(format!(
                    "model '{}' is not loaded on this tamad",
                    model_name
                )))
            }
            Err(e) => Err(tonic::Status::internal(format!(
                "failed to restart model '{}': {}",
                model_name, e
            ))),
        }
    }

    async fn pull_model(
        &self,
        request: tonic::Request<PullModelRequest>,
    ) -> std::result::Result<tonic::Response<JobIdResponse>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let req = request.into_inner();

        // Validate up front for a fast, actionable error; the runner
        // re-validates (defense in depth — path traversal safety).
        if !tama_core::models::is_valid_repo_id(&req.repo_id) {
            return Err(tonic::Status::invalid_argument(format!(
                "invalid repo_id: '{}'",
                req.repo_id
            )));
        }

        let models_dir = self.state.models_dir.clone();
        tracing::info!(
            repo = %req.repo_id,
            repo_pull = req.repo_pull,
            "pull job starting"
        );
        let hf_token = req.hf_token.clone();
        let job_id = self
            .jobs
            .start("pull", move |handle| {
                Box::pin(async move {
                    crate::pulls::run_pull(&req, &models_dir, &hf_token, handle).await
                })
            })
            .await;
        Ok(tonic::Response::new(JobIdResponse { job_id }))
    }

    async fn run_benchmark(
        &self,
        request: tonic::Request<RunBenchmarkRequest>,
    ) -> std::result::Result<tonic::Response<JobIdResponse>, tonic::Status> {
        check_auth(&request, &self.expected_token)?;
        let req = request.into_inner();

        // Validate up front for a fast, actionable error; the runner
        // re-validates against this host's own disks (paths are
        // tamad-relative — the proxy does not know our roots).
        crate::bench::validate_config_json(&req.kind, &req.config_json)
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        if req.model_path_rel.trim().is_empty() {
            return Err(tonic::Status::invalid_argument(
                "model_path_rel must not be empty",
            ));
        }
        if req.binary_path_rel.trim().is_empty() {
            return Err(tonic::Status::invalid_argument(
                "binary_path_rel must not be empty",
            ));
        }

        tracing::info!(
            model = %req.model_name,
            kind = %req.kind,
            "benchmark job starting"
        );
        let state = Arc::clone(&self.state);
        let executor = Arc::clone(&self.bench_executor);
        let job_id = self
            .jobs
            .start(crate::bench::KIND, move |handle| {
                Box::pin(async move {
                    crate::bench::run_benchmark(&req, &state, handle, &*executor).await
                })
            })
            .await;
        Ok(tonic::Response::new(JobIdResponse { job_id }))
    }

    async fn cancel_job(
        &self,
        request: tonic::Request<tama_core::tamad::tamad_service::CancelJobRequest>,
    ) -> std::result::Result<
        tonic::Response<tama_core::tamad::tamad_service::CancelJobResponse>,
        tonic::Status,
    > {
        check_auth(&request, &self.expected_token)?;
        let req = request.into_inner();
        // Idempotent: unknown or already-terminal ids report `false` instead
        // of erroring (the proxy retries cancels after reconnects).
        let cancelled = self.jobs.cancel(&req.job_id);
        Ok(tonic::Response::new(
            tama_core::tamad::tamad_service::CancelJobResponse { cancelled },
        ))
    }
}

pub async fn health_handler() -> String {
    serde_json::json!({ "status": "ok", "version": VERSION }).to_string()
}

pub async fn start(
    addr: &str,
    protocol: &str,
    state: Arc<TamadState>,
    table: Arc<ProcessTable>,
    lifecycle: Arc<TamadLifecycle>,
) -> Result<()> {
    let addr: SocketAddr = addr
        .parse()
        .map_err(|e| anyhow!("Invalid address '{}': {}", addr, e))?;

    let service = TamadServiceImpl::new(state.token().to_string(), state, table, lifecycle);

    let grpc_task = match protocol {
        "grpc" | "both" => {
            let grpc_addr = addr;
            Some(tokio::spawn(async move {
                info!(%grpc_addr, "Starting gRPC server");
                let serve = TonicServer::builder()
                    .add_service(tama_core::tamad::TamadServiceServer::new(service))
                    .serve(grpc_addr);

                if let Err(e) = serve.await {
                    tracing::error!(error = %e, "gRPC server error");
                }
            }))
        }
        _ => None,
    };

    let http_task = match protocol {
        "http" | "both" => {
            let http_addr: SocketAddr = if protocol == "both" {
                let mut a = addr;
                a.set_port(addr.port() + 1);
                a
            } else {
                addr
            };

            info!(%http_addr, "Starting HTTP server");

            let app = Router::new().route("/health", axum::routing::get(health_handler));

            Some(tokio::spawn(async move {
                match axum::serve(tokio::net::TcpListener::bind(http_addr).await.unwrap(), app)
                    .await
                {
                    Ok(()) => {}
                    Err(e) => tracing::error!(error = %e, "HTTP server error"),
                }
            }))
        }
        _ => None,
    };

    // Wait for all running tasks
    if let Some(task) = grpc_task {
        let _ = task.await;
    }
    if let Some(task) = http_task {
        let _ = task.await;
    }

    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use super::{TamadServiceImpl, TonicServer};
    use crate::process_table::ProcessTable;
    use crate::state::TamadState;

    /// Serializes tests that mutate process env (HF_ENDPOINT, PATH, ...).
    pub static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Build a `TamadState` rooted in a fresh tempdir (token file, models dir).
    pub fn test_state() -> (Arc<TamadState>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let args = crate::CliArgs {
            addr: "127.0.0.1:50051".to_string(),
            protocol: "grpc".to_string(),
            name: Some("test-box".to_string()),
            public_url: None,
            models_dir: Some(dir.path().join("models")),
            data_dir: Some(dir.path().join("data")),
        };
        (Arc::new(TamadState::from_cli(&args).unwrap()), dir)
    }

    /// Start the real gRPC service (token "secret") on an ephemeral port.
    /// Returns the endpoint plus the shared process table and job registry
    /// so tests can verify the streamed process snapshot and job events.
    pub async fn start_test_server() -> (
        tonic::transport::Endpoint,
        Arc<ProcessTable>,
        Arc<crate::jobs::JobRegistry>,
        tempfile::TempDir,
    ) {
        let (state, dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let lifecycle =
            crate::lifecycle::TamadLifecycle::new(Arc::clone(&table), Arc::clone(&state));
        let service = TamadServiceImpl::new(
            "secret".to_string(),
            state,
            Arc::clone(&table),
            Arc::new(lifecycle),
        );
        let jobs = Arc::clone(service.jobs());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Serve the pre-bound listener — no rebind race, no retries.
            let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
            if let Err(e) = TonicServer::builder()
                .add_service(tama_core::tamad::TamadServiceServer::new(service))
                .serve_with_incoming(incoming)
                .await
            {
                tracing::error!(error = %e, "test gRPC server error");
            }
        });
        (
            tonic::transport::Endpoint::from_shared(format!("http://{}", addr))
                .unwrap()
                .connect_timeout(std::time::Duration::from_secs(2)),
            table,
            jobs,
            dir,
        )
    }

    pub async fn connected_client(
        endpoint: tonic::transport::Endpoint,
    ) -> tama_core::tamad::TamadServiceClient<tonic::transport::Channel> {
        for _ in 0..20 {
            if let Ok(channel) = endpoint.clone().connect().await {
                return tama_core::tamad::TamadServiceClient::new(channel);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("test server did not become reachable");
    }

    /// Insert `authorization: Bearer {token}` into the request metadata.
    pub fn authed<T>(mut request: tonic::Request<T>, token: &str) -> tonic::Request<T> {
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use tama_core::tamad::{Empty, StatsRequest};

    /// StreamStats without auth → Unauthenticated.
    #[tokio::test]
    async fn test_stream_stats_requires_auth() {
        let (endpoint, _, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .stream_stats(tonic::Request::new(StatsRequest {}))
            .await
            .expect_err("missing auth must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// Integration: with auth, the stream emits ~1 snapshot/sec carrying
    /// real host memory and the shared process-table snapshot.
    #[tokio::test]
    async fn test_stream_stats_emits_ticks_with_process_snapshot() {
        use crate::process_table::ProcessEntry;

        let (endpoint, table, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;

        // Seed the table with one live and one dead entry; every tick must
        // carry both with the correct liveness flag.
        table
            .insert(ProcessEntry {
                model_name: "alive-model".to_string(),
                provider_name: "llama.cpp".to_string(),
                pid: std::process::id(),
                endpoint_url: "http://127.0.0.1:18080".to_string(),
                status: "ready".to_string(),
                started_at: std::time::Instant::now(),
                spec: tama_core::tamad::LoadModelRequest::default(),
            })
            .await;
        table
            .insert(ProcessEntry {
                model_name: "dead-model".to_string(),
                provider_name: "llama.cpp".to_string(),
                pid: crate::process_table::guaranteed_dead_pid(),
                endpoint_url: "http://127.0.0.1:18081".to_string(),
                status: "ready".to_string(),
                started_at: std::time::Instant::now(),
                spec: tama_core::tamad::LoadModelRequest::default(),
            })
            .await;

        let mut stream = client
            .stream_stats(authed(tonic::Request::new(StatsRequest {}), "secret"))
            .await
            .expect("stream must open with valid token")
            .into_inner();

        let first = stream
            .message()
            .await
            .expect("first tick")
            .expect("first tick payload");
        let second = stream
            .message()
            .await
            .expect("second tick")
            .expect("second tick payload");

        assert!(first.memory_total_bytes > 0, "host RAM must be reported");
        assert!(
            first.disk_total_bytes > 0,
            "models-dir disk must be reported"
        );
        assert!(!first.cpu_usage_percent.is_nan());

        assert_eq!(
            first.processes.len(),
            second.processes.len(),
            "both ticks must carry the same process snapshot"
        );
        assert_eq!(first.processes.len(), 2);

        let alive = first
            .processes
            .iter()
            .find(|p| p.model_name == "alive-model")
            .expect("alive-model in tick");
        assert!(alive.alive);
        let dead = first
            .processes
            .iter()
            .find(|p| p.model_name == "dead-model")
            .expect("dead-model in tick");
        assert!(!dead.alive, "dead PID must be flagged alive=false");
    }

    /// No authorization header → Unauthenticated on health_check.
    #[tokio::test]
    async fn test_auth_no_header_rejected() {
        let (endpoint, _table, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .health_check(tonic::Request::new(Empty {}))
            .await
            .expect_err("missing auth must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// Wrong bearer token → Unauthenticated on health_check.
    #[tokio::test]
    async fn test_auth_wrong_token_rejected() {
        let (endpoint, _table, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;
        let mut request = tonic::Request::new(Empty {});
        request
            .metadata_mut()
            .insert("authorization", "Bearer wrong".parse().unwrap());
        let err = client
            .health_check(request)
            .await
            .expect_err("wrong token must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// Correct bearer token → health_check succeeds with status + version.
    #[tokio::test]
    async fn test_auth_correct_token_accepted() {
        let (endpoint, _table, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;
        let mut request = tonic::Request::new(Empty {});
        request
            .metadata_mut()
            .insert("authorization", "Bearer secret".parse().unwrap());
        let response = client
            .health_check(request)
            .await
            .expect("correct token must be accepted");
        assert_eq!(response.get_ref().status, "ok");
        assert_eq!(response.get_ref().version, VERSION);
    }

    /// Auth is enforced on non-health RPCs too (list_providers).
    #[tokio::test]
    async fn test_auth_other_rpc_rejected_without_token() {
        let (endpoint, _table, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .list_providers(tonic::Request::new(Empty {}))
            .await
            .expect_err("missing auth must be rejected on every RPC");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    // ── check_auth unit tests (constant-time token compare) ────────────

    /// Wrong-length bearer token → Unauthenticated with the generic
    /// rejection message (the message must not hint at what was wrong).
    #[test]
    fn test_check_auth_wrong_length_token_rejected() {
        let mut request = tonic::Request::new(Empty {});
        request
            .metadata_mut()
            .insert("authorization", "Bearer short".parse().unwrap());
        let err = check_auth(&request, "secret").expect_err("wrong-length token must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(err.message(), "missing or invalid authorization");
    }

    /// Same-length token with one wrong mid-token byte (the mismatch a
    /// plain `==` would bail out on early) → Unauthenticated with the
    /// same generic rejection message.
    #[test]
    fn test_check_auth_wrong_char_token_rejected() {
        let mut request = tonic::Request::new(Empty {});
        request
            .metadata_mut()
            .insert("authorization", "Bearer secrot".parse().unwrap());
        let err = check_auth(&request, "secret").expect_err("wrong-char token must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
        assert_eq!(err.message(), "missing or invalid authorization");
    }

    #[test]
    fn test_const_time_eq_identical() {
        assert!(const_time_eq(b"Bearer secret", b"Bearer secret"));
        assert!(const_time_eq(b"", b""));
    }

    #[test]
    fn test_const_time_eq_different_lengths() {
        assert!(!const_time_eq(b"Bearer secret", b"Bearer secrets"));
        assert!(!const_time_eq(b"", b"x"));
        assert!(!const_time_eq(b"x", b""));
    }

    #[test]
    fn test_const_time_eq_single_byte_differs() {
        assert!(!const_time_eq(b"Bearer secret", b"Bearer secrot"));
        assert!(!const_time_eq(b"Bearer secret", b"Bearer Secret"));
    }

    /// PullModel validates the repo id and returns a job id for valid ones;
    /// the job (which fails: no real HF behind it) is streamable.
    #[tokio::test]
    async fn test_pull_model_rpc_validation_and_job_id() {
        let (endpoint, _table, jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;

        // Point HF at a closed local port so the (expected to fail)
        // download dies fast instead of touching the real network.
        {
            let _guard = test_support::ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", "http://127.0.0.1:9");
        }

        // Invalid repo id → InvalidArgument, no job created.
        let bad = client
            .pull_model(authed(
                tonic::Request::new(tama_core::tamad::PullModelRequest {
                    repo_id: "../evil".into(),
                    quants: vec!["x.gguf".into()],
                    model_name: String::new(),
                    backend: String::new(),
                    hf_token: String::new(),
                    repo_pull: false,
                    dest_dir: String::new(),
                }),
                "secret",
            ))
            .await
            .expect_err("invalid repo id must be rejected");
        assert_eq!(bad.code(), tonic::Code::InvalidArgument);

        // Valid repo id → a job id is returned; the download fails (no
        // reachable HF), but the job must exist and be streamable to a
        // terminal failed event.
        let resp = client
            .pull_model(authed(
                tonic::Request::new(tama_core::tamad::PullModelRequest {
                    repo_id: "org/definitely-missing-repo".into(),
                    quants: vec!["m.gguf".into()],
                    model_name: String::new(),
                    backend: "llama_cpp".into(),
                    hf_token: String::new(),
                    repo_pull: false,
                    dest_dir: String::new(),
                }),
                "secret",
            ))
            .await
            .expect("pull_model must return a job id")
            .into_inner();
        assert!(!resp.job_id.is_empty());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let job = loop {
            let job = jobs.get(&resp.job_id).expect("job exists");
            if job.is_terminal() {
                break job;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pull job did not finish"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        assert!(job.error.is_some(), "offline pull must fail with an error");

        std::env::remove_var("HF_ENDPOINT");
    }

    /// StreamJob without auth → Unauthenticated.
    #[tokio::test]
    async fn test_stream_job_requires_auth() {
        let (endpoint, _table, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .stream_job(tonic::Request::new(JobRequest {
                job_id: "job-1".into(),
            }))
            .await
            .expect_err("missing auth must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// StreamJob for an unknown job id → NotFound.
    #[tokio::test]
    async fn test_stream_job_unknown_id_not_found() {
        let (endpoint, _table, _jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .stream_job(authed(
                tonic::Request::new(JobRequest {
                    job_id: "job-does-not-exist".into(),
                }),
                "secret",
            ))
            .await
            .expect_err("unknown job id must be rejected");
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    /// StreamJob streams events in order and ends right after the terminal
    /// event (plan-191 Task 6: a stream that ends BEFORE a terminal event
    /// means the job is broken).
    #[tokio::test]
    async fn test_stream_job_emits_until_terminal_then_ends() {
        let (endpoint, _table, jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;

        // The runner is gated: the job stays running until the test releases
        // it, so the stream can be opened before it terminates.
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
        let job_id = jobs
            .start("pull", |h| {
                Box::pin(async move {
                    let _ = gate_rx.await;
                    h.report(25, "downloading");
                    Ok(r#"{"ok": true}"#.to_string())
                })
            })
            .await;

        let mut stream = client
            .stream_job(authed(
                tonic::Request::new(JobRequest {
                    job_id: job_id.clone(),
                }),
                "secret",
            ))
            .await
            .expect("stream must open with valid token")
            .into_inner();

        gate_tx.send(()).ok();

        let mut saw_running = false;
        let mut terminal = None;
        for _ in 0..200 {
            match stream.message().await {
                Ok(Some(ev)) => {
                    if ev.status == "running" {
                        saw_running = true;
                    } else {
                        terminal = Some(ev);
                        break;
                    }
                }
                Ok(None) => break, // stream ended
                Err(e) => panic!("stream error: {e}"),
            }
        }

        assert!(saw_running, "must observe running events");
        let terminal = terminal.expect("must receive the terminal event");
        assert_eq!(terminal.status, "succeeded");
        assert_eq!(terminal.job_id, job_id);
        assert_eq!(terminal.result_json, r#"{"ok": true}"#);

        // The stream must end immediately after the terminal event.
        let next = stream.message().await.expect("no error after terminal");
        assert!(next.is_none(), "stream must end after the terminal event");
    }

    /// A late subscriber (after the job already finished) receives the
    /// terminal event once, then the stream ends.
    #[tokio::test]
    async fn test_stream_job_replays_terminal_for_late_subscriber() {
        let (endpoint, _table, jobs, _dir) = start_test_server().await;
        let mut client = connected_client(endpoint).await;

        let job_id = jobs
            .start("pull", |_h| Box::pin(async { Ok("{}".to_string()) }))
            .await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let job = jobs.get(&job_id).expect("job exists");
            if job.is_terminal() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let mut stream = client
            .stream_job(authed(tonic::Request::new(JobRequest { job_id }), "secret"))
            .await
            .expect("late stream must open")
            .into_inner();

        let ev = stream
            .message()
            .await
            .expect("event")
            .expect("terminal event replayed");
        assert_eq!(ev.status, "succeeded");
        let ended = stream.message().await.expect("no error after replay");
        assert!(
            ended.is_none(),
            "stream must end after the replayed terminal"
        );
    }

    // ── Install / update / remove RPCs (plan-191 Task 7) ─────────────────

    /// Stub installer: writes a marker binary into the spec's target dir.
    #[derive(Clone)]
    struct MarkerInstaller;

    impl crate::installs::Installer for MarkerInstaller {
        fn run<'a>(
            &'a self,
            spec: &'a crate::installs::InstallSpec,
            sink: std::sync::Arc<dyn tama_core::installations::ProgressSink>,
        ) -> crate::installs::RunFuture<'a> {
            Box::pin(async move {
                sink.log("marker-installer: working");
                tokio::fs::create_dir_all(&spec.target_dir).await?;
                let bin = spec.target_dir.join("llama-server");
                tokio::fs::write(&bin, b"#!/bin/sh\necho marker").await?;
                Ok(bin)
            })
        }
    }

    /// Start a service with the marker installer bound to an ephemeral port.
    async fn start_install_test_server() -> (
        tonic::transport::Endpoint,
        Arc<ProcessTable>,
        Arc<crate::jobs::JobRegistry>,
        tempfile::TempDir,
    ) {
        let (state, dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let service = TamadServiceImpl::new(
            "secret".to_string(),
            Arc::clone(&state),
            Arc::clone(&table),
            Arc::new(crate::lifecycle::TamadLifecycle::new(
                Arc::clone(&table),
                Arc::clone(&state),
            )),
        )
        .with_installer(std::sync::Arc::new(MarkerInstaller));
        let jobs = Arc::clone(service.jobs());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
            if let Err(e) = TonicServer::builder()
                .add_service(tama_core::tamad::TamadServiceServer::new(service))
                .serve_with_incoming(incoming)
                .await
            {
                tracing::error!(error = %e, "test gRPC server error");
            }
        });
        (
            tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
                .unwrap()
                .connect_timeout(std::time::Duration::from_secs(2)),
            table,
            jobs,
            dir,
        )
    }

    /// InstallProvider without auth → Unauthenticated.
    #[tokio::test]
    async fn test_install_provider_requires_auth() {
        let (endpoint, _table, _jobs, _dir) = start_install_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .install_provider(tonic::Request::new(
                tama_core::tamad::InstallProviderRequest {
                    name: "llama_cpp".into(),
                    engine: "llama_cpp".into(),
                    version: "b100".into(),
                    gpu_variant: "cpu".into(),
                    force: false,
                    git_url: String::new(),
                },
            ))
            .await
            .expect_err("missing auth must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// InstallProvider with an unknown engine → InvalidArgument, no job.
    #[tokio::test]
    async fn test_install_provider_unknown_engine_rejected() {
        let (endpoint, _table, jobs, _dir) = start_install_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .install_provider(authed(
                tonic::Request::new(tama_core::tamad::InstallProviderRequest {
                    name: "docker_thing".into(),
                    engine: "docker".into(),
                    version: "1.0".into(),
                    gpu_variant: "cpu".into(),
                    force: false,
                    git_url: String::new(),
                }),
                "secret",
            ))
            .await
            .expect_err("docker is not host-installable");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(jobs.list().is_empty(), "no job may be created");
    }

    /// InstallProvider starts a job; StreamJob delivers the terminal
    /// succeeded event with the result JSON and the marker binary lands
    /// under `<data-dir>/install`. (Running-line streaming with ordered
    /// delivery is covered by the gated unit test in `installs` — a fast
    /// installer can legitimately finish before the stream opens, in which
    /// case the terminal is replayed.)
    #[tokio::test]
    async fn test_install_provider_job_streams_to_terminal() {
        let (endpoint, _table, _jobs, dir) = start_install_test_server().await;
        let mut client = connected_client(endpoint).await;

        let resp = client
            .install_provider(authed(
                tonic::Request::new(tama_core::tamad::InstallProviderRequest {
                    name: "llama_cpp".into(),
                    engine: "llama_cpp".into(),
                    version: "b100".into(),
                    gpu_variant: "cpu".into(),
                    force: false,
                    git_url: String::new(),
                }),
                "secret",
            ))
            .await
            .expect("install must accept with valid token")
            .into_inner();
        assert!(!resp.job_id.is_empty());

        let mut stream = client
            .stream_job(authed(
                tonic::Request::new(JobRequest {
                    job_id: resp.job_id,
                }),
                "secret",
            ))
            .await
            .expect("job stream must open")
            .into_inner();

        let mut terminal = None;
        for _ in 0..200 {
            match stream.message().await {
                Ok(Some(ev)) => {
                    if ev.status != "running" {
                        terminal = Some(ev);
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => panic!("stream error: {e}"),
            }
        }
        let terminal = terminal.expect("terminal event");
        assert_eq!(terminal.status, "succeeded");
        let result: serde_json::Value =
            serde_json::from_str(&terminal.result_json).expect("result JSON");
        assert_eq!(result["installed"], true);
        assert_eq!(result["version"], "b100");
        // The marker binary must live under <data-dir>/install.
        let expected = dir
            .path()
            .join("data/install/llama_cpp/cpu/b100/llama-server");
        assert_eq!(
            result["path"].as_str().unwrap(),
            expected.to_string_lossy().as_ref()
        );
        assert!(expected.exists());
    }

    /// UpdateProvider starts a job that installs the versioned directory
    /// (always overwritable) and reports the requested version.
    #[tokio::test]
    async fn test_update_provider_job_streams_to_terminal() {
        let (endpoint, _table, _jobs, dir) = start_install_test_server().await;
        let mut client = connected_client(endpoint).await;

        let resp = client
            .update_provider(authed(
                tonic::Request::new(tama_core::tamad::UpdateProviderRequest {
                    name: "llama_cpp".into(),
                    version: "b9123".into(),
                    engine: "llama_cpp".into(),
                    gpu_variant: "cuda".into(),
                    git_url: "https://example.com/repo.git".into(),
                }),
                "secret",
            ))
            .await
            .expect("update must accept with valid token")
            .into_inner();
        assert!(!resp.job_id.is_empty());

        let mut stream = client
            .stream_job(authed(
                tonic::Request::new(JobRequest {
                    job_id: resp.job_id,
                }),
                "secret",
            ))
            .await
            .expect("job stream must open")
            .into_inner();

        let mut terminal = None;
        for _ in 0..200 {
            match stream.message().await {
                Ok(Some(ev)) => {
                    if ev.status != "running" {
                        terminal = Some(ev);
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => panic!("stream error: {e}"),
            }
        }
        let terminal = terminal.expect("terminal event");
        assert_eq!(terminal.status, "succeeded");
        let result: serde_json::Value =
            serde_json::from_str(&terminal.result_json).expect("result JSON");
        assert_eq!(result["version"], "b9123");
        let expected = dir
            .path()
            .join("data/install/llama_cpp/cuda/b9123/llama-server");
        assert_eq!(
            result["path"].as_str().unwrap(),
            expected.to_string_lossy().as_ref()
        );
        assert!(expected.exists());
    }

    /// RemoveProvider kills the provider's processes and deletes the
    /// versioned install directories; unknown engines are a no-op.
    #[tokio::test]
    async fn test_remove_provider_kills_and_deletes() {
        let (endpoint, table, _jobs, dir) = start_install_test_server().await;
        let mut client = connected_client(endpoint).await;
        let state = test_state().0;

        // Seed: one live "llama_cpp" process + a versioned install dir.
        let install_root = dir.path().join("data/install");
        let version_dir = install_root.join("llama_cpp/cpu/b100");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(version_dir.join("llama-server"), b"binary").unwrap();

        let lifecycle = crate::lifecycle::TamadLifecycle::new(Arc::clone(&table), state.clone());
        let resp = lifecycle
            .load(&tama_core::tamad::LoadModelRequest {
                provider_name: "llama_cpp".into(),
                model_path: String::new(),
                gpu_variant: "cpu".into(),
                params: std::collections::HashMap::new(),
                model_name: "rm-me".into(),
                command: "sh".into(),
                args: vec!["-c".into(), "sleep 30".into()],
                env: std::collections::HashMap::new(),
                health_url: String::new(),
                health_timeout_ms: 0,
                gpu_device: String::new(),
            })
            .await
            .expect("seed load");
        let pid = resp.pid;

        client
            .remove_provider(authed(
                tonic::Request::new(tama_core::tamad::RemoveProviderRequest {
                    name: "llama_cpp".into(),
                    engine: "llama_cpp".into(),
                    gpu_variant: String::new(),
                    version: String::new(),
                }),
                "secret",
            ))
            .await
            .expect("remove must succeed");

        // Process dead + entry gone.
        for _ in 0..40 {
            if !crate::process::is_process_alive(pid as u32) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        assert!(
            !crate::process::is_process_alive(pid as u32),
            "backend must be killed"
        );
        assert!(table.get("rm-me").await.is_none(), "entry must be removed");

        // Whole backend dir (both variants) removed.
        assert!(!install_root.join("llama_cpp/cpu/b100").exists());
        assert!(!install_root.join("llama_cpp").exists());
    }

    // ── RunBenchmark RPC (plan-191 Task 8) ─────────────────────────────

    /// Minimal valid `llama_bench` config_json envelope (bench knobs +
    /// model metadata).
    const LLAMA_BENCH_CONFIG_JSON: &str = r#"{
        "bench": {"pp_sizes": [64], "tg_sizes": [16], "runs": 1, "warmup": 0,
                   "threads": null, "ngl_range": null, "ctx_override": null,
                   "batch_sizes": [], "ubatch_sizes": [],
                   "kv_cache_type": null, "depth": [], "flash_attn": null},
        "model_info": {"name": "m", "model_id": null, "quant": null,
                       "backend": "llama_cpp", "gpu_variant": "",
                       "context_length": null, "gpu_layers": null}
    }"#;

    fn bench_req(
        model_path_rel: &str,
        binary_path_rel: &str,
        config_json: &str,
    ) -> RunBenchmarkRequest {
        RunBenchmarkRequest {
            model_name: "test-model".into(),
            kind: "llama_bench".into(),
            config_json: config_json.to_string(),
            model_path_rel: model_path_rel.to_string(),
            binary_path_rel: binary_path_rel.to_string(),
        }
    }

    /// Stub bench executor: returns a scripted result JSON.
    #[derive(Clone)]
    struct StubBenchExecutor {
        json: String,
    }

    impl crate::bench::BenchExecutor for StubBenchExecutor {
        fn run<'a>(
            &'a self,
            _bench: &'a crate::bench::ResolvedBench,
            sink: std::sync::Arc<dyn tama_core::installations::ProgressSink>,
        ) -> crate::bench::BenchRunFuture<'a> {
            let json = self.json.clone();
            Box::pin(async move {
                sink.log("stub bench running");
                Ok(json)
            })
        }
    }

    /// Start a service with the stub bench executor on an ephemeral port.
    async fn start_bench_test_server() -> (
        tonic::transport::Endpoint,
        Arc<ProcessTable>,
        Arc<crate::jobs::JobRegistry>,
        tempfile::TempDir,
    ) {
        let (state, dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let service = TamadServiceImpl::new(
            "secret".to_string(),
            Arc::clone(&state),
            Arc::clone(&table),
            Arc::new(crate::lifecycle::TamadLifecycle::new(
                Arc::clone(&table),
                Arc::clone(&state),
            )),
        )
        .with_bench_executor(std::sync::Arc::new(StubBenchExecutor {
            json: r#"{"ok": true}"#.to_string(),
        }));
        let jobs = Arc::clone(service.jobs());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
            if let Err(e) = TonicServer::builder()
                .add_service(tama_core::tamad::TamadServiceServer::new(service))
                .serve_with_incoming(incoming)
                .await
            {
                tracing::error!(error = %e, "test gRPC server error");
            }
        });
        (
            tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
                .unwrap()
                .connect_timeout(std::time::Duration::from_secs(2)),
            table,
            jobs,
            dir,
        )
    }

    /// RunBenchmark without auth → Unauthenticated.
    #[tokio::test]
    async fn test_run_benchmark_requires_auth() {
        let (endpoint, _table, _jobs, _dir) = start_bench_test_server().await;
        let mut client = connected_client(endpoint).await;
        let err = client
            .run_benchmark(tonic::Request::new(bench_req(
                "m.gguf",
                "llama_cpp/cpu/v1/llama-server",
                "{}",
            )))
            .await
            .expect_err("missing auth must be rejected");
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    /// Invalid kind / bad config_json / empty rel paths → InvalidArgument,
    /// no job created.
    #[tokio::test]
    async fn test_run_benchmark_rejects_bad_requests() {
        let (endpoint, _table, jobs, _dir) = start_bench_test_server().await;
        let mut client = connected_client(endpoint).await;

        let mut bad_kind = bench_req("m.gguf", "b", "{}");
        bad_kind.kind = "docker".into();
        let err = client
            .run_benchmark(authed(tonic::Request::new(bad_kind), "secret"))
            .await
            .expect_err("unknown kind must be rejected");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        let err = client
            .run_benchmark(authed(
                tonic::Request::new(bench_req("m.gguf", "b", "not-json")),
                "secret",
            ))
            .await
            .expect_err("bad config_json must be rejected");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        let err = client
            .run_benchmark(authed(
                tonic::Request::new(bench_req("", "b", "{}")),
                "secret",
            ))
            .await
            .expect_err("empty model_path_rel must be rejected");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        assert!(jobs.list().is_empty(), "no job may be created");
    }

    /// A host-missing model fails the job with the actionable host-path
    /// error (paths are tamad-relative, so only the host can check).
    #[tokio::test]
    async fn test_run_benchmark_missing_model_fails_job() {
        let (endpoint, _table, jobs, _dir) = start_bench_test_server().await;
        let mut client = connected_client(endpoint).await;

        let resp = client
            .run_benchmark(authed(
                tonic::Request::new(bench_req("missing/m.gguf", "b", LLAMA_BENCH_CONFIG_JSON)),
                "secret",
            ))
            .await
            .expect("dispatch accepted; the job itself must fail")
            .into_inner();
        assert!(!resp.job_id.is_empty());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let job = loop {
            let job = jobs.get(&resp.job_id).expect("job exists");
            if job.is_terminal() {
                break job;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "benchmark job did not finish"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        assert!(
            job.error
                .as_deref()
                .unwrap_or_default()
                .contains("model not found on this host"),
            "got: {:?}",
            job.error
        );
    }

    /// A valid request streams a terminal succeeded event whose result
    /// JSON is the executor's report.
    #[tokio::test]
    async fn test_run_benchmark_job_streams_to_terminal() {
        let (endpoint, _table, jobs, dir) = start_bench_test_server().await;
        let mut client = connected_client(endpoint).await;

        // Seed the host files the stub executor will "benchmark".
        let model_path = dir.path().join("models/org/m/m.gguf");
        std::fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        std::fs::write(&model_path, b"gguf").unwrap();
        let binary_path = dir
            .path()
            .join("data/install/llama_cpp/cpu/v1/llama-server");
        std::fs::create_dir_all(binary_path.parent().unwrap()).unwrap();
        std::fs::write(&binary_path, b"binary").unwrap();

        let resp = client
            .run_benchmark(authed(
                tonic::Request::new(bench_req(
                    "org/m/m.gguf",
                    "llama_cpp/cpu/v1/llama-server",
                    LLAMA_BENCH_CONFIG_JSON,
                )),
                "secret",
            ))
            .await
            .expect("dispatch must be accepted")
            .into_inner();
        assert!(!resp.job_id.is_empty());

        let mut stream = client
            .stream_job(authed(
                tonic::Request::new(JobRequest {
                    job_id: resp.job_id,
                }),
                "secret",
            ))
            .await
            .expect("job stream must open")
            .into_inner();

        let mut terminal = None;
        for _ in 0..200 {
            match stream.message().await {
                Ok(Some(ev)) => {
                    if ev.status != "running" {
                        terminal = Some(ev);
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => panic!("stream error: {e}"),
            }
        }
        let terminal = terminal.expect("terminal event");
        assert_eq!(terminal.status, "succeeded");
        assert_eq!(terminal.result_json, r#"{"ok": true}"#);
        let job = jobs.get(&terminal.job_id).expect("job retained");
        assert!(job.is_terminal());
    }
}
