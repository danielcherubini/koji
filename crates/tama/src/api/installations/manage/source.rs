use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::SourceQuery;
use crate::api::error::error_response;
use crate::api::installations::types::{UpdateSourceRequest, UpdateSourceResponse};
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/source
/// Updates the build method (source vs prebuilt) for a backend.
pub async fn update_installation_source(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<SourceQuery>,
    Json(req): Json<UpdateSourceRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if let Err(resp) = crate::api::installations::reject_traversal(&name, "backend name") {
        return resp;
    }

    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool);

    // Determine gpu_variant: use explicit value or auto-infer from manager
    let gpu_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            let versions = match mgr.list_versions(&name, None).await {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        format!("Backend '{}' not found", name),
                        Some("NotFoundError"),
                    )
                }
                Err(e) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to query backend: {}", e),
                        None,
                    )
                }
            };
            let mut variants: Vec<String> =
                versions.iter().map(|v| v.gpu_variant.clone()).collect();
            variants.sort();
            variants.dedup();
            match variants.len() {
                1 => variants.into_iter().next().unwrap(),
                _ => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        format!(
                            "Backend '{}' has multiple variants. Please specify gpu_variant. Available: {}",
                            name,
                            variants.join(", ")
                        ),
                        Some("ValidationError"),
                    )
                }
            }
        }
    };

    // Validate resolved gpu_variant for path traversal
    if crate::api::installations::is_path_traversal(&gpu_variant) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid gpu_variant: path separators or traversal sequences not allowed".to_string(),
            Some("ValidationError"),
        );
    }

    // Check for active job conflict
    if let Some(jobs) = web_state.jobs.as_ref() {
        if let Some(active_job) = jobs.active().await {
            if active_job.backend_type.as_ref().map(|b| b.to_string()) == Some(name.clone()) {
                return error_response(
                    StatusCode::CONFLICT,
                    "another backend job is already running",
                    Some("ConflictError"),
                );
            }
        }
    }

    let build_from_source = req.build_from_source;

    if let Err(e) = mgr
        .update_build_method(&name, &gpu_variant, build_from_source)
        .await
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update build method: {}", e),
            None,
        );
    }

    Json(UpdateSourceResponse { build_from_source }).into_response()
}
