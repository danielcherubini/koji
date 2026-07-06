use super::*;

#[test]
fn test_rewrite_json_model_name_replaces_existing() {
    let json =
        serde_json::json!({"model": "old-model", "choices": [{"message": {"content": "Hello"}}]});
    let result = rewrite_json_model_name(json, Some("new-model"));

    assert_eq!(result["model"], "new-model");
    assert_eq!(result["choices"][0]["message"]["content"], "Hello");
}

#[test]
fn test_rewrite_json_model_name_adds_missing_field() {
    let json = serde_json::json!({"choices": [{"delta": {"content": "Hi"}}]});
    let result = rewrite_json_model_name(json, Some("my-model"));

    assert_eq!(result["model"], "my-model");
    assert!(result["choices"].is_array());
}

#[test]
fn test_rewrite_json_model_name_preserves_other_fields() {
    let json = serde_json::json!({
        "id": "chatcmpl-123",
        "object": "chat.completion",
        "created": 1234567890,
        "model": "old",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "Test"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 20}
    });
    let result = rewrite_json_model_name(json, Some("new-model"));

    assert_eq!(result["model"], "new-model");
    assert_eq!(result["id"], "chatcmpl-123");
    assert_eq!(result["object"], "chat.completion");
    assert_eq!(result["created"], 1234567890);
    assert_eq!(result["usage"]["prompt_tokens"], 10);
}

#[test]
fn test_rewrite_json_model_name_empty_string_ignored() {
    let json = serde_json::json!({"model": "old", "choices": []});
    let result = rewrite_json_model_name(json, Some(""));

    // Empty string should NOT rewrite the model field
    assert_eq!(result["model"], "old");
}

#[test]
fn test_rewrite_json_model_name_none_skips_rewrite() {
    let json = serde_json::json!({"model": "old", "choices": []});
    let result = rewrite_json_model_name(json, None);

    // None should NOT rewrite the model field
    assert_eq!(result["model"], "old");
}

#[test]
fn test_rewrite_json_model_name_long_model_name() {
    let json = serde_json::json!({"model": "m", "choices": []});
    let long_name = "unsloth/gemma-4-E2B-it-GGUF:q4_k_m";
    let result = rewrite_json_model_name(json, Some(long_name));

    assert_eq!(result["model"], long_name);
}

#[test]
fn test_build_forward_uri_simple_path() {
    let parts = make_parts("http://localhost/v1/chat/completions");

    let uri = build_forward_uri("http://backend:8080", &parts).unwrap();
    assert_eq!(uri, "http://backend:8080/v1/chat/completions");
}

#[test]
fn test_build_forward_uri_with_query_string() {
    let parts = make_parts("http://localhost/v1/models?limit=10");

    let uri = build_forward_uri("http://backend:8080", &parts).unwrap();
    assert_eq!(uri, "http://backend:8080/v1/models?limit=10");
}

#[test]
fn test_build_forward_uri_no_query_returns_path_only() {
    let parts = make_parts("http://localhost/v1/completions");

    let uri = build_forward_uri("http://backend:8080", &parts).unwrap();
    assert_eq!(uri, "http://backend:8080/v1/completions");
}
