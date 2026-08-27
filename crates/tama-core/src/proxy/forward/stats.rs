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
/// vLLM provides `metrics.tokens_per_second` (generation speed) and
/// `metrics.time_to_first_token_ms` (prefill + first token latency). When
/// `--enable-prompt-tokens-details` is set on the vLLM side (v0.26.0+, PR
/// #44887), `usage.prompt_tokens_details.cached_tokens` is populated and we
/// can compute a cache-aware prompt tok/s:
///
/// ```text
/// computed_tokens = prompt_tokens - cached_tokens
/// prompt_tps      = computed_tokens / (time_to_first_token_ms / 1000)
/// ```
///
/// Without the flag (cached_tokens absent), prompt_tps is None — dividing
/// total prompt_tokens by TTF inflates the number when the KV cache is warm.
fn extract_vllm_stats(
    backend_name: &str,
    json: &serde_json::Value,
    metrics_state: &MetricsState,
) -> Option<LatestInferenceStats> {
    let metrics = json.get("metrics")?;
    let tokens_per_second = metrics.get("tokens_per_second")?.as_f64()?;
    let time_to_first_token_ms = metrics.get("time_to_first_token_ms")?.as_f64()?;

    // Try to get cache-aware prompt stats from usage.prompt_tokens_details
    // (populated when --enable-prompt-tokens-details is set, vLLM v0.26.0+)
    let (prompt_tps, cache_hit_pct) = json
        .get("usage")
        .and_then(|u| u.get("prompt_tokens_details"))
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|c| c.as_u64())
        .map(|cached_tokens| {
            let prompt_tokens = json
                .get("usage")
                .and_then(|u| u.get("prompt_tokens"))
                .and_then(|p| p.as_u64())
                .unwrap_or(0);
            let computed = prompt_tokens.saturating_sub(cached_tokens) as f32;
            let ttft_secs = time_to_first_token_ms as f32 / 1000.0;
            let pps = if ttft_secs > 0.0 && computed > 0.0 {
                Some(computed / ttft_secs)
            } else {
                None
            };
            let hit_pct = if prompt_tokens > 0 {
                Some((cached_tokens as f32 / prompt_tokens as f32 * 100.0).clamp(0.0, 100.0))
            } else {
                None
            };
            (pps, hit_pct)
        })
        .unwrap_or((None, None));

    let now_ms = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    // Read previous spec fields (vLLM doesn't do spec decoding in the same
    // way as llama.cpp): the proxy's per-server map is written by BOTH the
    // tamad-merged spec observations (server/metrics.rs, ADR-0012) and this
    // per-response whole-entry replacement, so carry both across.
    //
    // `borrow()` is `watch::Sender::borrow` — it hands back a guard holding
    // the channel's internal RwLock as a *read* lock. Read both fields and
    // drop the guard BEFORE `record_inference_stats` takes the write lock
    // in `send_modify`; keeping it across that call self-deadlocks (the
    // rwlock is non-reentrant).
    let (prev_spec_pct, prev_active) = {
        let guard = metrics_state.inference_stats.borrow();
        let entry = guard.get(backend_name);
        (
            entry.and_then(|s| s.spec_accept_pct),
            entry.map(|s| s.spec_decoding_active).unwrap_or(false),
        )
    };

    let stats = LatestInferenceStats {
        tps: Some(tokens_per_second as f32),
        prompt_tps,
        cache_hit_pct,
        spec_accept_pct: prev_spec_pct, // tamad-merged value (ADR-0012) — preserve across per-response replacement
        spec_decoding_active: prev_active, // vLLM responses never set this true themselves
        last_updated_ms: now_ms,
    };

    metrics_state.record_inference_stats(backend_name, stats);

    Some(stats)
}
