use super::chat::handle_chat_completions;
use super::models::handle_get_model;
use super::models::handle_list_models;
use crate::config::ModelConfig;
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::Value as JsonValue;
use std::sync::Arc;

use super::tests::*;

// ── Alias resolution tests ───────────────────────────────────────────────

/// Test that handle_chat_completions resolves aliases before routing.
/// When an alias is used, the handler should attempt to load the resolved model name.
#[tokio::test]
async fn test_chat_completions_resolves_alias() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Populate alias cache: "my-alias" -> "real-model-name"
    {
        let mut aliases = state_arc.aliases.write().await;
        aliases.insert("my-alias".to_string(), "real-model-name".to_string());
    }

    // Create request with alias as model name
    let body = serde_json::json!({
        "model": "my-alias",
        "messages": [{"role": "user", "content": "Hello"}]
    });
    let req = Request::post("/v1/chat/completions")
        .body(Body::from(body.to_string().into_bytes()))
        .unwrap();

    let state = State(state_arc.clone());
    let response = handle_chat_completions(state, req).await;

    // Since no model is loaded, the handler will try to load the resolved name.
    // The error should reference the resolved name, not the alias.
    let status = response.status();
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let (_parts, body_bytes) = response.into_response().into_parts();
    let bytes = to_bytes(body_bytes, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // Verify it tried to load the resolved model name, not the alias
    let error_type = json["error"]["type"].as_str().unwrap();
    assert_eq!(error_type, "LoadModelError");
}

/// Test that handle_list_models includes alias entries with alias: true flag.
#[tokio::test]
async fn test_list_models_includes_aliases() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Populate alias cache
    {
        let mut aliases = state_arc.aliases.write().await;
        aliases.insert("short-alias".to_string(), "owner--real-model".to_string());
        aliases.insert(
            "another-alias".to_string(),
            "owner--other-model".to_string(),
        );
    }

    let state = State(state_arc.clone());
    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();

    // Find alias entries
    let short_alias = data
        .iter()
        .find(|e| e["id"] == "short-alias")
        .expect("short-alias should be in model list");
    assert_eq!(short_alias["alias"], true, "alias flag should be true");
    assert_eq!(
        short_alias["resolves_to"], "owner--real-model",
        "resolves_to should match the resolved model name"
    );
    assert_eq!(short_alias["owned_by"], "tama-proxy");

    let another_alias = data
        .iter()
        .find(|e| e["id"] == "another-alias")
        .expect("another-alias should be in model list");
    assert_eq!(another_alias["alias"], true);
    assert_eq!(
        another_alias["resolves_to"], "owner--other-model",
        "resolves_to should match the resolved model name"
    );
}

/// Test that handle_get_model resolves aliases and returns alias name in response id.
#[tokio::test]
async fn test_get_model_resolves_alias() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Add a model config
    {
        let mut mc = state_arc.model_configs.write().await;
        mc.insert(
            "real-config".to_string(),
            ModelConfig {
                backend: "llama.cpp".to_string(),
                api_name: Some("real-model-name".to_string()),
                model: Some("test/real-model".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    // Add alias: "my-alias" -> "real-model-name" (the api_name)
    {
        let mut aliases = state_arc.aliases.write().await;
        aliases.insert("my-alias".to_string(), "real-model-name".to_string());
    }

    let state = State(state_arc.clone());

    // Query by alias name
    let response = handle_get_model(state.clone(), Path("my-alias".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // The response id should be the alias name, not the resolved name
    assert_eq!(
        json["id"], "my-alias",
        "Response id should be the alias name when queried via alias"
    );
    assert_eq!(
        json["ready"], false,
        "Unloaded model should have ready: false"
    );

    // Also verify that querying by the real api_name still works
    let response = handle_get_model(state.clone(), Path("real-model-name".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // When queried directly (not via alias), id should be the api_name
    assert_eq!(
        json["id"], "real-model-name",
        "Response id should be the api_name when queried directly"
    );
}
