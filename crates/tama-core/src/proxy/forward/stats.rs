use crate::proxy::state::MetricsState;
use crate::proxy::types::LatestInferenceStats;
use std::time::SystemTime;

/// Extract inference stats from a backend response JSON.
///
/// Tries llama.cpp `timings` format first, then falls back to vLLM `metrics`
/// format. Inserts the stats into the per-backend HashMap keyed by `backend_name`
/// and sends the updated map via the watch channel. Returns the computed stats.
/// Division by zero (prompt_n == 0, draft_n == 0, or zero durations) produces
/// `None` for that field.
pub(crate) fn extract_inference_stats(
    backend_name: &str,
    json: &serde_json::Value,
    metrics_state: &MetricsState,
) -> Option<LatestInferenceStats> {
    // Try llama.cpp `timings` format first
    if let Some(stats) = extract_llama_cpp_stats(backend_name, json, metrics_state) {
        return Some(stats);
    }

    // Fall back to vLLM `metrics` format
    extract_vllm_stats(backend_name, json, metrics_state)
}

/// Extract stats from llama.cpp `timings` object.
fn extract_llama_cpp_stats(
    backend_name: &str,
    json: &serde_json::Value,
    metrics_state: &MetricsState,
) -> Option<LatestInferenceStats> {
    let timings = json.get("timings")?;

    let predicted_per_second = timings.get("predicted_per_second")?.as_f64()?;
    let prompt_per_second = timings.get("prompt_per_second")?.as_f64()?;
    let cache_n = timings.get("cache_n").and_then(|v| v.as_u64()).unwrap_or(0);
    let prompt_n = timings
        .get("prompt_n")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let draft_n = timings.get("draft_n").and_then(|v| v.as_u64()).unwrap_or(0);
    let draft_n_accepted = timings
        .get("draft_n_accepted")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let now_ms = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Read previous spec_decoding_active flag for THIS backend (sticky: once true, stays true)
    let prev_active = metrics_state
        .inference_stats
        .borrow()
        .get(backend_name)
        .map(|s| s.spec_decoding_active)
        .unwrap_or(false);

    let stats = LatestInferenceStats {
        tps: Some(predicted_per_second as f32),
        prompt_tps: Some(prompt_per_second as f32),
        cache_hit_pct: if prompt_n > 0 {
            Some((cache_n as f32 / prompt_n as f32 * 100.0).clamp(0.0, 100.0))
        } else {
            None
        },
        spec_accept_pct: if draft_n > 0 {
            Some((draft_n_accepted as f32 / draft_n as f32 * 100.0).clamp(0.0, 100.0))
        } else {
            None
        },
        spec_decoding_active: draft_n > 0 || prev_active,
        last_updated_ms: now_ms,
    };

    metrics_state.record_inference_stats(backend_name, stats);

    Some(stats)
}

/// Extract stats from vLLM `metrics` object.
///
/// vLLM provides `metrics.tokens_per_second` (generation speed). We use that
/// directly for TPS. Prompt tok/s is not derived — vLLM doesn't expose a
/// breakdown of cached vs. non-cached prompt tokens, so dividing
/// `usage.prompt_tokens` by `time_to_first_token_ms` inflates the number
/// dramatically when the KV cache is warm.
fn extract_vllm_stats(
    backend_name: &str,
    json: &serde_json::Value,
    metrics_state: &MetricsState,
) -> Option<LatestInferenceStats> {
    let metrics = json.get("metrics")?;
    let tokens_per_second = metrics.get("tokens_per_second")?.as_f64()?;

    let now_ms = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Read previous spec_decoding_active flag (vLLM doesn't do spec decoding
    // in the same way as llama.cpp, but preserve sticky state)
    let prev_active = metrics_state
        .inference_stats
        .borrow()
        .get(backend_name)
        .map(|s| s.spec_decoding_active)
        .unwrap_or(false);

    let stats = LatestInferenceStats {
        tps: Some(tokens_per_second as f32),
        prompt_tps: None, // vLLM doesn't expose cache hit details to compute this accurately
        cache_hit_pct: None, // vLLM doesn't expose cache hit breakdown
        spec_accept_pct: None, // vLLM doesn't expose spec decoding stats
        spec_decoding_active: prev_active,
        last_updated_ms: now_ms,
    };

    metrics_state.record_inference_stats(backend_name, stats);

    Some(stats)
}
