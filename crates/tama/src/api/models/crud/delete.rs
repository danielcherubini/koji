use crate::api::error::error_body;
use crate::api::helpers::{shared_repository, spawn_model_crud, DEFAULT_CRUD_STATUS};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use tama_core::proxy::ProxyState;

use crate::api::load_config_from_state;
use crate::api::models::resolve_model_id;
use crate::web_types::WebState;

/// DELETE /tama/v1/models/:id/quants/:quant_key — delete a single quant's file
/// and remove it from the config.
pub async fn delete_quant(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path((id, quant_key)): Path<(i64, String)>,
) -> impl IntoResponse {
    let state_clone = state.clone();

    // Load config first (async, handles its own spawn_blocking)
    let (cfg, _config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };

    spawn_model_crud(state_clone, DEFAULT_CRUD_STATUS, move || {
        // Open repository for reading
        let repo = repo_handle.lock().unwrap();

        // Find the model from DB
        let model_record = repo
            .get_model_config(id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body("Model not found", Some("NotFoundError")),
                )
            })?;

        let mut model_config = tama_core::config::ModelConfig::from_db_record(&model_record);

        // Find the quant entry
        let quant_entry = model_config.quants.get(&quant_key).ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                error_body("Quant not found", Some("NotFoundError")),
            )
        })?;

        // Clone the filename and repo_id before we mutate
        let filename = quant_entry.file.clone();
        let repo_id = model_record.repo_id.clone();

        // Clear active quant/mmproj if they referenced this quant
        if model_config.quant.as_deref() == Some(&quant_key) {
            model_config.quant = None;
        }
        if model_config.mmproj.as_deref() == Some(&quant_key) {
            model_config.mmproj = None;
        }

        // Remove the quant entry
        model_config.quants.remove(&quant_key);

        // Save to DB
        let config_key = tama_core::models::ConfigKey::from_repo_id(&repo_id);
        repo.save_model_config(config_key.as_str(), &model_config)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;

        // Clean up file (best-effort) - only after config is saved
        if !repo_id.is_empty() {
            if let Ok(models_dir) = cfg.models_dir() {
                let file_path = tama_core::models::repo_path(&models_dir, &repo_id).join(&filename);
                if file_path.exists() {
                    if let Err(e) = std::fs::remove_file(&file_path) {
                        tracing::warn!(
                            "Failed to delete quant file {}: {}",
                            file_path.display(),
                            e
                        );
                    }
                }
            }
        }

        // Clean up DB record (best-effort) - only after config is saved
        if !repo_id.is_empty() {
            let _ = repo.delete_file(id, &filename);
        }

        Ok(serde_json::json!({
            "ok": true,
            "id": id,
            "quant_key": quant_key,
            "deleted_file": filename
        }))
    })
    .await
}

/// DELETE /tama/v1/models/:id — delete a model.
pub async fn delete_model(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    let state_clone = state.clone();

    // Load config first (async, handles its own spawn_blocking)
    let (cfg, _config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };

    spawn_model_crud(state_clone, DEFAULT_CRUD_STATUS, move || {
        // Open repository for reading
        let repo = repo_handle.lock().unwrap();

        // Resolve model_id using Repository
        let model_id = resolve_model_id(&id_str, &repo)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    error_body(e.to_string(), Some("ValidationError")),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body("Model not found", Some("NotFoundError")),
                )
            })?;
        let model_record = repo
            .get_model_config(model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body("Model not found", Some("NotFoundError")),
                )
            })?;
        let _model_config = tama_core::config::ModelConfig::from_db_record(&model_record);

        // Step 1: Delete model config — all-or-nothing. CASCADE handles
        // model_files and model_pulls. If this fails, no files are touched yet
        // and the DB remains consistent.
        {
            tracing::debug!("Deleting model config for id={}", model_id);
            if let Err(e) = repo.delete_config(model_id) {
                tracing::error!("Failed to delete model records from database: {e}");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body("Failed to delete model records from database", None),
                ));
            }
        }

        // Step 1b: Delete the update check record (best-effort, separate from transaction).
        // Kept outside the transaction so a missing or corrupted update_checks table
        // doesn't block model deletion. Errors are logged for visibility.
        if let Err(e) = repo.delete_update_check("model", &model_id.to_string()) {
            tracing::warn!(
                "Failed to delete update check record for model {}: {}",
                model_id,
                e
            );
        }

        // Step 2: File cleanup (best-effort) — after successful DB commit.
        // If file deletion fails, the DB is already clean; orphaned files are
        // a benign cleanup issue. If it had succeeded before the DB commit,
        // a failed transaction would leave files deleted but DB records intact.
        let repo_id = model_record.repo_id.clone();
        if !repo_id.is_empty() {
            // 1. Delete model directory: models_dir / repo_id
            if let Ok(models_dir) = cfg.models_dir() {
                let model_dir = tama_core::models::repo_path(&models_dir, &repo_id);
                if model_dir.exists() {
                    if let Err(e) = std::fs::remove_dir_all(&model_dir) {
                        tracing::warn!(
                            "Failed to remove model directory {}: {}",
                            model_dir.display(),
                            e
                        );
                    } else {
                        // Clean up empty parent dir
                        if let Some(parent) = model_dir.parent() {
                            if parent
                                .read_dir()
                                .map(|mut d| d.next().is_none())
                                .unwrap_or(false)
                            {
                                let _ = std::fs::remove_dir(parent);
                            }
                        }
                    }
                }
            }
            // 2. Delete model card
            if let Ok(configs_dir) = cfg.configs_dir() {
                let card_path =
                    configs_dir.join(format!("{}.toml", tama_core::models::card_slug(&repo_id)));
                if card_path.exists() {
                    let _ = std::fs::remove_file(&card_path);
                }
            }
        }

        Ok(serde_json::json!({ "ok": true }))
    })
    .await
}
