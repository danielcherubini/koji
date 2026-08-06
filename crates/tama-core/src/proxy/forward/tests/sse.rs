use super::*;
use crate::proxy::state::MetricsState;

#[test]
fn test_process_sse_line_rewrites_model_in_data() {
    let mut out = String::new();
    process_sse_line(
        "data: {\"model\": \"backend-model\", \"choices\": []}",
        Some("user-model"),
        "test-server",
        &mut out,
        None,
    );
    // serde_json serializes without spaces by default
    assert!(out.contains("\"model\""), "output: {}", out);
    assert!(out.contains("user-model"), "output: {}", out);
}

#[test]
fn test_process_sse_line_skips_rewrite_when_none() {
    let mut out = String::new();
    process_sse_line(
        "data: {\"model\": \"backend-model\", \"choices\": []}",
        None,
        "test-server",
        &mut out,
        None,
    );
    // Model should NOT be rewritten when model_name is None
    assert!(out.contains("backend-model"), "output: {}", out);
    assert!(!out.contains("user-model"), "output: {}", out);
}

#[test]
fn test_process_sse_line_passes_done_unchanged() {
    let mut out = String::new();
    process_sse_line(
        "data: [DONE]",
        Some("any-model"),
        "test-server",
        &mut out,
        None,
    );
    // DONE is pushed as-is (no trailing newline added by this function)
    assert_eq!(out, "data: [DONE]");
}

#[test]
fn test_process_sse_line_passes_comment_unchanged() {
    let mut out = String::new();
    process_sse_line(
        ": heartbeat",
        Some("any-model"),
        "test-server",
        &mut out,
        None,
    );
    assert_eq!(out, ": heartbeat");
}

#[test]
fn test_process_sse_line_passes_empty_line_unchanged() {
    let mut out = String::new();
    process_sse_line("", Some("any-model"), "test-server", &mut out, None);
    assert_eq!(out, "");
}

#[test]
fn test_process_sse_line_handles_invalid_json() {
    let mut out = String::new();
    process_sse_line(
        "data: not valid json {",
        Some("any-model"),
        "test-server",
        &mut out,
        None,
    );
    assert_eq!(out, "data: not valid json {");
}

#[test]
fn test_process_sse_line_handles_non_data_lines() {
    let mut out = String::new();
    process_sse_line(
        "event: message",
        Some("any-model"),
        "test-server",
        &mut out,
        None,
    );
    assert_eq!(out, "event: message");
}

#[test]
fn test_process_sse_line_multiline_buffer() {
    // A single call to process_sse_line processes one line at a time.
    // Lines without trailing newline are not processed as complete SSE lines.
    let mut out = String::new();
    // First line with newline - should be processed
    process_sse_line(
        "data: {\"model\": \"a\"}\n",
        Some("user"),
        "test-server",
        &mut out,
        None,
    );
    assert!(out.contains("user"), "output: {}", out);
}

#[test]
fn test_process_sse_line_extracts_inference_stats() {
    let metrics_state = MetricsState::new();
    let mut out = String::new();

    // Simulate a streaming data line with timings
    process_sse_line(
        "data: {\"model\": \"test\", \"choices\": [], \"timings\": {\"predicted_per_second\": 42.5, \"prompt_per_second\": 100.0, \"cache_n\": 50, \"prompt_n\": 100, \"draft_n\": 0, \"draft_n_accepted\": 0}}",
        Some("user-model"),
        "test-server",
        &mut out,
        Some(&metrics_state),
    );

    // Verify the SSE output was rewritten
    assert!(out.contains("user-model"), "output: {}", out);
    // Verify inference stats were extracted and stored in the HashMap
    let map = metrics_state.inference_stats_snapshot();
    assert_eq!(map.len(), 1);
    let stats = map.get("test-server").unwrap();
    assert_eq!(stats.tps, Some(42.5f32));
    assert_eq!(stats.cache_hit_pct, Some(50.0f32)); // 50/100 * 100
}

#[test]
fn test_process_sse_line_extracts_vllm_stats() {
    let metrics_state = MetricsState::new();
    let mut out = String::new();

    // Simulate a vLLM streaming final chunk with metrics + usage
    process_sse_line(
        "data: {\"model\": \"vllm-model\", \"choices\": [], \"metrics\": {\"tokens_per_second\": 69.38, \"generation_time_ms\": 3271.95, \"time_to_first_token_ms\": 273.68, \"queue_time_ms\": 0.008}, \"usage\": {\"prompt_tokens\": 11, \"completion_tokens\": 246, \"total_tokens\": 257}}",
        Some("user-model"),
        "vllm-server",
        &mut out,
        Some(&metrics_state),
    );

    // Verify the SSE output was rewritten
    assert!(out.contains("user-model"), "output: {}", out);
    // Verify vLLM inference stats were extracted and stored
    let map = metrics_state.inference_stats_snapshot();
    assert_eq!(map.len(), 1);
    let stats = map.get("vllm-server").unwrap();
    assert!((stats.tps.unwrap() - 69.38f32).abs() < 0.01);
    // prompt_tps = 11 / (273.68 / 1000) ≈ 40.19
    let expected_prompt_tps = 11.0f32 / (273.68f32 / 1000.0);
    assert!((stats.prompt_tps.unwrap() - expected_prompt_tps).abs() < 0.1);
    assert_eq!(stats.cache_hit_pct, None);
    assert_eq!(stats.spec_accept_pct, None);
}
