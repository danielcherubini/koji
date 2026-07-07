use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;

use super::helpers::{call_list_models, create_state_with_model};
use crate::config::ModelConfig;
use crate::config::ModelModalities;

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
        let mut aliases = state.aliases.write().await;
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
        let mut aliases = state.aliases.write().await;
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
