use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use std::sync::Arc;

use crate::api::error::{error_response, error_response_simple};
use tama_core::installations::InstallationManager;
use tama_core::proxy::ProxyState;

/// Resolve the config directory from ProxyState (`db_dir`, set at startup),
/// falling back to the system default config dir. Never falls back to the
/// process CWD. Returns the canonical 404 response when unconfigured.
#[allow(clippy::result_large_err)]
pub fn resolve_config_dir(
    state: &ProxyState,
) -> Result<std::path::PathBuf, axum::response::Response> {
    state
        .db_dir()
        .clone()
        .or_else(|| tama_core::config::Config::config_dir().ok())
        .ok_or_else(|| {
            error_response(
                StatusCode::NOT_FOUND,
                "config directory not configured",
                Some("NotFoundError"),
            )
        })
}

/// Default status code for successful CRUD operations.
pub const DEFAULT_CRUD_STATUS: StatusCode = StatusCode::OK;

/// Build an InstallationManager from the shared Postgres pool held in
/// ProxyState.
pub async fn open_backend_manager(
    proxy_state: &Arc<ProxyState>,
) -> Result<InstallationManager, axum::response::Response> {
    Ok(InstallationManager::new(proxy_state.db_pool()))
}

/// Run a closure in spawn_blocking, handle the Result, trigger proxy reload on success.
pub async fn spawn_model_crud<F, T>(
    proxy_state: Arc<ProxyState>,
    default_status: StatusCode,
    f: F,
) -> axum::response::Response
where
    F: FnOnce() -> Result<T, (StatusCode, serde_json::Value)> + Send + 'static,
    T: Serialize + Send + 'static,
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

#[cfg(test)]
mod tests {
    use super::*;
    use tama_core::config::Config;
    use tama_core::proxy::ProxyState;

    /// resolve_config_dir should prefer the db_dir from ProxyState when set.
    #[tokio::test]
    async fn test_resolve_config_dir_prefers_db_dir() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let state = ProxyState::new(
            Config::default(),
            Some(tmp_dir.path().to_path_buf()),
            tama_core::db::pool::test_dummy_pool(),
        );

        let resolved = resolve_config_dir(&state).unwrap();
        assert_eq!(resolved, tmp_dir.path());
    }

    /// resolve_config_dir falls back to the system config directory when db_dir
    /// is not set. The 404 branch (both db_dir and system config unavailable)
    /// is unreachable in practice on well-configured systems, so we only test
    /// the fallback path here.
    #[tokio::test]
    async fn test_resolve_config_dir_falls_back_to_system_dir() {
        let state = ProxyState::new(
            Config::default(),
            None,
            tama_core::db::pool::test_dummy_pool(),
        );

        let resolved = resolve_config_dir(&state).unwrap();
        let system_dir = Config::config_dir().expect("system config dir should be available");
        assert_eq!(resolved, system_dir);
    }
}
