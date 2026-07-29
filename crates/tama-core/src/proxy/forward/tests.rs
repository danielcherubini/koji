use super::sse::process_sse_line;
use super::stats::extract_inference_stats;
use super::*;

use crate::proxy::state::MetricsState;

// ── Test helpers ────────────────────────────────────────────────────────

fn make_metrics_state() -> MetricsState {
    MetricsState::new()
}

mod extract_stats;
mod headers;
mod integration;
mod json;
mod request;
mod sse;
