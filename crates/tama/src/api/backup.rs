//! Backup and restore API endpoints.

use crate::api::error::error_response;
use crate::web_types::WebState;
use anyhow::Context;
use axum::{
    extract::{Extension, Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tama_core::proxy::ProxyState;
use uuid::Uuid;

/// Request body for restore preview.
#[derive(Deserialize)]
pub struct RestorePreviewRequest {
    pub upload_id: String,
}

/// Response body for restore preview.
#[derive(Serialize)]
pub struct RestorePreviewResponse {
    pub upload_id: String,
    pub created_at: String,
    pub tama_version: String,
    pub models: Vec<BackupModelEntry>,
    pub backends: Vec<BackendEntry>,
}

/// Request body for restore.
#[derive(Deserialize)]
pub struct RestoreRequest {
    pub upload_id: String,
    #[serde(default)]
    pub selected_models: Option<Vec<String>>,
    #[serde(default)]
    pub skip_backends: bool,
    #[serde(default)]
    pub skip_models: bool,
}

/// Response body for restore.
#[derive(Serialize)]
pub struct RestoreResponse {
    pub job_id: String,
}

/// Model entry for backup manifest.
#[derive(Serialize, Clone)]
pub struct BackupModelEntry {
    pub repo_id: String,
    pub quants: Vec<String>,
    pub total_size_bytes: i64,
}

/// Backend entry for backup manifest.
#[derive(Serialize, Clone)]
pub struct BackendEntry {
    pub name: String,
    pub version: String,
    pub backend_type: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub docker_config: Option<String>,
}

/// GET /tama/v1/backup - Create backup and return as file download
pub async fn create_backup(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let config_dir: std::path::PathBuf = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    // Spawn blocking task for backup
    let result = tokio::task::spawn_blocking(move || {
        let temp_dir = tempfile::tempdir().map_err(|e| anyhow::anyhow!(e))?;
        let output_path = temp_dir.path().join("backup.tar.gz");

        let manifest = tama_core::backup::create_backup(&config_dir, &output_path)
            .map_err(|e| anyhow::anyhow!(e))?;

        let size = std::fs::metadata(&output_path)
            .map(|m| m.len())
            .unwrap_or(0);

        // Read file inside blocking task to avoid blocking async runtime
        let file_bytes = std::fs::read(&output_path).map_err(|e| anyhow::anyhow!(e))?;

        let filename = output_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        Ok::<_, anyhow::Error>((file_bytes, filename, manifest, size))
    })
    .await;

    match result {
        Ok(Ok((file_bytes, filename, _manifest, _size))) => {
            let disposition = format!("attachment; filename=\"{}\"", filename);

            (
                StatusCode::OK,
                [
                    ("Content-Type", "application/gzip"),
                    ("Content-Disposition", disposition.as_str()),
                ],
                file_bytes,
            )
                .into_response()
        }
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// POST /tama/v1/restore/preview - Upload archive and return manifest preview
pub async fn restore_preview(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Save upload to temp file
    let config_dir =
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::env::temp_dir());
    let temp_dir = config_dir.join("uploads");
    if let Err(e) = std::fs::create_dir_all(&temp_dir) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create temp directory: {}", e),
            None,
        );
    }

    // TODO: Prune stale uploads (older than N hours) on startup or via periodic task
    let upload_id = Uuid::new_v4().simple().to_string();
    let upload_path = temp_dir.join(format!("{}.tar.gz", upload_id));

    let mut uploaded = false;
    while let Ok(Some(field)) = multipart.next_field().await {
        let bytes = match field.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Failed to read upload: {}", e),
                    Some("ValidationError"),
                )
            }
        };
        if let Err(e) = std::fs::write(&upload_path, &bytes) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to write upload: {}", e),
                None,
            );
        }
        uploaded = true;
    }

    if !uploaded {
        return error_response(
            StatusCode::BAD_REQUEST,
            "No file uploaded",
            Some("ValidationError"),
        );
    }

    // Extract manifest
    let upload_path_clone = upload_path.clone();
    let manifest_result = tokio::task::spawn_blocking(move || {
        tama_core::backup::extract_manifest(&upload_path_clone)
    })
    .await;

    match manifest_result {
        Ok(Ok(manifest)) => {
            // Store upload reference
            let upload_lock = web_state.upload_lock.clone();
            let mut uploads = upload_lock.write().await;
            uploads.insert(
                upload_id.clone(),
                UploadEntry {
                    path: upload_path.clone(),
                    created_at: chrono::Utc::now(),
                },
            );

            Json(RestorePreviewResponse {
                upload_id,
                created_at: manifest.created_at,
                tama_version: manifest.tama_version,
                models: manifest
                    .models
                    .into_iter()
                    .map(|m| BackupModelEntry {
                        repo_id: m.repo_id,
                        quants: m.quants,
                        total_size_bytes: m.total_size_bytes,
                    })
                    .collect(),
                backends: manifest
                    .backends
                    .into_iter()
                    .map(|b| BackendEntry {
                        name: b.name,
                        version: b.version,
                        backend_type: b.backend_type,
                        source: b.source,
                        docker_config: b.docker_config,
                    })
                    .collect(),
            })
            .into_response()
        }
        Ok(Err(e)) => error_response(
            StatusCode::BAD_REQUEST,
            e.to_string(),
            Some("ValidationError"),
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// Perform a full additive restore of a backup archive into the config directory.
///
/// Extraction and validation are atomic (all-or-nothing in the temp dir); the merge
/// phase is additive and best-effort — a failure mid-merge may leave partially-applied
/// changes.
///
/// `selected_models`, `skip_backends`, and `skip_models` from [`RestoreRequest`]
/// are accepted at the API level but NOT applied here — restore v1 always
/// performs the full additive merge.
fn run_restore(
    config_dir: &std::path::Path,
    archive_path: &std::path::Path,
) -> anyhow::Result<(tama_core::config::Config, String)> {
    let extract_dir = tempfile::tempdir().context("Failed to create restore temp dir")?;
    let extracted = tama_core::backup::extract_backup(archive_path, extract_dir.path())
        .context("Failed to extract backup archive")?;

    let copied_cards = tama_core::backup::merge_model_cards(
        &config_dir.join("configs"),
        &extract_dir.path().join("configs"),
    )
    .context("Failed to merge model cards")?;

    let open = tama_core::db::open(config_dir).context("Failed to open local database")?;
    let db_stats = tama_core::backup::merge_database(&open.conn, &extracted.db_path)
        .context("Failed to merge database")?;

    let db_path = config_dir.join("tama.db");
    let mut local =
        tama_core::config::Config::load_from(&db_path).context("Failed to load local config")?;
    let backup_cfg = tama_core::config::Config::load_from(&extracted.db_path)
        .context("Failed to load backup config")?;
    let cfg_stats = tama_core::backup::merge_config(&mut local, &backup_cfg);
    local
        .to_db(&db_path)
        .context("Failed to save merged config")?;

    let summary = format!(
        concat!(
            "Restored {} model card(s), {} new model pull(s), {} new model file(s), ",
            "{} new backend installation(s), {} new backend config(s), ",
            "{} skipped backend config(s), {} model(s) in manifest.\n",
            "Full merge performed; selected_models/skip_backends/skip_models are ",
            "accepted but not yet applied.",
        ),
        copied_cards.len(),
        db_stats.new_model_pulls,
        db_stats.new_model_files,
        db_stats.new_backend_installations,
        cfg_stats.new_backends.len(),
        cfg_stats.skipped_backends.len(),
        extracted.manifest.models.len(),
    );

    Ok((local, summary))
}

/// POST /tama/v1/restore - Start restore job
pub async fn start_restore(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Json(body): Json<RestoreRequest>,
) -> impl IntoResponse {
    // Look up upload
    let upload_lock = web_state.upload_lock.clone();
    let upload_path = {
        let uploads = upload_lock.read().await;
        uploads.get(&body.upload_id).map(|entry| entry.path.clone())
    };

    let upload_path = match upload_path {
        Some(path) => path,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                "Upload not found or expired",
                Some("NotFoundError"),
            )
        }
    };

    // Resolve config directory (same resolution as create_backup, but with a
    // 500 error when Config::config_dir() fails and there is no db_dir).
    let config_dir: std::path::PathBuf = match state.db_dir() {
        Some(dir) => dir.clone(),
        None => match tama_core::config::Config::config_dir() {
            Ok(dir) => dir,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to resolve config directory: {}", e),
                    None,
                )
            }
        },
    };

    // Submit restore job
    let jobs = match web_state.jobs.as_ref() {
        Some(j) => j.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "job manager not available",
                None,
            )
        }
    };

    let job = match jobs.submit(crate::web_types::JobKind::Restore, None).await {
        Ok(j) => j,
        Err(crate::web_types::JobError::AlreadyRunning(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                "another restore job is already running",
                Some("ConflictError"),
            )
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to create restore job: {}", e),
                None,
            )
        }
    };

    let job_id = job.id.clone();

    let jobs_for_spawn = jobs.clone();
    let job_for_spawn = job.clone();
    let state_for_spawn = state.clone();
    let cleanup_path = upload_path.clone();
    tokio::spawn(async move {
        jobs_for_spawn
            .append_log(
                &job_for_spawn,
                "Extracting and validating backup archive".to_string(),
            )
            .await;
        let config_dir_for_blocking = config_dir.clone();
        let archive_for_blocking = upload_path.clone();
        let result = tokio::task::spawn_blocking(move || {
            run_restore(&config_dir_for_blocking, &archive_for_blocking)
        })
        .await;
        match result {
            Ok(Ok((merged_config, summary))) => {
                state_for_spawn
                    .with_config_mut(|c| *c = merged_config)
                    .await;
                jobs_for_spawn.append_log(&job_for_spawn, summary).await;
                jobs_for_spawn
                    .finish(&job_for_spawn, crate::web_types::JobStatus::Succeeded, None)
                    .await;
            }
            Ok(Err(e)) => {
                tracing::error!("Restore job {} failed: {:#}", job_for_spawn.id, e);
                jobs_for_spawn
                    .finish(
                        &job_for_spawn,
                        crate::web_types::JobStatus::Failed,
                        Some(format!("{:#}", e)),
                    )
                    .await;
            }
            Err(join_err) => {
                tracing::error!("Restore task panicked: {:?}", join_err);
                jobs_for_spawn
                    .finish(
                        &job_for_spawn,
                        crate::web_types::JobStatus::Failed,
                        Some(format!("Restore task panicked: {}", join_err)),
                    )
                    .await;
            }
        }
        // Clean up uploaded archive after restore completes.
        let cleanup_path = cleanup_path.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = std::fs::remove_file(&cleanup_path) {
                tracing::warn!("Failed to delete upload file: {}", e);
            }
        })
        .await
        .ok();
    });

    Json(RestoreResponse { job_id }).into_response()
}

/// Re-export from local web_types for backward compatibility.
pub use crate::web_types::UploadEntry;

#[cfg(test)]
mod tests {
    use super::*;

    // ── Shared test helpers ───────────────────────────────────────────────

    /// Create a minimal WebState for route tests.
    fn test_web_state() -> crate::web_types::WebState {
        use std::collections::HashMap;
        crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            repository: None,
        }
    }

    /// Seed a config directory with a minimal `tama.db` for backup tests.
    ///
    /// Uses `tama_core::db::open` so migrations run and the schema is fully
    /// compatible with the local DB (required for merge_database to work).
    fn seed_config_dir(dir: &std::path::Path) {
        use tama_core::db;

        std::fs::create_dir_all(dir.join("configs")).expect("create configs dir");

        let open = db::open(dir).expect("open db");
        // Seed defaults so Config::load_from works.
        tama_core::db::queries::seed_defaults(&open.conn).expect("seed defaults");

        // Insert a model_config first (model_pulls has FK to model_configs).
        open.conn
            .execute(
                "INSERT INTO model_configs (repo_id, backend) VALUES ('test/repo', 'llama_cpp')",
                [],
            )
            .expect("insert model_config");
        let model_id: i64 = open
            .conn
            .query_row(
                "SELECT id FROM model_configs WHERE repo_id = 'test/repo'",
                [],
                |row| row.get(0),
            )
            .expect("get model_id");
        open.conn
            .execute(
                "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
                 VALUES (?1, 'test/repo', 'abc123', '2024-01-01T00:00:00Z')",
                [model_id],
            )
            .expect("insert model pull");
    }

    /// Build a test app router with the given ProxyState and WebState.
    fn test_app(
        state: Arc<ProxyState>,
        web_state: &Arc<crate::web_types::WebState>,
    ) -> axum::Router {
        crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()))
    }

    // ── RestorePreviewRequest tests ───────────────────────────────────────

    #[test]
    fn test_restore_preview_request_fields() {
        let req = RestorePreviewRequest {
            upload_id: "upload-abc123".to_string(),
        };

        assert_eq!(req.upload_id, "upload-abc123");
    }

    #[test]
    fn test_restore_preview_request_empty_upload_id() {
        let req = RestorePreviewRequest {
            upload_id: String::new(),
        };

        assert!(req.upload_id.is_empty());
    }

    // ── RestorePreviewResponse tests ──────────────────────────────────────

    #[test]
    fn test_restore_preview_response_fields() {
        let response = RestorePreviewResponse {
            upload_id: "upload-abc123".to_string(),
            created_at: "2026-04-18T10:00:00Z".to_string(),
            tama_version: "1.26.2".to_string(),
            models: vec![BackupModelEntry {
                repo_id: "unsloth/Qwen3.5-35B-A3B-GGUF".to_string(),
                quants: vec!["Q4_K_M".to_string(), "Q8_0".to_string()],
                total_size_bytes: 20_000_000,
            }],
            backends: vec![BackendEntry {
                name: "llama-cpp".to_string(),
                version: "b8407".to_string(),
                backend_type: "llama_cpp".to_string(),
                source: Some("prebuilt".to_string()),
                docker_config: None,
            }],
        };

        assert_eq!(response.upload_id, "upload-abc123");
        assert_eq!(response.tama_version, "1.26.2");
        assert_eq!(response.models.len(), 1);
        assert_eq!(response.backends.len(), 1);
        assert_eq!(response.models[0].repo_id, "unsloth/Qwen3.5-35B-A3B-GGUF");
    }

    #[test]
    fn test_restore_preview_response_empty() {
        let response = RestorePreviewResponse {
            upload_id: "upload-empty".to_string(),
            created_at: "2026-04-18T10:00:00Z".to_string(),
            tama_version: "1.26.2".to_string(),
            models: vec![],
            backends: vec![],
        };

        assert!(response.models.is_empty());
        assert!(response.backends.is_empty());
        assert_eq!(response.upload_id, "upload-empty");
    }

    // ── RestoreRequest tests ──────────────────────────────────────────────

    #[test]
    fn test_restore_request_fields() {
        let req = RestoreRequest {
            upload_id: "upload-abc123".to_string(),
            selected_models: None,
            skip_backends: true,
            skip_models: false,
        };

        assert_eq!(req.upload_id, "upload-abc123");
        assert!(!req.skip_models);
        assert!(req.skip_backends);
    }

    #[test]
    fn test_restore_request_all_skipped() {
        let req = RestoreRequest {
            upload_id: "upload-abc123".to_string(),
            selected_models: None,
            skip_backends: true,
            skip_models: true,
        };

        assert!(req.skip_models);
        assert!(req.skip_backends);
    }

    #[test]
    fn test_restore_request_with_selected_models() {
        let req = RestoreRequest {
            upload_id: "upload-abc123".to_string(),
            selected_models: Some(vec!["model1.gguf".to_string(), "model2.gguf".to_string()]),
            skip_backends: false,
            skip_models: false,
        };

        assert_eq!(req.selected_models.as_ref().unwrap().len(), 2);
        assert!(!req.skip_models);
    }

    // ── RestoreResponse tests ─────────────────────────────────────────────

    #[test]
    fn test_restore_response_fields() {
        let response = RestoreResponse {
            job_id: "restore-job-123".to_string(),
        };

        assert_eq!(response.job_id, "restore-job-123");
    }

    // ── BackupModelEntry tests ────────────────────────────────────────────

    #[test]
    fn test_backup_model_entry_fields() {
        let entry = BackupModelEntry {
            repo_id: "unsloth/Qwen3.5-35B-A3B-GGUF".to_string(),
            quants: vec!["Q4_K_M".to_string(), "Q8_0".to_string()],
            total_size_bytes: 20_000_000,
        };

        assert_eq!(entry.repo_id, "unsloth/Qwen3.5-35B-A3B-GGUF");
        assert_eq!(entry.quants.len(), 2);
        assert_eq!(entry.total_size_bytes, 20_000_000);
    }

    #[test]
    fn test_backup_model_entry_single_quant() {
        let entry = BackupModelEntry {
            repo_id: "test/model".to_string(),
            quants: vec!["Q4_K_M".to_string()],
            total_size_bytes: 5_000_000,
        };

        assert_eq!(entry.quants.len(), 1);
        assert_eq!(entry.quants[0], "Q4_K_M");
    }

    #[test]
    fn test_backup_model_entry_negative_size() {
        let entry = BackupModelEntry {
            repo_id: "test/model".to_string(),
            quants: vec!["Q4_K_M".to_string()],
            total_size_bytes: -1,
        };

        assert_eq!(entry.total_size_bytes, -1);
    }

    // ── BackendEntry tests ────────────────────────────────────────────────

    #[test]
    fn test_backend_entry_fields() {
        let entry = BackendEntry {
            name: "llama-cpp".to_string(),
            version: "b8407".to_string(),
            backend_type: "llama_cpp".to_string(),
            source: Some("prebuilt".to_string()),
            docker_config: None,
        };

        assert_eq!(entry.name, "llama-cpp");
        assert_eq!(entry.version, "b8407");
        assert_eq!(entry.backend_type, "llama_cpp");
        assert_eq!(entry.source, Some("prebuilt".to_string()));
    }

    #[test]
    fn test_backend_entry_source_build() {
        let entry = BackendEntry {
            name: "llama-cpp".to_string(),
            version: "b8407".to_string(),
            backend_type: "llama_cpp".to_string(),
            source: Some("build".to_string()),
            docker_config: None,
        };

        assert_eq!(entry.source, Some("build".to_string()));
    }

    #[test]
    fn test_run_restore_merges_backup() {
        use tama_core::db;

        // Source: create a properly-migrated config dir with test data.
        let source_dir = tempfile::tempdir().unwrap();
        {
            let open = db::open(source_dir.path()).expect("open source db");
            // Insert a model_config first (model_pulls has a FK to model_configs).
            open.conn
                .execute(
                    "INSERT INTO model_configs (repo_id, backend) VALUES ('test/repo', 'llama_cpp')",
                    [],
                )
                .expect("insert model_config");
            let model_id: i64 = open
                .conn
                .query_row(
                    "SELECT id FROM model_configs WHERE repo_id = 'test/repo'",
                    [],
                    |row| row.get(0),
                )
                .expect("get model_id");
            open.conn
                .execute(
                    "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
                     VALUES (?1, 'test/repo', 'abc123', '2024-01-01T00:00:00Z')",
                    [model_id],
                )
                .expect("insert model pull");
        } // connection dropped — close before create_backup

        // Add a model card so merge_model_cards has something to copy.
        let configs_dir = source_dir.path().join("configs");
        std::fs::create_dir_all(&configs_dir).unwrap();
        std::fs::write(
            configs_dir.join("source_card.toml"),
            "[model]\nid = \"source/card\"\n",
        )
        .unwrap();

        // Create a backup archive from the source.
        let archive_path = source_dir.path().join("backup.tar.gz");
        tama_core::backup::create_backup(source_dir.path(), &archive_path)
            .expect("create_backup should succeed");

        // Target: a fresh, empty config dir.
        let target_dir = tempfile::tempdir().unwrap();

        // Run restore — should merge everything into the target.
        let result = run_restore(target_dir.path(), &archive_path);
        assert!(
            result.is_ok(),
            "run_restore should succeed: {:?}",
            result.err()
        );

        let (_config, summary) = result.unwrap();

        // Summary should mention the full-merge disclaimer and the disclaimer
        // that selected_models/skip_backends/skip_models are not yet applied.
        assert!(
            summary.contains("Full merge performed"),
            "summary should mention full merge, got: {}",
            summary
        );
        assert!(
            summary.contains("selected_models/skip_backends/skip_models"),
            "summary should mention the accepted-but-not-applied disclaimer"
        );
        // The summary should report 1 model card copied from the backup.
        assert!(
            summary.starts_with("Restored 1 model card"),
            "summary should report 1 copied model card, got: {}",
            summary
        );

        // The model card should have been copied to the target configs dir.
        assert!(
            target_dir
                .path()
                .join("configs")
                .join("source_card.toml")
                .exists(),
            "model card should be copied to target"
        );
    }

    #[tokio::test]
    async fn test_start_restore_submits_job_and_returns_200() {
        use axum::{body::Body, extract::Extension, routing::post, Router};
        use serde_json::json;
        use tower::ServiceExt;

        // Config dir with a seeded DB so the restore task has a real DB to open.
        let config_temp = tempfile::tempdir().unwrap();
        seed_config_dir(config_temp.path());

        // Upload dir with a fake archive.
        let upload_temp = tempfile::tempdir().unwrap();
        let upload_id = "up-1".to_string();
        let upload_path = upload_temp.path().join(format!("{}.tar.gz", upload_id));
        std::fs::write(&upload_path, b"fake backup").unwrap();

        let proxy_state = Arc::new(tama_core::proxy::ProxyState::new(
            tama_core::config::Config::default(),
            Some(config_temp.path().to_path_buf()),
        ));

        let web_state = WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "2.0.0".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
                (
                    upload_id.clone(),
                    UploadEntry {
                        path: upload_path.clone(),
                        created_at: chrono::Utc::now(),
                    },
                ),
            ]))),
            repository: None,
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(Extension(web_state))
            .with_state(proxy_state);

        // Case 1: Valid upload_id with jobs available -> 200 with job_id
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": upload_id}).to_string()))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["job_id"].as_str().is_some(),
            "response should contain job_id, got: {}",
            json
        );

        // Give the background restore task a moment to finish (it will fail
        // on the fake archive, but we just want to avoid dangling tasks).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Case 2: Invalid upload_id -> 404
        let req_invalid = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": "unknown"}).to_string()))
            .unwrap();

        let response_invalid = app.oneshot(req_invalid).await.unwrap();
        assert_eq!(response_invalid.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_start_restore_no_job_manager_returns_503() {
        use axum::{body::Body, extract::Extension, routing::post, Router};
        use serde_json::json;
        use tower::ServiceExt;

        let upload_temp = tempfile::tempdir().unwrap();
        let upload_id = "up-no-jobs".to_string();
        let upload_path = upload_temp.path().join(format!("{}.tar.gz", upload_id));
        std::fs::write(&upload_path, b"fake").unwrap();

        let proxy_state = Arc::new(tama_core::proxy::ProxyState::new(
            tama_core::config::Config::default(),
            None,
        ));

        let web_state = WebState {
            jobs: None,
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "2.0.0".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
                (
                    upload_id.clone(),
                    UploadEntry {
                        path: upload_path.clone(),
                        created_at: chrono::Utc::now(),
                    },
                ),
            ]))),
            repository: None,
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(Extension(web_state))
            .with_state(proxy_state);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": upload_id}).to_string()))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "should return 503 when job manager is not configured"
        );
    }

    #[tokio::test]
    async fn test_create_backup_route_returns_gzip_download() {
        use axum::body::Body;
        use tower::ServiceExt;

        let temp_dir = tempfile::tempdir().unwrap();
        seed_config_dir(temp_dir.path());

        let state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            Some(temp_dir.path().to_path_buf()),
        ));

        let web_state = Arc::new(test_web_state());
        let app = test_app(state, &web_state);

        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/tama/v1/backup")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(content_type, "application/gzip");

        let content_disposition = response
            .headers()
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_disposition.starts_with("attachment"),
            "content-disposition should start with 'attachment', got: {}",
            content_disposition
        );

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        // Gzip magic bytes: 0x1f 0x8b
        assert!(
            body_bytes.len() >= 2 && body_bytes[..2] == [0x1f, 0x8b],
            "body should start with gzip magic bytes 0x1f 0x8b"
        );

        // Write body to temp file and verify manifest is parseable
        let archive_path = temp_dir.path().join("test_backup_archive.tar.gz");
        std::fs::write(&archive_path, &body_bytes).unwrap();

        let manifest = tama_core::backup::extract_manifest(&archive_path).unwrap();
        assert!(
            manifest.models.iter().any(|m| m.repo_id == "test/repo"),
            "manifest should contain test/repo model, got models: {:?}",
            manifest.models
        );
    }

    // ── Route-level restore tests (full router with CSRF middleware) ───────

    /// Seed a source config dir with extra data and create a backup archive.
    fn seed_source_and_make_archive(source_dir: &std::path::Path, archive_path: &std::path::Path) {
        use tama_core::db;

        std::fs::create_dir_all(source_dir.join("configs")).unwrap();

        let open = db::open(source_dir).unwrap();
        tama_core::db::queries::seed_defaults(&open.conn).unwrap();

        // Seed model_config + model_pulls for 'test/repo'.
        open.conn
            .execute(
                "INSERT INTO model_configs (repo_id, backend) VALUES ('test/repo', 'llama_cpp')",
                [],
            )
            .unwrap();
        let mid: i64 = open
            .conn
            .query_row(
                "SELECT id FROM model_configs WHERE repo_id='test/repo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        open.conn
            .execute(
                "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
                 VALUES (?1, 'test/repo', 'abc123', '2024-01-01T00:00:00Z')",
                [mid],
            )
            .unwrap();

        // Insert an EXTRA row that the local dir does NOT have (proves merge works).
        open.conn
            .execute(
                "INSERT INTO model_configs (repo_id, backend) VALUES ('source/only', 'llama_cpp')",
                [],
            )
            .unwrap();
        let mid2: i64 = open
            .conn
            .query_row(
                "SELECT id FROM model_configs WHERE repo_id='source/only'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        open.conn
            .execute(
                "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
                 VALUES (?1, 'source/only', 'def456', '2024-01-01T00:00:00Z')",
                [mid2],
            )
            .unwrap();

        // Insert a backend_configs row for 'llama_cpp'/'cpu'.
        open.conn
            .execute(
                "INSERT INTO backend_configs (name, gpu_variant, default_args, default_env, \
                 health_check_url) VALUES ('llama_cpp', 'cpu', '[]', '[]', NULL)",
                [],
            )
            .unwrap();

        // Insert a backend_installations row.
        open.conn
            .execute(
                "INSERT INTO backend_installations (name, backend_type, version, path, \
                 installed_at, gpu_variant, source, is_active) VALUES ('llama_cpp', 'llama_cpp', \
                 'v1.0', '/tmp/llama', 1234567890, 'cpu', 'prebuilt', 1)",
                [],
            )
            .unwrap();

        drop(open); // Close connection before create_backup.
        tama_core::backup::create_backup(source_dir, archive_path).expect("create fixture archive");
    }

    /// Poll a job until it reaches a terminal state (Succeeded or Failed).
    async fn wait_for_job(
        jobs: &Arc<crate::web_types::JobManager>,
        job_id: &str,
    ) -> (crate::web_types::JobStatus, Option<String>) {
        for _ in 0..100 {
            if let Some(job) = jobs.get(&job_id.to_string()).await {
                let state = job.state.read().await;
                if state.status != crate::web_types::JobStatus::Running {
                    return (state.status, state.error.clone());
                }
                drop(state);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("wait_for_job timed out for job {}", job_id);
    }

    // ── Test 1: unknown upload → 404 ──────────────────────────────────────

    #[tokio::test]
    async fn test_start_restore_unknown_upload_returns_404() {
        use axum::{body::Body, extract::Extension, routing::post, Router};
        use serde_json::json;
        use tower::ServiceExt;

        let config_temp = tempfile::tempdir().unwrap();
        seed_config_dir(config_temp.path());

        let proxy_state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            Some(config_temp.path().to_path_buf()),
        ));

        let web_state = WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            repository: None,
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(Extension(web_state))
            .with_state(proxy_state);

        // POST with unknown upload_id → 404.
        let csrf_token = "t";
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header(
                axum::http::header::COOKIE,
                format!("tama_csrf_token={}", csrf_token),
            )
            .header("X-CSRF-Token", csrf_token)
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": "nope"}).to_string()))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["type"], "NotFoundError");
    }

    // ── Test 2: happy path – merges archive and completes job ─────────────

    #[tokio::test]
    async fn test_start_restore_merges_archive_and_completes_job() {
        use axum::{body::Body, routing::post, Router};
        use serde_json::json;
        use tama_core::db;
        use tower::ServiceExt;

        // Local config dir (seeded with seed_config_dir).
        let local_temp = tempfile::tempdir().unwrap();
        seed_config_dir(local_temp.path());

        // Source dir: seeded + archive created.
        let source_temp = tempfile::tempdir().unwrap();
        let archive_path = source_temp.path().join("backup.tar.gz");
        seed_source_and_make_archive(source_temp.path(), &archive_path);

        // Create the JobManager first so we can keep a reference for wait_for_job.
        let jobs = Arc::new(crate::web_types::JobManager::new());

        // Upload lock entry (simulates upload endpoint having stored the file).
        let upload_id = "up-merge".to_string();
        let upload_lock = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
            (
                upload_id.clone(),
                UploadEntry {
                    path: archive_path.clone(),
                    created_at: chrono::Utc::now(),
                },
            ),
        ])));

        let proxy_state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            Some(local_temp.path().to_path_buf()),
        ));

        let web_state = WebState {
            jobs: Some(jobs.clone()),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: upload_lock.clone(),
            repository: None,
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(Extension(web_state))
            .with_state(proxy_state.clone());

        // POST → 200 with job_id.
        let csrf_token = "m";
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header(
                axum::http::header::COOKIE,
                format!("tama_csrf_token={}", csrf_token),
            )
            .header("X-CSRF-Token", csrf_token)
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": upload_id}).to_string()))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let job_id = json["job_id"]
            .as_str()
            .expect("job_id in response")
            .to_string();

        // Wait for the background job to finish.
        let (status, error) = wait_for_job(&jobs, &job_id).await;
        assert_eq!(status, crate::web_types::JobStatus::Succeeded);
        assert!(error.is_none(), "job should not have error: {:?}", error);

        // Verify: source/only model_pulls row was merged.
        let open = db::open(local_temp.path()).unwrap();
        let count: i64 = open
            .conn
            .query_row(
                "SELECT COUNT(*) FROM model_pulls WHERE repo_id = 'source/only'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "source/only should have been merged");

        // Verify: backends HashMap contains "llama_cpp" key.
        {
            let has_backend = proxy_state
                .with_config(|c| c.backends.contains_key("llama_cpp"))
                .await;
            assert!(has_backend, "backends should contain 'llama_cpp'");
        }

        // Verify: uploaded archive file no longer exists (cleanup after restore).
        assert!(
            !archive_path.exists(),
            "archive should have been deleted after restore"
        );
    }

    // ── Test 3: corrupt archive → job fails, local DB/configs untouched ──

    #[tokio::test]
    async fn test_start_restore_corrupt_archive_fails_job_and_leaves_config_untouched() {
        use axum::{body::Body, routing::post, Router};
        use serde_json::json;
        use tama_core::db;
        use tower::ServiceExt;

        let local_temp = tempfile::tempdir().unwrap();
        seed_config_dir(local_temp.path());

        // Write a non-gzip file as the "archive".
        let upload_id = "up-corrupt".to_string();
        let archive_path = local_temp.path().join("corrupt.tar.gz");
        std::fs::write(&archive_path, b"not a gzip").unwrap();

        // Create the JobManager first so we can keep a reference for wait_for_job.
        let jobs = Arc::new(crate::web_types::JobManager::new());

        let proxy_state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            Some(local_temp.path().to_path_buf()),
        ));

        let upload_lock = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
            (
                upload_id.clone(),
                UploadEntry {
                    path: archive_path.clone(),
                    created_at: chrono::Utc::now(),
                },
            ),
        ])));

        let web_state = WebState {
            jobs: Some(jobs.clone()),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: upload_lock.clone(),
            repository: None,
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(Extension(web_state))
            .with_state(proxy_state.clone());

        // POST → 200 (job accepted).
        let csrf_token = "c";
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header(
                axum::http::header::COOKIE,
                format!("tama_csrf_token={}", csrf_token),
            )
            .header("X-CSRF-Token", csrf_token)
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": upload_id}).to_string()))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let job_id = json["job_id"]
            .as_str()
            .expect("job_id in response")
            .to_string();

        // Wait for the background job to finish.
        let (status, error) = wait_for_job(&jobs, &job_id).await;
        assert_eq!(status, crate::web_types::JobStatus::Failed);
        let err_msg = error.expect("error text on failed job");
        assert!(
            err_msg.to_lowercase().contains("extract"),
            "error should mention extraction failure, got: {}",
            err_msg
        );

        // Verify: local DB still has exactly 1 model_pulls row (the seeded 'test/repo').
        let open = db::open(local_temp.path()).unwrap();
        let count: i64 = open
            .conn
            .query_row("SELECT COUNT(*) FROM model_pulls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "local DB should still have exactly 1 model_pulls row"
        );

        // Verify: local/configs/ has no new files.
        let configs_dir = local_temp.path().join("configs");
        let entries: Vec<_> = std::fs::read_dir(&configs_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "configs dir should have no new files after failed restore, got: {:?}",
            entries
        );
    }

    // ── Test 4: tampered archive (SHA mismatch) → job fails ───────────────

    #[tokio::test]
    async fn test_start_restore_tampered_sha_fails_job() {
        use axum::{body::Body, routing::post, Router};
        use serde_json::json;
        use tama_core::db;
        use tower::ServiceExt;

        let local_temp = tempfile::tempdir().unwrap();
        seed_config_dir(local_temp.path());

        // Source dir: create a valid archive.
        let source_temp = tempfile::tempdir().unwrap();
        let archive_path = source_temp.path().join("backup.tar.gz");
        seed_source_and_make_archive(source_temp.path(), &archive_path);

        // Read the file, tamper with it (XOR a byte in the middle of the
        // compressed data — avoids the gzip trailer where CRC32 checks may
        // be skipped by some decompressors), then rewrite.
        let mut file_bytes = std::fs::read(&archive_path).expect("read archive");
        if file_bytes.len() > 20 {
            // Pick a position well past the gzip header (10+ bytes) but before
            // the final 8-byte trailer. This corrupts actual compressed data.
            let tamper_idx = file_bytes.len() / 2;
            file_bytes[tamper_idx] ^= 0xFF;
        }
        // Rewrite the tampered archive.
        std::fs::write(&archive_path, &file_bytes).expect("rewrite tampered archive");

        let upload_id = "up-tampered".to_string();

        // Create the JobManager first so we can keep a reference for wait_for_job.
        let jobs = Arc::new(crate::web_types::JobManager::new());

        let upload_lock = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::from([
            (
                upload_id.clone(),
                UploadEntry {
                    path: archive_path.clone(),
                    created_at: chrono::Utc::now(),
                },
            ),
        ])));

        let proxy_state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            Some(local_temp.path().to_path_buf()),
        ));

        let web_state = WebState {
            jobs: Some(jobs.clone()),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: upload_lock.clone(),
            repository: None,
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(Extension(web_state))
            .with_state(proxy_state.clone());

        // POST → 200 (job accepted).
        let csrf_token = "t";
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header(
                axum::http::header::COOKIE,
                format!("tama_csrf_token={}", csrf_token),
            )
            .header("X-CSRF-Token", csrf_token)
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": upload_id}).to_string()))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let job_id = json["job_id"]
            .as_str()
            .expect("job_id in response")
            .to_string();

        // Wait for the background job to finish.
        let (status, _error) = wait_for_job(&jobs, &job_id).await;
        assert_eq!(status, crate::web_types::JobStatus::Failed);

        // Verify: local DB unchanged (still 1 model_pulls row).
        let open = db::open(local_temp.path()).unwrap();
        let count: i64 = open
            .conn
            .query_row("SELECT COUNT(*) FROM model_pulls", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "local DB should still have exactly 1 model_pulls row"
        );
    }
}
