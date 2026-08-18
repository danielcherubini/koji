//! E2E: system/GPU endpoints aggregated per tamad against a *real* tamad
//! binary (plan-191 Task 9).
//!
//! The chain under test:
//! 1. The proxy system routes (`health`, `gpu-devices`, `gpu-devices/refresh`)
//!    served in-process with a real [ProxyState]
//! 2. Zero-tamad back-compat: `hosts: []`, null GPU fields, `[]` devices
//! 3. A real `tamad` child process (real gRPC + bearer token + stats stream)
//!    is registered in the pool; the endpoints must then report per-tamad
//!    facts — `version` from the tamad's HealthCheck, per-tamad `gpus[]`,
//!    and every `gpu-devices` entry tagged with its tamad name (this dev
//!    box has an AMD GPU, so the tag is exercised for real when one is
//!    present; the shape assertions hold regardless)

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post};
use axum::Router;
use tower::ServiceExt;

use tama_core::providers::{Protocol, TamadConnection, TamadStatus};
use tama_core::proxy::tama_handlers::{
    handle_tama_system_gpu_devices, handle_tama_system_gpu_devices_refresh,
    handle_tama_system_health,
};
use tama_core::proxy::ProxyState;

use common::with_schema;

fn tamad_binary() -> std::path::PathBuf {
    let manifest =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let target = match std::env::var("CARGO_TARGET_DIR") {
        Ok(t) if !t.is_empty() => std::path::PathBuf::from(t),
        _ => manifest.join("../..").join("target"),
    };
    let p = target.join("debug").join("tamad");
    assert!(
        p.exists(),
        "tamad binary not found at {:?} — run `cargo build -p tamad` first",
        p
    );
    p
}

/// Spawn the real tamad binary against a temp data dir; its bearer token
/// is generated at `<data-dir>/tamad.token`.
async fn start_tamad() -> (
    tokio::process::Child,
    u16,
    std::sync::Arc<tempfile::TempDir>,
    String,
) {
    let port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("tamad port bind");
        l.local_addr().unwrap().port()
    };
    let dir = tempfile::tempdir().expect("tamad tempdir");
    let log_path = dir.path().join("tamad.log");
    let child = tokio::process::Command::new(tamad_binary())
        .args([
            "--name",
            "e2e-box",
            "--protocol",
            "grpc",
            "--addr",
            &format!("127.0.0.1:{port}"),
            "--data-dir",
            dir.path().to_str().unwrap(),
            "--models-dir",
            dir.path().join("models").to_str().unwrap(),
        ])
        .env_remove("TAMA_URL")
        .env_remove("TAMA_TOKEN")
        .stdout(std::process::Stdio::from(
            std::fs::File::create(&log_path).unwrap(),
        ))
        .stderr(std::process::Stdio::from(
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&log_path)
                .unwrap(),
        ))
        .spawn()
        .expect("spawn tamad");

    // Wait for the token file (tamad is serving once it has one).
    let token_path = dir.path().join("tamad.token");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if token_path.exists() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "tamad never created its token file"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let token = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(t) = std::fs::read_to_string(&token_path) {
                if !t.trim().is_empty() {
                    return t.trim().to_string();
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tamad token write timed out");

    (child, port, Arc::new(dir), token)
}

/// Router with the proxy system routes mounted (the same handlers the real
/// proxy serves under /tama/v1/system/*).
fn system_router(state: Arc<ProxyState>) -> Router {
    Router::new()
        .route("/tama/v1/system/health", get(handle_tama_system_health))
        .route(
            "/tama/v1/system/gpu-devices",
            get(handle_tama_system_gpu_devices),
        )
        .route(
            "/tama/v1/system/gpu-devices/refresh",
            post(handle_tama_system_gpu_devices_refresh),
        )
        .with_state(state)
}

async fn oneshot_json(router: &Router, method: &str, uri: &str) -> serde_json::Value {
    let resp = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_system_endpoints_real_tamad() {
    let guard = with_schema().await;
    let db_pool = Arc::new(guard.pool.clone());

    let state = Arc::new(ProxyState::new(
        tama_core::config::Config::default(),
        None,
        db_pool.clone(),
    ));
    let router = system_router(Arc::clone(&state));

    // ── 1. Zero-tamad back-compat (before any host is registered) ────────
    let health = oneshot_json(&router, "GET", "/tama/v1/system/health").await;
    assert_eq!(health["status"], "ok");
    assert_eq!(health["hosts"], serde_json::json!([]), "no tamads yet");
    assert!(
        health["gpu_utilization_pct"].is_null(),
        "legacy top-level GPU field must be null"
    );
    assert!(health["vram"].is_null(), "legacy vram field must be null");
    assert!(!health["version"].as_str().unwrap().is_empty());
    assert!(health["uptime_seconds"].as_f64().unwrap() >= 0.0);

    let devices = oneshot_json(
        &router,
        "GET",
        "/tama/v1/system/gpu-devices?backend=llama_cpp&gpu_variant=cpu",
    )
    .await;
    assert_eq!(devices, serde_json::json!([]), "zero tamads -> no devices");

    // ── 2. Start a real tamad and register it in the pool ────────────────
    let (mut child, port, dir, token) = start_tamad().await;
    let tamad_id = "e2e-system-0001";
    state
        .tamad_pool()
        .upsert_connection(&TamadConnection {
            id: tamad_id.to_string(),
            name: "e2e-box".to_string(),
            url: format!("grpc://127.0.0.1:{port}"),
            protocol: Protocol::Grpc,
            token: Some(token),
            status: TamadStatus::Unknown,
        })
        .await
        .unwrap();

    // Wait for the streaming connection to establish and cache its data.
    let handle = state.tamad_pool().get(tamad_id).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let has_snapshot = handle.latest().await.is_some();
        let has_version = handle.version().await.is_some();
        let online = handle.is_online().await;
        if has_snapshot && has_version && online {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "tamad never went online (snapshot={has_snapshot}, version={has_version}, online={online})"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // ── 3. Health now reports the tamad host ─────────────────────────────
    let health = oneshot_json(&router, "GET", "/tama/v1/system/health").await;
    let hosts = health["hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 1, "one host per registered tamad");
    let host = &hosts[0];
    assert_eq!(host["tamad_id"], tamad_id);
    assert_eq!(host["name"], "e2e-box");
    assert_eq!(host["online"], true);
    assert!(
        host["version"].is_string(),
        "version must come from the tamad HealthCheck, got {:?}",
        host["version"]
    );
    assert!(host["cpu_percent"].as_f64().unwrap() >= 0.0);
    assert!(
        (0.0..=100.0).contains(&host["memory_used_pct"].as_f64().unwrap()),
        "memory_used_pct in [0,100], got {}",
        host["memory_used_pct"]
    );
    assert!(host["gpus_online"].as_i64().unwrap() >= 0);

    // ── 4. gpu-devices: every entry is a tamad host device, tagged ───────
    let devices = oneshot_json(
        &router,
        "GET",
        "/tama/v1/system/gpu-devices?backend=llama_cpp&gpu_variant=cpu",
    )
    .await;
    let list = devices.as_array().unwrap();
    for entry in list {
        // Every device (if any exist on this machine) is a tamad host device.
        assert_eq!(
            entry["tamad"], "e2e-box",
            "device must carry its tamad name: {entry}"
        );
        assert!(
            entry["device_id"]
                .as_str()
                .unwrap()
                .strip_prefix("GPU")
                .unwrap()
                .chars()
                .all(|c| c.is_ascii_digit()),
            "device_id must be GPU<i>, got {:?}",
            entry["device_id"]
        );
        assert!(entry["vram_total_mib"].as_u64().unwrap() > 0);
        assert!(entry["temperature_c"].as_f64().is_some());
    }

    // The host has an AMD GPU (rocm-smi is available on this dev box) — the
    // gpus_online count and device list must agree. Keep the cross-check
    // truthful even on GPU-less CI machines (both sides are simply 0/[]).
    let gpus_online = hosts[0]["gpus_online"].as_i64().unwrap();
    assert_eq!(
        list.len() as i64,
        gpus_online,
        "device list length must match gpus_online"
    );

    // ── 5. refresh returns the same per-tamad union (no local re-scan) ───
    let refreshed = oneshot_json(
        &router,
        "POST",
        "/tama/v1/system/gpu-devices/refresh?backend=llama_cpp&gpu_variant=cpu",
    )
    .await;
    let refreshed_list = refreshed.as_array().unwrap();
    assert_eq!(refreshed_list.len(), list.len(), "refresh = current union");
    if !refreshed_list.is_empty() {
        for d in refreshed_list {
            assert_eq!(
                d["tamad"], "e2e-box",
                "refreshed entry must carry its tamad name"
            );
        }
    }

    child.kill().await.ok();
    let _ = dir.path(); // keep TempDir alive until after kill
    guard.finish().await;
}
