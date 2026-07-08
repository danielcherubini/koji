use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::RemoveVersionQuery;
use crate::api::backends::types::DeleteResponse;
use crate::api::error::error_response;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// DELETE /tama/v1/backends/:name/versions/:version
pub async fn remove_backend_version(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path((name, version)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<RemoveVersionQuery>,
) -> impl IntoResponse {
    // Validate path params (prevent path traversal)
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }
    if version.contains('/') || version.contains('\\') || version.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid version: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    // Open manager and get the specific version
    let config_dir_clone = config_dir.clone();
    let mgr_result: Result<tama_core::backends::BackendManager, _> =
        tokio::task::spawn_blocking(move || {
            tama_core::backends::BackendManager::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    let mgr = match mgr_result {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to open manager: {}", e),
                None,
            )
        }
    };

    // Use gpu_variant from query param if provided
    let gpu_variant_filter = query.gpu_variant.clone();

    // Get the specific version record before deleting
    let versions = match mgr.list_versions(&name, gpu_variant_filter.as_deref()) {
        Ok(Some(v)) => v,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Backend '{}' version '{}' not found", name, version)
                })),
            )
                .into_response();
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
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Backend '{}' version '{}' not found", name, version)
                })),
            )
                .into_response();
        }
        1 => matches[0].clone(),
        _ if gpu_variant_filter.is_some() => matches[0].clone(),
        _ => {
            // Multiple variants have the same version - require gpu_variant
            let variant_list: Vec<String> = matches.iter().map(|v| v.gpu_variant.clone()).collect();
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "Version '{}' exists in multiple variants for backend '{}'. Please specify gpu_variant. Available: {}",
                        version, name, variant_list.join(", ")
                    )
                })),
            )
                .into_response();
        }
    };

    // Delete files FIRST (before any DB changes)
    let info_to_remove = tama_core::backends::BackendInfo {
        name: info.name.clone(),
        backend_type: info.backend_type.clone(),
        version: info.version.clone(),
        path: std::path::PathBuf::from(&info.path),
        installed_at: info.installed_at,
        gpu_variant: info.gpu_variant.clone(),
        source: None,
    };

    // Check if a job is running for this backend
    if let Some(jobs) = web_state.jobs.as_ref() {
        if let Some(active_job) = jobs.active().await {
            let active_type = active_job
                .backend_type
                .as_ref()
                .map(|b| b.to_string())
                .unwrap_or_default();
            if active_type == info.backend_type.to_string() {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "a job is currently running for this backend"
                    })),
                )
                    .into_response();
            }
        }
    }

    if info_to_remove.path.exists() {
        if let Err(e) = tama_core::backends::safe_remove_installation(&info_to_remove) {
            let err_msg = e.to_string();
            if err_msg.contains("outside the managed backends directory") {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "path is outside the managed backends directory; remove manually"
                    })),
                )
                    .into_response();
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to remove files: {}", e),
                None,
            );
        }
    }

    // Remove from DB (activates another version if this was active)
    if let Err(e) = mgr.remove_version(&name, &info.gpu_variant, &version) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove version: {}", e),
            None,
        );
    }

    // Clean up update_check records — use LIKE pattern to match all variants
    // (e.g., "llama_cpp:cpu", "llama_cpp:cuda") plus legacy format.
    if let Ok(repo) = tama_core::db::repository::Repository::open(&config_dir) {
        let escaped_name = name
            .replace('\\', "\\\\")
            .replace('_', "\\_")
            .replace('%', "\\%");
        let pattern = format!("{}:%", escaped_name);
        let _ = repo.delete_update_checks_by_pattern("backend", &pattern);
        // Also delete legacy format (no variant separator)
        let _ = repo.delete_update_check("backend", &name);
    }

    Json(DeleteResponse { removed: true }).into_response()
}
