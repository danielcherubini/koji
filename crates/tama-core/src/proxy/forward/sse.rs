use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;

use super::stats::extract_inference_stats;

/// Process a complete SSE line, rewriting the `model` field in JSON data lines.
/// If `inference_stats` is provided, also extracts `timings` from parsed JSON
/// and updates the per-backend HashMap in the watch channel (streaming responses
/// include timings in a final data chunk before `[DONE]`).
pub(crate) fn process_sse_line(
    line: &str,
    model_name: Option<&str>,
    backend_name: &str,
    out: &mut String,
    inference_stats: Option<
        &Arc<
            tokio::sync::watch::Sender<HashMap<String, crate::proxy::types::LatestInferenceStats>>,
        >,
    >,
) {
    if let Some(data_content) = line.strip_prefix("data: ") {
        let trimmed = data_content.trim_end();
        if trimmed == "[DONE]" {
            out.push_str(line);
        } else if let Ok(mut json_value) = serde_json::from_str::<JsonValue>(trimmed) {
            // Extract inference stats from timings if sender is available
            if let Some(sender) = inference_stats {
                if let Some(_stats) = extract_inference_stats(backend_name, &json_value, sender) {
                    // stats are already inserted into the HashMap inside extract_inference_stats
                }
            }
            if let Some(name) = model_name {
                if !name.is_empty() {
                    json_value["model"] = JsonValue::String(name.to_string());
                }
            }
            out.push_str("data: ");
            out.push_str(
                &serde_json::to_string(&json_value).unwrap_or_else(|_| trimmed.to_string()),
            );
            out.push('\n');
        } else {
            out.push_str(line);
        }
    } else {
        // Comments, empty lines, and other lines pass through unchanged
        out.push_str(line);
    }
}
