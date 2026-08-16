use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::lifecycle::ensure_model_loaded;
use crate::proxy::ProxyState;
use anyhow::Context;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Json, Response},
};
use std::sync::Arc;
use tracing::info;

use super::json_error_response;
use crate::proxy::forward::{forward_request, normalize_reasoning_effort_body};

/// Resolve the model, find or load its server, and forward the request.
///
/// Both `handle_chat_completions` and `handle_stream_chat_completions`
/// delegate to this helper.
async fn resolve_and_load_server(
    state: &Arc<ProxyState>,
    model_name: &str,
    parts: Parts,
    body_bytes: &[u8],
    log_msg: &str,
) -> Response {
    info!("{log_msg}: {}", model_name);

    let resolved_model = state.resolve_alias(model_name).await;

    let backend_name = match ensure_model_loaded(state, model_name, |resolved, e| {
        tracing::warn!("Failed to load model {}: {}", resolved, e);
        Err(anyhow::anyhow!("Failed to load model: {}", e))
    })
    .await
    {
        Ok(name) => name,
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
    };

    // Check if this is a remote provider (sentinel from ensure_model_loaded)
    if let Some(provider_id_str) = backend_name.strip_prefix("remote:") {
        if let Ok(provider_id) = provider_id_str.parse::<i64>() {
            // Fetch provider from DB and forward
            let provider = match state.db_pool.as_deref() {
                Some(pool) => crate::db::queries::get_provider_by_id(pool, provider_id)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            if let Some(provider) = provider {
                // ADR-0009: rewrite reasoning_effort "off" → "none" before the
                // remote provider. Re-serialize only when the body was actually
                // modified — otherwise forward the original bytes untouched,
                // preserving zero-copy behavior.
                let body = if let Ok(mut value) =
                    serde_json::from_slice::<serde_json::Value>(body_bytes)
                {
                    if normalize_reasoning_effort_body(&mut value) {
                        bytes::Bytes::from(
                            serde_json::to_vec(&value).unwrap_or_else(|_| body_bytes.to_vec()),
                        )
                    } else {
                        bytes::Bytes::copy_from_slice(body_bytes)
                    }
                } else {
                    bytes::Bytes::copy_from_slice(body_bytes)
                };
                match state
                    .remote_forwarder
                    .forward(&provider, &parts, body)
                    .await
                {
                    Ok(response) => return response.into_response(),
                    Err(e) => {
                        tracing::error!(
                            "Failed to forward request to remote provider '{}': {}",
                            provider.name,
                            e
                        );
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(serde_json::json!({
                                "error": {
                                    "message": format!("Remote provider error: {}", e),
                                    "type": "RemoteForwardError"
                                }
                            })),
                        )
                            .into_response();
                    }
                }
            }
        }
    }

    forward_request(
        state,
        &backend_name,
        &parts,
        body_bytes,
        Some(&resolved_model),
    )
    .await
}

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

    resolve_and_load_server(
        &state.0,
        model_name,
        parts,
        &body_bytes,
        "Routing request for model",
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

    resolve_and_load_server(
        &state.0,
        model_name,
        parts,
        &body_bytes,
        "Streaming request for model",
    )
    .await
}
