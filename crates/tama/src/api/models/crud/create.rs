use crate::api::error::error_response;
use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use tama_core::proxy::tama_handlers::ModelMutationResponse;
use tama_core::proxy::ProxyState;

use tama_core::models::is_valid_repo_id;

use super::{apply_model_body, validate_model_body, ModelBody};
use crate::web_types::WebState;

/// POST /tama/v1/models — create a new model.
/// The body contains `repo_id` (HuggingFace repo name). Returns the auto-generated integer id.
#[derive(serde::Deserialize)]
pub struct CreateModelBody {
    pub repo_id: String,
    /// Optional HuggingFace metadata (README + API) to populate the stub.
    /// When provided, hf_* fields are merged into the model config.
    #[serde(default)]
    pub metadata: Option<tama_core::models::pull::HfModelMetadata>,
    #[serde(flatten)]
    pub model: ModelBody,
}

pub async fn create_model(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Json(body): Json<CreateModelBody>,
) -> impl IntoResponse {
    // Validate repo_id: non-empty, max 256 chars, valid regex pattern
    let repo_id = body.repo_id.trim().to_string();
    if repo_id.is_empty() {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "repo_id cannot be empty",
            Some("ValidationError"),
        );
    }
    if repo_id.len() > 256 {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "repo_id must be at most 256 characters",
            Some("ValidationError"),
        );
    }
    if !is_valid_repo_id(&repo_id) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "repo_id contains invalid characters (only alphanumeric, dots, underscores, hyphens, and slashes are allowed)", Some("ValidationError"));
    }

    // Validate ModelBody fields
    let mut body = body;
    if let Err(e) = validate_model_body(&mut body.model) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, e, Some("ValidationError"));
    }

    let Some(pool) = web_state.db_pool.as_ref() else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Postgres pool not available",
            None,
        );
    };

    // Reject if a model with this repo_id already exists.
    match tama_core::db::queries::get_model_config_by_repo_id(pool, &repo_id).await {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                format!("Model '{}' already exists", repo_id),
                Some("ConflictError"),
            )
        }
        Ok(None) => {}
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }

    let model_config = apply_model_body(body.model, None);
    // Merge HF metadata into model config if provided
    let model_config = if let Some(ref meta) = body.metadata {
        let mut mc = model_config;
        if mc.hf_format.is_none() {
            mc.hf_format = meta.hf_format.clone();
        }
        if mc.hf_base_model.is_none() {
            mc.hf_base_model = meta.hf_base_model.clone();
        }
        if mc.hf_pipeline_tag.is_none() {
            mc.hf_pipeline_tag = meta.hf_pipeline_tag.clone();
        }
        if mc.hf_total_params.is_none() {
            mc.hf_total_params = meta.hf_total_params.clone();
        }
        if mc.hf_active_params.is_none() {
            mc.hf_active_params = meta.hf_active_params.clone();
        }
        if mc.hf_architecture_type.is_none() {
            mc.hf_architecture_type = meta.hf_architecture_type.clone();
        }
        if mc.hf_context_length.is_none() {
            mc.hf_context_length = meta.hf_context_length;
        }
        if mc.hf_num_layers.is_none() {
            mc.hf_num_layers = meta.hf_num_layers;
        }
        if mc.hf_last_modified.is_none() {
            mc.hf_last_modified = meta.hf_last_modified.clone();
        }
        mc
    } else {
        model_config
    };

    let model_config = {
        let mut mc = model_config;
        // Make repo_id authoritative for the DB record when the body did not
        // supply an explicit model path.
        if mc.model.as_deref().is_none_or(str::is_empty) {
            mc.model = Some(repo_id.clone());
        }
        mc
    };

    let model_id = match tama_core::db::save_model_config(pool, &repo_id, &model_config).await {
        Ok(id) => id,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    // Reload the proxy's in-memory registry (best-effort).
    if let Err(e) = state.reload_model_configs().await {
        tracing::warn!("failed to reload model configs: {:?}", e);
    }
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!(error = %e, "Failed to reload aliases");
    }

    (
        StatusCode::CREATED,
        Json(ModelMutationResponse {
            ok: true,
            id: model_id,
        }),
    )
        .into_response()
}
