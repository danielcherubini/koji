//! E2E: backend install / update / remove executed by a *real* tamad
//! process (plan-191 Task 7).
//!
//! The chain under test:
//! 1. Proxy handler (install/update/remove) resolves the provider's tamad
//! 2. `InstallProvider` / `UpdateProvider` / `RemoveProvider` RPC to the
//!    spawned tamad binary (real gRPC + token auth)
//! 3. Tamad runs the *real* installer (`install_installation_with_progress`
//!    → prebuilt download + pure-Rust extract) rooted at
//!    `<tamad-data-dir>/install/...` ("marker" shell scripts stand in for
//!    the llama.cpp binary to keep the archive tiny; the code path —
//!    download → extract → chmod → verify — is identical)
//! 4. Job events stream back into the proxy `JobManager` (unchanged UX)
//! 5. The proxy (sole DB writer) persists the installation rows from the
//!    tamad's result JSON; removal deletes host dirs + DB rows
//! 6. Tamad process killed → a new install fails the job with an
//!    actionable dispatch error (fail loud, no local fallback)
//!
//! GitHub release URLs are redirected to a local mock via the
//! `TAMA_E2E_GITHUB_BASE` env seam (set on this process and inherited by
//! the tamad child), so the test is hermetic.

mod common;

use axum::routing::{delete, get, post};
use axum::Router;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

use tama_core::installations::InstallationManager;
use tama_core::providers::{Protocol, TamadConnection, TamadStatus};
use tama_core::proxy::ProxyState;
use tama_web::api::installations::install::{install_installation, remove_installation};
use tama_web::api::installations::jobs::get_job;
use tama_web::api::installations::manage::update_installation;
use tama_web::web_types::{JobManager, WebState};

const INSTALL_TAG: &str = "b9901";
const UPDATE_TAG: &str = "b9902";
const TAMAD_ID: &str = "6f0b9c2e-e2e4-4c1a-9f01-0000e2e00001";

/// In-memory .tar.gz with executable marker `llama-server`/`llama-bench`
/// shell scripts (the extract step chmods `llama-*` files to 0755).
fn build_marker_archive(tag: &str) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tar::Header;

    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(gz);
    let content = format!("#!/bin/sh\necho 'tama-e2e-marker {tag}'\n");
    for name in ["llama-server", "llama-bench"] {
        let mut header = Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, name, content.as_bytes())
            .unwrap();
    }
    let inner = builder.into_inner().unwrap();
    inner.finish().unwrap()
}

/// Serve the marker archive for the requested tag (bytes only; the
/// installer does not inspect headers).
async fn download_handler(
    axum::extract::Path((tag, _file)): axum::extract::Path<(String, String)>,
) -> Vec<u8> {
    build_marker_archive(&tag)
}

/// Start the mock release server; returns the bound address.
async fn start_preset_server() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route(
            "/repos/ggml-org/llama.cpp/releases",
            get(|| async {
                axum::Json(serde_json::json!([
                    {"tag_name": UPDATE_TAG, "prerelease": false},
                    {"tag_name": "b9900", "prerelease": true},
                ]))
            }),
        )
        .route(
            "/ggml-org/llama.cpp/releases/download/:tag/:file",
            get(download_handler),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("preset server bind");
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, handle)
}

fn tamad_binary() -> std::path::PathBuf {
    // Resolve the workspace target dir relative to the crate manifest so
    // the lookup works regardless of the test process working directory.
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
async fn start_tamad(
    preset_base: &str,
) -> (
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
        .env("TAMA_E2E_GITHUB_BASE", preset_base)
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

/// Build a request carrying the WebState extension (handlers extract via
/// `Extension<WebState>`; oneshot requests don't pass through the router's
/// extension layers).
/// Build a request carrying the WebState extension + an optional
/// `Content-Type` (handlers extract via `Extension<WebState>`; oneshot
/// requests don't pass through the router's layers).
fn web_req(
    method: &str,
    uri: &str,
    body: axum::body::Body,
    web: &Arc<WebState>,
) -> axum::http::Request<axum::body::Body> {
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if method == "POST" {
        builder = builder.header("content-type", "application/json");
    }
    let mut req = builder.body(body).expect("request builder");
    req.extensions_mut().insert(web.as_ref().clone());
    req
}

/// Poll GET /tama/v1/backends/jobs/:id until it leaves "running".
async fn wait_for_terminal(
    router: &Router,
    web: &Arc<WebState>,
    job_id: &str,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let resp = router
            .clone()
            .oneshot(web_req(
                "GET",
                &format!("/tama/v1/backends/jobs/{job_id}"),
                axum::body::Body::empty(),
                web,
            ))
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let status = v["status"].as_str().unwrap_or_default();
        if status != "running" {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "job did not finish in time: {v}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn test_install_update_remove_executed_on_tamad() {
    // ── Mock release server (GitHub surface redirect target) ──
    let (preset_addr, _preset_handle) = start_preset_server().await;
    let preset_base = format!("http://{preset_addr}");

    // Point github release lookups at the mock — in-process (for the proxy's
    // `check_latest_version` during update) and inherited by the tamad child
    // (for the prebuilt download). Isolated per test (nextest process model).
    std::env::set_var("TAMA_E2E_GITHUB_BASE", &preset_base);

    // ── Real tamad process ──
    let (mut tamad, tamad_port, tamad_dir, tamad_token) = start_tamad(&preset_base).await;
    let install_root = tamad_dir.path().join("install");

    // ── Proxy: isolated Postgres schema + PSK-style state ──
    let guard = common::with_schema().await;
    let pool = Arc::new(guard.pool.clone());
    let state = Arc::new(ProxyState::new(
        tama_core::config::Config::default(),
        None,
        pool.clone(),
    ));

    // Provider bound to this tamad + pool connection (token auth).
    tama_core::db::queries::insert_provider(
        pool.as_ref(),
        "e2e-loc-llama",
        "local",
        "llama_cpp",
        Some(TAMAD_ID),
        None,
        None,
    )
    .await
    .expect("insert provider");
    let conn = TamadConnection {
        id: TAMAD_ID.to_string(),
        name: "e2e-box".to_string(),
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

    let jobs = Arc::new(JobManager::new());
    let web = Arc::new(WebState {
        jobs: Some(jobs.clone()),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "e2e".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        db_pool: pool,
        log_filter: None,
        log_status: None,
        log_read: None,
        log_tail: None,
        log_events_tx: Arc::new(tokio::sync::Mutex::new(None)),
    });
    let router = Router::new()
        .route("/tama/v1/backends/install", post(install_installation))
        .route("/tama/v1/backends/:name/update", post(update_installation))
        .route("/tama/v1/backends/:name", delete(remove_installation))
        .route("/tama/v1/backends/jobs/:id", get(get_job))
        .with_state(state.clone());

    // ══ 1. INSTALL: executed on the tamad, registered by the proxy ══
    let resp = router
        .clone()
        .oneshot(web_req(
            "POST",
            "/tama/v1/backends/install",
            axum::body::Body::from(
                serde_json::json!({
                    "backend_type": "llama_cpp",
                    "version": INSTALL_TAG,
                    "gpu_variant": "cpu",
                    "build_from_source": false,
                    "force": false
                })
                .to_string(),
            ),
            &web,
        ))
        .await
        .unwrap();
    let resp_status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(
        resp_status,
        axum::http::StatusCode::OK,
        "install response: {}",
        String::from_utf8_lossy(&body)
    );
    let install_job: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let install_job_id = install_job["job_id"].as_str().unwrap().to_string();

    let job = wait_for_terminal(&router, &web, &install_job_id).await;
    assert_eq!(
        job["status"],
        "succeeded",
        "install job must succeed; error: {:?} log: {:?}",
        job["error"],
        job["log"].as_array().map(|a| a.len())
    );
    // The job log carries the tamad's progress lines (bridged StreamJob).
    let log: String = job["log"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| l.as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    // The job log carries the tamad's installer progress lines (bridged
    // StreamJob). The earliest states (install header / download URL) can be
    // overwritten in the latest-state snapshot before a late subscriber
    // connects, so we assert on the ones that must survive: the extract +
    // final lines. The *download* used the redirected URL is proven by the
    // marker file content check below (only our mock server produces it).
    assert!(
        log.contains("Extracting archive..."),
        "job log must show the real installer's extract line; log: {log}"
    );
    assert!(
        log.contains("Backend installed at"),
        "job log must show the installer's final line; log: {log}"
    );

    eprintln!("E2E: install job log:\n{log}\n");

    // DB row (proxy = sole writer) + file actually installed on the tamad host.
    let installed = install_root
        .join("llama_cpp")
        .join("cpu")
        .join(INSTALL_TAG)
        .join("llama-server");
    assert!(
        installed.exists(),
        "installer must extract into the tamad's install dir: {installed:?}"
    );
    let content = std::fs::read_to_string(&installed).unwrap();
    assert!(
        content.contains(&format!("tama-e2e-marker {INSTALL_TAG}")),
        "marker binary content: {content}"
    );
    eprintln!(
        "E2E: marker on tamad host ({:?}): {}",
        installed,
        content.trim()
    );

    let mgr = InstallationManager::new(state.db_pool());
    let row = mgr
        .get_active("llama_cpp", "cpu")
        .await
        .unwrap()
        .expect("installation row must be persisted by the proxy");
    assert_eq!(row.version, INSTALL_TAG);
    assert_eq!(row.path, installed);
    eprintln!(
        "E2E: proxy persisted DB row llama_cpp/cpu -> version {} path {:?}",
        row.version, row.path
    );

    // ══ 2. UPDATE: same chain, new version row activated ══
    let resp = router
        .clone()
        .oneshot(web_req(
            "POST",
            "/tama/v1/backends/llama_cpp/update?gpu_variant=cpu",
            axum::body::Body::empty(),
            &web,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let update_job: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let job = wait_for_terminal(&router, &web, update_job["job_id"].as_str().unwrap()).await;
    assert_eq!(
        job["status"], "succeeded",
        "update job must succeed; error: {:?}",
        job["error"]
    );
    let row = mgr
        .get_active("llama_cpp", "cpu")
        .await
        .unwrap()
        .expect("updated row");
    assert_eq!(row.version, UPDATE_TAG, "new version must be active");
    eprintln!(
        "E2E: update activated version {} on the tamad host",
        row.version
    );
    assert!(install_root
        .join("llama_cpp")
        .join("cpu")
        .join(UPDATE_TAG)
        .join("llama-server")
        .exists());

    // ══ 3. REMOVE: host dirs deleted by the tamad, DB rows by the proxy ══
    let resp = router
        .clone()
        .oneshot(web_req(
            "DELETE",
            "/tama/v1/backends/llama_cpp",
            axum::body::Body::empty(),
            &web,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let del: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(del["removed"], true);

    // After removal the variant dir is gone (or its version subdirs emptied).
    let variant_dir = install_root.join("llama_cpp").join("cpu");
    let variant_emptied = variant_dir
        .read_dir()
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    assert!(
        !variant_dir.is_dir() || variant_emptied,
        "version dirs must be deleted on the tamad host"
    );
    assert!(
        mgr.get_active("llama_cpp", "cpu").await.unwrap().is_none(),
        "DB rows must be gone after remove"
    );
    eprintln!("E2E: remove deleted the tamad version dirs + proxy DB rows");

    // ══ 4. TAMAD OFFLINE: dispatch failure → job fails with actionable ══
    tamad.kill().await.expect("kill tamad");
    let _ = tamad.wait().await;

    let resp = router
        .clone()
        .oneshot(web_req(
            "POST",
            "/tama/v1/backends/install",
            axum::body::Body::from(
                serde_json::json!({
                    "backend_type": "llama_cpp",
                    "version": INSTALL_TAG,
                    "gpu_variant": "cpu",
                    "build_from_source": false,
                    "force": true
                })
                .to_string(),
            ),
            &web,
        ))
        .await
        .unwrap();
    // The job endpoint still reports the failure (dispatch Unavailable).
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let job_id: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let job = wait_for_terminal(&router, &web, job_id["job_id"].as_str().unwrap()).await;
    assert_eq!(
        job["status"], "failed",
        "install with the tamad killed must fail; got: {job}"
    );
    let err = job["error"].as_str().unwrap_or_default().to_string();
    assert!(!err.is_empty(), "failed job must carry an actionable error");
    eprintln!("E2E: tamad offline -> job failed with: {err}");
    assert!(
        mgr.get_active("llama_cpp", "cpu").await.unwrap().is_none(),
        "no DB row may be written on dispatch failure"
    );

    _preset_handle.abort();
}
