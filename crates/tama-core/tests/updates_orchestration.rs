//! Postgres port of the update checker orchestration tests (plan-190, Task 4).
//!
//! The `update_checks` rows now live in Postgres (testcontainer harness), so
//! the assertions read them through the async query functions. Backend and
//! model seed data lives in Postgres (Tasks 8 and 5).
//!
//! The event-channel assertions require the `web-ui` feature (same gate as
//! the former in-file tests).

#![cfg(feature = "web-ui")]

mod common;

use common::with_schema;
use tama_core::db::queries;
use tama_core::installations::{InstallationInfo, InstallationManager, InstallationType};
use tama_core::models::pull::RemoteGguf;
use tama_core::updates::checker::{UpdateChecker, UpdateEvent};
use tempfile::tempdir;
use wiremock::matchers::{method, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Seed a TtsKokoro backend installation into Postgres.
async fn seed_backend(pool: &sqlx::PgPool, path_base: &std::path::Path) {
    let mgr = InstallationManager::new(std::sync::Arc::new(pool.clone()));
    mgr.add_installation(&InstallationInfo {
        name: "tts_kokoro".into(),
        backend_type: InstallationType::TtsKokoro,
        version: "1.0.0".into(),
        path: path_base.join("tts_kokoro"),
        installed_at: 0,
        gpu_variant: "cpu".into(),
        source: None,
        docker_config: None,
    })
    .await
    .unwrap();
}

/// Seed a model into Postgres with the given repo_id, commit SHA, and LFS OID.
/// Returns the auto-assigned model id.
async fn seed_model(pool: &sqlx::PgPool, repo_id: &str, commit_sha: &str, lfs_oid: &str) -> i64 {
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
        reasoning_levels: None,
    };
    let model_id = queries::upsert_model_config(pool, &record).await.unwrap();

    queries::upsert_model_pull(pool, model_id, repo_id, commit_sha)
        .await
        .unwrap();

    queries::upsert_model_file(
        pool,
        model_id,
        repo_id,
        "Test-Q4_K_M.gguf",
        Some("Q4_K_M"),
        Some(lfs_oid),
        None,
    )
    .await
    .unwrap();

    model_id
}

/// TEST 1: Full pipeline — run_check persists backend and model rows and emits events.
#[tokio::test]
async fn test_run_check_persists_backend_and_model_rows_and_emits_events() {
    let guard = with_schema().await;
    let tmp = tempdir().unwrap();
    let config_dir = tmp.path();

    // Seed a TtsKokoro backend and a model with "old" identifiers
    seed_backend(&guard.pool, config_dir).await;
    let _model_id = seed_model(
        &guard.pool,
        "unsloth/Test-GGUF",
        "aaaaaaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .await;

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
    checker.run_check(&guard.pool).await.unwrap();

    // ── DB assertions: backend row should be "up_to_date" ───────────────
    let checks = queries::get_all_update_checks(&guard.pool).await.unwrap();

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

    guard.finish().await;
}

/// TEST 2: Model update check via cache — no HTTP call needed.
#[tokio::test]
async fn test_run_check_model_up_to_date_via_cache_without_http() {
    let guard = with_schema().await;
    let tmp = tempdir().unwrap();
    let config_dir = tmp.path();

    // Seed a model with the same SHA as we'll put in the cache
    let sha_same = "same_commit_sha";
    let lfs_same = "same_lfs_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash_hash";
    seed_model(&guard.pool, "cached/Model", sha_same, lfs_same).await;

    // Seed a backend too
    seed_backend(&guard.pool, config_dir).await;

    // Do NOT set HF_ENDPOINT or start wiremock — we want to use the cache.

    // Create checker and pre-seed the gguf_listing_cache so the cache-hit path runs
    let mut checker = UpdateChecker::new();
    let files = vec![RemoteGguf {
        filename: "Test-Q4_K_M.gguf".into(),
        quant: Some("Q4_K_M".into()),
    }];
    checker
        .seed_gguf_listing_cache("cached/Model".into(), sha_same.into(), files, None)
        .await;

    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    checker.set_update_events_tx(tx);

    // Run the full check — should use cache and find model up_to_date
    checker.run_check(&guard.pool).await.unwrap();

    let checks = queries::get_all_update_checks(&guard.pool).await.unwrap();

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

    guard.finish().await;
}

/// TEST 3: Model without repo records → status "unknown" via CheckError.
#[tokio::test]
async fn test_run_check_model_without_repo_records_unknown() {
    let guard = with_schema().await;

    // Seed a model with an empty repo_id to trigger the "no source repo" path
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
        reasoning_levels: None,
    };
    let model_id = queries::upsert_model_config(&guard.pool, &record)
        .await
        .unwrap();

    // Create checker with event channel to capture events
    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    let mut checker = UpdateChecker::new();
    checker.set_update_events_tx(tx);

    // Call check_model directly — repo_id is None/empty → should go to unknown path
    checker
        .check_model(&guard.pool, model_id, None)
        .await
        .unwrap();

    let checks = queries::get_all_update_checks(&guard.pool).await.unwrap();

    // Should have a record for this model with status "unknown"
    let model_check = checks
        .iter()
        .find(|c| c.item_type == "model")
        .expect("should have a model check record");
    assert_eq!(model_check.status, "unknown");

    guard.finish().await;
}

/// TEST 4: Concurrent invocation — acquiring the lock first causes run_check to skip.
#[tokio::test]
async fn test_run_check_concurrent_invocation_skips() {
    let guard = with_schema().await;
    let tmp = tempdir().unwrap();
    let config_dir = tmp.path();

    // Seed a backend so there's something to check
    seed_backend(&guard.pool, config_dir).await;

    let checker = UpdateChecker::new();

    // Manually acquire the lock to simulate a concurrent check in progress
    let _lock_guard = checker.try_hold_run_lock().unwrap();

    // Now run_check should return Ok(()) immediately without doing any work
    checker.run_check(&guard.pool).await.unwrap();

    // The DB should have no check records since the check was skipped
    let checks = queries::get_all_update_checks(&guard.pool).await.unwrap();
    assert!(
        checks.is_empty(),
        "no check records should exist when run_check is skipped"
    );

    guard.finish().await;
}
