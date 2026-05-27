use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::ProxyState;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde_json::Value as JsonValue;
use std::sync::Arc;

use super::super::forward::forward_request;

/// Fallback handler for unmatched routes.
#[axum::debug_handler]
pub async fn handle_fallback() -> StatusCode {
    StatusCode::NOT_FOUND
}

/// Wildcard POST handler: forwards all non-/tama/* requests to the backend.
/// Extracts `model` from the request body for auto-loading support.
#[axum::debug_handler]
pub async fn handle_forward_post(
    Path(_path): Path<String>,
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Request body too large",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response()
        }
    };

    // Try to extract model for auto-loading
    let model_name: Option<String> = serde_json::from_slice::<JsonValue>(&body_bytes)
        .ok()
        .and_then(|v| v.get("model")?.as_str().map(String::from));

    // Resolve alias before routing
    let resolved_model: Option<String> = if let Some(ref m) = model_name {
        Some(state.resolve_alias(m).await)
    } else {
        None
    };

    let server_name = if let Some(ref model) = resolved_model {
        match state.get_available_server_for_model(model).await {
            Some(name) => name,
            None => {
                let _ = state.evict_lru_if_needed().await;
                let card = state.get_model_card(model).await;
                match state.load_model(model, card.as_ref()).await {
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
                            .into_response()
                    }
                }
            }
        }
    } else {
        // No model field — forward to first available server or return error
        let models = state.models.read().await;
        if let Some(name) = models.keys().next().cloned() {
            drop(models);
            name
        } else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": {
                        "message": "No backend server available",
                        "type": "ServiceUnavailableError"
                    }
                })),
            )
                .into_response();
        }
    };

    state.update_last_accessed(&server_name).await;
    forward_request(
        &state,
        &server_name,
        &parts,
        &body_bytes,
        model_name.as_deref(),
    )
    .await
}

/// Wildcard GET handler: forwards all non-/tama/* requests to the backend.
#[axum::debug_handler]
pub async fn handle_forward_get(
    Path(_path): Path<String>,
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = to_bytes(body, MAX_REQUEST_BODY_SIZE)
        .await
        .unwrap_or_default();

    // GET requests don't have a model field — forward to any available server
    let models = state.models.read().await;
    let server_name = models.keys().next().cloned().unwrap_or_else(String::new);
    drop(models);

    if server_name.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "message": "No backend server available",
                    "type": "ServiceUnavailableError"
                }
            })),
        )
            .into_response();
    }

    forward_request(&state, &server_name, &parts, &body_bytes, None).await
}
