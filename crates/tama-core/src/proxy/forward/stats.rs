use crate::proxy::state::MetricsState;
use crate::proxy::types::LatestInferenceStats;
use std::time::SystemTime;

/// Extract inference stats from a llama_cpp `timings` object in a JSON response.
///
/// Inserts the stats into the per-backend HashMap keyed by `backend_name` and sends
/// the updated map via the watch channel. Returns the computed stats.
/// Division by zero (prompt_n == 0, draft_n == 0) produces `None` for that field.
pub(crate) fn extract_inference_stats(
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
