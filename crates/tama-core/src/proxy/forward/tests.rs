use super::sse::process_sse_line;
use super::*;
use std::collections::HashMap;
use tokio::sync::watch;

use crate::proxy::types::LatestInferenceStats;

// ── Test helpers ────────────────────────────────────────────────────────

fn make_sender() -> watch::Sender<HashMap<String, LatestInferenceStats>> {
    watch::channel(HashMap::new()).0
}

mod extract_stats;
mod headers;
mod integration;
mod json;
mod request;
mod sse;
