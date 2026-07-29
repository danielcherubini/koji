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
