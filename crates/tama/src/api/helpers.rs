use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use std::sync::Arc;

use crate::api::error::error_response_simple;
use tama_core::backends::BackendManager;
use tama_core::proxy::ProxyState;

/// Default status code for successful CRUD operations.
pub const DEFAULT_CRUD_STATUS: StatusCode = StatusCode::OK;

/// Open a BackendManager from Arc<ProxyState>, returning an error response on failure.
pub async fn open_backend_manager(
    proxy_state: &Arc<ProxyState>,
) -> Result<BackendManager, axum::response::Response> {
    let config_dir = proxy_state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });
    let config_dir_clone = config_dir.clone();
    tokio::task::spawn_blocking(move || BackendManager::open(&config_dir_clone))
        .await
        .map_err(|e| {
            error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn error: {}", e),
            )
        })?
        .map_err(|e| error_response_simple(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Run a closure in spawn_blocking, handle the Result, trigger proxy reload on success.
pub async fn spawn_model_crud<F>(
    proxy_state: Arc<ProxyState>,
    default_status: StatusCode,
    f: F,
) -> axum::response::Response
where
    F: FnOnce() -> Result<serde_json::Value, (StatusCode, serde_json::Value)> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(val)) => {
            if let Err(e) = trigger_proxy_reload(&proxy_state).await {
                tracing::warn!("failed to trigger proxy reload: {:?}", e);
            }
            (default_status, Json(val)).into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(e) => error_response_simple(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Trigger the proxy to reload its model registry from the database.
async fn trigger_proxy_reload(
    proxy_state: &Arc<ProxyState>,
) -> Result<(), (StatusCode, serde_json::Value)> {
    use crate::api::error::error_body;

    proxy_state.reload_model_configs().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_body(format!("Failed to reload model configs: {}", e), None),
        )
    })?;
    // Aliases are nice-to-have; log a warning but don't fail the whole operation.
    if let Err(e) = proxy_state.reload_aliases().await {
        tracing::warn!(error = %e, "Failed to reload aliases");
    }
    Ok(())
}
