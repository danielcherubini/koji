use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

use tama_core::proxy::ProxyState;

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
