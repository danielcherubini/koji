//! Orchestration tests for the pull queue runner (ADR-0010, plan-191 Task 10).
//!
//! The old-world local-download tests (wiremock HEAD/GET feeding the
//! proxy's own `run_verification`) were removed with Task 6/10: every
//! download executes on a tamad host, and the dispatch + relay behavior is
//! covered in `tamad_pull.rs`. What remains here is the failure surface of
//! the relay itself:
//! - no `pull_backend` configured → loud, specific failure;
//! - the host reports a terminal failure (e.g. SHA-256 mismatch detected
//!   host-side) → the proxy mirrors it onto the PullJob + DB queue row.

use std::sync::Arc;

use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::tama_handlers::{start_pull_from_queue, QuantPullSpec};
use crate::proxy::ProxyState;
use crate::tamad::pool::test_support::{grpc_conn, job_event_failed, start_stub, StubTamad};

const REPO: &str = "test/repo";
const FILE: &str = "repo-Q4_K_M.gguf";
const TAMAD_ID: &str = "uuid-tamad";

/// ProxyState with NO `pull_backend` configured, an isolated Postgres
/// schema, and one seeded Pending PullJob. Returns `(state, models_tmp,
/// xdg_tmp, job_id, guard)`.
async fn create_pull_state() -> (
    Arc<ProxyState>,
    tempfile::TempDir,
    tempfile::TempDir,
    String,
    crate::testing::postgres::SchemaGuard,
) {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    std::env::set_var("XDG_CONFIG_HOME", xdg_tmp.path().to_str().unwrap());
    std::env::set_var("HOME", xdg_tmp.path().to_str().unwrap());

    let mut config = crate::config::Config::default();
    config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());

    let guard = crate::testing::postgres::with_schema().await;
    let pool = Arc::new(guard.pool.clone());
    let svc = crate::proxy::pull_queue::PullQueueService::new(pool.clone(), 2);

    let mut state = ProxyState::new(config, None, pool);
    assert!(state.pull_queue().is_some());
    state.pull.pull_queue = Some(Arc::new(svc));

    // Seed the in-memory job — start_pull_from_queue early-returns with
    // "Job not found" if it's absent from pull_jobs.
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

    (Arc::new(state), models_tmp, xdg_tmp, job_id, guard)
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

// ── No pull host configured → loud failure ──────────────────────────────────

/// Without `proxy.pull_backend` the pull must fail with a specific message
/// (the proxy never downloads locally — ADR-0010) and the DB queue row must
/// be marked failed.
#[tokio::test]
async fn test_pull_without_host_fails_loud() {
    let (state, models_tmp, _xdg_tmp, job_id, _guard) = create_pull_state().await;

    let svc = state
        .pull_queue()
        .as_ref()
        .expect("pull_queue should be set");
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
    let job = jobs.get(&job_id).expect("job should exist");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("no pull host"),
        "error should point at the missing pull host, got: {:?}",
        job.error
    );

    // No file was written locally — the proxy never downloads.
    let file_path = models_tmp.path().join(REPO).join(FILE);
    assert!(!file_path.exists(), "proxy must not download locally");

    // DB queue row reflects the failure.
    let db_row = svc.get_queue_item(&job_id).await.unwrap();
    assert_eq!(
        db_row.map(|r| r.status),
        Some("failed".to_string()),
        "DB queue status should be 'failed'"
    );
}

// ── Host-reported failure (verification) is mirrored ───────────────────────

/// The host executes the pull and reports a terminal failure (a SHA-256
/// mismatch detected host-side). The relay must mirror the failure onto the
/// PullJob with the host's error and mark the DB queue row failed.
#[tokio::test]
async fn test_pull_host_verification_failure_is_mirrored() {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    std::env::set_var("XDG_CONFIG_HOME", xdg_tmp.path().to_str().unwrap());
    std::env::set_var("HOME", xdg_tmp.path().to_str().unwrap());

    let (down_tx, _) = tokio::sync::watch::channel(false);
    let stub = StubTamad {
        fail_first_n: 0,
        succeed_until: usize::MAX,
        down: Arc::new(down_tx),
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        successes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        pull_job_id: "job-tamad".to_string(),
        pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
        install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        install_job_id: "job-install".to_string(),
        install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
        update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        update_job_id: "job-update".to_string(),
        update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
        remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
        stream_job_events: Arc::new(tokio::sync::Mutex::new(vec![job_event_failed(
            "job-tamad",
            "verification failed for 'repo-Q4_K_M.gguf': hash mismatch: expected abc1 got def2",
        )])),
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
        stats_processes: vec![],
        logs_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        log_messages: vec![],
        stream_log_frames: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
        stream_log_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        stream_log_refuse: false,
    };
    let addr = start_stub(stub.clone()).await;

    let mut config = crate::config::Config::default();
    config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());
    config.proxy.pull_backend = Some(TAMAD_ID.to_string());

    let _guard = crate::testing::postgres::with_schema().await;
    let pool = Arc::new(_guard.pool.clone());
    let svc = Arc::new(crate::proxy::pull_queue::PullQueueService::new(
        pool.clone(),
        2,
    ));
    let mut state = ProxyState::new(config, None, pool);
    state.pull.pull_queue = Some(Arc::clone(&svc));

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

    let conn = grpc_conn(TAMAD_ID, "stub", &format!("grpc://{addr}"));
    state.tamad_pool.upsert_connection(&conn).await.unwrap();

    let state = Arc::new(state);
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

    // The host was dispatched exactly once, with the proxy's repo dir.
    let reqs = stub.pull_requests.lock().await;
    assert_eq!(reqs.len(), 1, "exactly one PullModel dispatch");
    drop(reqs);

    // The host's verification error was mirrored onto the PullJob.
    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job exists");
    assert_eq!(job.status, PullJobStatus::Failed, "job should be Failed");
    assert!(
        job.error
            .as_deref()
            .unwrap_or_default()
            .contains("hash mismatch"),
        "job error should carry the host's hash mismatch, got: {:?}",
        job.error
    );

    // Queue row failed.
    let db_row = state
        .pull_queue()
        .as_ref()
        .unwrap()
        .get_queue_item(&job_id)
        .await
        .unwrap();
    assert_eq!(db_row.map(|r| r.status), Some("failed".to_string()));
}
