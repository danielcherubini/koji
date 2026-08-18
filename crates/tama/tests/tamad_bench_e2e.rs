//! E2E: llama-bench benchmark executed by a *real* tamad process
//! (plan-191 Task 8).
//!
//! The chain under test:
//! 1. Proxy benchmark run resolves the model's provider → its tamad, and
//!    computes the host-relative model + binary paths from the DB layouts
//!    (the proxy never touches the tamad's roots directly)
//! 2. `RunBenchmark` RPC to the spawned tamad binary (real gRPC + token
//!    auth)
//! 3. Tamad spawns the *real* llama-bench subprocess from its install dir
//!    ("marker" shell scripts stand in for the llama.cpp binaries; the
//!    llama-bench marker emits the real `-o json` shape the runner parses,
//!    and stalls long enough to be caught by `ps`)
//! 4. Job events stream back into the proxy `JobManager` (bridged
//!    `StreamJob`)
//! 5. The proxy (sole DB writer) persists the benchmark history row from
//!    the tamad's result JSON
//! 6. Verification: the marker left a "ran" file inside the tamad's own
//!    install dir (proving the subprocess ran under the tamad's roots, not
//!    the proxy's), `ps` catches the marker process with the tamad-side
//!    binary path, and the history row carries the parsed numbers.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tama_core::installations::InstallationManager;
use tama_core::providers::{Protocol, TamadConnection, TamadStatus};
use tama_core::proxy::ProxyState;

const TAMAD_ID: &str = "6f0b9c2e-e2e4-4c1a-9f01-0000e2e0010";
const BENCH_TAG: &str = "b9901";
const MARKER_ID: &str = "tama-bench-e2e-marker";

/// Marker llama-bench: real JSON shape (the runner parses stdout), plus a
/// stall so the process is observable via `ps`, plus a "ran" marker file
/// inside its own install dir.
fn marker_bench_script() -> String {
    [
        "#!/bin/sh",
        &format!("# {MARKER_ID}"),
        "case \"${1:-}\" in",
        &format!("--version) echo \"llama-bench ({MARKER_ID})\"; exit 0;;"),
        "esac",
        "echo \"$$ from $0\" >> \"$(dirname \"$0\")/e2e-bench-ran.txt\"",
        "sleep 2",
        "echo '[{\"n_prompt\":512,\"n_gen\":0,\"avg_ts\":1234.5,\"stddev_ts\":3.0},{\"n_prompt\":0,\"n_gen\":64,\"avg_ts\":66.6,\"stddev_ts\":0.5}]'",
    ]
    .join("\n")
}

fn marker_server_script() -> String {
    format!("#!/bin/sh\n# {MARKER_ID} (llama-server stand-in, not executed by benchmarks)\n")
}

/// Resolve the workspace target dir (independent of the test cwd).
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
async fn start_tamad() -> (tokio::process::Child, u16, Arc<tempfile::TempDir>, String) {
    let port = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("tamad port bind")
        .local_addr()
        .unwrap()
        .port();
    let dir = tempfile::tempdir().expect("tamad tempdir");
    let log_path = dir.path().join("tamad.log");
    let child = tokio::process::Command::new(tamad_binary())
        .args([
            "--name",
            "e2e-bench-box",
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

    let token_path = dir.path().join("tamad.token");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "tamad never created its token file"
        );
        if let Ok(t) = std::fs::read_to_string(&token_path) {
            if !t.trim().is_empty() {
                return (child, port, Arc::new(dir), t.trim().to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Place the marker binaries + the model file inside the tamad's roots.
/// Returns the llama-server (binary) path the proxy DB row will reference.
fn seed_tamad_host(tamad_dir: &std::path::Path) -> std::path::PathBuf {
    let install_version = tamad_dir
        .join("install")
        .join("llama_cpp")
        .join("cpu")
        .join(BENCH_TAG);
    std::fs::create_dir_all(&install_version).expect("install version dir");

    let bench = install_version.join("llama-bench");
    std::fs::write(&bench, marker_bench_script()).expect("marker llama-bench");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&bench, std::fs::Permissions::from_mode(0o755));

    let server = install_version.join("llama-server");
    std::fs::write(&server, marker_server_script()).expect("marker llama-server");
    let _ = std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755));

    let model_dir = tamad_dir
        .join("models")
        .join("test")
        .join("bench-e2e-model");
    std::fs::create_dir_all(&model_dir).expect("model dir");
    let model = model_dir.join("bench-e2e-Q4_K_M.gguf");
    std::fs::write(&model, b"tama-bench-e2e dummy gguf").expect("model file");

    server
}

/// Background `ps` sampler: records the first process line that shows the
/// marker llama-bench running.
fn spawn_ps_watch(saw: Arc<Mutex<Option<String>>>, marker: &std::path::Path) {
    let marker_str = marker.to_string_lossy().to_string();
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while tokio::time::Instant::now() < deadline {
            if let Ok(out) = tokio::process::Command::new("ps").arg("-ef").output().await {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    if line.contains(&marker_str) && !line.contains("ps -ef") {
                        let mut g = saw.lock().unwrap();
                        if g.is_none() {
                            *g = Some(line.to_string());
                        }
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

#[tokio::test]
async fn test_llama_bench_runs_on_the_tamad() {
    // ── Real tamad process ──
    let (mut tamad, tamad_port, tamad_dir, tamad_token) = start_tamad().await;
    let server_path = seed_tamad_host(tamad_dir.path());

    // ── Proxy: isolated Postgres schema + provider + pool connection ──
    let guard = common::with_schema().await;
    let pool = Arc::new(guard.pool.clone());
    let state = Arc::new(ProxyState::new(
        tama_core::config::Config::default(),
        None,
        pool.clone(),
    ));

    tama_core::db::queries::insert_provider(
        pool.as_ref(),
        "e2e-bench-llama",
        "local",
        "llama_cpp",
        Some(TAMAD_ID),
        None,
        None,
    )
    .await
    .expect("insert provider");
    tama_core::db::queries::insert_installation(
        pool.as_ref(),
        &tama_core::db::queries::InstallationRecord {
            id: 0,
            name: "llama_cpp".to_string(),
            backend_type: "llama_cpp".to_string(),
            version: BENCH_TAG.to_string(),
            path: server_path.to_string_lossy().to_string(),
            installed_at: 1_000,
            gpu_variant: "cpu".to_string(),
            source: None,
            is_active: true,
            docker_config: None,
            logical_id: String::new(),
        },
    )
    .await
    .expect("insert installation");
    let model_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO model_configs \
             (repo_id, display_name, backend, enabled, selected_quant, api_name) \
             VALUES ('test/bench-e2e-model', 'Bench E2E Model', 'llama_cpp', true, \
                     'Q4_K_M', 'test--bench-e2e-model') RETURNING id",
    )
    .fetch_one(pool.as_ref())
    .await
    .expect("insert model");
    tama_core::db::queries::upsert_model_file(
        pool.as_ref(),
        model_id,
        "test/bench-e2e-model",
        "bench-e2e-Q4_K_M.gguf",
        Some("Q4_K_M"),
        None,
        Some(16),
    )
    .await
    .expect("insert model file");

    let conn = TamadConnection {
        id: TAMAD_ID.to_string(),
        name: "e2e-bench-box".to_string(),
        url: format!("grpc://127.0.0.1:{tamad_port}"),
        protocol: Protocol::Grpc,
        token: Some(tamad_token),
        status: TamadStatus::Online,
    };
    state
        .tamad_pool()
        .upsert_connection(&conn)
        .await
        .expect("pool upsert");

    // ── Run the benchmark through the proxy inner (real dispatch) ──
    let jobs = Arc::new(tama_web::web_types::JobManager::new());
    let job = jobs
        .submit(tama_web::web_types::JobKind::Benchmark, None)
        .await
        .expect("job submit");

    // Watch for the marker llama-bench process while it runs.
    let marker = server_path
        .parent()
        .expect("install dir")
        .join("llama-bench");
    let saw: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    spawn_ps_watch(saw.clone(), &marker);

    let req = tama_web::api::benchmarks::BenchmarkRunRequest {
        model_id: model_id.to_string(),
        quant: None,
        backend_name: None,
        gpu_variant: None,
        pp_sizes: vec![512],
        tg_sizes: vec![64],
        runs: 1,
        warmup: 0,
        threads: None,
        ngl_range: None,
        ctx_override: None,
        batch_sizes: vec![],
        ubatch_sizes: vec![],
        kv_cache_type: None,
        depth: vec![],
        flash_attn: None,
        benchmark_type: Some("baseline".to_string()),
        suite_id: None,
    };

    eprintln!("E2E: dispatching real benchmark to tamad at grpc://127.0.0.1:{tamad_port}");
    tama_web::api::benchmarks::run_benchmark_inner(
        jobs.clone(),
        job.clone(),
        req,
        tamad_dir.path().join("proxy").join("tama.db"),
        "http://127.0.0.1:9".to_string(), // unload probe: intentionally unreachable
        reqwest::Client::new(),
        state.db_pool(),
        state.tamad_pool(),
    )
    .await
    .expect("benchmark inner");
    jobs.finish(&job, tama_web::web_types::JobStatus::Succeeded, None)
        .await;

    // ══ 1. The subprocess ran UNDER THE TAMAD's ROOTS ══
    let ran = server_path
        .parent()
        .expect("install dir")
        .join("e2e-bench-ran.txt");
    let ran_content = std::fs::read_to_string(&ran)
        .expect("marker must have written its ran-file inside the tamad's install dir");
    eprintln!("E2E: marker ran-file ({:?}): {}", ran, ran_content.trim());

    // ══ 2. `ps` caught the marker process with the tamad-side path ══
    let ps_line = saw
        .lock()
        .unwrap()
        .clone()
        .expect("ps must have observed the marker llama-bench process");
    eprintln!("E2E: ps line: {}", ps_line.trim());

    // ══ 3. Job log bridged the tamad's progress (real gRPC stream) ══
    // (short logs live in log_head; long ones spill into log_tail)
    let log_head = job
        .log_head
        .read()
        .await
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let log_tail = job
        .log_tail
        .read()
        .await
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let log: Vec<String> = log_head.into_iter().chain(log_tail).collect();
    let job_log = log.join("\n");
    eprintln!("E2E: job log:\n{job_log}");
    assert!(
        job_log.contains("Running: "),
        "job log must carry the tamad's 'Running: ...' line; log: {job_log}"
    );

    // ══ 4. History row persisted by the proxy with the parsed numbers ══
    let rows = tama_core::db::queries::list_benchmarks(pool.as_ref())
        .await
        .expect("list benchmarks");
    assert_eq!(rows.len(), 1, "one history row");
    let row = &rows[0];
    assert_eq!(row.model_id, model_id.to_string());
    assert_eq!(row.display_name.as_deref(), Some("Bench E2E Model"));
    assert_eq!(row.quant.as_deref(), Some("Q4_K_M"));
    assert_eq!(row.backend, "llama_cpp");
    assert_eq!(row.engine, "llama_bench");
    assert_eq!(row.status, "success");
    let results: serde_json::Value = serde_json::from_str(&row.results).expect("results JSON");
    let summaries = results["summaries"].as_array().expect("summaries array");
    assert_eq!(summaries.len(), 2, "pp + tg entries: {summaries:?}");
    let pp = summaries
        .iter()
        .find(|s| s["test_name"] == "pp512")
        .expect("pp512 summary");
    assert!((pp["pp_mean"].as_f64().unwrap() - 1234.5).abs() < 0.01);
    let tg = summaries
        .iter()
        .find(|s| s["test_name"] == "tg64")
        .expect("tg64 summary");
    assert!((tg["tg_mean"].as_f64().unwrap() - 66.6).abs() < 0.01);
    eprintln!(
        "E2E: persisted row: engine={} status={} pp512=1234.5 tg64=66.6",
        row.engine, row.status
    );

    // ══ 5. The DB row still points at the tamad host (proxy = sole writer) ══
    let mgr = InstallationManager::new(state.db_pool());
    let info = mgr
        .get_active("llama_cpp", "cpu")
        .await
        .unwrap()
        .expect("row");
    assert_eq!(info.path, server_path);

    let _ = tamad.kill().await;
    let _ = guard;
}
