use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::RemoveVersionQuery;
use crate::api::error::error_response;
use crate::api::installations::tamad_job;
use crate::api::installations::types::DeleteResponse;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// DELETE /tama/v1/backends/:name/versions/:version
///
/// Removes one version of a backend: `RemoveProvider` on the backend's
/// tamad (versioned directory deletion), then the DB row cleanup.
/// Tamad failure → 500 with nothing removed from the DB (fail loud).
pub async fn remove_installation_version(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path((name, version)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<RemoveVersionQuery>,
) -> impl IntoResponse {
    // Validate path params (prevent path traversal)
    if let Err(resp) = crate::api::installations::reject_traversal(&name, "backend name") {
        return resp;
    }
    if let Err(resp) = crate::api::installations::reject_traversal(&version, "version") {
        return resp;
    }

    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool);

    // Use gpu_variant from query param if provided
    let gpu_variant_filter = query.gpu_variant.clone();

    // Get the specific version record before deleting
    let versions = match mgr
        .list_versions(&name, gpu_variant_filter.as_deref())
        .await
    {
        Ok(Some(v)) => v,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("Backend '{}' version '{}' not found", name, version),
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

    // Find matching versions and check for ambiguity
    let matches: Vec<_> = versions.iter().filter(|v| v.version == version).collect();
    let info = match matches.len() {
        0 => {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("Backend '{}' version '{}' not found", name, version),
                Some("NotFoundError"),
            )
        }
        1 => matches[0].clone(),
        _ if gpu_variant_filter.is_some() => matches[0].clone(),
        _ => {
            // Multiple variants have the same version - require gpu_variant
            let variant_list: Vec<String> = matches.iter().map(|v| v.gpu_variant.clone()).collect();
            return error_response(
                StatusCode::BAD_REQUEST,
                format!(
                    "Version '{}' exists in multiple variants for backend '{}'. Please specify gpu_variant. Available: {}",
                    version, name, variant_list.join(", ")
                ),
                Some("ValidationError"),
            );
        }
    };

    // Block the removal if a backend job is currently running for this
    // backend type (same guard as before tamad dispatch).
    if let Some(jobs) = web_state.jobs.as_ref() {
        if let Some(active_job) = jobs.active().await {
            let active_type = active_job
                .backend_type
                .as_ref()
                .map(|b| b.to_string())
                .unwrap_or_default();
            if active_type == info.backend_type.to_string() {
                return error_response(
                    StatusCode::CONFLICT,
                    "a job is currently running for this backend",
                    Some("ConflictError"),
                );
            }
        }
    }

    // Delete the installation on the backend host FIRST (before any DB
    // changes): tamad RemoveProvider removes the versioned directory for
    // this backend/variant/version (idempotent if already gone).
    if let Err(e) = tamad_job::remove_on_tamad(
        &state,
        &info.backend_type,
        &name,
        Some(info.gpu_variant.as_str()),
        Some(version.as_str()),
    )
    .await
    {
        tracing::warn!(backend = %name, version = %version, error = %e, "tamad version removal failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove version on host: {}", e),
            None,
        );
    }

    // Remove from DB (activates another version if this was active)
    if let Err(e) = mgr.remove_version(&name, &info.gpu_variant, &version).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove version: {}", e),
            None,
        );
    }

    // Clean up update_check records — use LIKE pattern to match all variants
    // (e.g., "llama_cpp:cpu", "llama_cpp:cuda") plus legacy format.
    // (Postgres, plan-190 Task 4; best-effort.)
    let pool = state.db_pool();
    let _ = tama_core::db::queries::delete_update_checks_for_backend(&pool, &name).await;

    Json(DeleteResponse { removed: true }).into_response()
}
