use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::ActivateQuery;
use crate::api::backends::types::{ActivateRequest, ActivateResponse};
use crate::api::error::error_response;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/activate
pub async fn activate_backend_version(
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ActivateQuery>,
    Json(req): Json<ActivateRequest>,
) -> impl IntoResponse {
    // Validate name
    if let Err(resp) = crate::api::backends::reject_traversal(&name, "backend name") {
        return resp;
    }

    let config_dir = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    // Determine gpu_variant: use explicit value or auto-infer from manager
    let gpu_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            let config_dir_clone = config_dir.clone();
            let name_clone = name.clone();
            let version_clone = req.version.clone();
            let infer_result: Result<Option<Vec<tama_core::backends::BackendInfo>>, anyhow::Error> =
                tokio::task::spawn_blocking(move || {
                    let mgr = tama_core::backends::BackendManager::open(&config_dir_clone)?;
                    mgr.list_versions(&name_clone, None)
                })
                .await
                .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
                .and_then(|r| r);

            let versions = match infer_result {
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

            // Collect unique variants
            let mut variants: Vec<String> =
                versions.iter().map(|v| v.gpu_variant.clone()).collect();
            variants.sort();
            variants.dedup();

            if variants.len() == 1 {
                // Only one variant exists — use it
                variants.into_iter().next().unwrap()
            } else {
                // Multiple variants — find the one that has the requested version
                let matching: Vec<String> = versions
                    .iter()
                    .filter(|v| v.version == version_clone)
                    .map(|v| v.gpu_variant.clone())
                    .collect();
                let mut matching = matching;
                matching.sort();
                matching.dedup();

                match matching.len() {
                    1 => matching.into_iter().next().unwrap(),
                    0 => {
                        return error_response(
                            StatusCode::NOT_FOUND,
                            format!(
                                "Version '{}' not found for backend '{}'. Available variants: {}",
                                version_clone,
                                name,
                                variants.join(", ")
                            ),
                            Some("NotFoundError"),
                        )
                    }
                    _ => {
                        // Multiple variants have the same version — ambiguous
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            format!(
                                "Version '{}' exists in multiple variants for backend '{}'. Please specify gpu_variant. Available variants: {}",
                                version_clone,
                                name,
                                matching.join(", ")
                            ),
                            Some("ValidationError"),
                        );
                    }
                }
            }
        }
    };

    let config_dir_clone = config_dir.clone();
    let version_clone = req.version.clone();
    let name_clone = name.clone();
    let version_for_error = version_clone.clone();
    let gpu_variant_clone = gpu_variant.to_string();
    let mgr_result: Result<(tama_core::backends::BackendManager, bool), _> =
        tokio::task::spawn_blocking(move || {
            let mgr = tama_core::backends::BackendManager::open(&config_dir_clone)?;
            let activated = mgr.activate(&name_clone, &gpu_variant_clone, &version_clone)?;
            Ok((mgr, activated))
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    match mgr_result {
        Ok((_, activated)) => {
            if !activated {
                return error_response(
                    StatusCode::NOT_FOUND,
                    format!(
                        "Version '{}' not found for backend '{}'",
                        version_for_error, name
                    ),
                    Some("NotFoundError"),
                );
            }

            Json(ActivateResponse {
                version: req.version,
                is_active: true,
            })
            .into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to activate: {}", e),
            None,
        ),
    }
}
