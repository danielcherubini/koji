use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::{
    DefaultArgsQuery, DefaultEnvQuery, UpdateDefaultArgsRequest, UpdateDefaultEnvRequest,
};
use crate::api::error::error_response;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/default-args
/// Update default_args for a backend in the backend_configs DB table.
pub async fn update_backend_default_args(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultArgsQuery>,
    Json(req): Json<UpdateDefaultArgsRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if backend_name.contains('/') || backend_name.contains('\\') || backend_name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let backend_name = backend_name.clone();
    let gpu_variant = query.gpu_variant.clone();
    let default_args = req.default_args.clone();

    let result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
        let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
        // Preserve existing default_env when updating default_args
        let existing_env = mgr.get_default_env(&backend_name, &gpu_variant);
        mgr.save_config(
            &backend_name,
            &gpu_variant,
            &default_args,
            &existing_env,
            None,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
    .and_then(|r| r);

    match result {
        Ok(()) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update backend config: {}", e),
            None,
        ),
    }
}

/// POST /tama/v1/backends/:name/default-env
/// Update default_env for a backend in the backend_configs DB table.
pub async fn update_backend_default_env(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultEnvQuery>,
    Json(req): Json<UpdateDefaultEnvRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if backend_name.contains('/') || backend_name.contains('\\') || backend_name.contains("..") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name: path separators or traversal sequences not allowed",
            Some("ValidationError"),
        );
    }

    let config_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let backend_name = backend_name.clone();
    let gpu_variant = query.gpu_variant.clone();
    let default_env = req.default_env.clone();

    let result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
        let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
        // Preserve existing default_args when updating default_env
        let existing_args = mgr.get_default_args(&backend_name, &gpu_variant);
        mgr.save_config(
            &backend_name,
            &gpu_variant,
            &existing_args,
            &default_env,
            None,
        )?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
    .and_then(|r| r);

    match result {
        Ok(()) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update backend config: {}", e),
            None,
        ),
    }
}
