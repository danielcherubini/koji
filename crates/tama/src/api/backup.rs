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

    // Build the archive (manifest from Postgres + config cards, plan-190
    // Task 9), then read the bytes on the blocking pool to keep the async
    // runtime free.
    let pool = state.db_pool();
    let temp_dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to create temp dir: {}", e),
                None,
            )
        }
    };
    let output_path = temp_dir.path().join("backup.tar.gz");
    let manifest = match tama_core::backup::create_backup(pool.as_ref(), &config_dir, &output_path)
        .await
    {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    let file_result = tokio::task::spawn_blocking(move || {
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

    let result = file_result;
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

/// Restore a v3 backup archive into the config directory (plan-190 Task 9).
///
/// Extraction and validation are atomic (all-or-nothing in the temp dir);
/// the card merge is additive. v3 archives contain only `manifest.json` +
/// config cards — the model/backend lists in the manifest come from the
/// source's Postgres at backup time, and the local DB/global config are NOT
/// touched (they live in Postgres; `pg_dump` is the DB backup path).
///
/// `selected_models`, `skip_backends`, and `skip_models` from [`RestoreRequest`]
/// are accepted at the API level but NOT applied here — restore v3 always
/// merges every missing card.
async fn run_restore(
    config_dir: &std::path::Path,
    archive_path: &std::path::Path,
) -> anyhow::Result<String> {
    let extract_dir = tempfile::tempdir().context("Failed to create restore temp dir")?;
    let extracted = tama_core::backup::extract_backup(archive_path, extract_dir.path())
        .context("Failed to extract backup archive")?;

    let copied_cards = tama_core::backup::merge_model_cards(
        &config_dir.join("configs"),
        &extract_dir.path().join("configs"),
    )
    .context("Failed to merge model cards")?;

    let summary = format!(
        concat!(
            "Restored {} model card(s); {} model(s), {} backend(s) listed in ",
            "manifest.\n",
            "Note: v3 restores merge model cards only — the global app config ",
            "and DB state stay in Postgres (pg_dump is the DB backup path).",
        ),
        copied_cards.len(),
        extracted.manifest.models.len(),
        extracted.manifest.backends.len(),
    );

    Ok(summary)
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
    let cleanup_path = upload_path.clone();
    tokio::spawn(async move {
        jobs_for_spawn
            .append_log(
                &job_for_spawn,
                "Extracting and validating backup archive".to_string(),
            )
            .await;
        let result = run_restore(&config_dir, &upload_path).await;
        match result {
            Ok(summary) => {
                jobs_for_spawn.append_log(&job_for_spawn, summary).await;
                jobs_for_spawn
                    .finish(&job_for_spawn, crate::web_types::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                tracing::error!("Restore job {} failed: {:#}", job_for_spawn.id, e);
                jobs_for_spawn
                    .finish(
                        &job_for_spawn,
                        crate::web_types::JobStatus::Failed,
                        Some(format!("{:#}", e)),
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

    /// Build a valid v3 archive (manifest + config cards) without a DB.
    ///
    /// `cards` are (filename, content) pairs. The manifest lists one model
    /// and one backend so restore summaries have data to report.
    fn make_v3_archive(archive_path: &std::path::Path, cards: &[(&str, &str)]) {
        use tama_core::backup::archive::StreamingHasher;

        let temp = tempfile::tempdir().unwrap();
        let configs = temp.path().join("configs");
        std::fs::create_dir_all(&configs).unwrap();

        let mut names: Vec<&str> = cards.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();

        let mut hasher = StreamingHasher::new();
        for name in &names {
            let content = cards.iter().find(|(n, _)| n == name).unwrap().1;
            std::fs::write(configs.join(name), content).unwrap();
            hasher.update(content.as_bytes());
        }

        let mut manifest = tama_core::backup::manifest::BackupManifest::new("2.1.0");
        manifest.sha256 = hasher.finalize_hex();
        manifest.models = vec![tama_core::backup::manifest::BackupModelEntry {
            repo_id: "source/repo".to_string(),
            quants: vec!["Q4_K_M".to_string()],
            total_size_bytes: 1000,
        }];
        manifest.backends = vec![tama_core::backup::manifest::BackendEntry {
            name: "llama_cpp".to_string(),
            version: "v1.0".to_string(),
            backend_type: "llama_cpp".to_string(),
            source: Some("prebuilt".to_string()),
            docker_config: None,
        }];

        tama_core::backup::archive::write_archive(temp.path(), archive_path, &manifest).unwrap();
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

    // ── run_restore unit test (no DB involved in v3) ──────────────────────

    #[tokio::test]
    async fn test_run_restore_copies_missing_cards_only() {
        // Source archive with two cards; target already has one.
        let archive_path = tempfile::tempdir().unwrap().path().join("backup.tar.gz");
        make_v3_archive(
            &archive_path,
            &[
                ("existing.toml", "[model]\nid = \"source/existing\"\n"),
                ("new_card.toml", "[model]\nid = \"source/new\"\n"),
            ],
        );

        let target_dir = tempfile::tempdir().unwrap();
        let local_configs = target_dir.path().join("configs");
        std::fs::create_dir_all(&local_configs).unwrap();
        std::fs::write(
            local_configs.join("existing.toml"),
            "[model]\nid = \"local/existing\"\n",
        )
        .unwrap();

        let result = run_restore(target_dir.path(), &archive_path).await;
        assert!(
            result.is_ok(),
            "run_restore should succeed: {:?}",
            result.err()
        );

        let summary = result.unwrap();
        assert!(
            summary.starts_with("Restored 1 model card(s)"),
            "only the missing card should be copied, got: {}",
            summary
        );
        // Existing local card untouched.
        assert!(std::fs::read_to_string(local_configs.join("existing.toml"))
            .unwrap()
            .contains("local/existing"));
        // New card copied.
        assert!(std::fs::read_to_string(local_configs.join("new_card.toml"))
            .unwrap()
            .contains("source/new"));
    }

    // ── Route tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_start_restore_submits_job_and_returns_200() {
        use axum::{body::Body, extract::Extension, routing::post, Router};
        use serde_json::json;
        use tower::ServiceExt;

        let config_temp = tempfile::tempdir().unwrap();
        let _ = config_temp.path();

        // Upload dir with a fake archive.
        let upload_temp = tempfile::tempdir().unwrap();
        let upload_id = "up-1".to_string();
        let upload_path = upload_temp.path().join(format!("{}.tar.gz", upload_id));
        std::fs::write(&upload_path, b"fake backup").unwrap();

        let proxy_state = Arc::new(tama_core::proxy::ProxyState::new(
            tama_core::config::Config::default(),
            Some(config_temp.path().to_path_buf()),
            tama_test_support::test_dummy_pool(),
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
            db_pool: tama_test_support::test_dummy_pool(),
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
            tama_test_support::test_dummy_pool(),
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
            db_pool: tama_test_support::test_dummy_pool(),
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

        let proxy_state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            Some(config_temp.path().to_path_buf()),
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_test_support::test_dummy_pool(),
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
        use tower::ServiceExt;

        // Local config dir (no DB in v3 — cards only).
        let local_temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(local_temp.path().join("configs")).unwrap();

        // Source archive built by make_v3_archive (manifest from "Postgres"
        // at backup time; here hand-built to avoid a real DB).
        let source_temp = tempfile::tempdir().unwrap();
        let archive_path = source_temp.path().join("backup.tar.gz");
        make_v3_archive(
            &archive_path,
            &[("source_card.toml", "[model]\nid = \"source/card\"\n")],
        );

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
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = WebState {
            jobs: Some(jobs.clone()),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: upload_lock.clone(),
            db_pool: tama_test_support::test_dummy_pool(),
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(axum::extract::Extension(web_state))
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

        // The model card was copied to the target configs dir.
        let card = local_temp.path().join("configs").join("source_card.toml");
        assert!(card.exists(), "model card should be copied to target");
        assert!(
            std::fs::read_to_string(&card)
                .unwrap()
                .contains("source/card"),
            "model card content should round-trip"
        );

        // Verify: uploaded archive file no longer exists (cleanup after restore).
        assert!(
            !archive_path.exists(),
            "archive should have been deleted after restore"
        );
    }

    // ── Test 3: corrupt archive → job fails, local configs untouched ──────

    #[tokio::test]
    async fn test_start_restore_corrupt_archive_fails_job_and_leaves_config_untouched() {
        use axum::{body::Body, routing::post, Router};
        use serde_json::json;
        use tower::ServiceExt;

        let local_temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(local_temp.path().join("configs")).unwrap();

        // Write a non-gzip file as the "archive".
        let upload_id = "up-corrupt".to_string();
        let archive_path = local_temp.path().join("corrupt.tar.gz");
        std::fs::write(&archive_path, b"not a gzip").unwrap();

        // Create the JobManager first so we can keep a reference for wait_for_job.
        let jobs = Arc::new(crate::web_types::JobManager::new());

        let proxy_state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            Some(local_temp.path().to_path_buf()),
            tama_test_support::test_dummy_pool(),
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
            db_pool: tama_test_support::test_dummy_pool(),
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(axum::extract::Extension(web_state))
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
        use tower::ServiceExt;

        let local_temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(local_temp.path().join("configs")).unwrap();

        // Source dir: create a valid archive.
        let source_temp = tempfile::tempdir().unwrap();
        let archive_path = source_temp.path().join("backup.tar.gz");
        make_v3_archive(
            &archive_path,
            &[("source_card.toml", "[model]\nid = \"source/card\"\n")],
        );

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
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = WebState {
            jobs: Some(jobs.clone()),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: upload_lock.clone(),
            db_pool: tama_test_support::test_dummy_pool(),
        };

        let app = Router::new()
            .route("/tama/v1/restore", post(start_restore))
            .layer(axum::extract::Extension(web_state))
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
    }
}
