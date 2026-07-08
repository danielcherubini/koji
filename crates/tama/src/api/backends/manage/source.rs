use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::SourceQuery;
use crate::api::backends::types::{UpdateSourceRequest, UpdateSourceResponse};
use crate::api::error::error_response;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/source
/// Updates the build method (source vs prebuilt) for a backend.
pub async fn update_backend_source(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<SourceQuery>,
    Json(req): Json<UpdateSourceRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    // Open manager and determine gpu_variant
    let config_dir_clone = config_dir.clone();
    let name_clone = name.clone();
    let query_gpu_variant = query.gpu_variant.clone();
    let mgr_result: Result<(tama_core::backends::BackendManager, String), _> =
        tokio::task::spawn_blocking(move || {
            let mgr = tama_core::backends::BackendManager::open(&config_dir_clone)?;

            // Determine gpu_variant: use explicit value or auto-infer from manager
            let gpu_variant = match query_gpu_variant {
                Some(v) => v,
                None => {
                    let versions = mgr.list_versions(&name_clone, None)?;
                    let versions = match versions {
                        Some(v) => v,
                        None => {
                            return Err(anyhow::anyhow!(
                                "Backend '{}' not found",
                                name_clone
                            ));
                        }
                    };
                    let mut variants: Vec<String> =
                        versions.iter().map(|v| v.gpu_variant.clone()).collect();
                    variants.sort();
                    variants.dedup();
                    match variants.len() {
                        1 => variants.into_iter().next().unwrap(),
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Backend '{}' has multiple variants. Please specify gpu_variant. Available: {}",
                                name_clone,
                                variants.join(", ")
                            ));
                        }
                    }
                }
            };

            // Validate resolved gpu_variant for path traversal
            if gpu_variant.contains('/') || gpu_variant.contains('\\') || gpu_variant.contains("..")
            {
                return Err(anyhow::anyhow!("Invalid gpu_variant: path separators or traversal sequences not allowed"));
            }

            Ok((mgr, gpu_variant))
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    let (mgr, gpu_variant) = match mgr_result {
        Ok(r) => r,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not found") {
                return error_response(StatusCode::NOT_FOUND, err_msg, Some("NotFoundError"));
            }
            if err_msg.contains("Invalid gpu_variant") || err_msg.contains("multiple variants") {
                return error_response(StatusCode::BAD_REQUEST, err_msg, Some("ValidationError"));
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to open manager: {}", e),
                None,
            );
        }
    };

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

    let name_for_update = name.clone();
    let gpu_variant_for_update = gpu_variant.clone();
    let build_from_source = req.build_from_source;

    let update_result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
        mgr.update_build_method(&name_for_update, &gpu_variant_for_update, build_from_source)
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
    .and_then(|r| r);

    match update_result {
        Ok(()) => Json(UpdateSourceResponse { build_from_source }).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update build method: {}", e),
            None,
        ),
    }
}
