use crate::api::error::{error_body, error_response};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::{apply_model_body, validate_model_body, ModelBody};
use crate::api::helpers::{spawn_model_crud, DEFAULT_CRUD_STATUS};
use crate::api::load_config_from_state;
use crate::api::models::resolve_model_id;
use tama_core::proxy::ProxyState;

/// PUT /tama/v1/models/:id — update an existing model.
pub async fn update_model(
    State(state): State<Arc<ProxyState>>,
    Path(id_str): Path<String>,
    Json(body): Json<ModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();

    // Validate ModelBody fields
    if let Err(e) = validate_model_body(&body) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, e, Some("ValidationError"));
    }

    // Load config first (async, handles its own spawn_blocking)
    let (_cfg, config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    spawn_model_crud(state_clone, DEFAULT_CRUD_STATUS, move || {
        // Load existing from DB
        let mgr = tama_core::models::ModelManager::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_body(e.to_string(), None),
            )
        })?;
        let model_id = resolve_model_id(&id_str, &mgr)
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
        let existing_record = mgr
            .get_config(model_id)
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
        let existing = tama_core::config::ModelConfig::from_db_record(&existing_record);

        let updated_config = apply_model_body(body, Some(existing));

        // Save to DB (save_model_config converts config_key to repo_id internally)
        let config_key = existing_record.repo_id.to_lowercase().replace('/', "--");
        let new_model_id = mgr
            .save_model_config(&config_key, &updated_config)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;
        Ok(serde_json::json!({ "ok": true, "id": new_model_id }))
    })
    .await
}
