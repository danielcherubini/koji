use crate::api::error::error_response;
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
use crate::api::models::resolve_model_record;
use crate::web_types::WebState;

/// PUT /tama/v1/models/:id — update an existing model.
pub async fn update_model(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id_str): Path<String>,
    Json(body): Json<ModelBody>,
) -> impl IntoResponse {
    // Validate ModelBody fields
    let mut body = body;
    if let Err(e) = validate_model_body(&mut body) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, e, Some("ValidationError"));
    }

    let pool = web_state.db_pool.as_ref();

    let (_, existing_record) = match resolve_model_record(pool, &id_str).await {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };
    let existing = tama_core::config::ModelConfig::from_db_record(&existing_record);

    let updated_config = apply_model_body(body, Some(existing));

    // Save to DB (save_model_config converts config_key to repo_id internally)
    let config_key = tama_core::models::ConfigKey::from_repo_id(&existing_record.repo_id);
    let new_model_id =
        match tama_core::db::save_model_config(pool, config_key.as_str(), &updated_config).await {
            Ok(id) => id,
            Err(e) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
            }
        };

    if let Err(e) = state.reload_model_configs().await {
        tracing::warn!("failed to reload model configs: {:?}", e);
    }
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!(error = %e, "Failed to reload aliases");
    }

    (
        StatusCode::OK,
        Json(ModelMutationResponse {
            ok: true,
            id: new_model_id,
        }),
    )
        .into_response()
}

/// PATCH /tama/v1/models/:id — surgical partial update.
pub async fn patch_model(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id_str): Path<String>,
    Json(body): Json<ModelPatchBody>,
) -> impl IntoResponse {
    // Validate ModelPatchBody fields
    let mut body = body;
    if let Err(e) = validate_model_patch(&mut body) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, e, Some("ValidationError"));
    }

    let pool = web_state.db_pool.as_ref();

    let (_, existing_record) = match resolve_model_record(pool, &id_str).await {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };
    let existing = tama_core::config::ModelConfig::from_db_record(&existing_record);

    let updated_config = apply_model_patch(body, &existing);

    // Save to DB (save_model_config converts config_key to repo_id internally)
    let config_key = tama_core::models::ConfigKey::from_repo_id(&existing_record.repo_id);
    let new_model_id =
        match tama_core::db::save_model_config(pool, config_key.as_str(), &updated_config).await {
            Ok(id) => id,
            Err(e) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
            }
        };

    if let Err(e) = state.reload_model_configs().await {
        tracing::warn!("failed to reload model configs: {:?}", e);
    }
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!(error = %e, "Failed to reload aliases");
    }

    (
        StatusCode::OK,
        Json(ModelMutationResponse {
            ok: true,
            id: new_model_id,
        }),
    )
        .into_response()
}
