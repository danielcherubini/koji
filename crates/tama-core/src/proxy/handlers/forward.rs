use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::lifecycle::ensure_model_loaded;
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

    let backend_name = if let Some(ref model) = resolved_model {
        match ensure_model_loaded(&state, model, |resolved, e| {
            tracing::warn!("Failed to load model {}: {}", resolved, e);
            Err(anyhow::anyhow!("Failed to load model: {}", e))
        })
        .await
        {
            Ok(name) => name,
            Err(e) => {
                if let Some(resp) = crate::proxy::lifecycle::budget_exhausted_response_for(&e) {
                    return resp;
                }
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
    } else {
        // No model field — forward to the first live row (no host = no
        // models, plan-193): rows are the wire truth, not the mirror.
        let rows = crate::proxy::live_rows(state.tamad_pool().as_ref()).await;
        if let Some(first) = rows.all().first() {
            first.key.clone()
        } else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": {
                        "message": "No backend available",
                        "type": "ServiceUnavailableError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Check if this is a remote provider (sentinel from ensure_model_loaded)
    if let Some(provider_id_str) = backend_name.strip_prefix("remote:") {
        if let Ok(provider_id) = provider_id_str.parse::<i64>() {
            let pool = state.db_pool.as_ref();
            let provider = crate::db::queries::get_provider_by_id(pool, provider_id)
                .await
                .ok()
                .flatten();
            if let Some(provider) = provider {
                let body = bytes::Bytes::copy_from_slice(&body_bytes);
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
        &state,
        &backend_name,
        &parts,
        &body_bytes,
        resolved_model.as_deref(),
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
    forward_to_backend(&state, parts, body).await
}

/// Forward a request to the first available backend.
///
/// Used by both the proxy's `handle_forward_get` and the web UI's root-level
/// fallback (`/*path`). GET requests don't carry a `model` field, so we
/// simply pick the first available backend.
pub async fn forward_to_backend(
    state: &Arc<ProxyState>,
    parts: http::request::Parts,
    body: Body,
) -> Response {
    let body_bytes = to_bytes(body, MAX_REQUEST_BODY_SIZE)
        .await
        .unwrap_or_default();

    let rows = crate::proxy::live_rows(state.tamad_pool().as_ref()).await;
    let backend_name = rows
        .all()
        .first()
        .map(|r| r.key.clone())
        .unwrap_or_else(String::new);

    if backend_name.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": {
                    "message": "No backend available",
                    "type": "ServiceUnavailableError"
                }
            })),
        )
            .into_response();
    }

    forward_request(state, &backend_name, &parts, &body_bytes, None).await
}
