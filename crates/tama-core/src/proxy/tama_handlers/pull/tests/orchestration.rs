use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::helpers::ENV_GUARD;
use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::pull_queue::PullQueueService;
use crate::proxy::tama_handlers::{start_pull_from_queue, QuantPullSpec};
use crate::proxy::ProxyState;

const REPO: &str = "test/repo";
const FILE: &str = "repo-Q4_K_M.gguf";

/// Create a ProxyState wired to a wiremock server, with an in-memory PullJob seeded.
///
/// Each test acquires ENV_GUARD itself and sets `HF_ENDPOINT` before calling this
/// helper. This helper sets `XDG_CONFIG_HOME` and `HOME` so model cards never
/// land in a real home directory (Linux uses XDG, macOS uses `$HOME`).
///
/// Returns `(state, models_tmp, xdg_tmp, job_id)`.
async fn create_pull_state(
    _server: &wiremock::MockServer,
) -> (
    Arc<ProxyState>,
    tempfile::TempDir,
    tempfile::TempDir,
    String,
) {
    let models_tmp = tempfile::tempdir().unwrap();
    let xdg_tmp = tempfile::tempdir().unwrap();

    std::env::set_var("XDG_CONFIG_HOME", xdg_tmp.path().to_str().unwrap());
    std::env::set_var("HOME", xdg_tmp.path().to_str().unwrap());

    let mut config = crate::config::Config::default();
    config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());

    // Reuse models_tmp as the DB directory (same dir used by create_test_state)
    let db_dir = models_tmp.path().to_path_buf();
    let mgr = crate::models::ModelManager::open(&db_dir).unwrap();
    let svc = PullQueueService::new(mgr, 2);

    let mut state = ProxyState::new(config, Some(db_dir));
    state.set_pull_queue(Some(Arc::new(svc)));

    // Seed the in-memory job — start_pull_from_queue early-returns with "Job not found"
    // if it's absent from pull_jobs.
    let job_id = uuid::Uuid::new_v4().to_string();
    state.pull_jobs.write().await.insert(
        job_id.clone(),
        PullJob {
            job_id: job_id.clone(),
            repo_id: REPO.to_string(),
            filename: FILE.to_string(),
            status: PullJobStatus::Pending,
            ..Default::default()
        },
    );

    (Arc::new(state), models_tmp, xdg_tmp, job_id)
}

/// Compute SHA-256 hex digest of raw bytes.
fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

// ── Test 1: hash mismatch → Failed + file deleted ───────────────────────────

/// Verifies that when the downloaded file's SHA-256 does not match the
/// upstream LFS hash, `run_verification` deletes the file and marks the
/// job as Failed with a "hash mismatch" error.
#[tokio::test]
async fn test_pull_hash_mismatch_fails_job_and_deletes_file() {
    // --- Prepare mismatched hash ---
    let body: &[u8] = b"corrupt gguf bytes";
    // Intentionally use a DIFFERENT SHA — the one for "different content", not `body`.
    let sha = sha256_hex(b"different content");

    // --- Mount wiremock mocks ---
    let server = wiremock::MockServer::start().await;

    // Set HF_ENDPOINT under guard (drop before .await to avoid await_holding_lock)
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    // 1. Blob metadata (used by run_verification to get expected SHA)
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/models/test/repo"))
        .and(wiremock::matchers::query_param("blobs", "true"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "siblings": [{
                    "rfilename": FILE,
                    "blobId": "b1",
                    "size": body.len(),
                    "lfs": { "sha256": sha }
                }]
            })),
        )
        .mount(&server)
        .await;

    // 2. HEAD — provides content-length, NO accept-ranges → single-stream path
    wiremock::Mock::given(wiremock::matchers::method("HEAD"))
        .and(wiremock::matchers::path(
            "/test/repo/resolve/main/repo-Q4_K_M.gguf",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-length", body.len().to_string()),
        )
        .mount(&server)
        .await;

    // 3. GET — serves the corrupt file body
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/test/repo/resolve/main/repo-Q4_K_M.gguf",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(&server)
        .await;

    // --- Execute pull ---
    let orig_home = std::env::var("HOME").ok();
    let (state, models_tmp, _xdg_tmp, job_id) = create_pull_state(&server).await;

    // Enqueue a DB queue row — start_pull_from_queue calls svc.update_status()
    // which requires the row to already exist.
    let svc = state.pull_queue.as_ref().expect("pull_queue should be set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .unwrap();

    // Verify the queue row was created (pre-condition for start_pull_from_queue).
    assert!(
        svc.test_model_mgr()
            .queue_get_by_job_id(&job_id)
            .unwrap()
            .is_some(),
        "queue row should exist after enqueue"
    );

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        QuantPullSpec {
            filename: FILE.into(),
            quant: Some("Q4_K_M".into()),
            context_length: None,
        },
    )
    .await;

    // Restore env vars before assertions (so panics don't leak redirected paths)
    std::env::remove_var("HF_ENDPOINT");
    std::env::remove_var("XDG_CONFIG_HOME");
    match orig_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }

    // --- Assertions ---
    let jobs = state.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job should exist");
    assert_eq!(
        job.status,
        PullJobStatus::Failed,
        "job status should be Failed on hash mismatch"
    );
    assert!(
        job.verify_error
            .as_deref()
            .unwrap_or_default()
            .contains("hash mismatch"),
        "verify_error should mention 'hash mismatch', got: {:?}",
        job.verify_error
    );

    // File should have been deleted by run_verification
    let file_path = models_tmp.path().join(REPO).join(FILE);
    assert!(
        !file_path.exists(),
        "corrupt file should be deleted at {}",
        file_path.display()
    );

    // DB queue row should reflect failure
    let svc = state.pull_queue.as_ref().expect("pull_queue should be set");
    let db_row = svc.test_model_mgr().queue_get_by_job_id(&job_id).unwrap();
    assert!(db_row.is_some(), "DB queue row should exist");
    assert_eq!(
        db_row.unwrap().status,
        "failed",
        "DB queue status should be 'failed'"
    );
}

// ── Test 2: matching hash → Completed + model_files recorded ────────────────

/// Verifies that when the downloaded file's SHA-256 matches the upstream LFS
/// hash, `run_verification` succeeds and `start_pull_from_queue` completes
/// the job, writes a model_files row, and persists the model card.
#[tokio::test]
async fn test_pull_success_completes_and_records_model_files() {
    // --- Prepare matching hash ---
    let body: &[u8] = b"fake but consistent gguf bytes";
    let sha = sha256_hex(body); // matches the actual file content

    // --- Mount wiremock mocks ---
    let server = wiremock::MockServer::start().await;

    // Set HF_ENDPOINT under guard (drop before .await to avoid await_holding_lock)
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    // 1. Blob metadata (used by run_verification to get expected SHA)
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/models/test/repo"))
        .and(wiremock::matchers::query_param("blobs", "true"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "siblings": [{
                    "rfilename": FILE,
                    "blobId": "b1",
                    "size": body.len(),
                    "lfs": { "sha256": sha }
                }]
            })),
        )
        .mount(&server)
        .await;

    // 2. HEAD — provides content-length, NO accept-ranges → single-stream path
    wiremock::Mock::given(wiremock::matchers::method("HEAD"))
        .and(wiremock::matchers::path(
            "/test/repo/resolve/main/repo-Q4_K_M.gguf",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-length", body.len().to_string()),
        )
        .mount(&server)
        .await;

    // 3. GET — serves the file body
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/test/repo/resolve/main/repo-Q4_K_M.gguf",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(&server)
        .await;

    // --- Execute pull ---
    let orig_home = std::env::var("HOME").ok();
    let (state, models_tmp, xdg_tmp, job_id) = create_pull_state(&server).await;

    // Enqueue a DB queue row — start_pull_from_queue calls svc.update_status()
    // which requires the row to already exist.
    let svc = state.pull_queue.as_ref().expect("pull_queue should be set");
    svc.enqueue(&job_id, REPO, FILE, None, "model", Some("Q4_K_M"), None)
        .unwrap();

    // Verify the queue row was created (pre-condition for start_pull_from_queue).
    assert!(
        svc.test_model_mgr()
            .queue_get_by_job_id(&job_id)
            .unwrap()
            .is_some(),
        "queue row should exist after enqueue"
    );

    start_pull_from_queue(
        state.clone(),
        job_id.clone(),
        REPO.into(),
        FILE.into(),
        QuantPullSpec {
            filename: FILE.into(),
            quant: Some("Q4_K_M".into()),
            context_length: None,
        },
    )
    .await;

    // Restore env vars before assertions (so panics don't leak redirected paths)
    std::env::remove_var("HF_ENDPOINT");
    std::env::remove_var("XDG_CONFIG_HOME");
    match orig_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }

    // --- Assertions ---

    // 1. Job should be Completed with verified_ok == Some(true)
    let jobs = state.pull_jobs.read().await;
    let job = jobs.get(&job_id).expect("job should exist");
    assert_eq!(
        job.status,
        PullJobStatus::Completed,
        "job status should be Completed on hash match"
    );
    assert_eq!(
        job.verified_ok,
        Some(true),
        "verified_ok should be true on success"
    );

    // 2. File should exist at models_dir/test/repo/repo-Q4_K_M.gguf
    let file_path = models_tmp.path().join(REPO).join(FILE);
    assert!(
        file_path.exists(),
        "downloaded file should exist at {}",
        file_path.display()
    );

    // 3. DB should have exactly 1 model_files row with filename == FILE
    let mgr = state.model_mgr().expect("model_mgr should be available");
    let files = mgr.get_all_files().unwrap();
    assert_eq!(
        files.len(),
        1,
        "should have exactly 1 model_files row, got {}",
        files.len()
    );
    assert_eq!(files[0].filename, FILE);

    // 4. Model card should exist under redirected config dir:
    //    XDG_CONFIG_HOME/tama/configs/test--repo.toml
    let card_path = xdg_tmp
        .path()
        .join("tama")
        .join("configs")
        .join("test--repo.toml");
    assert!(
        card_path.exists(),
        "model card should exist at {}",
        card_path.display()
    );

    // 5. DB queue row should be "completed"
    let svc = state.pull_queue.as_ref().expect("pull_queue should be set");
    let db_row = svc.test_model_mgr().queue_get_by_job_id(&job_id).unwrap();
    assert!(db_row.is_some(), "DB queue row should exist");
    assert_eq!(
        db_row.unwrap().status,
        "completed",
        "DB queue status should be 'completed'"
    );
}
