use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;

use super::helpers::{call_list_models, create_state_with_model};
use crate::config::ModelConfig;
use crate::config::ModelModalities;
use crate::proxy::tama_handlers::models::OpencodeModelsResponse;

/// Loaded model: capabilities from /props appear in opencode response.
#[tokio::test]
async fn test_loaded_model_capabilities_from_props() {
    let mock_server = MockServer::start().await;

    // Mock /props with tool_call: true, reasoning: true
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": true
            }
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    // model-a and model-b are loaded, model-c is not
    for model in models {
        let id = model.get("id").unwrap().as_str().unwrap();
        if id == "api-model-a" || id == "api-model-b" {
            // Loaded models: should have capabilities from /props
            assert!(
                model.get("tool_call").unwrap().as_bool().unwrap(),
                "tool_call should be true for loaded model {}",
                id
            );
            assert!(
                model.get("reasoning").unwrap().as_bool().unwrap(),
                "reasoning should be true for loaded model {}",
                id
            );
            assert!(
                model.get("temperature").unwrap().as_bool().unwrap(),
                "temperature should always be true"
            );
        } else if id == "api-model-c" {
            // Unloaded model: should have defaults
            assert!(
                model.get("tool_call").unwrap().as_bool().unwrap(),
                "tool_call should default to true for unloaded model"
            );
            assert!(
                !model.get("reasoning").unwrap().as_bool().unwrap(),
                "reasoning should default to false for unloaded model"
            );
        }
    }
}

/// Unloaded model: defaults tool_call: true, reasoning: false.
#[tokio::test]
async fn test_unloaded_model_defaults() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-unloaded".to_string()),
        model: Some("test/unloaded".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);

    let model = &models[0];
    assert!(
        model.get("tool_call").unwrap().as_bool().unwrap(),
        "tool_call should default to true for unloaded model"
    );
    assert!(
        !model.get("reasoning").unwrap().as_bool().unwrap(),
        "reasoning should default to false for unloaded model"
    );
    assert!(
        model.get("temperature").unwrap().as_bool().unwrap(),
        "temperature should always be true"
    );
}

/// Model with modalities.input containing "image" → attachment: true.
#[tokio::test]
async fn test_model_with_image_modality_has_attachment() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-vision".to_string()),
        model: Some("test/vision".to_string()),
        enabled: true,
        modalities: Some(ModelModalities {
            input: vec!["text".to_string(), "image".to_string()],
            output: vec!["text".to_string()],
        }),
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);

    let model = &models[0];
    assert!(
        model.get("attachment").unwrap().as_bool().unwrap(),
        "attachment should be true when modalities.input contains 'image'"
    );
}

/// Model with modalities.input: ["text"] → attachment: false.
#[tokio::test]
async fn test_model_text_only_modality_no_attachment() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-text".to_string()),
        model: Some("test/text".to_string()),
        enabled: true,
        modalities: Some(ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["text".to_string()],
        }),
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);

    let model = &models[0];
    assert!(
        !model.get("attachment").unwrap().as_bool().unwrap(),
        "attachment should be false when modalities.input only contains 'text'"
    );
}

/// Model with no modalities → attachment: false.
#[tokio::test]
async fn test_model_no_modality_no_attachment() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-nomod".to_string()),
        model: Some("test/nomod".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);

    let model = &models[0];
    assert!(
        !model.get("attachment").unwrap().as_bool().unwrap(),
        "attachment should be false when modalities is None"
    );
}

/// Alias entries inherit capabilities from target model.
#[tokio::test]
async fn test_alias_inherits_capabilities() {
    let mock_server = MockServer::start().await;

    // Mock /props with reasoning: true
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": true
            }
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    // Add an alias pointing to model-a
    {
        let mut aliases = state.registry.aliases.write().await;
        aliases.insert("my-alias".to_string(), "api-model-a".to_string());
    }

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    // Find the alias entry
    let alias_entry = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "my-alias")
        .expect("alias entry should exist");

    assert!(
        alias_entry.get("tool_call").unwrap().as_bool().unwrap(),
        "alias should inherit tool_call from target"
    );
    assert!(
        alias_entry.get("reasoning").unwrap().as_bool().unwrap(),
        "alias should inherit reasoning from target"
    );
    assert!(
        alias_entry.get("temperature").unwrap().as_bool().unwrap(),
        "alias should have temperature: true"
    );
}

/// Alias entry's name is derived from the alias slug, not the target model.
#[tokio::test]
async fn test_alias_name_derived_from_slug() {
    let mock_server = MockServer::start().await;

    // Mock /props
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": false
            }
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    // Add an alias pointing to model-a (model: "test/model-a" → name would be "Test: Model A")
    {
        let mut aliases = state.registry.aliases.write().await;
        aliases.insert("my-awesome-alias".to_string(), "api-model-a".to_string());
    }

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    // Find the alias entry
    let alias_entry = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "my-awesome-alias")
        .expect("alias entry should exist");

    // The name should be derived from the alias slug, NOT from the target model.
    assert_eq!(
        alias_entry.get("name").unwrap().as_str().unwrap(),
        "My Awesome Alias",
        "Alias name should be derived from the alias slug, not the target model"
    );

    // The target model entry should still have its own name
    let target_entry = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "api-model-a")
        .expect("target model entry should exist");
    assert_eq!(
        target_entry.get("name").unwrap().as_str().unwrap(),
        "Test: Model A",
        "Target model should keep its own name"
    );
}

/// vLLM backend: max_model_len from /v1/models populates context_length.
#[tokio::test]
async fn test_vllm_context_length_from_backend_models() {
    let mock_server = MockServer::start().await;

    // Mock /props (llama.cpp endpoint, vLLM may not have it)
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": false
            }
        })))
        .mount(&mock_server)
        .await;

    // Mock /v1/models with vLLM-style max_model_len
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "test/model-a",
                    "object": "model",
                    "owned_by": "",
                    "ready": true,
                    "max_model_len": 32768
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    // Find model-a entry
    let model_a = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "api-model-a")
        .expect("model-a entry should exist");

    assert_eq!(
        model_a.get("context_length").unwrap().as_u64(),
        Some(32768),
        "context_length should be 32768 from vLLM max_model_len"
    );
}

/// llama.cpp backend: meta.n_ctx from /v1/models populates context_length.
#[tokio::test]
async fn test_llamacpp_context_length_from_backend_models() {
    let mock_server = MockServer::start().await;

    // Mock /props
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": false
            }
        })))
        .mount(&mock_server)
        .await;

    // Mock /v1/models with llama.cpp-style meta.n_ctx
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "test/model-a",
                    "object": "model",
                    "owned_by": "",
                    "ready": true,
                    "meta": {
                        "n_ctx": 16384
                    }
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    let model_a = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "api-model-a")
        .expect("model-a entry should exist");

    assert_eq!(
        model_a.get("context_length").unwrap().as_u64(),
        Some(16384),
        "context_length should be 16384 from llama.cpp meta.n_ctx"
    );
}

/// Config-level context_length takes precedence over backend value.
#[tokio::test]
async fn test_config_context_length_overrides_backend() {
    let mock_server = MockServer::start().await;

    // Mock /props
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": false
            }
        })))
        .mount(&mock_server)
        .await;

    // Mock /v1/models with max_model_len: 32768
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "test/model-a",
                    "object": "model",
                    "owned_by": "",
                    "ready": true,
                    "max_model_len": 32768
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    // Override model-a config with explicit context_length
    {
        let mut mc = state.registry.model_configs.write().await;
        if let Some(cfg) = mc.get_mut("model-a") {
            cfg.context_length = Some(8192);
        }
    }

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    let model_a = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "api-model-a")
        .expect("model-a entry should exist");

    assert_eq!(
        model_a.get("context_length").unwrap().as_u64(),
        Some(8192),
        "config-level context_length should override backend value"
    );
}

/// Alias entries inherit backend-derived context_length from target model.
#[tokio::test]
async fn test_alias_inherits_backend_context_length() {
    let mock_server = MockServer::start().await;

    // Mock /props
    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": false
            }
        })))
        .mount(&mock_server)
        .await;

    // Mock /v1/models with max_model_len: 32768
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "test/model-a",
                    "object": "model",
                    "owned_by": "",
                    "ready": true,
                    "max_model_len": 32768
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    // Add an alias pointing to model-a
    {
        let mut aliases = state.registry.aliases.write().await;
        aliases.insert("my-alias".to_string(), "api-model-a".to_string());
    }

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    let alias_entry = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "my-alias")
        .expect("alias entry should exist");

    assert_eq!(
        alias_entry.get("context_length").unwrap().as_u64(),
        Some(32768),
        "alias should inherit context_length from target model's backend"
    );
}

/// Model ID preserves original casing from api_name (not lowercased).
#[tokio::test]
async fn test_model_id_preserves_original_casing() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("Unsloth/Qwen3.5-35B-A3B-GGUF".to_string()),
        model: Some("Unsloth/Qwen3.5-35B-A3B-GGUF".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);

    let model = &models[0];
    assert_eq!(
        model.get("id").unwrap().as_str().unwrap(),
        "Unsloth/Qwen3.5-35B-A3B-GGUF",
        "model id should preserve original casing from api_name"
    );
}

/// Alias ID preserves original casing from alias name (not lowercased).
#[tokio::test]
async fn test_alias_id_preserves_original_casing() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("Unsloth/Qwen3.5-35B".to_string()),
        model: Some("Unsloth/Qwen3.5-35B".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Add an alias with mixed case
    {
        let mut aliases = state.registry.aliases.write().await;
        aliases.insert(
            "My-Custom-Alias".to_string(),
            "Unsloth/Qwen3.5-35B".to_string(),
        );
    }

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    // Find the alias entry
    let alias_entry = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "My-Custom-Alias")
        .expect("alias entry should exist with original casing");

    assert_eq!(
        alias_entry.get("id").unwrap().as_str().unwrap(),
        "My-Custom-Alias",
        "alias id should preserve original casing"
    );

    // Also verify the model entry preserves its casing
    let model_entry = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "Unsloth/Qwen3.5-35B")
        .expect("model entry should exist with original casing");

    assert_eq!(
        model_entry.get("id").unwrap().as_str().unwrap(),
        "Unsloth/Qwen3.5-35B",
        "model id should preserve original casing"
    );
}

// ── Reasoning-effort fields (plan 189, Task 3) ────────────────────────────

/// Model with reasoning levels: the entry exposes supportsReasoningEffort,
/// raw reasoningLevels (pi vocabulary), and derived reasoning_options
/// (off → none). Effective reasoning is true even without /props.
#[tokio::test]
async fn test_model_with_reasoning_levels_exposes_derived_fields() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-leveled".to_string()),
        model: Some("test/leveled".to_string()),
        enabled: true,
        reasoning_levels: Some(vec![
            "off".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "xhigh".to_string(),
        ]),
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);

    let model = &models[0];
    assert_eq!(
        model
            .get("supportsReasoningEffort")
            .and_then(|v| v.as_bool()),
        Some(true),
        "supportsReasoningEffort should be true when levels are configured"
    );
    assert_eq!(
        model.get("reasoningLevels"),
        Some(&serde_json::json!(["off", "low", "medium", "xhigh"])),
        "reasoningLevels should keep the raw stored levels"
    );
    assert_eq!(
        model.get("reasoning_options"),
        Some(&serde_json::json!([
            { "type": "effort", "values": ["none", "low", "medium", "xhigh"] }
        ])),
        "reasoning_options should map off to none"
    );
    // No backend loaded → props reasoning defaults to false, but the
    // derived flag must still make effective reasoning true.
    assert!(
        model.get("reasoning").unwrap().as_bool().unwrap(),
        "reasoning should be true from derived levels when props is false"
    );
}

/// Model without reasoning levels: supportsReasoningEffort is emitted as
/// false; the reasoningLevels and reasoning_options keys are absent, and
/// effective reasoning stays false (props false + derived false).
#[tokio::test]
async fn test_model_without_reasoning_levels_omits_optional_fields() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-plain".to_string()),
        model: Some("test/plain".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 1);

    let model = &models[0];
    assert_eq!(
        model
            .get("supportsReasoningEffort")
            .and_then(|v| v.as_bool()),
        Some(false),
        "supportsReasoningEffort should always be emitted, false when unset"
    );
    assert!(
        model.get("reasoningLevels").is_none(),
        "reasoningLevels should be absent when no levels are configured"
    );
    assert!(
        model.get("reasoning_options").is_none(),
        "reasoning_options should be absent when no levels are configured"
    );
    assert_eq!(
        model.get("reasoning").and_then(|v| v.as_bool()),
        Some(false),
        "reasoning should be false when props is false and no levels"
    );
}

/// Effective reasoning is the OR of props and derived: props reasoning
/// true with no levels still yields reasoning: true.
#[tokio::test]
async fn test_reasoning_props_true_without_levels_still_true() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/props"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "chat_template_caps": {
                "supports_tool_calls": true,
                "supports_preserve_reasoning": true
            }
        })))
        .mount(&mock_server)
        .await;

    let backend_url = mock_server.uri();
    let state =
        crate::proxy::handlers::tests::create_state_with_two_backends(&backend_url, &backend_url)
            .await;

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    let model_a = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "api-model-a")
        .expect("model-a entry should exist");
    assert!(
        model_a.get("reasoning").unwrap().as_bool().unwrap(),
        "reasoning should be true from props when no levels are configured"
    );
    assert_eq!(
        model_a
            .get("supportsReasoningEffort")
            .and_then(|v| v.as_bool()),
        Some(false),
        "supportsReasoningEffort should be false without levels"
    );
    assert!(
        model_a.get("reasoningLevels").is_none(),
        "reasoningLevels should be absent without levels"
    );
    assert!(
        model_a.get("reasoning_options").is_none(),
        "reasoning_options should be absent without levels"
    );
}

/// Alias of a leveled model inherits all three reasoning-effort fields
/// (the alias entry is a whole-entry copy of the target).
#[tokio::test]
async fn test_alias_inherits_reasoning_effort_fields() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-leveled".to_string()),
        model: Some("test/leveled".to_string()),
        enabled: true,
        reasoning_levels: Some(vec![
            "off".to_string(),
            "low".to_string(),
            "high".to_string(),
        ]),
        ..Default::default()
    })
    .await;

    {
        let mut aliases = state.registry.aliases.write().await;
        aliases.insert("my-alias".to_string(), "test-leveled".to_string());
    }

    let result = call_list_models(state).await;
    let models = result.get("models").unwrap().as_array().unwrap();

    let alias_entry = models
        .iter()
        .find(|m| m.get("id").unwrap().as_str().unwrap() == "my-alias")
        .expect("alias entry should exist");

    assert_eq!(
        alias_entry
            .get("supportsReasoningEffort")
            .and_then(|v| v.as_bool()),
        Some(true),
        "alias should inherit supportsReasoningEffort from target"
    );
    assert_eq!(
        alias_entry.get("reasoningLevels"),
        Some(&serde_json::json!(["off", "low", "high"])),
        "alias should inherit reasoningLevels from target"
    );
    assert_eq!(
        alias_entry.get("reasoning_options"),
        Some(&serde_json::json!([
            { "type": "effort", "values": ["none", "low", "high"] }
        ])),
        "alias should inherit derived reasoning_options from target"
    );
    assert!(
        alias_entry.get("reasoning").unwrap().as_bool().unwrap(),
        "alias should inherit effective reasoning: true"
    );
}

// ── Drift-guard: opencode response round-trip ───────────────────────────────

/// The OpencodeModelsResponse struct must faithfully represent the full wire
/// shape returned by handle_opencode_list_models. Deserializing the response
/// into OpencodeModelsResponse and comparing against the raw Value ensures no
/// fields are silently dropped or invented.
#[tokio::test]
async fn test_opencode_response_deserializes_into_typed() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("org/repo".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    let result = call_list_models(state).await;

    // Deserialize into the typed struct.
    let parsed: OpencodeModelsResponse = serde_json::from_value(result.clone())
        .expect("opencode body must deserialize into OpencodeModelsResponse");

    // Lossless round-trip: re-serialize parsed and compare to original Value.
    assert_eq!(
        serde_json::to_value(&parsed).expect("parsed must serialize"),
        result,
        "OpencodeModelsResponse round-trip must be lossless — struct fields must match wire shape exactly"
    );
}
