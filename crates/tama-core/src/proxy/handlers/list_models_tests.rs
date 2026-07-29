use super::models::{handle_list_models, parse_models_response, BackendModelEntry};
use crate::config::{Config, ModelConfig};
use crate::proxy::ProxyState;
use axum::{body::to_bytes, extract::State, response::IntoResponse};
use serde_json::Value as JsonValue;
use std::sync::Arc;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

use super::tests::*;

// ── parse_models_response tests ──────────────────────────────────────────

#[test]
fn test_parse_models_response_valid_data() {
    let body = serde_json::json!({
        "object": "list",
        "data": [
            {"id": "model-1", "object": "model"},
            {"id": "model-2", "object": "model"}
        ]
    });
    let result = parse_models_response(body.to_string().as_bytes());
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].id.as_deref(), Some("model-1"));
    assert_eq!(result[1].id.as_deref(), Some("model-2"));
}

#[test]
fn test_parse_models_response_invalid_json() {
    let result = parse_models_response(b"this is not json");
    assert!(result.is_empty());
}

#[test]
fn test_parse_models_response_missing_data_field() {
    let body = serde_json::json!({
        "object": "list"
    });
    let result = parse_models_response(body.to_string().as_bytes());
    assert!(result.is_empty());
}

#[test]
fn test_parse_models_response_data_not_array() {
    let body = serde_json::json!({
        "object": "list",
        "data": "not an array"
    });
    let result = parse_models_response(body.to_string().as_bytes());
    assert!(result.is_empty());
}

#[test]
fn test_parse_models_response_empty_data_array() {
    let body = serde_json::json!({
        "object": "list",
        "data": []
    });
    let result = parse_models_response(body.to_string().as_bytes());
    assert!(result.is_empty());
}

#[test]
fn test_parse_models_response_empty_body() {
    let result = parse_models_response(b"");
    assert!(result.is_empty());
}

#[test]
fn test_parse_models_response_data_is_object() {
    let body = serde_json::json!({
        "data": {"id": "single-model"}
    });
    let result = parse_models_response(body.to_string().as_bytes());
    assert!(result.is_empty());
}

// ── handle_list_models tests ─────────────────────────────────────────────

#[tokio::test]
async fn test_handle_list_models_returns_api_name() {
    let state_inner = create_test_state();
    let state_arc = Arc::new(state_inner);

    // Populate model_configs
    {
        let mut mc = state_arc.registry.model_configs.write().await;
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

    let state = State(state_arc.clone());

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();
    // 2 models (no wildcard entry)
    assert_eq!(data.len(), 2);

    // Collect all model ids
    let ids: Vec<&str> = data
        .iter()
        .map(|m| m.get("id").unwrap().as_str().unwrap())
        .collect();

    // Verify all expected ids are present
    assert!(
        ids.contains(&"api-name-1"),
        "Expected 'api-name-1' in model ids, got: {:?}",
        ids
    );
    assert!(
        ids.contains(&"config-key-2"),
        "Expected 'config-key-2' in model ids, got: {:?}",
        ids
    );
}

// ── handle_list_models: backend merge tests ──────────────────────────────

/// Test that handle_list_models merges models from two mock backends,
/// preserves `meta` data from backend responses, and injects `ready`.
#[tokio::test]
async fn test_handle_list_models_merges_backend_responses_with_meta() {
    let mock_server1 = MockServer::start().await;
    let mock_server2 = MockServer::start().await;

    // Mock backend 1: returns model with meta
    let backend1_response = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "llama3.gguf",
                "object": "model",
                "created": 1700000000,
                "owned_by": "backend1",
                "meta": {
                    "general_name": "Llama 3",
                    "general_tags": ["llama"],
                    "architecture": "llama"
                }
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&backend1_response))
        .expect(1)
        .mount(&mock_server1)
        .await;

    // Mock backend 2: returns model with meta
    let backend2_response = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "mistral.gguf",
                "object": "model",
                "created": 1700000001,
                "owned_by": "backend2",
                "meta": {
                    "general_name": "Mistral",
                    "architecture": "mistral"
                }
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&backend2_response))
        .expect(1)
        .mount(&mock_server2)
        .await;

    let state_arc = create_state_with_two_backends(&mock_server1.uri(), &mock_server2.uri()).await;
    let state = State(state_arc.clone());

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();

    // Should have: 2 from backends + 3 from config = 5
    // Backend IDs (llama3.gguf, mistral.gguf) don't match config api_names,
    // so all config entries are also added.
    assert_eq!(data.len(), 5, "Expected 5 entries, got: {}", data.len());

    // Collect model entries
    let model_entries: Vec<_> = data.iter().collect();

    // Find the model from backend 1
    let backend1_model = model_entries
        .iter()
        .find(|e| e["id"] == "llama3.gguf")
        .expect("llama3.gguf should be in response");

    // Verify meta is preserved from backend response
    assert!(
        backend1_model.get("meta").is_some(),
        "meta should be preserved from backend response"
    );
    assert_eq!(
        backend1_model["meta"]["general_name"], "Llama 3",
        "meta.general_name should match backend response"
    );
    assert_eq!(
        backend1_model["ready"], true,
        "Loaded model should have ready: true"
    );

    // Find the model from backend 2
    let backend2_model = model_entries
        .iter()
        .find(|e| e["id"] == "mistral.gguf")
        .expect("mistral.gguf should be in response");

    assert!(
        backend2_model.get("meta").is_some(),
        "meta should be preserved from backend response"
    );
    assert_eq!(
        backend2_model["ready"], true,
        "Loaded model should have ready: true"
    );

    // Find the unloaded model (from config)
    let unloaded_model = model_entries
        .iter()
        .find(|e| e["id"] == "api-model-c")
        .expect("api-model-c should be in response as unloaded");
    assert_eq!(
        unloaded_model["ready"], false,
        "Unloaded model should have ready: false"
    );
    assert!(
        unloaded_model.get("meta").is_none(),
        "Unloaded model should not have meta"
    );
}

/// Test that unloaded models (in config but not loaded on any backend)
/// still appear with ready: false and no meta.
#[tokio::test]
async fn test_handle_list_models_unloaded_from_config() {
    let config = Config::default();
    let state = ProxyState::new(config, None);

    // Add model configs — all enabled, none loaded
    {
        let mut mc = state.registry.model_configs.write().await;
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
        // Disabled model should NOT appear
        mc.insert(
            "disabled-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("disabled-model".to_string()),
                model: Some("test/disabled".to_string()),
                enabled: false,
                ..Default::default()
            },
        );
    }

    // No models loaded
    let state_arc = Arc::new(state);
    let state = State(state_arc.clone());

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();

    // Should have: 1 unloaded (disabled excluded)
    assert_eq!(data.len(), 1, "Expected 1 entry, got: {}", data.len());

    // Unloaded model
    assert_eq!(data[0]["id"], "my-unloaded-model");
    assert_eq!(data[0]["ready"], false);
    assert!(data[0].get("meta").is_none());
}

/// Test that duplicate model IDs across backends are deduplicated.
#[tokio::test]
async fn test_handle_list_models_deduplicates_model_ids() {
    let mock_server1 = MockServer::start().await;
    let mock_server2 = MockServer::start().await;

    // Both backends return the same model id
    let same_response = serde_json::json!({
        "object": "list",
        "data": [
            {"id": "duplicate-model", "object": "model", "created": 100}
        ]
    });
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&same_response))
        .mount(&mock_server1)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&same_response))
        .mount(&mock_server2)
        .await;

    let state_arc = create_state_with_two_backends(&mock_server1.uri(), &mock_server2.uri()).await;
    let state = State(state_arc.clone());

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();

    // Count occurrences of "duplicate-model"
    let dup_count = data.iter().filter(|e| e["id"] == "duplicate-model").count();
    assert_eq!(
        dup_count, 1,
        "duplicate-model should appear exactly once, found {} times",
        dup_count
    );
}

/// Test that backend failure falls back to config-based entry.
#[tokio::test]
async fn test_handle_list_models_backend_failure_fallback() {
    // Don't mount any mock — the backend URL will be unreachable
    let state_arc = create_state_with_two_backends(
        "http://localhost:59999", // unreachable
        "http://localhost:59998", // unreachable
    )
    .await;
    let state = State(state_arc.clone());

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();

    // Should still have entries from config fallback
    // model-a + model-b + model-c = 3
    assert_eq!(data.len(), 3, "Expected 3 entries from config fallback");
}

/// Test response shape matches OpenAI spec.
#[tokio::test]
async fn test_handle_list_models_response_shape() {
    let config = Config::default();
    let state = ProxyState::new(config, None);
    let state_arc = Arc::new(state);
    let state = State(state_arc.clone());

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    // Must have "object": "list" at top level
    assert_eq!(json["object"], "list");
    assert!(json["data"].is_array());
}

// ── Alias-based deduplication tests ──────────────────────────────────────

/// Test that when a backend entry has an alias matching a config's api_name,
/// the entry's id is normalized and no duplicate fallback entry is added.
#[tokio::test]
async fn test_handle_list_models_alias_deduplication() {
    let mock_server = MockServer::start().await;

    // Backend returns a model with filename id and an alias matching the api_name
    let backend_response = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "gemma-4-E2B-it-UD-IQ3_XXS.gguf",
                "object": "model",
                "created": 1779728594,
                "owned_by": "llamacpp",
                "aliases": ["unsloth/gemma-4-E2B-it-GGUF"],
                "tags": [],
                "meta": {
                    "n_ctx": 32768,
                    "n_params": "4647450147"
                }
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

    // Config has api_name matching the alias
    {
        let mut mc = state.registry.model_configs.write().await;
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

    // Add a Ready model
    {
        let mut models = state.registry.models.write().await;
        models.insert(
            "gemma-e2b".to_string(),
            crate::proxy::BackendState::Ready {
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

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();

    // Should have: 1 from backend (normalized) = 1
    // The fallback config entry should NOT be added because the alias matched.
    assert_eq!(data.len(), 1, "Expected 1 entry, got: {:?}", data);

    // The model entry should have the normalized id (api_name), not the filename
    assert_eq!(
        data[0]["id"], "unsloth/gemma-4-E2B-it-GGUF",
        "Entry id should be normalized to api_name"
    );
    // Meta should be preserved from backend
    assert!(
        data[0].get("meta").is_some(),
        "meta should be preserved from backend response"
    );
    assert_eq!(data[0]["ready"], true, "Loaded model should be ready");

    // Verify no duplicate with the original filename id
    let has_filename_id = data
        .iter()
        .any(|e| e["id"] == "gemma-4-E2B-it-UD-IQ3_XXS.gguf");
    assert!(
        !has_filename_id,
        "Should not have entry with original filename id"
    );
}

/// Test that backend entries without matching aliases are NOT normalized.
#[tokio::test]
async fn test_handle_list_models_no_alias_no_normalization() {
    let mock_server = MockServer::start().await;

    // Backend returns a model without aliases (or with non-matching aliases)
    let backend_response = serde_json::json!({
        "object": "list",
        "data": [
            {
                "id": "some-random-model.gguf",
                "object": "model",
                "created": 100,
                "owned_by": "llamacpp",
                "aliases": ["some-other-alias"],
                "meta": {"n_ctx": 4096}
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
        let mut mc = state.registry.model_configs.write().await;
        mc.insert(
            "my-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("my-api-name".to_string()),
                model: Some("test/model".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    {
        let mut models = state.registry.models.write().await;
        models.insert(
            "my-model".to_string(),
            crate::proxy::BackendState::Ready {
                model_name: "my-model".to_string(),
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

    let response = handle_list_models(state).await;
    let (_parts, body) = response.into_response().into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let json: JsonValue = serde_json::from_slice(&bytes).unwrap();

    let data = json.get("data").unwrap().as_array().unwrap();

    // Should have: 1 from backend + 1 from config (unloaded fallback) = 2
    // The backend entry is NOT normalized (alias doesn't match any api_name)
    // The config fallback IS added (api_name not in seen_ids)
    assert_eq!(data.len(), 2, "Expected 2 entries, got: {:?}", data);

    // Backend entry keeps its original filename id
    let backend_entry = data.iter().find(|e| e["id"] == "some-random-model.gguf");
    assert!(
        backend_entry.is_some(),
        "Backend entry should keep original id"
    );

    // Config fallback entry is present
    let config_entry = data.iter().find(|e| e["id"] == "my-api-name");
    assert!(config_entry.is_some(), "Config fallback should be present");
}

/// Parse an entry with a custom (unknown) field and verify it round-trips
/// through parse → serialize with the custom field intact (flatten passthrough).
#[test]
fn test_parse_models_response_preserves_extra_fields() {
    let body = serde_json::json!({
        "object": "list",
        "data": [
            {"id": "m", "custom_field": 42, "another": "hello"}
        ]
    });
    let result = parse_models_response(body.to_string().as_bytes());
    assert_eq!(result.len(), 1);

    // Typed field read
    assert_eq!(result[0].id.as_deref(), Some("m"));

    // Extra fields preserved in the flatten map
    assert_eq!(
        result[0].extra.get("custom_field"),
        Some(&serde_json::json!(42))
    );
    assert_eq!(
        result[0].extra.get("another"),
        Some(&serde_json::json!("hello"))
    );

    // Round-trip: serialize back to JSON and verify custom fields survive
    let serialized = serde_json::to_value(&result[0]).unwrap();
    assert_eq!(serialized["id"], "m");
    assert_eq!(serialized["custom_field"], 42);
    assert_eq!(serialized["another"], "hello");
}

/// Test that find_model_in_entries matches by aliases array.
#[test]
fn test_find_model_in_entries_matches_by_alias() {
    let entries = vec![
        BackendModelEntry {
            id: Some("model-a.gguf".to_string()),
            aliases: Some(vec!["api-name-a".to_string()]),
            ..Default::default()
        },
        BackendModelEntry {
            id: Some("model-b.gguf".to_string()),
            aliases: Some(vec!["api-name-b".to_string()]),
            ..Default::default()
        },
    ];

    // Match by alias
    let result = super::models::find_model_in_entries(&entries, Some("api-name-b"));
    assert!(result.is_some());
    assert_eq!(result.unwrap().id.as_deref(), Some("model-b.gguf"));

    // Match by id (file path)
    let result = super::models::find_model_in_entries(&entries, Some("model-a.gguf"));
    assert!(result.is_some());
    assert_eq!(result.unwrap().id.as_deref(), Some("model-a.gguf"));

    // No match
    let result = super::models::find_model_in_entries(&entries, Some("not-found"));
    // Returns first entry as best guess
    assert!(result.is_some());
    assert_eq!(result.unwrap().id.as_deref(), Some("model-a.gguf"));
}

/// Test that find_model_in_entries prefers single entry without matching.
#[test]
fn test_find_model_in_entries_single_entry() {
    let entries = vec![BackendModelEntry {
        id: Some("only-model.gguf".to_string()),
        aliases: Some(vec!["some-alias".to_string()]),
        ..Default::default()
    }];

    // Single entry: should return it regardless of config_model
    let result = super::models::find_model_in_entries(&entries, Some("different-name"));
    assert!(result.is_some());
    assert_eq!(result.unwrap().id.as_deref(), Some("only-model.gguf"));
}
