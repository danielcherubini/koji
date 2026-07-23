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

use super::is_valid_repo_id;
use crate::api::models::resolve_model_id;
use crate::web_types::WebState;

/// Body for rename endpoint.
#[derive(serde::Deserialize)]
pub struct RenameBody {
    pub new_repo_id: String,
}

/// POST /tama/v1/models/:id/rename — rename a model config entry.
pub async fn rename_model(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id_str): Path<String>,
    Json(body): Json<RenameBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();

    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };

    spawn_model_crud(state_clone, DEFAULT_CRUD_STATUS, move || {
        // Open repository for reading
        let repo = repo_handle.lock().unwrap();

        // Check source ID exists
        let model_id = resolve_model_id(&id_str, &repo)
            .map_err(|e| {
                (StatusCode::BAD_REQUEST, error_body(e.to_string(), Some("ValidationError")))
            })?
            .ok_or_else(|| {
                (StatusCode::NOT_FOUND, error_body("Model not found", Some("NotFoundError")))
            })?;
        let existing_record = repo
            .get_model_config(model_id)
            .map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, error_body(e.to_string(), None))
            })?
            .ok_or_else(|| {
                (StatusCode::NOT_FOUND, error_body("Model not found", Some("NotFoundError")))
            })?;
        let mut model_config = tama_core::config::ModelConfig::from_db_record(&existing_record);

        let new_repo_id = body.new_repo_id.trim().to_string();
        if new_repo_id.is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                error_body("New repo_id cannot be empty", Some("ValidationError")),
            ));
        }
        if new_repo_id.len() > 256 {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                error_body("New repo_id must be at most 256 characters", Some("ValidationError")),
            ));
        }
        if !is_valid_repo_id(&new_repo_id) {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                error_body("New repo_id contains invalid characters (only alphanumeric, dots, underscores, hyphens, and slashes are allowed)", Some("ValidationError")),
            ));
        }

        // Check target repo_id doesn't already exist
        if repo
            .get_model_config_by_repo_id(&new_repo_id)
            .map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, error_body(e.to_string(), None))
            })?
            .is_some()
        {
            return Err((
                StatusCode::CONFLICT,
                error_body(
                    format!("Model '{}' already exists", new_repo_id),
                    Some("ConflictError"),
                ),
            ));
        }

        // Update the model field (repo_id) in the config to reflect the rename
        model_config.model = Some(new_repo_id.clone());

        // Save with new repo_id (keeps same integer id)
        let config_key = new_repo_id.to_lowercase().replace('/', "--");
        let _ = repo
            .save_model_config(&config_key, &model_config)
            .map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, error_body(e.to_string(), None))
            })?;

        // Clean up update_check record for old repo_id
        let _ = repo.delete_update_check("model", &existing_record.repo_id);

        Ok(serde_json::json!({ "ok": true, "id": model_id }))
    })
    .await
}
