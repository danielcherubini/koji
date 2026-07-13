use std::time::Instant;

/// Per-request token usage extracted from backend response.
#[derive(Debug, Clone)]
pub struct LangfuseUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Per-request timings extracted from backend response.
#[derive(Debug, Clone)]
pub struct LangfuseTimings {
    pub prompt_ms: f64,
    pub predicted_ms: f64,
}

/// Accumulated telemetry data for a single inference request.
#[derive(Debug)]
pub struct LangfuseTelemetry {
    // From request
    pub model: String,
    pub input: Option<serde_json::Value>, // messages array (if capture_input)
    pub model_params: Option<serde_json::Value>, // max_tokens, temperature, etc.

    // From response
    pub output: Option<String>, // accumulated completion text (if capture_output)
    pub usage: Option<LangfuseUsage>,
    pub timings: Option<LangfuseTimings>,

    // Timing
    pub start_time: Instant,
    pub end_time: Option<Instant>,

    // From langfuse_* headers
    pub trace_id: Option<String>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,

    // Computed
    pub energy_cost: Option<f64>, // in user's currency
    pub energy_wh: Option<f64>,   // watt-hours consumed
    pub gpu_watts: Option<f64>,   // from GpuDeviceStats (best-effort)
}

use axum::http::HeaderMap;

/// Extract Langfuse trace context from request headers.
/// Compatible with LiteLLM Proxy convention (langfuse_* prefixed headers).
/// Returns (trace_id, user_id, session_id, metadata, tags).
#[allow(clippy::type_complexity)]
pub fn extract_langfuse_headers(
    headers: &HeaderMap,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<serde_json::Value>,
    Option<Vec<String>>,
) {
    let trace_id = headers
        .get("langfuse_trace_id")
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()));
    let user_id = headers
        .get("langfuse_trace_user_id")
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()));
    let session_id = headers
        .get("langfuse_session_id")
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()));
    let metadata = headers
        .get("langfuse_trace_metadata")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| serde_json::from_str(s).ok());
    let tags = headers
        .get("langfuse_trace_tags")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());

    (trace_id, user_id, session_id, metadata, tags)
}

/// Compute energy cost per inference.
///
/// Returns Some((energy_wh, cost_in_currency)) when price_per_kwh > 0.
/// `power_w` is from GpuDeviceStats.power_w (in watts).
/// `prompt_ms` + `predicted_ms` are from llama.cpp timings.
/// `price_per_kwh` is from LangfuseConfig.electricity_price_per_kwh.
///
/// Formula: energy_wh = power_w × duration_s / 3600.0
///          cost = (energy_wh / 1000.0) × price_per_kwh
pub fn compute_energy_cost(
    power_w: f64,
    prompt_ms: f64,
    predicted_ms: f64,
    price_per_kwh: f64,
) -> Option<(f64, f64)> {
    if price_per_kwh <= 0.0 {
        return None;
    }
    let duration_s = (prompt_ms + predicted_ms) / 1000.0;
    let energy_wh = power_w * duration_s / 3600.0;
    let cost = (energy_wh / 1000.0) * price_per_kwh;
    Some((energy_wh, cost))
}

/// Extract LangfuseUsage from an OpenAI-compatible response JSON.
/// Works for both non-streaming responses and the final streaming chunk.
pub fn extract_usage(json: &serde_json::Value) -> Option<LangfuseUsage> {
    let usage = json.get("usage")?;
    let prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_u64())?;
    let completion_tokens = usage.get("completion_tokens").and_then(|v| v.as_u64())?;
    let total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64())?;
    Some(LangfuseUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

/// Extract LangfuseTimings from an OpenAI-compatible response JSON.
/// Works for both non-streaming responses and the final streaming chunk.
pub fn extract_timings(json: &serde_json::Value) -> Option<LangfuseTimings> {
    let timings = json.get("timings")?;
    let prompt_ms = timings.get("prompt_ms").and_then(|v| v.as_f64())?;
    let predicted_ms = timings.get("predicted_ms").and_then(|v| v.as_f64())?;
    Some(LangfuseTimings {
        prompt_ms,
        predicted_ms,
    })
}

/// Extract telemetry-relevant fields from an OpenAI-compatible request body.
/// Returns (model, input_messages_or_prompt, model_params).
pub fn extract_request_fields(
    body_bytes: &[u8],
) -> Option<(String, Option<serde_json::Value>, Option<serde_json::Value>)> {
    let body: serde_json::Value = serde_json::from_slice(body_bytes).ok()?;
    let model = body.get("model").and_then(|v| v.as_str())?.to_string();
    let input = if body.get("messages").is_some() {
        Some(body["messages"].clone())
    } else if body.get("prompt").is_some() {
        Some(body["prompt"].clone())
    } else {
        None
    };
    // Model params: everything except model, messages, prompt, stream, stream_options
    let mut params = serde_json::Map::new();
    if let Some(obj) = body.as_object() {
        for (k, v) in obj {
            if !["model", "messages", "prompt", "stream", "stream_options"].contains(&k.as_str()) {
                params.insert(k.clone(), v.clone());
            }
        }
    }
    let model_params = if params.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(params))
    };
    Some((model, input, model_params))
}

/// Get GPU power in watts from system metrics (best-effort).
/// Returns the first GPU's power_w if available. This is a simplification —
/// per-backend GPU mapping would require resolving backend->GPU device assignment.
pub fn get_gpu_power_watts(system_metrics: &crate::gpu::SystemMetrics) -> Option<f64> {
    system_metrics
        .gpus
        .first()
        .and_then(|g| g.power_w)
        .map(|w| w as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{GpuDeviceStats, GpuVendor, SystemMetrics};
    use axum::http::HeaderMap;

    // ── compute_energy_cost ──────────────────────────────────────────

    #[test]
    fn test_compute_energy_cost_returns_values() {
        // 300W GPU, 5000ms total (3000ms prompt + 2000ms predicted), 1.0 krone/kWh
        let result = compute_energy_cost(300.0, 3000.0, 2000.0, 1.0);
        assert!(result.is_some(), "Expected Some with price_per_kwh=1.0");
        let (energy_wh, cost) = result.unwrap();
        // energy_wh = 300 * 5.0 / 3600.0 = 0.4167 Wh
        assert!(
            (energy_wh - 0.4167).abs() < 0.001,
            "Expected energy_wh ≈ 0.4167, got {}",
            energy_wh
        );
        // cost = 0.4167 / 1000.0 * 1.0 = 0.000417 krone
        assert!(
            (cost - 0.000417).abs() < 0.000001,
            "Expected cost ≈ 0.000417, got {}",
            cost
        );
    }

    #[test]
    fn test_compute_energy_cost_zero_price_returns_none() {
        let result = compute_energy_cost(300.0, 3000.0, 2000.0, 0.0);
        assert!(result.is_none(), "Expected None when price_per_kwh=0");
    }

    #[test]
    fn test_compute_energy_cost_negative_price_returns_none() {
        let result = compute_energy_cost(300.0, 3000.0, 2000.0, -1.0);
        assert!(result.is_none(), "Expected None when price_per_kwh<0");
    }

    // ── extract_langfuse_headers ─────────────────────────────────────

    #[test]
    fn test_extract_langfuse_headers_all_present() {
        let mut headers = HeaderMap::new();
        headers.insert("langfuse_trace_id", "trace-123".parse().unwrap());
        headers.insert("langfuse_trace_user_id", "user-456".parse().unwrap());
        headers.insert("langfuse_session_id", "session-789".parse().unwrap());
        headers.insert(
            "langfuse_trace_metadata",
            r#"{"env":"test"}"#.parse().unwrap(),
        );
        headers.insert("langfuse_trace_tags", "tag1, tag2, tag3".parse().unwrap());

        let (trace_id, user_id, session_id, metadata, tags) = extract_langfuse_headers(&headers);

        assert_eq!(trace_id, Some("trace-123".to_string()));
        assert_eq!(user_id, Some("user-456".to_string()));
        assert_eq!(session_id, Some("session-789".to_string()));
        assert_eq!(metadata, Some(serde_json::json!({"env": "test"})));
        assert_eq!(
            tags,
            Some(vec![
                "tag1".to_string(),
                "tag2".to_string(),
                "tag3".to_string()
            ])
        );
    }

    #[test]
    fn test_extract_langfuse_headers_empty() {
        let headers = HeaderMap::new();
        let (trace_id, user_id, session_id, metadata, tags) = extract_langfuse_headers(&headers);

        assert!(trace_id.is_none());
        assert!(user_id.is_none());
        assert!(session_id.is_none());
        assert!(metadata.is_none());
        assert!(tags.is_none());
    }

    // ── extract_usage ────────────────────────────────────────────────

    #[test]
    fn test_extract_usage_from_response() {
        let json = serde_json::json!({
            "id": "chat-123",
            "choices": [{"message": {"content": "Hello"}}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let usage = extract_usage(&json);
        assert!(usage.is_some());
        let usage = usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn test_extract_usage_missing_usage_field() {
        let json = serde_json::json!({
            "id": "chat-123",
            "choices": [{"message": {"content": "Hello"}}],
        });

        let usage = extract_usage(&json);
        assert!(usage.is_none());
    }

    #[test]
    fn test_extract_usage_missing_prompt_tokens() {
        let json = serde_json::json!({
            "usage": {
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let usage = extract_usage(&json);
        assert!(usage.is_none());
    }

    // ── extract_timings ──────────────────────────────────────────────

    #[test]
    fn test_extract_timings_from_response() {
        let json = serde_json::json!({
            "id": "chat-123",
            "choices": [{"message": {"content": "Hello"}}],
            "timings": {
                "prompt_ms": 3000.0,
                "predicted_ms": 2000.0
            }
        });

        let timings = extract_timings(&json);
        assert!(timings.is_some());
        let timings = timings.unwrap();
        assert_eq!(timings.prompt_ms, 3000.0);
        assert_eq!(timings.predicted_ms, 2000.0);
    }

    #[test]
    fn test_extract_timings_missing_timings_field() {
        let json = serde_json::json!({
            "id": "chat-123",
            "choices": [{"message": {"content": "Hello"}}],
        });

        let timings = extract_timings(&json);
        assert!(timings.is_none());
    }

    // ── extract_request_fields ───────────────────────────────────────

    #[test]
    fn test_extract_request_fields_chat_completions() {
        let body = serde_json::json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "Hello"}
            ],
            "max_tokens": 100,
            "temperature": 0.7,
            "stream": false
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();

        let (model, input, model_params) = extract_request_fields(&body_bytes).unwrap();

        assert_eq!(model, "gpt-4");
        assert!(input.is_some());
        let input_val = input.unwrap();
        assert!(input_val.is_array());
        assert!(model_params.is_some());
        let params = model_params.unwrap();
        assert!(params.as_object().unwrap().contains_key("max_tokens"));
        assert!(params.as_object().unwrap().contains_key("temperature"));
    }

    #[test]
    fn test_extract_request_fields_with_prompt() {
        let body = serde_json::json!({
            "model": "text-davinci-003",
            "prompt": "Once upon a time",
            "max_tokens": 50,
            "temperature": 0.5
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();

        let (model, input, _model_params) = extract_request_fields(&body_bytes).unwrap();

        assert_eq!(model, "text-davinci-003");
        assert!(input.is_some());
        let input_val = input.unwrap();
        assert!(input_val.is_string());
        assert_eq!(input_val.as_str().unwrap(), "Once upon a time");
    }

    #[test]
    fn test_extract_request_fields_no_params() {
        let body = serde_json::json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": true
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();

        let (model, input, _model_params) = extract_request_fields(&body_bytes).unwrap();

        assert_eq!(model, "gpt-4");
        assert!(input.is_some());
        assert!(
            _model_params.is_none(),
            "Expected no params when only model/messages/stream present"
        );
    }

    #[test]
    fn test_extract_request_fields_invalid_json() {
        let body_bytes = b"not valid json {{{";
        let result = extract_request_fields(body_bytes);
        assert!(result.is_none());
    }

    // ── get_gpu_power_watts ──────────────────────────────────────────

    #[test]
    fn test_get_gpu_power_watts_returns_first() {
        let metrics = SystemMetrics {
            gpus: vec![
                GpuDeviceStats {
                    device_id: "GPU0".to_string(),
                    vendor: GpuVendor::Nvidia,
                    name: "RTX 4090".to_string(),
                    utilization_pct: Some(85),
                    vram: None,
                    temperature_c: Some(72),
                    power_w: Some(300),
                    fan_pct: Some(60),
                    pci_bus: None,
                    uuid: None,
                },
                GpuDeviceStats {
                    device_id: "GPU1".to_string(),
                    vendor: GpuVendor::Nvidia,
                    name: "RTX 4090".to_string(),
                    utilization_pct: Some(50),
                    vram: None,
                    temperature_c: None,
                    power_w: Some(250),
                    fan_pct: None,
                    pci_bus: None,
                    uuid: None,
                },
            ],
            ..Default::default()
        };

        let power = get_gpu_power_watts(&metrics);
        assert_eq!(power, Some(300.0));
    }

    #[test]
    fn test_get_gpu_power_watts_no_gpus() {
        let metrics = SystemMetrics::default();
        let power = get_gpu_power_watts(&metrics);
        assert!(power.is_none());
    }

    #[test]
    fn test_get_gpu_power_watts_first_no_power() {
        let metrics = SystemMetrics {
            gpus: vec![GpuDeviceStats {
                device_id: "GPU0".to_string(),
                vendor: GpuVendor::Nvidia,
                name: "RTX 4090".to_string(),
                utilization_pct: Some(85),
                vram: None,
                temperature_c: Some(72),
                power_w: None, // No power reading
                fan_pct: Some(60),
                pci_bus: None,
                uuid: None,
            }],
            ..Default::default()
        };

        let power = get_gpu_power_watts(&metrics);
        assert!(power.is_none());
    }
}
