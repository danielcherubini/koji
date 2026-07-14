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
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        });

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

// ── LangfuseClient wrapper ───────────────────────────────────────────────

use std::sync::Arc;

use langfuse_ergonomic::ClientBuilder;

/// Wrapper around the `langfuse-ergonomic` SDK client.
///
/// Provides lazy initialization from `LangfuseConfig` and a single public method
/// for reporting per-request telemetry as a Langfuse trace + generation.
#[derive(Clone)]
pub struct LangfuseClient {
    inner: Arc<langfuse_ergonomic::LangfuseClient>,
}

impl LangfuseClient {
    /// Create a new `LangfuseClient` from config.
    ///
    /// Returns `None` when langfuse is disabled or credentials are missing.
    pub fn from_config(config: &crate::config::LangfuseConfig) -> Option<Self> {
        if !config.enabled {
            tracing::info!("Langfuse disabled in config (enabled=false)");
            return None;
        }
        if config.public_key.is_empty() {
            tracing::warn!("Langfuse enabled but public_key is empty");
            return None;
        }
        if config.secret_key.is_empty() {
            tracing::warn!("Langfuse enabled but secret_key is empty");
            return None;
        }

        let inner = match ClientBuilder::new()
            .public_key(&config.public_key)
            .secret_key(&config.secret_key)
            .base_url(config.host.clone())
            .build()
        {
            Ok(client) => {
                tracing::info!(
                    host = %config.host,
                    environment = %config.environment,
                    "Langfuse client initialized successfully"
                );
                client
            }
            Err(e) => {
                tracing::error!("Langfuse client build failed: {e}");
                return None;
            }
        };

        Some(Self {
            inner: Arc::new(inner),
        })
    }

    /// Report a generation to Langfuse.
    ///
    /// Creates a trace + generation with token usage, input/output, energy cost,
    /// and trace context from headers. Runs asynchronously — failures are logged
    /// but don't affect the response to the client.
    pub async fn report_generation(&self, telemetry: LangfuseTelemetry) {
        tracing::info!(
            model = %telemetry.model,
            has_input = telemetry.input.is_some(),
            has_output = telemetry.output.is_some(),
            "Langfuse report_generation called"
        );
        let inner = Arc::clone(&self.inner);
        let model = telemetry.model.clone();
        let user_id = telemetry.user_id;
        let session_id = telemetry.session_id;
        let tags = telemetry.tags.unwrap_or_default();
        let input = telemetry.input;
        let output = telemetry.output;
        let energy_wh = telemetry.energy_wh;
        let gpu_watts = telemetry.gpu_watts;
        let trace_id_header = telemetry.trace_id; // User-provided trace ID from header
        let user_metadata = telemetry.metadata;
        let usage = telemetry.usage;
        let model_params = telemetry.model_params;

        // Convert Instant to chrono DateTime<Utc> for the SDK.
        // Instant → SystemTime: now - (now - instant)
        let now = std::time::Instant::now();
        let start_dt = chrono::DateTime::<chrono::Utc>::from(
            std::time::SystemTime::now() - (now - telemetry.start_time),
        );
        let end_dt = match telemetry.end_time {
            Some(end) => {
                chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now() - (now - end))
            }
            None => chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()),
        };

        tokio::spawn(async move {
            // Build trace metadata: merge user-provided metadata with energy info.
            let mut meta_map = serde_json::Map::new();
            if let Some(user_meta) = &user_metadata {
                if let Some(obj) = user_meta.as_object() {
                    for (k, v) in obj {
                        meta_map.insert(k.clone(), v.clone());
                    }
                }
            }
            if let Some(wh) = energy_wh {
                meta_map.insert("energy_wh".to_string(), serde_json::json!(wh));
            }
            if let Some(w) = gpu_watts {
                meta_map.insert("gpu_watts".to_string(), serde_json::json!(w));
            }

            // Build trace input/output.
            let trace_input = input.clone();
            let trace_output = output.as_ref().map(|o| serde_json::json!({ "content": o }));

            // Create trace using the SDK's builder API.
            // bon generates methods that accept `T` directly for `Option<T>` parameters.
            // We pass values directly (bon wraps them as Some internally).
            // Pass user-provided trace ID when present (respects langfuse_trace_id header).
            // When no header is present, generate a UUID ourselves so the SDK gets a
            // valid (non-empty) trace ID. Passing an empty string would create a trace
            // with an empty ID, which doesn't show up in the Langfuse UI.
            tracing::info!("Langfuse: calling inner.trace().call().await");
            let resolved_trace_id = trace_id_header
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let trace_result = inner
                .trace()
                .id(resolved_trace_id)
                .name(model.clone())
                .user_id(user_id.clone().unwrap_or_default())
                .session_id(session_id.clone().unwrap_or_default())
                .input(trace_input.clone().unwrap_or_default())
                .output(trace_output.clone().unwrap_or_default())
                .metadata(serde_json::Value::Object(meta_map.clone()))
                .tags(tags.clone())
                .call()
                .await;

            let trace_id = match trace_result {
                Ok(resp) => {
                    tracing::info!(trace_id = %resp.id, "Langfuse trace created successfully");
                    resp.id
                }
                Err(e) => {
                    tracing::error!("Langfuse trace creation failed: {e}");
                    return;
                }
            };

            // Build generation metadata: embed usage, model_params, and energy cost.
            // NOTE: langfuse-ergonomic v0.6.3 accepts _model_parameters, _prompt_tokens,
            // _completion_tokens, _total_tokens params but they are prefixed with `_` and
            // never wired to CreateGenerationBody — the SDK discards them. Embedding in
            // metadata ensures data reaches Langfuse and is queryable.
            let mut gen_meta_map = serde_json::Map::new();
            if let Some(cost) = telemetry.energy_cost {
                gen_meta_map.insert("energy".to_string(), serde_json::json!(cost));
            }
            if let Some(wh) = energy_wh {
                gen_meta_map.insert("energy_wh".to_string(), serde_json::json!(wh));
            }
            // Embed token usage in metadata (SDK's dedicated fields are no-ops).
            if let Some(u) = &usage {
                gen_meta_map.insert(
                    "prompt_tokens".to_string(),
                    serde_json::json!(u.prompt_tokens),
                );
                gen_meta_map.insert(
                    "completion_tokens".to_string(),
                    serde_json::json!(u.completion_tokens),
                );
                gen_meta_map.insert(
                    "total_tokens".to_string(),
                    serde_json::json!(u.total_tokens),
                );
            }
            // Embed model parameters in metadata (SDK's dedicated field is a no-op).
            if let Some(mp) = &model_params {
                gen_meta_map.insert("model_parameters".to_string(), mp.clone());
            }

            // Build generation input/output.
            let gen_input = input;
            let gen_output = output.map(|o| serde_json::json!({ "content": o }));

            // Create generation using the SDK's builder API.
            tracing::info!(trace_id = %trace_id, "Langfuse: calling inner.generation().call().await");
            let gen_result = inner
                .generation()
                .trace_id(trace_id)
                .name(model.clone())
                .input(gen_input.unwrap_or_default())
                .output(gen_output.unwrap_or_default())
                .start_time(start_dt)
                .end_time(end_dt)
                .model(model)
                .metadata(serde_json::Value::Object(gen_meta_map))
                .call()
                .await;

            if let Err(e) = gen_result {
                tracing::error!("Langfuse generation creation failed: {e}");
            } else {
                tracing::info!("Langfuse generation created successfully");
            }
        });
    }
}

/// Parse accumulated SSE text for Langfuse telemetry.
///
/// Extracts: accumulated content (delta.content concatenated), usage
/// (from final chunk with `usage` field), and timings (from the same chunk).
/// Skips malformed JSON lines and `[DONE]` markers gracefully.
pub fn parse_sse_accumulated(
    raw: &str,
) -> (
    Option<String>,
    Option<LangfuseUsage>,
    Option<LangfuseTimings>,
) {
    let mut content_parts = Vec::new();
    let mut usage = None;
    let mut timings = None;

    for line in raw.lines() {
        if let Some(data_content) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        {
            let trimmed = data_content.trim_end();
            if trimmed == "[DONE]" {
                continue;
            }
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                // Accumulate content from delta
                if let Some(content) = json
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|c| c.first())
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if !content.is_empty() {
                        content_parts.push(content.to_string());
                    }
                }
                // Extract usage from final chunk (empty choices)
                if json.get("usage").is_some() {
                    usage = extract_usage(&json);
                }
                // Extract timings
                if json.get("timings").is_some() {
                    timings = extract_timings(&json);
                }
            }
        }
    }

    let content = if content_parts.is_empty() {
        None
    } else {
        Some(content_parts.join(""))
    };
    (content, usage, timings)
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

    #[test]
    fn test_extract_langfuse_headers_empty_tags_filtered() {
        let mut headers = HeaderMap::new();
        headers.insert("langfuse_trace_tags", ",,,".parse().unwrap());

        let (_, _, _, _, tags) = extract_langfuse_headers(&headers);
        // Empty strings after trim should be filtered out
        assert_eq!(tags, Some(vec![]));
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

    // ── LangfuseClient::from_config ────────────────────────────────────────

    #[test]
    fn test_from_config_disabled_returns_none() {
        let config = crate::config::LangfuseConfig {
            enabled: false,
            public_key: "pk-test".to_string(),
            secret_key: "sk-test".to_string(),
            host: "https://cloud.langfuse.com".to_string(),
            environment: "test".to_string(),
            capture_input: true,
            capture_output: true,
            capture_streaming: true,
            telemetry_max_bytes: 1048576,
            electricity_price_per_kwh: 0.0,
        };

        assert!(
            LangfuseClient::from_config(&config).is_none(),
            "Expected None when langfuse is disabled"
        );
    }

    #[test]
    fn test_from_config_empty_public_key_returns_none() {
        let config = crate::config::LangfuseConfig {
            enabled: true,
            public_key: String::new(), // Empty public key
            secret_key: "sk-test".to_string(),
            host: "https://cloud.langfuse.com".to_string(),
            environment: "test".to_string(),
            capture_input: true,
            capture_output: true,
            capture_streaming: true,
            telemetry_max_bytes: 1048576,
            electricity_price_per_kwh: 0.0,
        };

        assert!(
            LangfuseClient::from_config(&config).is_none(),
            "Expected None when public_key is empty"
        );
    }

    #[test]
    fn test_from_config_empty_secret_key_returns_none() {
        let config = crate::config::LangfuseConfig {
            enabled: true,
            public_key: "pk-test".to_string(),
            secret_key: String::new(), // Empty secret key
            host: "https://cloud.langfuse.com".to_string(),
            environment: "test".to_string(),
            capture_input: true,
            capture_output: true,
            capture_streaming: true,
            telemetry_max_bytes: 1048576,
            electricity_price_per_kwh: 0.0,
        };

        assert!(
            LangfuseClient::from_config(&config).is_none(),
            "Expected None when secret_key is empty"
        );
    }

    #[test]
    fn test_from_config_valid_returns_some() {
        let config = crate::config::LangfuseConfig {
            enabled: true,
            public_key: "pk-test-langfuse".to_string(),
            secret_key: "sk-test-langfuse".to_string(),
            host: "https://cloud.langfuse.com".to_string(),
            environment: "test".to_string(),
            capture_input: true,
            capture_output: true,
            capture_streaming: true,
            telemetry_max_bytes: 1048576,
            electricity_price_per_kwh: 0.0,
        };

        assert!(
            LangfuseClient::from_config(&config).is_some(),
            "Expected Some with valid credentials"
        );
    }

    // ── parse_sse_accumulated ────────────────────────────────────────

    fn sample_sse_stream() -> &'static str {
        r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"role":"assistant"}}]}
data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":"Hello"}}]}
data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":" world"}}]}
data: {"id":"chat-1","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15},"timings":{"prompt_ms":3000.0,"predicted_ms":2000.0}}
data: [DONE]
"#
    }

    #[test]
    fn test_parse_sse_accumulated_full_stream() {
        let raw = sample_sse_stream();
        let (content, usage, timings) = parse_sse_accumulated(raw);

        // Content should be accumulated from delta chunks
        assert!(content.is_some(), "Expected content to be Some, got None");
        assert_eq!(content.unwrap(), "Hello world");

        // Usage should be extracted from the final chunk
        assert!(usage.is_some(), "Expected usage to be Some, got None");
        let u = usage.unwrap();
        assert_eq!(u.prompt_tokens, 10);
        assert_eq!(u.completion_tokens, 5);
        assert_eq!(u.total_tokens, 15);

        // Timings should be extracted from the final chunk
        assert!(timings.is_some(), "Expected timings to be Some, got None");
        let t = timings.unwrap();
        assert!((t.prompt_ms - 3000.0).abs() < f64::EPSILON);
        assert!((t.predicted_ms - 2000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_sse_accumulated_empty_content() {
        let raw = r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"role":"assistant"}}]}
data: [DONE]
"#;
        let (content, usage, timings) = parse_sse_accumulated(raw);

        // No content deltas — should be None
        assert!(
            content.is_none(),
            "Expected content to be None when no delta.content present"
        );
        assert!(usage.is_none());
        assert!(timings.is_none());
    }

    #[test]
    fn test_parse_sse_accumulated_malformed_json() {
        let raw = r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":"Hello"}}]}
data: this is not json
 data: [DONE]
"#;
        let (content, usage, timings) = parse_sse_accumulated(raw);

        // Should gracefully skip malformed JSON and extract content from valid chunks
        assert!(
            content.is_some(),
            "Expected content to be extracted despite malformed JSON lines"
        );
        assert_eq!(content.unwrap(), "Hello");
        assert!(usage.is_none());
        assert!(timings.is_none());
    }

    #[test]
    fn test_parse_sse_accumulated_no_usage() {
        let raw = r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":"Hi"}}]}
data: [DONE]
"#;
        let (content, usage, timings) = parse_sse_accumulated(raw);

        assert!(content.is_some());
        assert_eq!(content.unwrap(), "Hi");
        assert!(
            usage.is_none(),
            "Expected usage to be None when no usage field present"
        );
        assert!(timings.is_none());
    }

    #[test]
    fn test_parse_sse_accumulated_empty_string() {
        let (content, usage, timings) = parse_sse_accumulated("");
        assert!(content.is_none());
        assert!(usage.is_none());
        assert!(timings.is_none());
    }

    #[test]
    fn test_parse_sse_accumulated_only_done() {
        let raw = "data: [DONE]\n";
        let (content, usage, timings) = parse_sse_accumulated(raw);
        assert!(content.is_none());
        assert!(usage.is_none());
        assert!(timings.is_none());
    }

    #[test]
    fn test_parse_sse_accumulated_content_with_empty_deltas() {
        // Some deltas have content: null or content: "" — should skip empty strings
        let raw = r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":null}}]}
data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":""}}]}
data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":"Real content"}}]}
data: [DONE]
"#;
        let (content, _usage, _timings) = parse_sse_accumulated(raw);

        // Only non-empty content should be accumulated
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "Real content");
    }

    #[test]
    fn test_parse_sse_accumulated_no_space_prefix() {
        // SSE spec allows "data:" without trailing space — some servers emit it
        let raw = r#"data:{"id":"chat-1","choices":[{"index":0,"delta":{"content":"NoSpace"}}]}
data: [DONE]
"#;
        let (content, _usage, _timings) = parse_sse_accumulated(raw);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "NoSpace");
    }
}
