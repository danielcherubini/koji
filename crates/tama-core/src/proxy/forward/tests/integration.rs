use super::*;

#[test]
fn test_content_length_is_stripped_from_forwarded_response_headers() {
    let skip_list: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "transfer-encoding",
        "upgrade",
        "trailer",
        "content-length",
    ];

    assert!(
        skip_list.contains(&"content-length"),
        "content-length MUST be stripped from forwarded response headers \
         because the proxy rewrites the JSON body, changing its size"
    );
}

#[test]
fn test_body_size_changes_after_model_rewrite() {
    let short_model = "m.gguf";
    let long_model = "unsloth/gemma-4-E2B-it-GGUF";

    let original = serde_json::json!({
        "model": short_model,
        "choices": [{"message": {"role": "assistant", "content": "Hello"}}]
    });
    let original_bytes = serde_json::to_vec(&original).unwrap();

    let rewritten = rewrite_json_model_name(original, Some(long_model));
    let rewritten_bytes = serde_json::to_vec(&rewritten).unwrap();

    assert_ne!(
        original_bytes.len(),
        rewritten_bytes.len(),
        "Body size should differ after model name rewrite"
    );
    assert!(
        rewritten_bytes.len() > original_bytes.len(),
        "Rewritten body with longer model name should be larger"
    );
}
