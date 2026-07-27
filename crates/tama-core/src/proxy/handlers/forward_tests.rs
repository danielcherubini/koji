use super::forward::{handle_forward_get, handle_forward_post};
use axum::{
    body::to_bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::Value as JsonValue;
use std::sync::Arc;

use super::tests::*;

// ── handle_forward_post tests ───────────────────────────────────────────

#[tokio::test]
async fn test_handle_forward_post_model_extraction_from_json_body() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    let body = serde_json::json!({
        "model": "my-test-model",
        "messages": [{"role": "user", "content": "Hello"}]
    });
    let req = create_forward_post_request(&body.to_string().into_bytes());

    let response = handle_forward_post(
        Path("v1/chat/completions".to_string()),
        State(state_arc.clone()),
        req,
    )
    .await;

    // Since no model is loaded and load_model will fail, expect an error response.
    // The key assertion: the handler DID extract the model name and attempted loading.
    let status = response.status();
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let (_parts, body_bytes) = response.into_response().into_parts();
    let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let error_type = json["error"]["type"].as_str().unwrap();
    assert_eq!(error_type, "LoadModelError");
}

#[tokio::test]
async fn test_handle_forward_post_non_json_body_does_not_crash() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Non-JSON body — model extraction should yield None
    let req = create_forward_post_request(b"not a json body at all");

    let response = handle_forward_post(
        Path("v1/chat/completions".to_string()),
        State(state_arc.clone()),
        req,
    )
    .await;

    // No servers available and no model field → 503
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let (_parts, body_bytes) = response.into_response().into_parts();
    let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(
        json["error"]["type"].as_str(),
        Some("ServiceUnavailableError")
    );
}

#[tokio::test]
async fn test_handle_forward_post_missing_model_no_servers_returns_503() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Empty body — no model field, no servers → 503
    let req = create_forward_post_request(b"");

    let response = handle_forward_post(
        Path("v1/chat/completions".to_string()),
        State(state_arc.clone()),
        req,
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let (_parts, body_bytes) = response.into_response().into_parts();
    let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(
        json["error"]["message"].as_str(),
        Some("No backend available")
    );
}

#[tokio::test]
async fn test_handle_forward_post_load_model_error_returns_500() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Request with a model that doesn't exist in config — load_model will fail
    let body = serde_json::json!({
        "model": "nonexistent-model-xyz",
        "messages": [{"role": "user", "content": "Test"}]
    });
    let req = create_forward_post_request(&body.to_string().into_bytes());

    let response = handle_forward_post(
        Path("v1/chat/completions".to_string()),
        State(state_arc.clone()),
        req,
    )
    .await;

    // load_model fails because the model isn't in config → 500
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let (_parts, body_bytes) = response.into_response().into_parts();
    let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["error"]["type"].as_str(), Some("LoadModelError"));
}

// ── handle_forward_get tests ──────────────────────────────────────────────

#[tokio::test]
async fn test_handle_forward_get_no_servers_returns_503() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    let req = create_forward_get_request();

    let response =
        handle_forward_get(Path("v1/models".to_string()), State(state_arc.clone()), req).await;

    // No models loaded → 503
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let (_parts, body_bytes) = response.into_response().into_parts();
    let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(
        json["error"]["message"].as_str(),
        Some("No backend available")
    );
}

#[tokio::test]
async fn test_handle_forward_get_empty_body_does_not_crash() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // GET with empty body — should not panic or crash
    let req = create_forward_get_request();

    let response =
        handle_forward_get(Path("v1/models".to_string()), State(state_arc.clone()), req).await;

    // With no servers, returns 503 (not a crash)
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
