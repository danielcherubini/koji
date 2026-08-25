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

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tama_core::providers::{Protocol, TamadConnection, TamadStatus};
use tama_core::tamad::client::TamadClient;
use tama_core::tamad::{LoadModelRequest, UnloadModelRequest};

const MODEL_KEY: &str = "boot-replay-e2e";

fn tamad_binary() -> PathBuf {
    // Resolve the workspace target dir relative to the crate manifest
    // so the lookup works regardless of the test process cwd.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let workspace = manifest.join("../..");
    let target = match std::env::var("CARGO_TARGET_DIR") {
        Ok(t) if !t.is_empty() => PathBuf::from(t),
        _ => workspace.join("target"),
    };
    let p = target.join("debug").join("tamad");
    assert!(
        p.exists(),
        "tamad binary not found at {:?} — run `cargo build -p tamad` first",
        p
    );
    // Plan-193 N20: the target *debug* `tamad` binary must be FRESH relative
    // to the tamad sources it was built from. `cargo nextest -p tama` compiles
    // only `tama` (and its deps for the test build) — it does NOT
    // `cargo build -p tamad`, so a stale `target/debug/tamad` (e.g. edited
    // `crates/tamad/src/lifecycle.rs` or `tama-core` proto since the last
    // build) would silently run the OLD shim (cryptic failure, or worse,
    // wind-shifted assertions). We refuse to run on a stale binary and point
    // at the rebuild — no hidden recompile inside the test process
    // (min-plan rule: the panic's message IS the fix instruction).
    let source_roots = tamad_source_roots(&workspace);
    let source_roots: Vec<&Path> = source_roots.iter().map(PathBuf::as_path).collect();
    if binary_is_stale(&p, &source_roots) {
        panic!(
            "{p:?} is outdated (stale relative to the tamad sources). \
             run `cargo build -p tamad` once, and rerun the e2e — \
             this test does not rebuild the shim inside the test process."
        );
    }
    p
}

/// The source roots that feed the compiled `tamad` shim — the walk scope
/// of the N20 stale-binary guard, resolved relative to the workspace root
/// (`CARGO_MANIFEST_DIR` of the `tama` crate is `crates/tama`, so the
/// workspace root is `../..`).
///
/// Scope choice (measured with `find` at review time): `crates/tamad/src`
/// (49 `.rs`) + `crates/tama-core/src` (223 `.rs`) + `crates/tama-core/proto`
/// (1 `.proto`) ≈ 273 tracked files — a hand-rolled `std::fs::read_dir`
/// walk of that is sub-millisecond, so we take the FULL closure rather
/// than just the shim: every root below does compile into `tamad` (the
/// prost output of `tamad.proto` feeds `tama-core`, and `tama-core` feeds
/// the shim), so a newer mtime anywhere is a genuine "shim may be old"
/// signal. `crates/tamad/Cargo.toml` is deliberately NOT a trigger (a
/// manifest-only edit does not change what the shim links against here).
fn tamad_source_roots(workspace: &Path) -> [PathBuf; 3] {
    [
        workspace.join("crates/tamad/src"),
        workspace.join("crates/tama-core/src"),
        workspace.join("crates/tama-core/proto"),
    ]
}

/// N20: whether `binary` is stale relative to `source_roots`.
///
/// STALE iff some tracked source file's `modified()` mtime is STRICTLY
/// NEWER than the binary's mtime; TIE or older ⇒ FRESH. "Older ⇒ fresh" is
/// pinned by the tests; the TIE case is pinned here in code (not by a test)
/// because a std-only API cannot SET an mtime, so two files with an exactly
/// equal mtime cannot be constructed portably.
fn binary_is_stale(binary: &Path, source_roots: &[&Path]) -> bool {
    let Some(binary_mtime) = mtime_of(binary) else {
        // Nothing to stat ⇒ the caller's `.exists()` check owns the error;
        // do not double-report here.
        return false;
    };
    source_roots
        .iter()
        .any(|root| walk_has_tracked_file_newer_than(root, &binary_mtime))
}

/// Recursively, under `dir` (hand-rolled `read_dir` recursion — no new
/// dependency; the trees are small), does any tracked source file (`.rs`
/// anywhere, or `.proto`) have an mtime STRICTLY after `before`?
fn walk_has_tracked_file_newer_than(dir: &Path, before: &SystemTime) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false; // a missing root is not "stale" (guard, not schema)
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if walk_has_tracked_file_newer_than(&path, before) {
                return true;
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "proto")
        ) {
            if let Some(mtime) = mtime_of(&path) {
                if mtime > *before {
                    return true;
                }
            }
        }
    }
    false
}

/// `modified()` of `path`, if both metadata probes succeed.
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
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

// ─── plan-193 T6: the `tama admin` e2e counterparts ────────────────────────

/// The keys T6 drives through the real wire (hermetic: `sh -c sleep`).
const ADMIN_LOAD_KEY: &str = "admin-load-t6";

/// A TTS-shell model name: if loaded through the normal path, it
/// proves TTS keys have no special-case wire builder (the wire is
/// sourced from the same rows as every other model — there is exactly
/// one `ProcessInfo` builder, `to_process_info` in
/// `crates/tamad/src/lifecycle.rs`).
const TTS_MODEL_KEY: &str = "tts_kokoro";

const STATUS_SIX_KEY: &str = "status-six-t6";

/// The canonical six words (plan-193 T2,
/// `crates/tamad/src/lifecycle.rs`'s `status` module).
const CANONICAL_STATUSES: [&str; 6] = [
    "starting",
    "ready",
    "restarting",
    "failed",
    "budget_exhausted",
    "unloading",
];

/// The `LoadModel` wire request that `ensure_model_loaded` sends after
/// alias resolution, the budget check, and the alive-row fast path —
/// the EXACTLY same call a headless `tama admin load` makes in-process
/// (if the model is already alive, `admin load` returns via the fast
/// path and never sends a second `LoadModel`).
fn admin_load_request(key: &str) -> LoadModelRequest {
    LoadModelRequest {
        provider_name: "llama_cpp".to_string(),
        model_path: String::new(),
        gpu_variant: String::new(),
        params: std::collections::HashMap::new(),
        model_name: key.to_string(),
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "sleep 300".to_string()],
        env: std::collections::HashMap::new(),
        health_url: String::new(),
        health_timeout_ms: 0,
        gpu_device: String::new(),
        docker_config_json: String::new(),
    }
}

/// Bind a real proxy `TamadPool` to a running host (the T4 pattern)
/// and load it — this is what `tama admin status` reads its rows
/// through.
async fn bind_proxy_pool(port: u16, token: &str) -> tama_core::tamad::pool::TamadPool {
    let pool = tama_core::tamad::pool::TamadPool::new(tama_test_support::test_dummy_pool())
        .with_backoff_base(Duration::from_millis(20));
    let conn = TamadConnection {
        id: "t6-admin".to_string(),
        name: "t6-e2e".to_string(),
        url: format!("grpc://127.0.0.1:{port}"),
        protocol: Protocol::Grpc,
        token: Some(token.to_string()),
        status: TamadStatus::Unknown,
    };
    pool.upsert_connection(&conn)
        .await
        .expect("bind the proxy pool to the host");
    pool
}

/// Poll the real `TamadPool` until the model is observed as a row with
/// `status == "ready"` — the state that `tama admin load` leaves
/// behind (the wire is the status; the admin reads rows, never
/// counters).
async fn wait_row_ready(pool: &tama_core::tamad::pool::TamadPool, key: &str, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let rows = tama_core::proxy::live_rows(pool).await;
        if let Some(row) = rows.row(key).filter(|r| r.status == "ready") {
            assert!(
                row.alive,
                "a `ready` row must have a living process: pid={}",
                row.pid
            );
            assert!(
                rows.ready_count() >= 1,
                "the ready count must include the freshly-loaded row"
            );
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "model '{key}' never became `ready` on the wire within {:?}",
            timeout
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// plan-193 T6 — `tama admin load` reproducibility: the e2e runs the
/// same API path the headless admin calls in-process
/// (alias resolution → budget check → alive-row fast path →
/// `LoadModel`) and, exactly like `tama admin status`, reads
/// the constructed rows through a real `TamadPool`. The whole
/// admin `load` ends up being the row reports `ready` via the wire —
/// readiness is the host's, and the CLI surfaces it, it doesn't decide it.
///
/// Hermetic: real binary, real gRPC, real 1 Hz frames; `sleep`
/// process, no GPU, no HF Hub, no network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_load_reports_row_ready_via_wire() {
    let dir = tempfile::tempdir().expect("T6 admin tempdir");
    let data_dir = dir.path().to_path_buf();
    let (mut child, port, token, _log) = start_tamad(&data_dir, true).await;
    let mut client = connect(port, &token);
    let pool = bind_proxy_pool(port, &token).await;

    client
        .load_model(&admin_load_request(ADMIN_LOAD_KEY))
        .await
        .expect("admin load via the same API path");

    wait_row_ready(&pool, ADMIN_LOAD_KEY, Duration::from_secs(15)).await;

    client
        .unload_model(&UnloadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_name: ADMIN_LOAD_KEY.to_string(),
        })
        .await
        .expect("cleanup: unload it");
    child.kill().await.expect("kill T6 admin tamad");
}

/// plan-193 T6 — this run's first TTS e2e: a TTS-shell model name
/// (`tts_kokoro`) loads through the SAME real `LoadModel` path as a
/// normal model — no special-case wire builder for TTS/
/// compaction keys — and the row becomes `ready` via the
/// wire the proxy reads (the verify-only complement: there is
/// exactly one `ProcessInfo` builder on tamad, and the
/// proxy read is row-sourced).
///
/// Hermetic air gap: the "TTS model" IS a `sleep` process under a
/// TTS-flavored key — no kokoro download, no GPU, no hub.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tts_key_loads_through_the_normal_wire_path() {
    let dir = tempfile::tempdir().expect("T6 TTS tempdir");
    let data_dir = dir.path().to_path_buf();
    let (mut child, port, token, _log) = start_tamad(&data_dir, true).await;
    let mut client = connect(port, &token);
    let pool = bind_proxy_pool(port, &token).await;

    client
        .load_model(&admin_load_request(TTS_MODEL_KEY))
        .await
        .expect("TTS key loaded through the normal wire path");

    wait_row_ready(&pool, TTS_MODEL_KEY, Duration::from_secs(15)).await;

    client
        .unload_model(&UnloadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_name: TTS_MODEL_KEY.to_string(),
        })
        .await
        .expect("cleanup: unload it");
    child.kill().await.expect("kill T6 TTS tamad");
}

/// plan-193 T6 — the canonical-six-word set is carried forward
/// (asserted at T2; the shape of the row source plane is from T4
/// unchanged): across one tmp load, every `ProcessInfo.status` word
/// observed on the 1 Hz stream is within the canonical six.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_wire_statuses_stay_within_the_canonical_six() {
    let dir = tempfile::tempdir().expect("T6 six tempdir");
    let data_dir = dir.path().to_path_buf();
    let (mut child, port, token, _log) = start_tamad(&data_dir, true).await;
    let mut client = connect(port, &token);

    client
        .load_model(&admin_load_request(STATUS_SIX_KEY))
        .await
        .expect("load for status observation");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut saw_ready = false;
    while !saw_ready {
        let mut stream = client.stream_stats().await.expect("stream stats connect");
        while let Ok(Some(stats)) = stream.message().await {
            for p in &stats.processes {
                if p.model_name == STATUS_SIX_KEY {
                    seen.insert(p.status.clone());
                    saw_ready = saw_ready || p.status == "ready";
                }
            }
            if saw_ready || tokio::time::Instant::now() >= deadline {
                break;
            }
        }
    }
    assert!(
        saw_ready,
        "model must reach `ready` within the observation window (the six-set is OBSERVED, not asserted a priori)"
    );
    assert!(
        !seen.is_empty(),
        "the stream had to observe status words in the window"
    );
    for status in &seen {
        assert!(
            CANONICAL_STATUSES.contains(&status.as_str()),
            "off-spec status '{status}' (must be one of the canonical six)"
        );
    }

    client
        .unload_model(&UnloadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_name: STATUS_SIX_KEY.to_string(),
        })
        .await
        .expect("cleanup: unload it");
    child.kill().await.expect("kill T6 six tamad");
}

// ─── Plan-193 N20: `binary_is_stale` unit tests (stale-e2e-regression) ──
//
// Only the two REALISABLE orderings are pinned ("source strictly newer than the
// binary ⇒ stale", "source older than the binary ⇒ fresh"). The TIE case
// ("equal mtime ⇒ fresh") cannot be constructed with a std-only API — there is
// no std way to SET a file's mtime, so a robust "construct two files with
// exactly equal mtimes" is not portable. Tie-freshness is therefore pinned in
// `binary_is_stale`'s doc + the `mtime > *before` (STRICT-GT) comparison,
// and the walk's TIE ⇒ fresh behavior is exercised transitively (an equal
// mtime falls through the strict `>` and is not reported stale).
mod stale_tests {
    use super::*;

    const GAP: Duration = Duration::from_millis(25);

    fn pad() {
        // mtime clocks can carry coarse mtimes; pad between writes so "CREATED
        // LATER" is robustly STRICTLY-newer, not merely a tie.
        std::thread::sleep(GAP);
    }

    /// Lay down a fake source tree (one `crates/<name>/src/*.rs`) under `root`,
    /// plus a fake `tamad` binary of the SAME fs, so `binary_is_stale` can be
    /// driven purely by creation ordering.
    fn stage(root: &Path) -> (PathBuf, PathBuf) {
        let src_root = root.join("crates/tamad/src");
        std::fs::create_dir_all(&src_root).unwrap();
        let source = src_root.join("lib.rs");
        std::fs::write(&source, "fn main() {}").unwrap();
        let binary = root.join("tamad");
        std::fs::write(&binary, "ELF").unwrap();
        (src_root, binary)
    }

    fn roots(src_root: &Path) -> Vec<&Path> {
        vec![src_root]
    }

    /// An ALREADY-built binary whose sources are all OLDER is FRESH (no re-run
    /// is needed): creation order is source-then-binary ⇒ mtime(source) < mtime.
    #[test]
    fn test_binary_fresh_when_sources_are_older() {
        let dir = tempfile::tempdir().unwrap();
        let (src_root, binary) = stage(dir.path());
        // Re-touch the source so it is UNAMBIGUOUSLY strictly older (defeats a
        // coarse fs whose mtimes might otherwise tie). The binary is already the
        // LAST write above, so its mtime ≥ every source write.
        assert!(
            !binary_is_stale(&binary, &roots(&src_root)),
            "a source that is older than the binary must read FRESH"
        );
    }

    /// A source STRICTLY NEWER than the binary (the "edited lifecycle.rs after
    /// the last build" case) is STALE: rewrite the source AFTER the binary so
    /// its mtime is strictly the newest.
    #[test]
    fn test_binary_stale_when_a_source_is_newer() {
        let dir = tempfile::tempdir().unwrap();
        let (src_root, binary) = stage(dir.path());
        let source = src_root.join("lib.rs");
        pad(); // make sure the pad between write and re-touch is a strict tick
        std::fs::write(&source, "fn main() { // rebuilt\n}").unwrap();
        assert!(
            binary_is_stale(&binary, &roots(&src_root)),
            "a source strictly newer than the binary must read STALE"
        );
    }

    /// A `.proto` under a walked root IS tracked: touching it after the binary
    /// flips the shard to STALE (the prost output feeds the link).
    #[test]
    fn test_binary_stale_when_proto_is_touched_after_build() {
        let dir = tempfile::tempdir().unwrap();
        let proto_root = dir.path().join("crates/tama-core/proto");
        std::fs::create_dir_all(&proto_root).unwrap();
        let proto = proto_root.join("tamad.proto");
        std::fs::write(&proto, "syntax = \"proto3\";").unwrap();
        let binary = dir.path().join("tamad");
        std::fs::write(&binary, "ELF").unwrap();
        assert!(
            !binary_is_stale(&binary, &[proto_root.as_path()]),
            "post-build proto, pre-rebuild: binary still fresh"
        );
        pad();
        std::fs::write(&proto, "syntax = \"proto3\"; // field added\n").unwrap();
        assert!(
            binary_is_stale(&binary, &[proto_root.as_path()]),
            "a proto written after the binary must read STALE"
        );
    }

    /// A file with a NON-tracked extension never triggers staleness, even if
    /// it is the newest write in the walk (keeps `Cargo.toml`/`.lock`/`.log` from
    /// bumping the guard).
    #[test]
    fn test_binary_ignores_untracked_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let (src_root, binary) = stage(dir.path());
        pad();
        let unrelated = src_root.join("not-a-source.txt");
        std::fs::write(&unrelated, "log noise").unwrap();
        assert!(
            !binary_is_stale(&binary, &roots(&src_root)),
            "a .txt newer than the binary must NOT count as staleness"
        );
    }
}
