//! Status, health, and metrics handlers.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::proxy::ProxyState;

/// Returns the current proxy status.
///
/// Builds a JSON status response from the proxy state including backend
/// information and runtime status.
#[axum::debug_handler]
pub async fn handle_status(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let response = state.build_status_response().await;
    Json(response)
}

/// Reloads model configurations from disk.
///
/// Triggers a hot reload of all model configurations. Returns JSON
/// `{ "ok": true }` on success or a 500 error with details on failure.
#[axum::debug_handler]
pub async fn handle_reload_configs(state: State<Arc<ProxyState>>) -> impl IntoResponse {
    match state.reload_model_configs().await {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Health check endpoint.
///
/// Returns a static JSON response indicating service health.
/// Useful for container orchestration and monitoring systems.
#[axum::debug_handler]
pub async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "tama-proxy"
    }))
}

/// Returns current proxy metrics.
///
/// Provides JSON metrics including request counters, model load/unload
/// counts, and the current number of active models.
#[axum::debug_handler]
pub async fn handle_metrics(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    use std::sync::atomic::Ordering::Relaxed;
    let metrics = &state.metrics;
    Json(serde_json::json!({
        "total_requests": metrics.total_requests.load(Relaxed),
        "successful_requests": metrics.successful_requests.load(Relaxed),
        "failed_requests": metrics.failed_requests.load(Relaxed),
        "models_loaded": metrics.models_loaded.load(Relaxed),
        "models_unloaded": metrics.models_unloaded.load(Relaxed),
        "active_models": state.models.read().await.len(),
    }))
}
