use crate::api::error::{error_body, error_response};
use crate::api::helpers::{shared_repository, spawn_model_crud};
use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
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
    let state_clone = state.clone();

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
    if let Err(e) = validate_model_body(&body.model) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, e, Some("ValidationError"));
    }

    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };

    spawn_model_crud(state_clone, StatusCode::CREATED, move || {
        let repo = repo_handle.lock().unwrap();
        if repo
            .get_model_config_by_repo_id(&repo_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?
            .is_some()
        {
            return Err((
                StatusCode::CONFLICT,
                error_body(
                    format!("Model '{}' already exists", repo_id),
                    Some("ConflictError"),
                ),
            ));
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
        let model_id = repo
            .save_model_config(&repo_id, &model_config)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;

        Ok(serde_json::json!({ "ok": true, "id": model_id }))
    })
    .await
}
