use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::ProxyState;
use anyhow::Context;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use std::sync::Arc;
use tracing::info;

use super::json_error_response;
use crate::proxy::forward::forward_request;

#[axum::debug_handler]
pub async fn handle_chat_completions(
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (mut parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return json_error_response(),
    };

    // Normalise: clients that set base_url=http://host/v1 may POST to /v1 directly.
    // Rewrite to /v1/chat/completions so the backend gets the right path.
    if parts.uri.path() == "/v1" {
        if let Ok(uri) = "/v1/chat/completions".parse::<axum::http::Uri>() {
            parts.uri = uri;
        }
    }

    let request: serde_json::Value =
        match serde_json::from_slice(&body_bytes).context("Failed to parse request body") {
            Ok(r) => r,
            Err(_) => {
                return json_error_response();
            }
        };

    let model_name = match request.get("model").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Missing required field: model",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Resolve alias before routing
    let resolved_model = state.resolve_alias(model_name).await;
    info!("Routing request for model: {}", resolved_model);

    let server_name = match state.get_available_server_for_model(&resolved_model).await {
        Some(name) => name,
        None => {
            let _ = state.evict_lru_if_needed().await;
            let model_card = state.get_model_card(&resolved_model).await;
            match state.load_model(&resolved_model, model_card.as_ref()).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to load model {}: {}", resolved_model, e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!("Failed to load model: {}", e),
                                "type": "LoadModelError"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    state.update_last_accessed(&server_name).await;

    forward_request(
        &state,
        &server_name,
        &parts,
        &body_bytes,
        Some(&resolved_model),
    )
    .await
}

#[axum::debug_handler]
pub async fn handle_stream_chat_completions(
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return json_error_response(),
    };

    let request: serde_json::Value =
        match serde_json::from_slice(&body_bytes).context("Failed to parse request body") {
            Ok(r) => r,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Bad Request",
                            "type": "BadRequestError"
                        }
                    })),
                )
                    .into_response();
            }
        };

    let model_name = match request.get("model").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Missing required field: model",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Resolve alias before routing
    let resolved_model = state.resolve_alias(model_name).await;
    info!("Streaming request for model: {}", resolved_model);

    let server_name = match state.get_available_server_for_model(&resolved_model).await {
        Some(name) => name,
        None => {
            let _ = state.evict_lru_if_needed().await;
            let model_card = state.get_model_card(&resolved_model).await;
            match state.load_model(&resolved_model, model_card.as_ref()).await {
                Ok(s) => s,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!("Failed to load model: {}", e),
                                "type": "LoadModelError"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }
    };

    state.update_last_accessed(&server_name).await;

    forward_request(
        &state,
        &server_name,
        &parts,
        &body_bytes,
        Some(&resolved_model),
    )
    .await
}
