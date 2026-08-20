use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{sse::Event, sse::KeepAlive, IntoResponse, Response, Sse},
    Json,
};
use futures_util::Stream;

use serde::{Deserialize, Serialize};

use super::types::QuantEntry;
use crate::gpu::VramInfo;
use crate::proxy::ProxyState;

/// Typed response for the system health endpoint.
///
/// The top-level fields describe the **proxy** (its process + host CPU/RAM);
/// `hosts[]` carries the per-tamad health facts (plan-191 Task 9 — the proxy
/// itself no longer presents hardware as the inference host, so
/// `gpu_utilization_pct`/`vram` are always `null` now).
#[derive(Debug, Serialize)]
pub struct SystemHealthResponse {
    pub status: &'static str,
    pub service: &'static str,
    pub models_loaded: usize,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    /// Legacy field — the proxy no longer samples local GPUs (plan-191); always `null`.
    pub gpu_utilization_pct: Option<u8>,
    /// Legacy field — the proxy no longer samples local GPUs (plan-191); always `null`.
    pub vram: Option<VramInfo>,
    /// The proxy binary's version.
    pub version: String,
    /// Proxy process uptime in seconds.
    pub uptime_seconds: f64,
    /// One entry per registered tamad (per-tamad host health).
    pub hosts: Vec<SystemHostHealth>,
}

/// Handle system health check (Tama management API).
///
/// Top-level = proxy process facts; `hosts[]` = per-tamad host facts from
/// the stats-stream pool (plan-191 Task 9). Zero-tamad deployments get
/// `hosts: []` plus the legacy proxy fields (back-compat).
pub async fn handle_tama_system_health(
    state: State<Arc<ProxyState>>,
) -> Json<SystemHealthResponse> {
    let models_loaded = state.registry.models.read().await.len();
    let metrics = state.metrics.system_metrics_snapshot().await;

    Json(SystemHealthResponse {
        status: "ok",
        service: "tama",
        models_loaded,
        cpu_usage_pct: metrics.cpu_usage_pct,
        ram_used_mib: metrics.ram_used_mib,
        ram_total_mib: metrics.ram_total_mib,
        gpu_utilization_pct: metrics.gpu_utilization_pct,
        vram: metrics.vram.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.started_at.elapsed().as_secs_f64(),
        hosts: build_health_hosts(&state.tamad_pool).await,
    })
}

// TODO(plan-172): unrouted after plan-169 — delete
/// Handle listing available GGUF quants for a HuggingFace repo (Tama management API).
///
/// `repo_id` is captured as a wildcard path segment (e.g. `bartowski/Qwen3-8B-GGUF`)
/// because HF repo IDs contain a `/`. Registered as `GET /tama/v1/hf/*repo_id`.
pub async fn handle_hf_list_quants(Path(repo_id): Path<String>) -> Response {
    // Reject repo_id segments containing traversal sequences or null bytes (SSRF mitigation).
    if !crate::models::is_valid_repo_id(&repo_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid repo_id" })),
        )
            .into_response();
    }

    match crate::models::pull::lookup_blob_metadata(&repo_id).await {
        Ok(blobs) => {
            let mut quants: Vec<QuantEntry> = crate::models::pull::group_sharded_quants(blobs)
                .into_iter()
                .map(|g| QuantEntry {
                    filename: g.filename,
                    quant: g.quant,
                    size_bytes: g.size_bytes,
                    kind: g.kind,
                    shards: g.shards,
                })
                .collect();
            quants.sort_by(|a, b| a.filename.cmp(&b.filename));
            (StatusCode::OK, Json(quants)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Handle system restart (Tama management API).
/// Triggers a graceful shutdown and then exits the process.
pub async fn handle_tama_system_restart(state: State<Arc<ProxyState>>) -> Response {
    // Trigger graceful shutdown first
    state.0.shutdown().await;

    // Schedule process exit on a short delay so the HTTP response can be delivered.
    // We use std::process::exit(0) here because this is a hard restart operation
    // - we want to immediately terminate all background tasks (metrics, DB, etc.)
    // without waiting for them to drain. The shutdown() call above has already
    // cleared in-memory state (models, pull jobs, metrics channel).
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::process::exit(0);
    });

    // Return a response to the client
    Response::builder()
        .status(200)
        .body(axum::body::Body::from("Tama is shutting down"))
        .expect("Response::builder with valid status and body should not fail")
}

/// Stream live system metrics snapshots as SSE events.
///
/// Subscribes to the `metrics_tx` broadcast channel in `ProxyState`. Each
/// tick (every 2s), the metrics task broadcasts a [`MetricsSnapshot`] that
/// splits a rolling history of graphable fields (CPU, RAM, Network) from
/// point-in-time state (GPU devices, model statuses, inference stats). This
/// handler serializes the snapshot as JSON and emits it as `event: "snapshot"`.
///
/// On subscriber lag, the handler silently skips the missed tick — the next
/// snapshot will contain the full history. On channel close (empty Arc
/// sentinel), the stream ends.
///
/// Registered as `GET /tama/v1/system/metrics/stream`.
pub async fn handle_system_metrics_stream(
    State(state): State<Arc<ProxyState>>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.metrics.subscribe_metrics();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(snapshot) => {
                    // Shutdown sentinel: empty buckets signals stream end.
                    if snapshot.buckets.is_empty() { break; }
                    match serde_json::to_value(&snapshot) {
                        Ok(mut value) => {
                            // Additive `hosts` field: per-tamad cpu/memory/gpus
                            // from the pool's latest snapshots (plan-191
                            // Task 4). Ignored by the old UI.
                            value["hosts"] =
                                serde_json::Value::Array(build_hosts(&state.tamad_pool).await);
                            yield Ok(Event::default().event("snapshot").data(value.to_string()));
                        }
                        Err(e) => tracing::warn!("failed to serialize MetricsSnapshot: {}", e),
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Subscriber lagged — next snapshot will have full history, no action needed
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Query parameters for GPU device listing.
#[derive(Debug, Deserialize)]
pub struct GpuDevicesQuery {
    pub backend: String,
    pub gpu_variant: String,
}

/// One per-tamad entry in `GET /tama/v1/system/health` (plan-191 Task 9).
/// Built from the pool's latest stats snapshot + cached `HealthCheck`.
#[derive(Debug, Clone, Serialize)]
pub struct SystemHostHealth {
    pub tamad_id: String,
    pub name: String,
    pub online: bool,
    /// The tamad's self-reported version (last successful HealthCheck).
    pub version: Option<String>,
    pub cpu_percent: f64,
    pub memory_used_pct: f64,
    /// Number of GPUs reported in the latest stats snapshot.
    pub gpus_online: i32,
}

/// Map one tamad handle's latest data into a [`SystemHostHealth`].
///
/// A `None` stats snapshot (tamad never connected, or HTTP protocol) yields
/// zeroed metrics — the host is still listed so the UI shows it as offline.
fn host_health(
    tamad_id: &str,
    name: &str,
    online: bool,
    version: Option<String>,
    stats: Option<&crate::tamad::SystemStats>,
) -> SystemHostHealth {
    let (cpu_percent, memory_used_pct, gpus_online) = match stats {
        Some(s) => {
            let pct = if s.memory_total_bytes > 0 {
                s.memory_used_bytes as f64 / s.memory_total_bytes as f64 * 100.0
            } else {
                0.0
            };
            (s.cpu_usage_percent, pct, s.gpus.len() as i32)
        }
        None => (0.0, 0.0, 0),
    };
    SystemHostHealth {
        tamad_id: tamad_id.to_string(),
        name: name.to_string(),
        online,
        version,
        cpu_percent,
        memory_used_pct,
        gpus_online,
    }
}

/// Build the `hosts` array for the system health response: one entry per
/// tamad in the pool (plan-191 Task 9).
async fn build_health_hosts(pool: &crate::tamad::pool::TamadPool) -> Vec<SystemHostHealth> {
    let handles = pool.list_handles().await;
    let mut out = Vec::with_capacity(handles.len());
    for h in &handles {
        let online = h.is_online().await;
        let version = h.version().await;
        let stats = h.latest().await;
        out.push(host_health(
            &h.connection.id,
            &h.connection.name,
            online,
            version,
            stats.as_ref(),
        ));
    }
    out
}

/// Build the additive `hosts` array for the dashboard metrics stream:
/// one entry per tamad in the pool with its latest stats (~1s fresh), live
/// online flag, and the cached `HealthCheck` version. Old UIs ignore the
/// field (plan-191 Task 9 frontend consumes it).
async fn build_hosts(pool: &crate::tamad::pool::TamadPool) -> Vec<serde_json::Value> {
    let handles = pool.list_handles().await;
    let mut hosts: Vec<serde_json::Value> = Vec::with_capacity(handles.len());
    for handle in &handles {
        let stats = handle.latest().await;
        let version = handle.version().await;
        let gpus = stats
            .iter()
            .flat_map(|s| s.gpus.iter())
            .map(|g| {
                serde_json::json!({
                    "index": g.index,
                    "name": g.name,
                    "driver_version": g.driver_version,
                    "vram_total_bytes": g.vram_total_bytes,
                    "vram_used_bytes": g.vram_used_bytes,
                    "utilization_percent": g.utilization_percent,
                    "temperature_c": g.temperature_c,
                    "power_w": g.power_w,
                })
            })
            .collect::<Vec<_>>();
        hosts.push(serde_json::json!({
            "tamad_id": handle.connection.id,
            "name": handle.connection.name,
            "online": handle.is_online().await,
            "version": version,
            "cpu_percent": stats.as_ref().map(|s| s.cpu_usage_percent).unwrap_or(0.0),
            "memory": {
                "total_bytes": stats.as_ref().map(|s| s.memory_total_bytes).unwrap_or(0),
                "used_bytes": stats.as_ref().map(|s| s.memory_used_bytes).unwrap_or(0),
            },
            "gpus": gpus,
        }));
    }
    hosts
}

/// Convert a tamad `GpuInfo` into the gpu-devices response shape, tagged
/// with its tamad name.
fn tamad_device_value(gpu: &crate::tamad::GpuInfo, tamad_name: &str) -> serde_json::Value {
    let mib = 1024 * 1024;
    let vram_total_mib = (gpu.vram_total_bytes as u64) / mib;
    let vram_free_mib = (gpu.vram_total_bytes - gpu.vram_used_bytes).max(0) as u64 / mib;
    serde_json::json!({
        "device_id": format!("GPU{}", gpu.index),
        "name": gpu.name,
        // The gRPC GpuInfo message has no vendor field yet — kept empty for
        // response-shape stability; the frontend ignores it for per-tamad
        // devices.
        "vendor": "",
        "vram_total_mib": vram_total_mib,
        "vram_free_mib": vram_free_mib,
        "utilization_pct": gpu.utilization_percent,
        "temperature_c": gpu.temperature_c,
        "tamad": tamad_name,
    })
}

/// Build the gpu-devices response: the GPUs reported by every tamad in the
/// pool (latest stats snapshot, ~1s fresh), each tagged with its tamad name
/// (plan-191 Task 9 — the proxy no longer runs local `--list-devices`
/// rescans; `hosts: []` ⇒ `[]`, the legacy zero-hardware shape).
async fn gpu_devices_union(pool: &crate::tamad::pool::TamadPool) -> Vec<serde_json::Value> {
    let handles = pool.list_handles().await;
    let mut values: Vec<serde_json::Value> = Vec::new();
    for handle in &handles {
        if let Some(stats) = handle.latest().await {
            values.extend(
                stats
                    .gpus
                    .iter()
                    .map(|g| tamad_device_value(g, &handle.connection.name)),
            );
        }
    }
    values
}

/// Handle listing GPU devices for a backend (Tama management API).
///
/// Returns the GPUs reported by every registered tamad, each tagged with
/// its tamad name — this is how the model editor lists inference-host GPUs
/// per host (plan-191 Task 9; the proxy itself presents no local hardware
/// as the inference host). The `backend`/`gpu_variant` query params are
/// kept for client compatibility but do not filter the host-level device
/// list.
///
/// Registered as `GET /tama/v1/system/gpu-devices?backend=<name>`.
pub async fn handle_tama_system_gpu_devices(
    State(state): State<Arc<ProxyState>>,
    Query(_query): Query<GpuDevicesQuery>,
) -> Response {
    let devices = gpu_devices_union(&state.tamad_pool).await;
    (StatusCode::OK, Json(devices)).into_response()
}

/// Handle refreshing GPU devices for a backend (Tama management API).
///
/// The per-tamad stats streams keep device data continuously fresh (~1s
/// cadence), so a "refresh" simply returns the current per-tamad union —
/// no local re-scan (plan-191 Task 9). Same response shape as
/// `GET /tama/v1/system/gpu-devices`.
///
/// Registered as `POST /tama/v1/system/gpu-devices/refresh?backend=<name>`.
pub async fn handle_tama_system_gpu_devices_refresh(
    State(state): State<Arc<ProxyState>>,
    Query(_query): Query<GpuDevicesQuery>,
) -> Response {
    let devices = gpu_devices_union(&state.tamad_pool).await;
    (StatusCode::OK, Json(devices)).into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::tamad::pool::test_support::*;
    use crate::tamad::pool::TamadPool;
    use crate::tamad::GpuInfo;
    use crate::testing::postgres::with_schema;

    /// The per-tamad health entry maps the pool's latest stats: cpu%, memory
    /// used% (0 when unknown), and the count of gpus; `None` stats (offline
    /// host with no snapshot) yields zeroed metrics (plan-191 Task 9).
    #[test]
    fn test_host_health_fields() {
        let with_gpus = crate::tamad::SystemStats {
            cpu_usage_percent: 42.5,
            memory_total_bytes: 1024,
            memory_used_bytes: 512,
            swap_total_bytes: 0,
            swap_used_bytes: 0,
            disk_total_bytes: 0,
            disk_free_bytes: 0,
            gpus: vec![
                GpuInfo {
                    index: 0,
                    name: "A".to_string(),
                    driver_version: String::new(),
                    vram_total_bytes: 0,
                    vram_used_bytes: 0,
                    utilization_percent: 0.0,
                    temperature_c: 0.0,
                    power_w: 0.0,
                },
                GpuInfo {
                    index: 1,
                    name: "B".to_string(),
                    driver_version: String::new(),
                    vram_total_bytes: 0,
                    vram_used_bytes: 0,
                    utilization_percent: 0.0,
                    temperature_c: 0.0,
                    power_w: 0.0,
                },
            ],
            processes: vec![],
        };

        let h = host_health(
            "uuid-1",
            "host-a",
            true,
            Some("9.9.9".to_string()),
            Some(&with_gpus),
        );
        assert_eq!(h.tamad_id, "uuid-1");
        assert_eq!(h.name, "host-a");
        assert!(h.online);
        assert_eq!(h.version.as_deref(), Some("9.9.9"));
        assert_eq!(h.cpu_percent, 42.5);
        assert!((h.memory_used_pct - 50.0).abs() < 1e-9, "512/1024 = 50%");
        assert_eq!(h.gpus_online, 2);

        // Zero total memory: pct must be 0, not NaN/inf.
        let no_mem = crate::tamad::SystemStats {
            cpu_usage_percent: 1.0,
            memory_total_bytes: 0,
            memory_used_bytes: 0,
            ..with_gpus.clone()
        };
        let h = host_health("uuid-1", "host-a", true, None, Some(&no_mem));
        assert_eq!(h.memory_used_pct, 0.0);

        // No snapshot at all: zeroed metrics, no version.
        let h = host_health("uuid-2", "host-b", false, None, None);
        assert!(!h.online);
        assert!(h.version.is_none());
        assert_eq!(h.cpu_percent, 0.0);
        assert_eq!(h.memory_used_pct, 0.0);
        assert_eq!(h.gpus_online, 0);
    }

    /// `GET /tama/v1/system/health` with a live stub tamad: `hosts[]`
    /// carries the per-tamad fields (id, name, online, version, cpu, memory
    /// pct, gpus online) while the legacy top-level proxy fields stay (plan-191
    /// Task 9).
    #[tokio::test]
    async fn test_health_endpoint_with_stub_tamad() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        let addr = start_stub(stub_default()).await;
        let url = format!("grpc://{addr}");

        let state = Arc::new(ProxyState::new(
            crate::config::Config::default(),
            None,
            db_pool.clone(),
        ));
        state
            .tamad_pool()
            .upsert_connection(&grpc_conn("uuid-h1", "host-a", &url))
            .await
            .unwrap();

        let handle = state
            .tamad_pool()
            .get("uuid-h1")
            .await
            .expect("handle registered");
        assert!(
            wait_for(|| async { handle.latest().await.is_some() }).await,
            "a snapshot should arrive from the stub stream"
        );
        assert!(
            wait_for(|| async { handle.version().await.is_some() }).await,
            "health check should be cached"
        );

        let resp = handle_tama_system_health(axum::extract::State(Arc::clone(&state)))
            .await
            .0;

        assert_eq!(resp.status, "ok");
        assert_eq!(resp.service, "tama");
        assert!(!resp.version.is_empty(), "proxy version must be reported");
        assert!(resp.uptime_seconds >= 0.0);
        assert_eq!(resp.hosts.len(), 1, "one host per registered tamad");
        let host = &resp.hosts[0];
        assert_eq!(host.tamad_id, "uuid-h1");
        assert_eq!(host.name, "host-a");
        assert!(host.online);
        assert_eq!(host.version.as_deref(), Some("9.9.9-stub"));
        assert_eq!(host.cpu_percent, 42.5);
        assert!((host.memory_used_pct - 50.0).abs() < 1e-9);
        assert_eq!(host.gpus_online, 0, "stub stats carry no gpus");

        // Serialized shape: additive fields coexist with the legacy ones.
        let json = serde_json::to_value(&resp).unwrap();
        for key in [
            "status",
            "service",
            "models_loaded",
            "cpu_usage_pct",
            "ram_used_mib",
            "ram_total_mib",
            "gpu_utilization_pct",
            "vram",
            "version",
            "uptime_seconds",
            "hosts",
        ] {
            assert!(json.get(key).is_some(), "missing key: {key}");
        }
        assert!(json["hosts"][0]["cpu_percent"].is_number());

        guard.finish().await;
    }

    /// Zero-tamad deployment: the response keeps the full legacy shape with
    /// `hosts: []` and the GPU fields `null` (back-compat, plan-191 Task 9).
    #[tokio::test]
    async fn test_health_zero_tamads_back_compat() {
        let state = Arc::new(ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        ));

        let resp = handle_tama_system_health(axum::extract::State(Arc::clone(&state)))
            .await
            .0;

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "tama");
        assert!(json["hosts"].is_array());
        assert_eq!(
            json["hosts"].as_array().unwrap().len(),
            0,
            "hosts must be []"
        );
        assert!(json["gpu_utilization_pct"].is_null());
        assert!(json["vram"].is_null());
        assert!(json["version"].as_str().unwrap().split('.').count() == 3);
        assert!(json["uptime_seconds"].as_f64().unwrap() >= 0.0);
    }

    /// The additive `hosts` array carries per-tamad cpu/memory/gpus and the
    /// live online flag from the pool's latest snapshot (plan-191 Task 4
    /// acceptance: dashboard SSE payload contains `hosts[]`).
    #[tokio::test]
    async fn test_build_hosts_includes_populated_tamad() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        let (keep_open, _) = tokio::sync::watch::channel(false);
        let stub = StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::new(keep_open),
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
            stats_processes: vec![],
            logs_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            log_messages: vec![],
        };
        let addr = start_stub(stub).await;
        let url = format!("grpc://{addr}");

        let pool = TamadPool::new(db_pool).with_backoff_base(Duration::from_millis(20));
        let conn = grpc_conn("uuid-hosts", "host-a", &url);
        pool.upsert_connection(&conn).await.unwrap();

        let handle = pool.get("uuid-hosts").await.expect("handle registered");
        assert!(
            wait_for(|| async { handle.latest().await.is_some() }).await,
            "a snapshot should arrive from the stub stream"
        );
        assert!(
            wait_for(|| async { handle.version().await.is_some() }).await,
            "the HealthCheck version should be cached after the stream opens"
        );

        let hosts = build_hosts(&pool).await;
        assert_eq!(hosts.len(), 1, "one host per registered tamad");
        let host = &hosts[0];
        assert_eq!(host["tamad_id"], "uuid-hosts");
        assert_eq!(host["name"], "host-a");
        assert_eq!(host["online"], true);
        assert_eq!(
            host["version"], "9.9.9-stub",
            "host version should be the cached HealthCheck version"
        );
        assert_eq!(host["cpu_percent"], 42.5);
        assert_eq!(host["memory"]["total_bytes"], 1024);
        assert_eq!(host["memory"]["used_bytes"], 512);
        assert!(host["gpus"].is_array());

        guard.finish().await;
    }

    /// The gpu-devices union is the per-tamad GPU list (each entry tagged
    /// with its tamad name); with zero tamads the response is `[]` (the
    /// legacy zero-hardware shape, plan-191 Task 9).
    #[tokio::test]
    async fn test_gpu_devices_union_per_tamad() {
        let guard = with_schema().await;
        let db_pool = Arc::new(guard.pool.clone());

        // Stub host with two GPUs in every snapshot.
        let (down_tx, _) = tokio::sync::watch::channel(false);
        let make_gpus = || {
            vec![
                GpuInfo {
                    index: 0,
                    name: "gfx-a".to_string(),
                    driver_version: String::new(),
                    vram_total_bytes: 16 * 1024 * 1024 * 1024,
                    vram_used_bytes: 4 * 1024 * 1024 * 1024,
                    utilization_percent: 12.0,
                    temperature_c: 45.0,
                    power_w: 60.0,
                },
                GpuInfo {
                    index: 1,
                    name: "gfx-b".to_string(),
                    driver_version: String::new(),
                    vram_total_bytes: 8 * 1024 * 1024 * 1024,
                    vram_used_bytes: 0,
                    utilization_percent: 0.0,
                    temperature_c: 40.0,
                    power_w: 10.0,
                },
            ]
        };
        let mut stub = stub_default();
        stub.stats_gpus = make_gpus();
        let _ = down_tx;
        let addr = start_stub(stub).await;
        let url = format!("grpc://{addr}");

        let state = Arc::new(ProxyState::new(
            crate::config::Config::default(),
            None,
            db_pool.clone(),
        ));
        state
            .tamad_pool()
            .upsert_connection(&grpc_conn("uuid-gpu", "host-a", &url))
            .await
            .unwrap();

        let handle = state.tamad_pool().get("uuid-gpu").await.unwrap();
        assert!(
            wait_for(|| async { handle.latest().await.is_some() }).await,
            "snapshot should arrive"
        );

        // Direct union + both handler entry points return the same list.
        let union = gpu_devices_union(&state.tamad_pool).await;
        assert_eq!(union.len(), 2, "one entry per tamad GPU");
        for entry in &union {
            assert_eq!(entry["tamad"], "host-a", "entry must carry its tamad name");
        }
        assert_eq!(union[0]["device_id"], "GPU0");
        assert_eq!(union[0]["name"], "gfx-a");
        assert_eq!(union[0]["vram_total_mib"], 16 * 1024);
        assert_eq!(union[0]["vram_free_mib"], 12 * 1024);
        assert_eq!(union[1]["vram_total_mib"], 8 * 1024);

        let list_body = axum::body::to_bytes(
            handle_tama_system_gpu_devices(
                axum::extract::State(Arc::clone(&state)),
                axum::extract::Query(GpuDevicesQuery {
                    backend: "llama_cpp".to_string(),
                    gpu_variant: "cuda".to_string(),
                }),
            )
            .await
            .into_body(),
            usize::MAX,
        )
        .await
        .unwrap();
        let list_resp: serde_json::Value = serde_json::from_slice(&list_body).unwrap();
        assert_eq!(list_resp, serde_json::json!(union));

        let refresh_body = axum::body::to_bytes(
            handle_tama_system_gpu_devices_refresh(
                axum::extract::State(Arc::clone(&state)),
                axum::extract::Query(GpuDevicesQuery {
                    backend: "llama_cpp".to_string(),
                    gpu_variant: "cuda".to_string(),
                }),
            )
            .await
            .into_body(),
            usize::MAX,
        )
        .await
        .unwrap();
        let refresh_resp: serde_json::Value = serde_json::from_slice(&refresh_body).unwrap();
        assert_eq!(
            refresh_resp,
            serde_json::json!(union),
            "refresh = current per-tamad union"
        );

        guard.finish().await;
    }

    /// Zero-tamad deployments: both gpu endpoints return `[]` (back-compat).
    #[tokio::test]
    async fn test_gpu_devices_zero_tamads_empty_array() {
        let state = Arc::new(ProxyState::new(
            crate::config::Config::default(),
            None,
            crate::db::pool::test_dummy_pool(),
        ));
        let body = axum::body::to_bytes(
            handle_tama_system_gpu_devices(
                axum::extract::State(Arc::clone(&state)),
                axum::extract::Query(GpuDevicesQuery {
                    backend: "llama_cpp".to_string(),
                    gpu_variant: "cpu".to_string(),
                }),
            )
            .await
            .into_body(),
            usize::MAX,
        )
        .await
        .unwrap();
        let devices: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(devices.is_empty(), "zero tamads → legacy empty device list");
    }

    /// `tamad_device_value` maps a tamad `GpuInfo` into the device shape with
    /// its tamad name tag (plan-191 Task 9: no local devices — every entry
    /// is a tamad host device).
    #[test]
    fn test_gpu_device_value_tamad_tagging() {
        let remote = tamad_device_value(
            &GpuInfo {
                index: 0,
                name: "RTX".to_string(),
                driver_version: String::new(),
                vram_total_bytes: 24 * 1024 * 1024 * 1024,
                vram_used_bytes: 4 * 1024 * 1024 * 1024,
                utilization_percent: 55.0,
                temperature_c: 60.0,
                power_w: 100.0,
            },
            "host-a",
        );
        assert_eq!(remote["tamad"], "host-a");
        assert_eq!(remote["device_id"], "GPU0");
        assert_eq!(remote["vram_total_mib"], 24 * 1024);
        assert_eq!(remote["vram_free_mib"], 20 * 1024);
    }
}
