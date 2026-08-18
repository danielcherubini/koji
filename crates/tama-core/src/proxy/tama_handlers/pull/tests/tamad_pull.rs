//! Tamad-hosted pull relay tests (plan-191 Task 6).
//!
//! The proxy's `pull_backend` names a stub tamad (gRPC `TamadService`
//! test stub); the relay dispatches `PullModel`, mirrors `StreamJob`
//! events into the PullJob/queue, and consumes the terminal event's
//! `result_json` — the host verified the file on its own disk and reports
//! hashes/size/metadata, so the proxy persists the registry rows from that
//! payload and never reads (or re-hashes) proxy-local files.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::tama_handlers::start_pull_from_queue;
use crate::proxy::tama_handlers::QuantPullSpec;
use crate::proxy::ProxyState;
use crate::tamad::pool::test_support::{
    grpc_conn, job_event, job_event_bytes, start_stub, terminal_success, StubTamad,
};

const REPO: &str = "test/repo";
const FILE: &str = "repo-Q4_K_M.gguf";
const TAMAD_ID: &str = "uuid-tamad";

fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// StubTamad with all-default pull config except the scripted events.
fn make_stub(
    events: Vec<crate::tamad::JobEvent>,
    pull_model_fail: bool,
) -> (StubTamad, Arc<tokio::sync::watch::Sender<bool>>) {
    let (down, _) = tokio::sync::watch::channel(false);
    let stub = StubTamad {
        fail_first_n: 0,
        succeed_until: usize::MAX,
        down: Arc::new(down.clone()),
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        successes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        pull_job_id: "job-tamad".to_string(),
        pull_model_fail: Arc::new(tokio::sync::Mutex::new(pull_model_fail)),
        install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        install_job_id: "job-install".to_string(),
        install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
        update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        update_job_id: "job-update".to_string(),
        update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
        remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
        stream_job_events: Arc::new(tokio::sync::Mutex::new(events)),
        stream_job_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        stream_job_events_by_id: Arc::new(
            tokio::sync::Mutex::new(std::collections::HashMap::new()),
        ),
        bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        bench_job_id: "job-bench".to_string(),
        bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
        stats_gpus: vec![],
        load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        load_delays: std::collections::HashMap::new(),
        load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
    };
    (stub, Arc::new(down))
}

/// ProxyState with `pull_backend` → TAMAD_ID, an isolated Postgres schema,
/// an in-memory PullJob seeded, and the stub tamad registered in the pool.
async fn create_tamad_pull_state(
    models_tmp: &tempfile::TempDir,
    xdg_tmp: &tempfile::TempDir,
    stub_addr: std::net::SocketAddr,
) -> (
    Arc<ProxyState>,
    String,
    crate::testing::postgres::SchemaGuard,
) {
    std::env::set_var("XDG_CONFIG_HOME", xdg_tmp.path().to_str().unwrap());
    std::env::set_var("HOME", xdg_tmp.path().to_str().unwrap());

    let mut config = crate::config::Config::default();
    config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());
    config.proxy.pull_backend = Some(TAMAD_ID.to_string());

    let guard = crate::testing::postgres::with_schema().await;
    let pool = Arc::new(guard.pool.clone());
    let state = ProxyState::new(config, None, pool);
    assert!(state.pull_queue().is_some());

    let job_id = uuid::Uuid::new_v4().to_string();
    state.pull.pull_jobs.write().await.insert(
        job_id.clone(),
        PullJob {
            job_id: job_id.clone(),
            repo_id: REPO.to_string(),
            filename: FILE.to_string(),
            status: PullJobStatus::Pending,
            ..Default::default()
        },
    );

    let conn = grpc_conn(TAMAD_ID, "stub", &format!("grpc://{stub_addr}"));
    state.tamad_pool.upsert_connection(&conn).await.unwrap();

    (Arc::new(state), job_id, guard)
}

fn spec() -> QuantPullSpec {
    QuantPullSpec {
        filename: FILE.into(),
        quant: Some("Q4_K_M".into()),
        context_length: None,
    }
}

fn restore_env() {
    std::env::remove_var("HF_ENDPOINT");
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("HOME");
}

// ── Test 1: success — relay, host result JSON consumed, no local disk ───

/// Tamad reports success with a result JSON carrying the precomputed
/// SHA-256, size, and GGUF metadata. The completion phase must persist the
/// registry rows from that payload — not from a proxy-local file (which on
/// a remote host does not exist at all) and not from a fresh HF blobs-API
/// fetch — so the test plants a DECOY file at the dest path whose content
/// hashes to neither the JSON hash nor the blobs-mock hash: any disk read,
/// stat, or re-verification leaves a detectable trace in the rows.
#[tokio::test]
async fn test_tamad_pull_success_consumes_host_result_json() {
    // The upstream LFS hash the (mocked) blobs API would serve. Must differ
    // from the host's reported expected_sha: if the completion phase
    // consulted the blobs API, the row would carry this value.
    let blobs_sha = "1111111111111111111111111111111111111111111111111111111111111111";
    let server = wiremock::MockServer::start().await;
    std::env::set_var("HF_ENDPOINT", server.uri());

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(format!("/api/models/{REPO}")))
        .and(wiremock::matchers::query_param("blobs", "true"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "siblings": [{
                    "rfilename": FILE,
                    "blobId": "b1",
                    "size": 64,
                    "lfs": { "sha256": blobs_sha }
                }]
            })),
        )
        .mount(&server)
        .await;

    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    // Decoy at the exact destination: content hashing to NEITHER the JSON's
    // sha256/expected_sha NOR the blobs-mock hash. A completion phase that
    // reads/hashes/stats proxy-local disk either fails verification
    // (expected vs actual mismatch) or persists the decoy's size/hash.
    let decoy: Vec<u8> = b"a decoy file matching no reported hash".to_vec();
    let file_path = models_tmp.path().join(REPO).join(FILE);
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    std::fs::write(&file_path, &decoy).unwrap();

    // What the host claims for the real (remote) file.
    let host_body: Vec<u8> = b"weights as the host downloaded them".to_vec();
    let host_sha = sha256_hex(&host_body);
    let host_expected_sha = "2222222222222222222222222222222222222222222222222222222222222222";
    let host_size = host_body.len() as u64; // != decoy.len()
    let result_json = serde_json::json!({
        "dir": models_tmp.path().join(REPO).to_string_lossy(),
        "files": [{
            "path": FILE,
            "size": host_size,
            "sha256": host_sha,
            "expected_sha": host_expected_sha,
            "verified": true,
            "is_primary_shard": true
        }],
        "gguf_metadata": {
            "architecture": "llama",
            "context_length": 8192,
            "block_count": 36,
            "quantization": "Q4_K_M"
        }
    })
    .to_string();

    let progress = job_event_bytes(
        "job-tamad",
        100,
        "Verifying...",
        "running",
        host_size as i64,
        host_size as i64,
    );
    let (stub, _down) = make_stub(
        vec![progress, terminal_success("job-tamad", &result_json)],
        false,
    );
    let addr = start_stub(stub.clone()).await;
    let (state, job_id, _schema) = create_tamad_pull_state(&models_tmp, &xdg_tmp, addr).await;

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        spec(),
    )
    .await;

    restore_env();

    // The stub received the dispatch with the proxy's repo dir as dest_dir.
    let reqs = stub.pull_requests.lock().await;
    assert_eq!(reqs.len(), 1, "exactly one PullModel dispatch");
    assert_eq!(reqs[0].repo_id, REPO);
    assert_eq!(reqs[0].quants, vec![FILE.to_string()]);
    let expected_dest = models_tmp.path().join(REPO);
    assert_eq!(
        reqs[0].dest_dir,
        expected_dest.to_string_lossy(),
        "dest_dir must be the proxy's repo directory"
    );
    drop(reqs);

    // Job completed from the host payload (not from any local re-check).
    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(
        job.status,
        PullJobStatus::Completed,
        "job should be Completed, error: {:?}",
        job.error
    );
    assert_eq!(job.verified_ok, Some(true), "host verified the file");
    assert_eq!(
        job.bytes_pulled, host_size,
        "size from result JSON, not the decoy"
    );
    assert_eq!(job.total_bytes, Some(host_size));
    assert_eq!(
        job.gguf_context_length,
        Some(8192),
        "GGUF context length from host metadata"
    );
    drop(jobs);

    // No upstream re-verification: the blobs API was never called.
    let received = server.received_requests().await.unwrap();
    let urls: Vec<String> = received.iter().map(|r| r.url.to_string()).collect();
    assert!(
        urls.iter().all(|u| !u.contains("blobs")),
        "completion phase must not re-fetch the upstream hash, saw: {}",
        urls.join(", ")
    );

    // The decoy was never read, verified, or deleted (the proxy has no
    // business touching proxy-local files for a host pull).
    assert!(
        file_path.exists(),
        "completion phase must not delete proxy-local files"
    );

    // Queue item completed with the host-reported size.
    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "completed");
    assert_eq!(db_row.bytes_pulled, host_size as i64);

    // model_files row: hash + size + verification state from the result JSON.
    let files = state.model_mgr().get_all_files().await.unwrap();
    assert_eq!(files.len(), 1, "one model_files row");
    assert_eq!(files[0].filename, FILE);
    assert_eq!(
        files[0].lfs_oid.as_deref(),
        Some(host_expected_sha),
        "lfs_oid must come from the result JSON, not the blobs API (or a local re-hash)"
    );
    assert_eq!(
        files[0].size_bytes,
        Some(host_size as i64),
        "size from the result JSON"
    );
    assert_eq!(
        files[0].verified_ok,
        Some(true),
        "host's verified flag persisted"
    );
    assert!(files[0].verify_error.is_none());

    // Model row: the host-parsed GGUF metadata drives the hf_* fields.
    let model_id = files[0].model_id;
    let record = crate::db::queries::get_model_config(&state.db_pool(), model_id)
        .await
        .unwrap()
        .expect("model row should exist");
    assert_eq!(
        record.hf_architecture_type.as_deref(),
        Some("llama"),
        "architecture from host metadata"
    );
    assert_eq!(record.hf_context_length, Some(8192));
    assert_eq!(
        record.hf_num_layers,
        Some(36),
        "block_count from host metadata"
    );

    // Model card: size + context from the result JSON, not a disk stat / parse.
    let card = xdg_tmp
        .path()
        .join("tama")
        .join("configs")
        .join("test--repo.toml");
    assert!(card.exists(), "model card should exist");
    let card_toml = crate::models::card::ModelToml::load(&card).expect("card parses");
    let quant = card_toml
        .quants
        .get("Q4_K_M")
        .expect("card has the quant entry");
    assert_eq!(
        quant.size_bytes,
        Some(host_size),
        "card size must come from the result JSON, not a local stat"
    );
    assert_eq!(
        quant.context_length,
        Some(8192),
        "card context from host metadata"
    );
}

// ── Tri-state parity: no upstream hash → verified_ok stays NULL ───────────

/// When the host's result payload carries NO `expected_sha` (HF blobs API
/// was unavailable on the host, so nothing to compare against) but reports
/// `verified = true` (best-effort pass), the proxy must persist
/// `verified_ok = NULL` — the tri-state's "no upstream hash" state — not
/// `Some(true)`, which falsely asserts "hash matched" for a file that was
/// never hash-verified. Mirrors the local path (`run_verification` returns
/// `ok = None` when `expected_sha == None`).
#[tokio::test]
async fn test_tamad_pull_no_expected_sha_persists_verified_ok_null() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    // Host hashed the file (sha256 present) but had no upstream LFS hash.
    let body: Vec<u8> = b"weights as the host downloaded them".to_vec();
    let result_json = serde_json::json!({
        "dir": "/remote/models/test/repo",
        "files": [{
            "path": FILE,
            "size": body.len(),
            "sha256": sha256_hex(&body),
            "verified": true,
            "is_primary_shard": true
        }]
    })
    .to_string();

    let (stub, _down) = make_stub(vec![terminal_success("job-tamad", &result_json)], false);
    let addr = start_stub(stub).await;
    let (state, job_id, _schema) = create_tamad_pull_state(&models_tmp, &xdg_tmp, addr).await;

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        spec(),
    )
    .await;

    restore_env();

    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(
        job.status,
        PullJobStatus::Completed,
        "job should be Completed, error: {:?}",
        job.error
    );
    assert_eq!(
        job.verified_ok, None,
        "no upstream hash → verified_ok must be NULL, not Some(true)"
    );
    drop(jobs);

    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "completed");

    // model_files row: NULL verified_ok (no upstream hash), no error.
    let files = state.model_mgr().get_all_files().await.unwrap();
    assert_eq!(files.len(), 1, "one model_files row");
    assert_eq!(files[0].filename, FILE);
    assert_eq!(
        files[0].lfs_oid, None,
        "no expected_sha → lfs_oid must persist as NULL"
    );
    assert_eq!(
        files[0].verified_ok, None,
        "no upstream hash → verified_ok must persist as NULL (Some(true) is \"hash matched\")"
    );
    assert!(files[0].verify_error.is_none());
}

// ── Fail-loud: success terminal without a usable result payload ────────────

/// Terminal success with an empty result payload (a host predating the
/// enriched result, or a broken terminal event): the proxy must fail the
/// pull with a specific error rather than fabricating registry data from
/// proxy-local disk (which on a remote host does not exist).
#[tokio::test]
async fn test_tamad_pull_success_without_result_payload_fails_loud() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    let (stub, _down) = make_stub(vec![terminal_success("job-tamad", "")], false);
    let addr = start_stub(stub).await;
    let (state, job_id, _schema) = create_tamad_pull_state(&models_tmp, &xdg_tmp, addr).await;

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        spec(),
    )
    .await;

    restore_env();

    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("result payload"),
        "error should mention the missing result payload, got: {:?}",
        job.error
    );

    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "failed");

    // No registry rows without a payload to feed them.
    assert!(state.model_mgr().get_all_files().await.unwrap().is_empty());
}

/// Same as above but with a MALFORMED payload: the relay must not panic or
/// half-persist — the pull fails with the parse error surfaced.
#[tokio::test]
async fn test_tamad_pull_success_malformed_result_payload_fails_loud() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    let (stub, _down) = make_stub(vec![terminal_success("job-tamad", "{not json")], false);
    let addr = start_stub(stub).await;
    let (state, job_id, _schema) = create_tamad_pull_state(&models_tmp, &xdg_tmp, addr).await;

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        spec(),
    )
    .await;

    restore_env();

    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("result payload"),
        "error should mention the malformed result payload, got: {:?}",
        job.error
    );

    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "failed");

    assert!(state.model_mgr().get_all_files().await.unwrap().is_empty());
}

/// Defensive consistency: a success payload that claims a file failed
/// verification is contradictory — the proxy consumes the host's verdict
/// (no local disk involvement) and fails the pull with the host's error.
#[tokio::test]
async fn test_tamad_pull_success_unverified_payload_fails() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    let result_json = serde_json::json!({
        "dir": "/remote/models/test/repo",
        "files": [{
            "path": FILE,
            "size": 100,
            "sha256": "abc",
            "expected_sha": "def",
            "verified": false,
            "verify_error": "hash mismatch: expected abc1 got def2",
            "is_primary_shard": true
        }]
    })
    .to_string();

    let (stub, _down) = make_stub(vec![terminal_success("job-tamad", &result_json)], false);
    let addr = start_stub(stub).await;
    let (state, job_id, _schema) = create_tamad_pull_state(&models_tmp, &xdg_tmp, addr).await;

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        spec(),
    )
    .await;

    restore_env();

    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("hash mismatch"),
        "job error should carry the host's verification error, got: {:?}",
        job.error
    );
    assert_eq!(job.verified_ok, Some(false));

    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "failed");

    // No registry rows for a file the host itself rejected.
    assert!(state.model_mgr().get_all_files().await.unwrap().is_empty());
}

// ── Test 2: dispatch failure → pull failed, no local download ─────────────

/// `pull_model` RPC fails (tamad offline): the pull is marked failed with
/// the dispatch error — the proxy does NOT fall back to a local download.
#[tokio::test]
async fn test_tamad_pull_dispatch_failure_fails_loud() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    let (stub, _down) = make_stub(Vec::new(), true); // pull_model → unavailable
    let addr = start_stub(stub.clone()).await;
    let (state, job_id, _schema) = create_tamad_pull_state(&models_tmp, &xdg_tmp, addr).await;

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        spec(),
    )
    .await;

    restore_env();

    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("tamad pull dispatch failed"),
        "error should mention the dispatch failure, got: {:?}",
        job.error
    );

    // No local download happened: no file on disk.
    assert!(!models_tmp.path().join(REPO).join(FILE).exists());

    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "failed");
}

// ── Test 3: stream disconnects mid-pull → pull failed ─────────────────────

/// The job stream ends before any terminal event (tamad died): the relay
/// fails the pull with a disconnect error.
#[tokio::test]
async fn test_tamad_pull_stream_disconnect_fails() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    // One progress event, NO terminal — then the stream is cut.
    let (stub, down) = make_stub(
        vec![job_event("job-tamad", 10, "downloading", "running")],
        false,
    );
    let addr = start_stub(stub.clone()).await;
    let (state, job_id, _schema) = create_tamad_pull_state(&models_tmp, &xdg_tmp, addr).await;

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    // Run the pull; once the dispatch has reached the stub, cut the stream.
    let pull_task = tokio::spawn({
        let state = state.clone();
        let job_id = job_id.clone();
        async move { start_pull_from_queue(state, job_id, REPO.into(), FILE.into(), spec()).await }
    });

    // Wait until the relay has opened the job stream (the disconnect must
    // land AFTER the subscription, or the stub would wait forever).
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
    loop {
        if stub
            .stream_job_calls
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "relay never opened the job stream"
        );
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    down.send_replace(true);
    pull_task.await.unwrap();

    restore_env();

    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("disconnected mid-pull"),
        "error should mention the mid-pull disconnect, got: {:?}",
        job.error
    );

    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "failed");
}

// ── Test 4: unregistered pull_backend → pull failed ───────────────────────

/// `pull_backend` names a tamad with no registered connection: fail loud
/// with a clear error (no silent fallback to local pulls).
#[tokio::test]
async fn test_tamad_pull_unregistered_backend_fails() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    std::env::set_var("XDG_CONFIG_HOME", xdg_tmp.path().to_str().unwrap());
    std::env::set_var("HOME", xdg_tmp.path().to_str().unwrap());

    let mut config = crate::config::Config::default();
    config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());
    config.proxy.pull_backend = Some("uuid-missing".to_string());

    let _schema = crate::testing::postgres::with_schema().await;
    let pool = Arc::new(_schema.pool.clone());
    let state = Arc::new(ProxyState::new(config, None, pool));

    let job_id = uuid::Uuid::new_v4().to_string();
    state.pull.pull_jobs.write().await.insert(
        job_id.clone(),
        PullJob {
            job_id: job_id.clone(),
            repo_id: REPO.to_string(),
            filename: FILE.to_string(),
            status: PullJobStatus::Pending,
            ..Default::default()
        },
    );

    let svc = state.pull_queue().as_ref().expect("pull_queue set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .await
        .unwrap();

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        spec(),
    )
    .await;

    restore_env();

    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("not a registered tamad"),
        "error should mention the unregistered backend, got: {:?}",
        job.error
    );

    let db_row = svc
        .get_queue_item(&job_id)
        .await
        .unwrap()
        .expect("queue row");
    assert_eq!(db_row.status, "failed");
}
