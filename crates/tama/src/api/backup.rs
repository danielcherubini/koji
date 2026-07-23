//! Backup and restore API endpoints.

use crate::api::error::error_response;
use crate::web_types::WebState;
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
    pub source: String,
}

/// GET /tama/v1/backup - Create backup and return as file download
pub async fn create_backup(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let config_dir: std::path::PathBuf = {
        state.db_dir().clone().unwrap_or_else(|| {
            tama_core::config::Config::config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
        })
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

/// POST /tama/v1/restore - Start restore job
pub async fn start_restore(
    State(_state): State<Arc<ProxyState>>,
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

    // Return 501 as a stopgap until plan-163 is implemented.
    // We clean up the uploaded archive and the lock entry to avoid resource leaks.
    let mut uploads = upload_lock.write().await;
    uploads.remove(&body.upload_id);
    drop(uploads);

    let _ = std::fs::remove_file(&upload_path);

    error_response(
        StatusCode::NOT_IMPLEMENTED,
        "Backup restore is not yet implemented. The uploaded archive has been removed.",
        Some("NotImplementedError"),
    )
}

/// Re-export from local web_types for backward compatibility.
pub use crate::web_types::UploadEntry;

#[cfg(test)]
mod tests {
    use super::*;

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
                source: "prebuilt".to_string(),
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
            source: "prebuilt".to_string(),
        };

        assert_eq!(entry.name, "llama-cpp");
        assert_eq!(entry.version, "b8407");
        assert_eq!(entry.backend_type, "llama_cpp");
        assert_eq!(entry.source, "prebuilt");
    }

    #[test]
    fn test_backend_entry_source_build() {
        let entry = BackendEntry {
            name: "llama-cpp".to_string(),
            version: "b8407".to_string(),
            backend_type: "llama_cpp".to_string(),
            source: "build".to_string(),
        };

        assert_eq!(entry.source, "build");
    }

    #[tokio::test]
    async fn test_start_restore_returns_501_and_cleans_up() {
        use axum::{body::Body, extract::Extension, routing::post, Router};
        use serde_json::json;
        use tower::ServiceExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let upload_id = "up-1".to_string();
        let upload_path = temp_dir.path().join(format!("{}.tar.gz", upload_id));
        std::fs::write(&upload_path, b"fake backup").unwrap();

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

        // Case 1: Valid upload_id -> 501 and cleanup
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/tama/v1/restore")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"upload_id": upload_id}).to_string()))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["type"], "NotImplementedError");
        assert!(
            !std::path::Path::new(&upload_path).exists(),
            "Upload file should be removed"
        );

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
}
