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

use crate::api::models::resolve_model_record;
use crate::web_types::WebState;
use tama_core::models::is_valid_repo_id;

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
    let pool = web_state.db_pool.as_ref();

    let (model_id, existing_record) = match resolve_model_record(pool, &id_str).await {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };
    let old_repo_id = existing_record.repo_id.clone();
    let mut model_config = tama_core::config::ModelConfig::from_db_record(&existing_record);

    let new_repo_id = body.new_repo_id.trim().to_string();
    if new_repo_id.is_empty() {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "New repo_id cannot be empty",
            Some("ValidationError"),
        );
    }
    if new_repo_id.len() > 256 {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "New repo_id must be at most 256 characters",
            Some("ValidationError"),
        );
    }
    if !is_valid_repo_id(&new_repo_id) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "New repo_id contains invalid characters (only alphanumeric, dots, underscores, hyphens, and slashes are allowed)", Some("ValidationError"));
    }

    // Check target repo_id doesn't already exist
    match tama_core::db::queries::get_model_config_by_repo_id(pool, &new_repo_id).await {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                format!("Model '{}' already exists", new_repo_id),
                Some("ConflictError"),
            )
        }
        Ok(None) => {}
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }

    // Update the model field (repo_id) in the config to reflect the rename
    model_config.model = Some(new_repo_id.clone());

    // Save with new repo_id (keeps same integer id)
    let config_key = tama_core::models::ConfigKey::from_repo_id(&new_repo_id);
    if let Err(e) = tama_core::db::save_model_config(pool, config_key.as_str(), &model_config).await
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None);
    }

    if let Err(e) = state.reload_model_configs().await {
        tracing::warn!("failed to reload model configs: {:?}", e);
    }
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!(error = %e, "Failed to reload aliases");
    }

    // Clean up update_check record for old repo_id (best-effort, kept outside
    // the rename transaction so a DB hiccup doesn't fail the rename).
    if let Err(e) = tama_core::db::queries::delete_update_check(pool, "model", &old_repo_id).await {
        tracing::warn!("Failed to delete update check record for model {old_repo_id}: {e}");
    }

    Json(ModelMutationResponse {
        ok: true,
        id: model_id,
    })
    .into_response()
}
