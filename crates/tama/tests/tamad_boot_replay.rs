//! plan-193 T2 e2e — **tamad is the source of truth for the
//! lifecycle**: the boot replay runs off the persistent
//! process-table store.
//!
//! Spawns the real tamad binary (same pattern as
//! `tamad_installs_e2e.rs`: fresh data dir, token-file handoff,
//! gRPC against the control plane):
//!
//! 1. **Default boot, seeded store**: a `desired: true`,
//!    `user_flagged: false` manifest that is pre-seeded BEFORE the
//!    process starts is re-fired by the boot sweep — the model
//!    shows up alive in the `stream_stats` process snapshot.
//!
//! 2. **`--no-replay-desired` boot, seeded store**: nothing
//!    replays (the process snapshot stays empty for the whole probe
//!    window), the manifest survives as written (tamad read its own
//!    store and simply declined the replay), and the daemon logs
//!    the sweep is off.
//!
//! Seeding happens BEFORE the spawn, so the sweep never races the
//! fixture.

use std::path::PathBuf;
use std::time::Duration;

use tama_core::providers::{Protocol, TamadConnection, TamadStatus};
use tama_core::tamad::client::TamadClient;
use tama_core::tamad::{LoadModelRequest, UnloadModelRequest};

const MODEL_KEY: &str = "boot-replay-e2e";

fn tamad_binary() -> PathBuf {
    // Resolve the workspace target dir relative to the crate manifest
    // so the lookup works regardless of the test process cwd.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let target = match std::env::var("CARGO_TARGET_DIR") {
        Ok(t) if !t.is_empty() => PathBuf::from(t),
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

/// Pre-seed the process-table store with ONE desired, un-flagged
/// model row: a live model command (`sh -c "sleep 300"` — lives
/// long enough across the whole of the test) as `MODEL_KEY`. The
/// seed is the exact schema the daemon writes (`StoredProcess`
/// JSON).
fn seed_desired_model(data_dir: &std::path::Path) -> PathBuf {
    let state_dir = data_dir.join("state");
    std::fs::create_dir_all(&state_dir).expect("seed state dir");
    let path = state_dir.join(format!("{MODEL_KEY}.json"));
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let json = format!(
        r#"{{
  "provider_name": "llama_cpp",
  "model_path": "",
  "gpu_variant": "",
  "params": {{}},
  "model_name": "{MODEL_KEY}",
  "command": "/bin/sh",
  "args": ["-c", "sleep 300"],
  "env": {{}},
  "health_url": "",
  "health_timeout_ms": 0,
  "gpu_device": "",
  "docker_config_json": "",
  "desired": true,
  "user_flagged": false,
  "max_restarts": 3,
  "updated_at_ms": {now_ms}
}}"#
    );
    std::fs::write(&path, json).expect("seed desired model manifest");
    path
}

/// Spawn a tamad for a pre-seeded data-dir, either with or
/// without `--no-replay-desired`, and wait for the token
/// publication (the daemon serves once it has one).
async fn start_tamad(
    data_dir: &std::path::Path,
    no_replay: bool,
) -> (tokio::process::Child, u16, String, PathBuf) {
    let port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("tamad port bind");
        l.local_addr().unwrap().port()
    };

    let mut argv: Vec<String> = vec![
        "--name".to_string(),
        "e2e-boot-replay".to_string(),
        "--protocol".to_string(),
        "grpc".to_string(),
        "--addr".to_string(),
        format!("127.0.0.1:{port}"),
        "--data-dir".to_string(),
        data_dir.to_str().unwrap().to_string(),
        "--models-dir".to_string(),
        data_dir.join("models").to_str().unwrap().to_string(),
    ];
    if no_replay {
        argv.push("--no-replay-desired".to_string());
    }

    let log_path = data_dir.join("tamad.log");
    let stdout = std::fs::File::create(&log_path).expect("log file");
    let stderr = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&log_path)
        .expect("log file stderr");
    let child = tokio::process::Command::new(tamad_binary())
        .args(&argv)
        .env("RUST_LOG", "info")
        .env_remove("TAMA_URL")
        .env_remove("TAMA_TOKEN")
        .stdout(std::process::Stdio::from(stdout))
        .stderr(std::process::Stdio::from(stderr))
        .spawn()
        .expect("spawn tamad");
    // Wait for the token file (tamad serves once it has one).
    let token_path = data_dir.join("tamad.token");
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

    (child, port, token, log_path)
}

fn connect(port: u16, token: &str) -> TamadClient {
    let connection = TamadConnection {
        id: "e2e-boot-replay".to_string(),
        name: "e2e-box".to_string(),
        url: format!("grpc://127.0.0.1:{port}"),
        protocol: Protocol::Grpc,
        token: Some(token.to_string()),
        status: TamadStatus::Online,
    };
    TamadClient::new(&connection)
}

/// Poll the gRPC stats stream until the model shows up in the
/// process snapshot (it re-fired AND is alive) — or the deadline
/// blows (panic = regression).
async fn wait_replayed(client: &TamadClient, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let mut stream = client.stream_stats().await.expect("stream stats connect");
        if let Ok(Some(stats)) = stream.message().await {
            for p in &stats.processes {
                if p.model_name == MODEL_KEY {
                    assert!(
                        p.alive,
                        "replay entry present but not alive: status={} pid={}",
                        p.status, p.pid
                    );
                    return;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "boot sweep didn't replay the desired model in time"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The opposite: the model key never shows up in the process
/// snapshot for the whole probe window.
async fn assert_not_replayed(client: &TamadClient, window: Duration) {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let mut stream = client.stream_stats().await.expect("stream stats connect");
        if let Ok(Some(stats)) = stream.message().await {
            for p in &stats.processes {
                assert_ne!(
                    p.model_name, MODEL_KEY,
                    "--no-replay-desired must not replay any stored fixture"
                );
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tamad_boot_replay_is_the_source_of_truth() {
    // ══ 1. Default boot: sweep re-fires the seed line. ═════════
    {
        let dir = tempfile::tempdir().expect("s1 tempdir");
        let data_dir = dir.path().to_path_buf();
        seed_desired_model(&data_dir);
        let (mut child, port, token, _log) = start_tamad(&data_dir, false).await;
        let mut client = connect(port, &token);

        wait_replayed(&client, Duration::from_secs(15)).await;

        client
            .unload_model(&UnloadModelRequest {
                provider_name: "llama_cpp".to_string(),
                model_name: MODEL_KEY.to_string(),
            })
            .await
            .expect("cleanup: unload the replayed model");

        child.kill().await.expect("kill s1 tamad");
    }

    // ══ 2. `--no-replay-desired`: nothing replays; seed survives.
    {
        let dir = tempfile::tempdir().expect("s2 tempdir");
        let data_dir = dir.path().to_path_buf();
        let seeded = seed_desired_model(&data_dir);
        let (mut child, port, token, log_path) = start_tamad(&data_dir, true).await;
        let client = connect(port, &token);

        assert_not_replayed(&client, Duration::from_secs(6)).await;

        // The store stayed as written: the daemon read its own store
        // and declined the replay (no re-stamp, no `user_flagged`
        // tampering — that bit is budget-only territory).
        let after = std::fs::read_to_string(&seeded).expect("seed file survived");
        assert!(after.contains("\"desired\": true"), "desired persists");
        assert!(
            after.contains("\"user_flagged\": false"),
            "flag stays false (never a budget trip here)"
        );

        // The no-op announcement: exactly ONE log line, and NO replay
        // line whatsoever.
        let log = std::fs::read_to_string(&log_path).expect("s2 log");
        assert_eq!(
            log.matches("replay-desired is disabled").count(),
            1,
            "exactly one `replay-desired is disabled` log line"
        );
        assert_eq!(
            log.matches("boot sweep: replayed desired model").count(),
            0,
            "sweep must NOT have replayed anything"
        );

        child.kill().await.expect("kill s2 tamad");
    }
}

/// The wired model key this file's plan-193 T3 test loads explicitly.
const LOADED_KEY: &str = "t3-wire-e2e";

/// Poll the gRPC stats stream until the wire-loaded model shows up, then
/// assert the three T3 wire-field extensions on that frame: `desired`
/// true (the host persisted the load as desired), `restart_count` zero,
/// `max_restarts` at the default budget (10), and the status within the
/// canonical six words.
async fn wait_loaded(client: &TamadClient, key: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let mut stream = client.stream_stats().await.expect("stream stats connect");
        if let Ok(Some(stats)) = stream.message().await {
            for p in &stats.processes {
                if p.model_name == key && p.desired {
                    assert!(
                        p.alive,
                        "T3 loaded model must be alive: status={} pid={}",
                        p.status, p.pid
                    );
                    assert!(p.desired, "T3 loaded model reports desired=true");
                    assert_eq!(
                        p.restart_count, 0,
                        "T3 loaded model reports restart_count=0"
                    );
                    assert_eq!(
                        p.max_restarts, 10,
                        "T3 loaded model reports max_restarts=10 (DEFAULT_MAX_RESTARTS)"
                    );
                    assert!(
                        p.status == "starting"
                            || p.status == "ready"
                            || p.status == "restarting"
                            || p.status == "failed"
                            || p.status == "budget_exhausted"
                            || p.status == "unloading",
                        "status '{}' is one of the canonical six",
                        p.status
                    );
                    return;
                }
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "wire-loaded model never appeared on the stream"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// plan-193 T3 — the wire extension: a model loaded through the proto
/// reports `desired=true` (the host persisted the desired on the load
/// path), `restart_count=0`, `max_restarts=10`, and a status the lifecycle
/// accepts (one of the canonical six) on the StreamStats frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loaded_model_reports_wire_field_extensions() {
    let dir = tempfile::tempdir().expect("T3 tempdir");
    let data_dir = dir.path().to_path_buf();
    // No replay seeding: we drive the load explicitly over the wire.
    let (mut child, port, token, _log) = start_tamad(&data_dir, true).await;
    let mut client = connect(port, &token);

    let req = LoadModelRequest {
        provider_name: "llama_cpp".to_string(),
        model_path: "".to_string(),
        gpu_variant: "".to_string(),
        params: std::collections::HashMap::new(),
        model_name: LOADED_KEY.to_string(),
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "sleep 300".to_string()],
        env: std::collections::HashMap::new(),
        health_url: "".to_string(),
        health_timeout_ms: 0,
        gpu_device: "".to_string(),
        docker_config_json: "".to_string(),
    };
    client
        .load_model(&req)
        .await
        .expect("load model via the wire");

    wait_loaded(&client, LOADED_KEY, Duration::from_secs(15)).await;

    client
        .unload_model(&UnloadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_name: LOADED_KEY.to_string(),
        })
        .await
        .expect("cleanup: unload the wire-loaded model");

    child.kill().await.expect("kill T3 tamad");
}

/// plan-193 T4 — the proxy read side is row-backed: a loaded model shows up
/// as a live row via `tama_core::proxy::live_rows` (endpoint routed from the
/// wire, `desired=true`, `restart_count=0`), and stopping the host empties
/// the row set (no host = no models) once the last frame goes stale.
///
/// This is the e2e counterpart of the in-module `rows.rs` unit tests:
/// row discovery now runs against the tamad's actual 1 Hz stream
/// (with no proxy-side state in between, plan 193 T5c), not a synthetic handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_reads_live_rows_from_wire_then_empty_when_host_missing() {
    let dir = tempfile::tempdir().expect("T4 tempdir");
    let data_dir = dir.path().to_path_buf();
    let (mut child, port, token, _log) = start_tamad(&data_dir, true).await;
    let mut client = connect(port, token.as_str());

    // Drive an explicit load over the wire (T3's flow), then let the proxy
    // read the SAME live frame through a real `TamadPool`.
    let req = LoadModelRequest {
        provider_name: "llama_cpp".to_string(),
        model_path: "".to_string(),
        gpu_variant: "".to_string(),
        params: std::collections::HashMap::new(),
        model_name: LOADED_KEY.to_string(),
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "sleep 300".to_string()],
        env: std::collections::HashMap::new(),
        health_url: "".to_string(),
        health_timeout_ms: 0,
        gpu_device: "".to_string(),
        docker_config_json: "".to_string(),
    };
    client
        .load_model(&req)
        .await
        .expect("load the model over the wire");
    wait_loaded(&client, LOADED_KEY, Duration::from_secs(15)).await;

    // Bind a proxy `TamadPool` to the same running host and read live rows.
    let pool = tama_core::tamad::pool::TamadPool::new(tama_test_support::test_dummy_pool())
        .with_backoff_base(Duration::from_millis(20));
    let conn = TamadConnection {
        id: "e2e-boot-replay".to_string(),
        name: "e2e-box".to_string(),
        url: format!("grpc://127.0.0.1:{port}"),
        protocol: Protocol::Grpc,
        token: Some(token.to_string()),
        status: TamadStatus::Unknown,
    };
    pool.upsert_connection(&conn)
        .await
        .expect("proxy pool connect");

    // The proxy row set converges to >= 1 for the loaded model: rows.ready
    // and the tamad stream's own process snapshot AGREE at the 0/1 level
    // (both *have* the model).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        let rows = tama_core::proxy::live_rows(&pool).await;
        if let Some(r) = rows.row(LOADED_KEY) {
            assert!(r.desired, "proxy row reports desired=true (wire T3 flag)");
            assert_eq!(r.restart_count, 0, "proxy row reports restart_count=0");
            assert!(
                rows.ready_count() >= 1,
                "proxy ready count >= 1 (both sources agree at 0/1)"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let rows = tama_core::proxy::live_rows(&pool).await;
    assert!(
        rows.row(LOADED_KEY).is_some(),
        "proxy must read the loaded model row off the wire"
    );

    // Stop the host: the frame goes stale past LIVE_FRAME_MAX_AGE (5s), and
    // the proxy read drops to ZERO rows (no host = no models) — same index
    // pattern as `rows.rs`'s stale-frame unit test.
    child.kill().await.expect("kill T4 tamad");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let rows = tama_core::proxy::live_rows(&pool).await;
        if rows.all().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        tama_core::proxy::live_rows(&pool).await.all().is_empty(),
        "offline host → 0 proxy rows (no host = no models)"
    );
}
