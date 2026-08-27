use super::*;
use crate::proxy::types::LatestInferenceStats;

#[test]
fn test_extract_inference_stats_full_timings() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "model": "test-model",
        "choices": [],
        "timings": {
            "predicted_per_second": 50.5,
            "prompt_per_second": 200.0,
            "cache_n": 80,
            "prompt_n": 100,
            "draft_n": 10,
            "draft_n_accepted": 8
        }
    });

    let result = extract_inference_stats("test-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    assert_eq!(stats.tps, Some(50.5f32));
    assert_eq!(stats.prompt_tps, Some(200.0f32));
    assert_eq!(stats.cache_hit_pct, Some(80.0f32)); // 80/100 * 100
    assert_eq!(stats.spec_accept_pct, Some(80.0f32)); // 8/10 * 100
    assert!(stats.spec_decoding_active);
    assert!(stats.last_updated_ms > 0);
    // Verify stats are stored in the HashMap under the server key
    let map = metrics_state.inference_stats_snapshot();
    assert!(map.contains_key("test-server"));
    let stored = map.get("test-server").unwrap();
    assert_eq!(stored.tps, Some(50.5f32));
}

#[test]
fn test_extract_inference_stats_missing_timings() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "model": "test-model",
        "choices": []
    });

    let result = extract_inference_stats("test-server", &json, &metrics_state);

    assert!(result.is_none());
}

#[test]
fn test_extract_inference_stats_zero_prompt_n() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "timings": {
            "predicted_per_second": 50.0,
            "prompt_per_second": 100.0,
            "cache_n": 0,
            "prompt_n": 0,
            "draft_n": 5,
            "draft_n_accepted": 3
        }
    });

    let result = extract_inference_stats("test-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    assert_eq!(stats.cache_hit_pct, None); // division by zero
    let spec = stats.spec_accept_pct.unwrap();
    assert!((spec - 60.0).abs() < 0.1); // 3/5 * 100 ≈ 60.0
}

#[test]
fn test_extract_inference_stats_zero_draft_n() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "timings": {
            "predicted_per_second": 50.0,
            "prompt_per_second": 100.0,
            "cache_n": 50,
            "prompt_n": 100,
            "draft_n": 0,
            "draft_n_accepted": 0
        }
    });

    let result = extract_inference_stats("test-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    assert_eq!(stats.spec_accept_pct, None); // division by zero
    assert!(!stats.spec_decoding_active); // draft_n == 0 and no previous active
}

#[test]
fn test_extract_inference_stats_partial_timings() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "timings": {
            "predicted_per_second": 30.0,
            "prompt_per_second": 150.0
        }
    });

    let result = extract_inference_stats("test-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    assert_eq!(stats.tps, Some(30.0f32));
    assert_eq!(stats.prompt_tps, Some(150.0f32));
    assert_eq!(stats.cache_hit_pct, None); // prompt_n defaults to 0
    assert_eq!(stats.spec_accept_pct, None); // draft_n defaults to 0
    assert!(!stats.spec_decoding_active);
}

#[test]
fn test_extract_inference_stats_spec_decoding_sticky() {
    let metrics_state = make_metrics_state();

    // Pre-seed with spec_decoding_active=true (sticky behavior)
    metrics_state.record_inference_stats(
        "test-server",
        LatestInferenceStats {
            tps: Some(10.0),
            prompt_tps: None,
            cache_hit_pct: None,
            spec_accept_pct: None,
            spec_decoding_active: true,
            last_updated_ms: 100,
        },
    );

    let json = serde_json::json!({
        "timings": {
            "predicted_per_second": 40.0,
            "prompt_per_second": 80.0,
            "cache_n": 0,
            "prompt_n": 0,
            "draft_n": 0,
            "draft_n_accepted": 0
        }
    });

    let result = extract_inference_stats("test-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    // spec_decoding_active stays true because previous was true (sticky)
    assert!(stats.spec_decoding_active);
}

// ── vLLM metrics format ────────────────────────────────────────────────────

#[test]
fn test_extract_inference_stats_vllm_full_metrics() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "model": "Qwen/Qwen3.6-27B-FP8",
        "choices": [
            {
                "finish_reason": "stop",
                "index": 0,
                "message": {
                    "content": "Hello!",
                    "role": "assistant"
                }
            }
        ],
        "metrics": {
            "tokens_per_second": 69.38,
            "generation_time_ms": 3271.95,
            "time_to_first_token_ms": 273.68,
            "queue_time_ms": 0.008
        },
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 246,
            "total_tokens": 257
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    // tokens_per_second from metrics
    assert!((stats.tps.unwrap() - 69.38f32).abs() < 0.01);
    // No prompt_tokens_details.cached_tokens → prompt_tps is None
    assert_eq!(stats.prompt_tps, None);
    assert_eq!(stats.cache_hit_pct, None);
    assert_eq!(stats.spec_accept_pct, None);
    assert!(!stats.spec_decoding_active);
    assert!(stats.last_updated_ms > 0);

    // Verify stats are stored in the HashMap
    let map = metrics_state.inference_stats_snapshot();
    assert!(map.contains_key("vllm-server"));
    let stored = map.get("vllm-server").unwrap();
    assert!((stored.tps.unwrap() - 69.38f32).abs() < 0.01);
}

#[test]
fn test_extract_inference_stats_vllm_with_cached_tokens() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "metrics": {
            "tokens_per_second": 80.0,
            "generation_time_ms": 2000.0,
            "time_to_first_token_ms": 150.0,
            "queue_time_ms": 0.01
        },
        "usage": {
            "prompt_tokens": 10000,
            "completion_tokens": 200,
            "total_tokens": 10200,
            "prompt_tokens_details": {
                "cached_tokens": 9500
            }
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    assert!((stats.tps.unwrap() - 80.0f32).abs() < 0.01);
    // computed = 10000 - 9500 = 500, prompt_tps = 500 / 0.15 = 3333.33
    let expected_pp = 500.0f32 / 0.15f32;
    assert!((stats.prompt_tps.unwrap() - expected_pp).abs() < 1.0);
    // cache_hit_pct = 9500 / 10000 * 100 = 95%
    assert!((stats.cache_hit_pct.unwrap() - 95.0f32).abs() < 0.1);
}

#[test]
fn test_extract_inference_stats_vllm_cached_tokens_all_cached() {
    // Edge case: all tokens cached, computed = 0 → prompt_tps is None
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "metrics": {
            "tokens_per_second": 100.0,
            "time_to_first_token_ms": 5.0
        },
        "usage": {
            "prompt_tokens": 500,
            "prompt_tokens_details": {
                "cached_tokens": 500
            }
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);
    assert!(result.is_some());
    let stats = result.unwrap();
    // All cached → computed = 0 → prompt_tps is None
    assert_eq!(stats.prompt_tps, None);
    // cache_hit_pct = 100%
    assert!((stats.cache_hit_pct.unwrap() - 100.0f32).abs() < 0.1);
}

#[test]
fn test_extract_inference_stats_vllm_missing_usage() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "metrics": {
            "tokens_per_second": 50.0,
            "generation_time_ms": 2000.0,
            "time_to_first_token_ms": 100.0
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);
    // No usage field — still succeeds now that we don't derive prompt_tps from it
    assert!(result.is_some());
    assert_eq!(result.unwrap().tps, Some(50.0f32));
}

#[test]
fn test_extract_inference_stats_vllm_missing_metrics() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 50,
            "total_tokens": 60
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);
    // No metrics field — should return None
    assert!(result.is_none());
}

#[test]
fn test_extract_inference_stats_vllm_zero_time_to_first_token() {
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "metrics": {
            "tokens_per_second": 100.0,
            "generation_time_ms": 500.0,
            "time_to_first_token_ms": 0.0
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    assert_eq!(stats.tps, Some(100.0f32));
    // prompt_tps is always None for vLLM
    assert_eq!(stats.prompt_tps, None);
}

#[test]
fn test_extract_inference_stats_vllm_prefers_timings_over_metrics() {
    // When both timings and metrics are present, timings (llama.cpp format) wins
    let metrics_state = make_metrics_state();
    let json = serde_json::json!({
        "timings": {
            "predicted_per_second": 999.0,
            "prompt_per_second": 888.0,
            "cache_n": 0,
            "prompt_n": 0,
            "draft_n": 0,
            "draft_n_accepted": 0
        },
        "metrics": {
            "tokens_per_second": 69.0,
            "generation_time_ms": 3000.0,
            "time_to_first_token_ms": 200.0
        },
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 200,
            "total_tokens": 210
        }
    });

    let result = extract_inference_stats("test-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    // Should use timings values (llama.cpp format takes priority)
    assert_eq!(stats.tps, Some(999.0f32));
    assert_eq!(stats.prompt_tps, Some(888.0f32));
}

#[test]
fn test_extract_inference_stats_vllm_sticky_spec_decoding() {
    // vLLM should preserve sticky spec_decoding_active from previous state
    let metrics_state = make_metrics_state();

    // Pre-seed with spec_decoding_active=true
    metrics_state.record_inference_stats(
        "vllm-server",
        LatestInferenceStats {
            tps: Some(50.0),
            prompt_tps: None,
            cache_hit_pct: None,
            spec_accept_pct: None,
            spec_decoding_active: true,
            last_updated_ms: 100,
        },
    );

    let json = serde_json::json!({
        "metrics": {
            "tokens_per_second": 60.0,
            "generation_time_ms": 2500.0,
            "time_to_first_token_ms": 150.0
        },
        "usage": {
            "prompt_tokens": 8,
            "completion_tokens": 150,
            "total_tokens": 158
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    // spec_decoding_active stays true because previous was true (sticky)
    assert!(stats.spec_decoding_active);
}

#[test]
fn test_extract_inference_stats_vllm_preserves_tamad_spec_accept() {
    // The proxy's merge loop folds the tamad-observed spec-decode values into
    // the entry (ADR-0012). A per-response vLLM extract REPLACES the whole
    // entry — the merged values must survive that replacement.
    let metrics_state = make_metrics_state();

    // Pre-seed as the proxy's merge (server/metrics.rs) does.
    metrics_state.record_inference_stats(
        "vllm-server",
        LatestInferenceStats {
            tps: Some(10.0),
            prompt_tps: None,
            cache_hit_pct: None,
            spec_accept_pct: Some(44.5),
            spec_decoding_active: true,
            last_updated_ms: 100,
        },
    );

    let json = serde_json::json!({
        "metrics": {
            "tokens_per_second": 62.0,
            "generation_time_ms": 3000.0,
            "time_to_first_token_ms": 140.0
        },
        "usage": {
            "prompt_tokens": 9,
            "completion_tokens": 140,
            "total_tokens": 149
        }
    });

    let result = extract_inference_stats("vllm-server", &json, &metrics_state);

    assert!(result.is_some());
    let stats = result.unwrap();
    // The response updates tps…
    assert!((stats.tps.unwrap() - 62.0f32).abs() < 0.01);
    // …but the tamad-merged spec field is preserved across the replacement.
    assert_eq!(stats.spec_accept_pct, Some(44.5f32));
    assert!(stats.spec_decoding_active);

    // The stored entry carries the preserved value too.
    let map = metrics_state.inference_stats_snapshot();
    let stored = map.get("vllm-server").unwrap();
    assert_eq!(stored.spec_accept_pct, Some(44.5f32));
    assert!((stored.tps.unwrap() - 62.0f32).abs() < 0.01);
}
