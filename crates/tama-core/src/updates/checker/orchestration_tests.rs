use super::{UpdateChecker, UpdateEvent};
use crate::db::{self, queries};
use crate::installations::{InstallationInfo, InstallationManager, InstallationType};
use crate::models::pull::RemoteGguf;
use tempfile::tempdir;
use wiremock::matchers::{method, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Seed a TtsKokoro backend installation into the config directory's DB.
fn seed_backend(config_dir: &std::path::Path) {
    let mgr = InstallationManager::open(config_dir).unwrap();
    mgr.add_installation(&InstallationInfo {
        name: "tts_kokoro".into(),
        backend_type: InstallationType::TtsKokoro,
        version: "1.0.0".into(),
        path: config_dir.join("tts_kokoro"),
        installed_at: 0,
        gpu_variant: "cpu".into(),
        source: None,
        docker_config: None,
    })
    .unwrap();
}

/// Seed a model into the config directory's DB with the given repo_id, commit SHA,
/// and LFS OID. Returns the auto-assigned model id.
fn seed_model(config_dir: &std::path::Path, repo_id: &str, commit_sha: &str, lfs_oid: &str) -> i64 {
    let open = db::open(config_dir).unwrap();
    let record = queries::ModelConfigRecord {
        id: 0,
        repo_id: repo_id.to_string(),
        display_name: Some("Test Model".to_string()),
        backend: "llama_cpp".into(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: Some("Q4_K_M".into()),
        selected_mmproj: None,
        selected_mtp_model: None,
        context_length: None,
        num_parallel: Some(1),
        kv_unified: false,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        port: None,
        args: None,
        sampling: None,
        modalities: None,
        profile: None,
        api_name: None,
        health_check: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        spec_decoding: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
    };
    let model_id = queries::upsert_model_config(&open.conn, &record).unwrap();

    queries::upsert_model_pull(&open.conn, model_id, repo_id, commit_sha).unwrap();

    queries::upsert_model_file(
        &open.conn,
        model_id,
        repo_id,
        "Test-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some(lfs_oid),
        None,
    )
    .unwrap();

    model_id
}

/// TEST 1: Full pipeline — run_check persists backend and model rows and emits events.
#[tokio::test]
async fn test_run_check_persists_backend_and_model_rows_and_emits_events() {
    let tmp = tempdir().unwrap();
    let config_dir = tmp.path();

    // Seed a TtsKokoro backend and a model with "old" identifiers
    seed_backend(config_dir);
    let _model_id = seed_model(
        config_dir,
        "unsloth/Test-GGUF",
        "aaaaaaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    );

    // Start mock server to simulate HF API
    let server = MockServer::start().await;

    // The hf-hub crate constructs URLs like: {endpoint}/api/models/{repo_id}/revision/main
    // Mock: GET /api/models/unsloth/Test-GGUF/revision/main → returns new commit SHA and siblings
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/models/unsloth/Test-GGUF/revision/.*$"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"sha": "new_commit_sha", "siblings": [{"rfilename": "Test-Q4_K_M.gguf"}]}"#,
        ))
        .mount(&server)
        .await;

    // Mock: GET /api/models/unsloth/Test-GGUF?blobs=true → returns LFS info with new hash
    // lookup_blob_metadata uses {endpoint}/api/models/{repo_id}?blobs=true (custom tama URL).
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/models/unsloth/Test-GGUF\b.*"))
        .and(query_param("blobs", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"siblings": [{"rfilename": "Test-Q4_K_M.gguf", "blobId": "b1", "size": 123, "lfs": {"sha256": "new_lfs_sha256", "size": 123}}]}"#,
        ))
        .mount(&server)
        .await;

    // Point HF_ENDPOINT at our mock server
    std::env::set_var("HF_ENDPOINT", server.uri());

    // Create UpdateChecker with event broadcast channel
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let mut checker = UpdateChecker::new();
    checker.set_update_events_tx(tx);

    // Run the full check
    checker.run_check(config_dir).await.unwrap();

    // ── DB assertions: backend row should be "up_to_date" ───────────────
    let repo = crate::db::repository::Repository::open(config_dir).unwrap();
    let checks = repo.get_all_update_checks().unwrap();

    let backend_check = checks
        .iter()
        .find(|c| c.item_type == "backend")
        .expect("should have a backend check record");
    assert_eq!(backend_check.status, "up_to_date");
    assert!(!backend_check.update_available);

    // ── DB assertions: model row should be "update_available" (different SHA) ──
    let model_check = checks
        .iter()
        .find(|c| c.item_type == "model")
        .expect("should have a model check record");
    assert_eq!(model_check.status, "update_available");
    // The model has a newer commit SHA and LFS hash
    assert!(model_check
        .details_json
        .as_ref()
        .map(|d| d.contains("new_commit_sha"))
        .unwrap_or(false));

    // ── Event assertions: drain broadcast channel and verify emissions ───
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    assert!(
        events.iter().any(|ev| {
            matches!(ev, UpdateEvent::CheckStarted { item_type, .. } if item_type == "backend")
        }),
        "should have CheckStarted{{backend}}"
    );
    assert!(
        events.iter().any(|ev| {
            matches!(ev, UpdateEvent::CheckStarted { item_type, .. } if item_type == "model")
        }),
        "should have CheckStarted{{model}}"
    );
    assert!(
        events.iter().any(|ev| {
            matches!(ev, UpdateEvent::CheckCompleted { item_type, .. } if item_type == "backend")
        }),
        "should have CheckCompleted{{backend}}"
    );
    assert!(
        events.iter().any(|ev| {
            matches!(ev, UpdateEvent::CheckCompleted { item_type, .. } if item_type == "model")
        }),
        "should have CheckCompleted{{model}}"
    );

    std::env::remove_var("HF_ENDPOINT");
}

/// TEST 2: Model update check via cache — no HTTP call needed.
#[tokio::test]
async fn test_run_check_model_up_to_date_via_cache_without_http() {
    let tmp = tempdir().unwrap();
    let config_dir = tmp.path();

    // Seed a model with the same SHA as we'll put in the cache
    let sha_same = "same_commit_sha";
    let lfs_same = "same_lfs_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash";
    seed_model(config_dir, "cached/Model", sha_same, lfs_same);

    // Seed a backend too
    seed_backend(config_dir);

    // Do NOT set HF_ENDPOINT or start wiremock — we want to use the cache.

    // Create checker and pre-seed the gguf_listing_cache so the cache-hit path runs
    let mut checker = UpdateChecker::new();
    let files = vec![RemoteGguf {
        filename: "Test-Q4_K_M.gguf".into(),
        quant: Some("Q4_K_M".into()),
    }];
    // Access private field — allowed because this module is a child of `checker`
    checker
        .gguf_listing_cache
        .insert("cached/Model".into(), sha_same.into(), files, None)
        .await;

    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    checker.set_update_events_tx(tx);

    // Run the full check — should use cache and find model up_to_date
    checker.run_check(config_dir).await.unwrap();

    let repo = crate::db::repository::Repository::open(config_dir).unwrap();
    let checks = repo.get_all_update_checks().unwrap();

    // Model should be up_to_date because commit SHA matches the cached listing
    let model_check = checks
        .iter()
        .find(|c| c.item_type == "model")
        .expect("should have a model check record");
    assert_eq!(model_check.status, "up_to_date");
    assert!(!model_check.update_available);

    // Backend should also be up_to_date (TtsKokoro arm returns None for latest_version)
    let backend_check = checks
        .iter()
        .find(|c| c.item_type == "backend")
        .expect("should have a backend check record");
    assert_eq!(backend_check.status, "up_to_date");
}

/// TEST 3: Model without repo records → status "unknown" via CheckError.
#[tokio::test]
async fn test_run_check_model_without_repo_records_unknown() {
    let tmp = tempdir().unwrap();
    let config_dir = tmp.path();

    // Seed a model with an empty repo_id to trigger the "no source repo" path
    let open = db::open(config_dir).unwrap();
    let record = queries::ModelConfigRecord {
        id: 0,
        repo_id: "".to_string(), // empty repo_id triggers the unknown path
        display_name: Some("Test Model".to_string()),
        backend: "llama_cpp".into(),
        gpu_variant: None,
        gpu_device: None,
        enabled: true,
        selected_quant: Some("Q4_K_M".into()),
        selected_mmproj: None,
        selected_mtp_model: None,
        context_length: None,
        num_parallel: Some(1),
        kv_unified: false,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        port: None,
        args: None,
        sampling: None,
        modalities: None,
        profile: None,
        api_name: None,
        health_check: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        spec_decoding: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        n_batch: None,
        n_ubatch: None,
        vllm_config: None,
        provider_name: None,
    };
    let model_id = queries::upsert_model_config(&open.conn, &record).unwrap();

    // Create checker with event channel to capture events
    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    let mut checker = UpdateChecker::new();
    checker.set_update_events_tx(tx);

    // Call check_model directly — repo_id is None/empty → should go to unknown path
    checker
        .check_model(config_dir, model_id, None)
        .await
        .unwrap();

    let repo = crate::db::repository::Repository::open(config_dir).unwrap();
    let checks = repo.get_all_update_checks().unwrap();

    // Should have a record for this model with status "unknown"
    let model_check = checks
        .iter()
        .find(|c| c.item_type == "model")
        .expect("should have a model check record");
    assert_eq!(model_check.status, "unknown");
}

/// TEST 4: Concurrent invocation — acquiring the lock first causes run_check to skip.
#[tokio::test]
async fn test_run_check_concurrent_invocation_skips() {
    let tmp = tempdir().unwrap();
    let config_dir = tmp.path();

    // Seed a backend so there's something to check
    seed_backend(config_dir);

    let checker = UpdateChecker::new();

    // Manually acquire the lock to simulate a concurrent check in progress
    let _lock_guard = checker.lock.try_lock().unwrap();

    // Now run_check should return Ok(()) immediately without doing any work
    checker.run_check(config_dir).await.unwrap();

    // The DB should have no check records since the check was skipped
    let repo = crate::db::repository::Repository::open(config_dir).unwrap();
    let checks = repo.get_all_update_checks().unwrap();
    assert!(
        checks.is_empty(),
        "no check records should exist when run_check is skipped"
    );
}
