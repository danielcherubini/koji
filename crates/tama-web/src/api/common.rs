use axum::{
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

use tama_core::backends::BackendManager;
use tama_core::proxy::ProxyState;
use tama_core::web_types::JobManager;

/// Extract the config directory from the proxy state.
/// Returns an HTTP 404 response if not configured.
pub fn get_config_dir(
    state: &Arc<ProxyState>,
) -> Result<PathBuf, impl IntoResponse> {
    state
        .db_dir
        .clone()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "config_dir not configured"})),
            )
        })
}

/// Extract the job manager from the proxy state.
/// Returns an HTTP 500 response if not configured.
pub fn get_jobs(state: &Arc<ProxyState>) -> Result<Arc<JobManager>, impl IntoResponse> {
    state
        .web_jobs
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "job manager not configured"})),
            )
        })
}

/// Validate a path parameter to prevent path traversal attacks.
/// Returns an HTTP 400 response if the name contains path separators or traversal sequences.
pub fn validate_path_param(name: &str) -> Result<(), impl IntoResponse> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid name: path separators or traversal sequences not allowed"})),
        ));
    }
    Ok(())
}

/// Open a BackendManager on a blocking thread.
/// Returns an error if the spawn fails or the manager cannot be opened.
pub async fn open_backend_manager(
    config_dir: PathBuf,
) -> anyhow::Result<BackendManager> {
    tokio::task::spawn_blocking(move || BackendManager::open(&config_dir))
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r)
}
