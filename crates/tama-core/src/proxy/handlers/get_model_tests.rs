use super::models::handle_get_model;
use crate::config::{Config, ModelConfig};
use crate::proxy::ProxyState;
use axum::{
    body::to_bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

use super::tests::*;

// ── handle_get_model: basic config lookup tests ──────────────────────────

#[tokio::test]
async fn test_handle_get_model_by_config_key_returns_api_name() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Populate model_configs
    {
        let mut mc = state_arc.model_configs.write().await;
        mc.insert(
            "config-key-1".to_string(),
            ModelConfig {
                backend: "llama.cpp".to_string(),
                api_name: Some("api-name-1".to_string()),
                model: Some("test/model-1".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    let state = State(state_arc);

    let response = handle_get_model(state, Path("config-key-1".to_string())).await;
    let status = response.status();
    assert_eq!(status, StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json.get("id").unwrap().as_str(), Some("api-name-1"));
}

#[tokio::test]
async fn test_handle_get_model_by_api_name_returns_api_name() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Populate model_configs
    {
        let mut mc = state_arc.model_configs.write().await;
        mc.insert(
            "config-key-1".to_string(),
            ModelConfig {
                backend: "llama.cpp".to_string(),
                api_name: Some("api-name-1".to_string()),
                model: Some("test/model-1".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    let state = State(state_arc);

    let response = handle_get_model(state, Path("api-name-1".to_string())).await;
    let status = response.status();
    assert_eq!(status, StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json.get("id").unwrap().as_str(), Some("api-name-1"));
}

#[tokio::test]
async fn test_handle_get_model_without_api_name_falls_back_to_config_key() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Populate model_configs
    {
        let mut mc = state_arc.model_configs.write().await;
        mc.insert(
            "config-key-2".to_string(),
            ModelConfig {
                backend: "llama.cpp".to_string(),
                api_name: None,
                model: Some("test/model-2".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    let state = State(state_arc);

    let response = handle_get_model(state, Path("config-key-2".to_string())).await;
    let status = response.status();
    assert_eq!(status, StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json.get("id").unwrap().as_str(), Some("config-key-2"));
}

// ── handle_get_model: backend fetch tests ──────────────────────────────

/// Test that handle_get_model fetches from backend when model is loaded,
/// preserves `meta` data, and injects `ready: true`.
#[tokio::test]
async fn test_handle_get_model_fetches_from_backend_with_meta() {
    let mock_server = MockServer::start().await;

    // Mock backend returns model with meta
    let backend_response = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "llama3.gguf",
                "object": "model",
                "created": 1700000000,
                "owned_by": "backend1",
                "meta": {
                    "general_name": "Llama 3",
                    "architecture": "llama"
                }
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&backend_response))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = Config::default();
    let state = ProxyState::new(config, None);

    // Add model config
    {
        let mut mc = state.model_configs.write().await;
        mc.insert(
            "test-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("my-api-model".to_string()),
                model: Some("llama3.gguf".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    // Add a Ready model state
    {
        let mut models = state.models.write().await;
        models.insert(
            "test-model".to_string(),
            crate::proxy::ModelState::Ready {
                model_name: "test-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 1234,
                backend_url: mock_server.uri(),
                load_time: std::time::SystemTime::now(),
                last_accessed: std::time::Instant::now(),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
    }

    let state_arc = Arc::new(state);
    let state = State(state_arc.clone());

    // Query by config key
    let response = handle_get_model(state.clone(), Path("test-model".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // Should have meta from backend
    assert!(
        json.get("meta").is_some(),
        "meta should be preserved from backend response"
    );
    assert_eq!(
        json["meta"]["general_name"], "Llama 3",
        "meta.general_name should match backend response"
    );
    // ready should be injected as true
    assert_eq!(json["ready"], true, "Loaded model should have ready: true");
}

/// Test that handle_get_model falls back to config when model is not loaded.
/// Response should have no `meta` and `ready: false`.
#[tokio::test]
async fn test_handle_get_model_fallback_to_config_when_not_loaded() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Add model config but do NOT add it to loaded models
    {
        let mut mc = state_arc.model_configs.write().await;
        mc.insert(
            "unloaded-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("my-unloaded-model".to_string()),
                model: Some("test/unloaded".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    let state = State(state_arc.clone());

    // Query by config key
    let response = handle_get_model(state.clone(), Path("unloaded-model".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // Should use api_name as id
    assert_eq!(json["id"], "my-unloaded-model");
    // Should NOT have meta
    assert!(
        json.get("meta").is_none(),
        "Unloaded model should not have meta"
    );
    // ready should be false
    assert_eq!(
        json["ready"], false,
        "Unloaded model should have ready: false"
    );
}

/// Test that handle_get_model returns 404 for unknown model IDs.
#[tokio::test]
async fn test_handle_get_model_404_for_unknown_model() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    let state = State(state_arc.clone());

    // Query with a model_id that doesn't exist in config
    let response = handle_get_model(state.clone(), Path("totally-unknown-model".to_string())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["error"]["type"], "NotFoundError");
}

/// Test that handle_get_model works when backend returns multiple models
/// and matches by config's model field (file path).
#[tokio::test]
async fn test_handle_get_model_matches_by_model_field_when_multiple() {
    let mock_server = MockServer::start().await;

    // Backend returns multiple models
    let backend_response = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "/path/to/model-a.gguf",
                "object": "model",
                "created": 1700000000,
                "owned_by": "backend1"
            },
            {
                "id": "/path/to/model-b.gguf",
                "object": "model",
                "created": 1700000001,
                "owned_by": "backend1"
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&backend_response))
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = Config::default();
    let state = ProxyState::new(config, None);

    // Config's model field matches model-b
    {
        let mut mc = state.model_configs.write().await;
        mc.insert(
            "my-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("my-api-name".to_string()),
                model: Some("/path/to/model-b.gguf".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    {
        let mut models = state.models.write().await;
        models.insert(
            "my-model".to_string(),
            crate::proxy::ModelState::Ready {
                model_name: "my-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 5678,
                backend_url: mock_server.uri(),
                load_time: std::time::SystemTime::now(),
                last_accessed: std::time::Instant::now(),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
    }

    let state_arc = Arc::new(state);
    let state = State(state_arc.clone());

    let response = handle_get_model(state.clone(), Path("my-model".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // Should match model-b by config's model field, and normalize id to api_name
    assert_eq!(json["id"], "my-api-name", "Should normalize id to api_name");
    assert_eq!(json["ready"], true);
}

/// Test that handle_get_model falls back to config when backend query fails.
#[tokio::test]
async fn test_handle_get_model_backend_failure_fallback() {
    let config = Config::default();
    let state = ProxyState::new(config, None);

    // Add model config
    {
        let mut mc = state.model_configs.write().await;
        mc.insert(
            "fail-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("fail-api".to_string()),
                model: Some("test/fail".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    // Add a Ready model with unreachable backend URL
    {
        let mut models = state.models.write().await;
        models.insert(
            "fail-model".to_string(),
            crate::proxy::ModelState::Ready {
                model_name: "fail-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 9999,
                backend_url: "http://localhost:59999".to_string(),
                load_time: std::time::SystemTime::now(),
                last_accessed: std::time::Instant::now(),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
    }

    let state_arc = Arc::new(state);
    let state = State(state_arc.clone());

    let response = handle_get_model(state.clone(), Path("fail-model".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // Should fall back to config-based response
    assert_eq!(json["id"], "fail-api");
    assert!(json.get("meta").is_none());
    assert_eq!(json["ready"], false);
}

/// Test handle_get_model normalizes id when backend entry is found via alias.
#[tokio::test]
async fn test_handle_get_model_normalizes_id_from_alias() {
    let mock_server = MockServer::start().await;

    let backend_response = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "gemma-4-E2B-it-UD-IQ3_XXS.gguf",
                "object": "model",
                "created": 1779728594,
                "owned_by": "llamacpp",
                "aliases": ["unsloth/gemma-4-E2B-it-GGUF"],
                "meta": {"n_ctx": 32768}
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&backend_response))
        .mount(&mock_server)
        .await;

    let config = Config::default();
    let state = ProxyState::new(config, None);

    {
        let mut mc = state.model_configs.write().await;
        mc.insert(
            "gemma-e2b".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("unsloth/gemma-4-E2B-it-GGUF".to_string()),
                model: Some("unsloth/gemma-4-E2B-it-GGUF".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    {
        let mut models = state.models.write().await;
        models.insert(
            "gemma-e2b".to_string(),
            crate::proxy::ModelState::Ready {
                model_name: "gemma-e2b".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 1001,
                backend_url: mock_server.uri(),
                load_time: std::time::SystemTime::now(),
                last_accessed: std::time::Instant::now(),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
    }

    let state_arc = Arc::new(state);
    let state = State(state_arc.clone());

    // Look up by config key
    let response = handle_get_model(state.clone(), Path("gemma-e2b".to_string())).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // ID should be normalized to api_name
    assert_eq!(
        json["id"], "unsloth/gemma-4-E2B-it-GGUF",
        "ID should be normalized to api_name"
    );
    // Meta should be preserved
    assert!(json.get("meta").is_some(), "meta should be preserved");
    assert_eq!(json["ready"], true);

    // Also look up by api_name
    let response = handle_get_model(
        state.clone(),
        Path("unsloth/gemma-4-E2B-it-GGUF".to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(json["id"], "unsloth/gemma-4-E2B-it-GGUF");
    assert_eq!(json["ready"], true);
}
