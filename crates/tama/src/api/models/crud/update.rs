use crate::api::error::{error_body, error_response};
use crate::api::helpers::{spawn_model_crud, DEFAULT_CRUD_STATUS};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use tama_core::proxy::tama_handlers::ModelMutationResponse;
use tama_core::proxy::ProxyState;

use super::{
    apply_model_body, apply_model_patch, validate_model_body, validate_model_patch, ModelBody,
    ModelPatchBody,
};
use crate::api::load_config_from_state;
use crate::api::models::resolve_model_record;
use crate::web_types::WebState;

/// PUT /tama/v1/models/:id — update an existing model.
pub async fn update_model(
    State(state): State<Arc<ProxyState>>,
    Extension(_web_state): Extension<WebState>,
    Path(id_str): Path<String>,
    Json(body): Json<ModelBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();

    // Validate ModelBody fields
    let mut body = body;
    if let Err(e) = validate_model_body(&mut body) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, e, Some("ValidationError"));
    }

    let (_, config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    spawn_model_crud(state_clone, DEFAULT_CRUD_STATUS, move || {
        let (repo, model_id, existing_record) = resolve_model_record(&config_dir, &id_str)?;
        let _ = model_id;
        let existing = tama_core::config::ModelConfig::from_db_record(&existing_record);

        let updated_config = apply_model_body(body, Some(existing));

        // Save to DB (save_model_config converts config_key to repo_id internally)
        let config_key = tama_core::models::ConfigKey::from_repo_id(&existing_record.repo_id);
        let new_model_id = repo
            .save_model_config(config_key.as_str(), &updated_config)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;
        Ok(ModelMutationResponse {
            ok: true,
            id: new_model_id,
        })
    })
    .await
}

/// PATCH /tama/v1/models/:id — surgical partial update.
pub async fn patch_model(
    State(state): State<Arc<ProxyState>>,
    Extension(_web_state): Extension<WebState>,
    Path(id_str): Path<String>,
    Json(body): Json<ModelPatchBody>,
) -> impl IntoResponse {
    let state_clone = state.clone();

    // Validate ModelPatchBody fields
    let mut body = body;
    if let Err(e) = validate_model_patch(&mut body) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, e, Some("ValidationError"));
    }

    let (_, config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    spawn_model_crud(state_clone, DEFAULT_CRUD_STATUS, move || {
        let (repo, model_id, existing_record) = resolve_model_record(&config_dir, &id_str)?;
        let _ = model_id;
        let existing = tama_core::config::ModelConfig::from_db_record(&existing_record);

        let updated_config = apply_model_patch(body, &existing);

        // Save to DB (save_model_config converts config_key to repo_id internally)
        let config_key = tama_core::models::ConfigKey::from_repo_id(&existing_record.repo_id);
        let new_model_id = repo
            .save_model_config(config_key.as_str(), &updated_config)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;
        Ok(ModelMutationResponse {
            ok: true,
            id: new_model_id,
        })
    })
    .await
}
