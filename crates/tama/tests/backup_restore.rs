//! End-to-end backup/restore tests for the v3 format (plan-190 Task 9):
//! the archive contains only `manifest.json` (built from Postgres) +
//! config cards; restore = extract → merge model cards → done.

mod common;

use std::sync::Arc;
use tower::ServiceExt;

/// Seed one pulled model (config + pull + file) and one active backend.
async fn seed_postgres(pool: &sqlx::PgPool) {
    let mc = tama_core::config::ModelConfig {
        backend: "llama_cpp".to_string(),
        model: Some("source/repo".to_string()),
        ..Default::default()
    };
    let key = tama_core::models::ConfigKey::from_repo_id("source/repo");
    let model_id = tama_core::db::save_model_config(pool, key.as_str(), &mc)
        .await
        .unwrap();
    tama_core::db::queries::upsert_model_pull(pool, model_id, "source/repo", "abc123")
        .await
        .unwrap();
    tama_core::db::queries::upsert_model_file(
        pool,
        model_id,
        "source/repo",
        "model.gguf",
        Some("Q4_K_M"),
        None,
        Some(1000),
    )
    .await
    .unwrap();
    tama_core::db::queries::insert_installation(
        pool,
        &tama_core::db::queries::InstallationRecord {
            id: 0,
            name: "llama_cpp".to_string(),
            backend_type: "llama_cpp".to_string(),
            version: "v1.0".to_string(),
            path: "/tmp/llama".to_string(),
            installed_at: 1234567890,
            gpu_variant: "cpu".to_string(),
            source: Some("prebuilt".to_string()),
            is_active: true,
            docker_config: None,
            logical_id: String::new(),
        },
    )
    .await
    .unwrap();
}

/// The v3 backup archive is manifest (from Postgres) + config cards only:
/// no `tama.db` entry, and the manifest lists match the DB rows.
#[tokio::test]
async fn test_backup_archive_is_manifest_and_cards_only() {
    let guard = common::with_schema().await;
    let config_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config_dir.path().join("configs")).unwrap();
    std::fs::write(
        config_dir.path().join("configs").join("card.toml"),
        "[model]\nid = \"source/repo\"\n",
    )
    .unwrap();

    seed_postgres(&guard.pool).await;

    let output = config_dir.path().join("backup.tar.gz");
    let manifest = tama_core::backup::create_backup(&guard.pool, config_dir.path(), &output)
        .await
        .expect("create_backup should succeed");

    // Manifest lists come from Postgres.
    assert_eq!(
        manifest.models.len(),
        1,
        "manifest should list the pulled model"
    );
    assert_eq!(manifest.models[0].repo_id, "source/repo");
    assert_eq!(manifest.models[0].quants, vec!["Q4_K_M".to_string()]);
    assert_eq!(manifest.models[0].total_size_bytes, 1000);
    assert_eq!(
        manifest.backends.len(),
        1,
        "manifest should list the active backend"
    );
    assert_eq!(manifest.backends[0].name, "llama_cpp");
    assert_eq!(manifest.sha256.len(), 64, "sha256 should be hex");

    // The archive must not contain a database entry.
    let extract_dir = tempfile::tempdir().unwrap();
    let extracted =
        tama_core::backup::extract_backup(&output, extract_dir.path()).expect("extract");
    assert_eq!(
        extracted.card_paths.len(),
        1,
        "only the card should extract"
    );
    assert!(
        !extract_dir.path().join("tama.db").exists(),
        "v3 archive must not contain tama.db"
    );

    guard.finish().await;
}

/// Restore from a v3 archive works end-to-end: POST /tama/v1/restore → job
/// succeeds, the model card is merged into the target config dir, and the
/// local Postgres schema is untouched (v3 restores never merge DB rows).
#[tokio::test]
async fn test_restore_from_v3_archive_end_to_end() {
    use axum::{body::Body, routing::post, Router};
    use serde_json::json;

    // Source: an isolated schema with one model + one backend.
    let source_guard = common::with_schema().await;
    let source_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(source_dir.path().join("configs")).unwrap();
    std::fs::write(
        source_dir.path().join("configs").join("source_card.toml"),
        "[model]\nid = \"source/repo\"\n",
    )
    .unwrap();
    seed_postgres(&source_guard.pool).await;

    let archive_path = source_dir.path().join("backup.tar.gz");
    tama_core::backup::create_backup(&source_guard.pool, source_dir.path(), &archive_path)
        .await
        .expect("source archive should be created");

    // Target: a fresh, empty schema + config dir.
    let target_guard = common::with_schema().await;
    let target_dir = tempfile::tempdir().unwrap();

    let jobs = Arc::new(tama_web::web_types::JobManager::new());
    let upload_id = "up-e2e".to_string();
    let upload_lock = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
        (
            upload_id.clone(),
            tama_web::web_types::UploadEntry {
                path: archive_path.clone(),
                created_at: chrono::Utc::now(),
            },
        ),
    ])));

    let proxy_state = Arc::new(tama_core::proxy::ProxyState::new(
        tama_core::config::Config::default(),
        Some(target_dir.path().to_path_buf()),
        Arc::new(target_guard.pool.clone()),
    ));
    let web_state = tama_web::web_types::WebState {
        jobs: Some(jobs.clone()),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock,
        db_pool: Arc::new(target_guard.pool.clone()),
        log_filter: None,
        log_status: None,
        log_read: None,
        log_tail: None,
        log_events_tx: Arc::new(tokio::sync::Mutex::new(None)),
    };

    let app = Router::new()
        .route(
            "/tama/v1/restore",
            post(tama_web::api::backup::start_restore),
        )
        .layer(axum::extract::Extension(web_state))
        .with_state(proxy_state);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/tama/v1/restore")
        .header("Content-Type", "application/json")
        .body(Body::from(json!({"upload_id": upload_id}).to_string()))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let job_id = json["job_id"].as_str().expect("job_id").to_string();

    // Wait for the job to finish.
    let mut status = tama_web::web_types::JobStatus::Running;
    for _ in 0..100 {
        if let Some(job) = jobs.get(&job_id).await {
            let state = job.state.read().await;
            if state.status != tama_web::web_types::JobStatus::Running {
                status = state.status;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        status,
        tama_web::web_types::JobStatus::Succeeded,
        "restore job should succeed"
    );

    // The model card was merged into the target config dir.
    let card = target_dir.path().join("configs").join("source_card.toml");
    assert!(card.exists(), "model card should be merged into target");
    assert!(std::fs::read_to_string(&card)
        .unwrap()
        .contains("source/repo"));

    // The target DB was NOT touched by the restore (v3: cards only).
    let pulls = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM model_pulls")
        .fetch_one(&target_guard.pool)
        .await
        .unwrap();
    assert_eq!(pulls, 0, "v3 restore must not merge model_pulls rows");
    let installs = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM provider_installations")
        .fetch_one(&target_guard.pool)
        .await
        .unwrap();
    assert_eq!(
        installs, 0,
        "v3 restore must not merge provider_installations rows"
    );

    // Uploaded archive is cleaned up after the restore.
    assert!(
        !archive_path.exists(),
        "archive should be deleted after restore"
    );

    source_guard.finish().await;
    target_guard.finish().await;
}
