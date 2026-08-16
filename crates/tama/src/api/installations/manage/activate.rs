use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::ActivateQuery;
use crate::api::error::error_response;
use crate::api::installations::types::{ActivateRequest, ActivateResponse};
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/activate
pub async fn activate_installation_version(
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ActivateQuery>,
    Json(req): Json<ActivateRequest>,
) -> impl IntoResponse {
    // Validate name
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

            let version_clone = req.version.clone();

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

    let version_clone = req.version.clone();
    let version_for_error = version_clone.clone();

    let activated = match mgr.activate(&name, &gpu_variant, &version_clone).await {
        Ok(activated) => activated,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to activate: {}", e),
                None,
            )
        }
    };

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
