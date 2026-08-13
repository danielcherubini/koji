use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;

// ────────────────────────────────────────────────────────────────
// Anthropic API constants
// ────────────────────────────────────────────────────────────────

/// Anthropic API version header value
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic API base URL
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

// ────────────────────────────────────────────────────────────────
// Request translation: OpenAI → Anthropic
// ────────────────────────────────────────────────────────────────

/// Translate an OpenAI chat/completions request body to an Anthropic messages request.
///
/// Key transformations:
/// - Extract `system` messages from the messages array → Anthropic `system` field
/// - Map roles directly (user/assistant map 1:1)
/// - Copy `model`, `temperature`, `max_tokens`, `stream`
///
/// # Arguments
/// * `openai_body` - Parsed OpenAI request body as a JSON Value
///
/// # Returns
/// Anthropic request body as a JSON Value, or an error if the body is invalid.
pub fn translate_request_body(openai_body: &Value) -> anyhow::Result<Value> {
    let messages = openai_body
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or_else(|| anyhow::anyhow!("OpenAI request must contain a 'messages' array"))?;

    if messages.is_empty() {
        anyhow::bail!("OpenAI request 'messages' array must not be empty");
    }

    // Extract system messages (first messages with role "system")
    let mut system_parts = Vec::new();
    let mut non_system_messages = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|r| r.as_str());
        match role {
            Some("system") => {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    system_parts.push(content.to_string());
                }
            }
            _ => {
                non_system_messages.push(msg.clone());
            }
        }
    }

    // Build Anthropic messages array (without system messages)
    let anthropic_messages: Vec<Value> = non_system_messages
        .iter()
        .map(|msg| {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let content = msg
                .get("content")
                .cloned()
                .unwrap_or_else(|| Value::String("".to_string()));

            let mut new_msg = serde_json::Map::new();
            new_msg.insert("role".to_string(), Value::String(role.to_string()));
            new_msg.insert("content".to_string(), content);
            Value::Object(new_msg)
        })
        .collect();

    // Build Anthropic request body
    let mut body = serde_json::Map::new();

    // Model
    if let Some(model) = openai_body.get("model") {
        body.insert("model".to_string(), model.clone());
    }

    // System (combined from all system messages)
    if !system_parts.is_empty() {
        body.insert(
            "system".to_string(),
            Value::String(system_parts.join("\n\n")),
        );
    }

    // Messages
    body.insert("messages".to_string(), Value::Array(anthropic_messages));

    // Temperature (optional)
    if let Some(temperature) = openai_body.get("temperature") {
        body.insert("temperature".to_string(), temperature.clone());
    }

    // Max tokens (optional)
    if let Some(max_tokens) = openai_body.get("max_tokens") {
        body.insert("max_tokens".to_string(), max_tokens.clone());
    }

    // Stream (optional) — we pass this through but handle streaming separately
    if let Some(stream) = openai_body.get("stream") {
        body.insert("stream".to_string(), stream.clone());
    }

    Ok(Value::Object(body))
}

// ────────────────────────────────────────────────────────────────
// Response translation: Anthropic → OpenAI
// ────────────────────────────────────────────────────────────────

/// Translate an Anthropic messages response to an OpenAI chat/completions response.
///
/// Key transformations:
/// - Anthropic `content[0].text` → OpenAI `choices[0].message.content`
/// - Anthropic `model` → OpenAI `model`
/// - Anthropic `usage` → OpenAI `usage`
/// - Anthropic `stop_reason` → OpenAI `finish_reason`
///
/// # Arguments
/// * `anthropic_body` - Parsed Anthropic response body as a JSON Value
///
/// # Returns
/// OpenAI response body as a JSON Value, or an error if the body is invalid.
pub fn translate_response_body(anthropic_body: &Value) -> anyhow::Result<Value> {
    let model = anthropic_body
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("");

    // Extract content text from Anthropic response
    let content = anthropic_body
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    // Extract finish reason
    let stop_reason = anthropic_body
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("stop");

    let finish_reason = match stop_reason {
        "end_turn" | "stop" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    };

    // Extract usage
    let usage = anthropic_body.get("usage").map(|u| {
        let mut usage_obj = serde_json::Map::new();

        if let Some(input_tokens) = u.get("input_tokens") {
            usage_obj.insert("prompt_tokens".to_string(), input_tokens.clone());
        }
        if let Some(output_tokens) = u.get("output_tokens") {
            usage_obj.insert("completion_tokens".to_string(), output_tokens.clone());
        }

        // Calculate total_tokens
        let input: i64 = u.get("input_tokens").and_then(|t| t.as_i64()).unwrap_or(0);
        let output: i64 = u.get("output_tokens").and_then(|t| t.as_i64()).unwrap_or(0);
        usage_obj.insert(
            "total_tokens".to_string(),
            Value::Number(serde_json::Number::from(input + output)),
        );

        Value::Object(usage_obj)
    });

    // Build OpenAI response
    let mut choice_message = serde_json::Map::new();
    choice_message.insert("role".to_string(), Value::String("assistant".to_string()));
    choice_message.insert("content".to_string(), Value::String(content.to_string()));

    let mut choice = serde_json::Map::new();
    choice.insert(
        "index".to_string(),
        Value::Number(serde_json::Number::from(0)),
    );
    choice.insert("message".to_string(), Value::Object(choice_message));
    choice.insert(
        "finish_reason".to_string(),
        Value::String(finish_reason.to_string()),
    );

    let mut response = serde_json::Map::new();
    response.insert(
        "id".to_string(),
        Value::String(format!("chatcmpl-{}", uuid::Uuid::new_v4())),
    );
    response.insert(
        "object".to_string(),
        Value::String("chat.completion".to_string()),
    );
    response.insert(
        "created".to_string(),
        Value::Number(serde_json::Number::from(chrono::Utc::now().timestamp())),
    );
    response.insert("model".to_string(), Value::String(model.to_string()));
    response.insert(
        "choices".to_string(),
        Value::Array(vec![Value::Object(choice)]),
    );

    if let Some(usage) = usage {
        response.insert("usage".to_string(), usage);
    }

    Ok(Value::Object(response))
}

// ────────────────────────────────────────────────────────────────
// Streaming translation: Anthropic SSE → OpenAI SSE
// ────────────────────────────────────────────────────────────────

/// Translate a single Anthropic SSE event to an OpenAI SSE delta chunk.
///
/// Anthropic streaming events:
/// - `message_start` — contains model, usage, content array start
/// - `content_block_start` — content block starting
/// - `content_block_delta` — text delta
/// - `content_block_stop` — content block ending
/// - `message_delta` — usage delta, stop_reason
/// - `message_stop` — stream end
///
/// We convert `content_block_delta` events to OpenAI format and
/// `message_delta` with stop_reason to a final chunk.
///
/// # Arguments
/// * `event_type` - Anthropic SSE event type (e.g. "content_block_delta")
/// * `data` - Parsed JSON data from the SSE event
///
/// # Returns
/// Some OpenAI SSE formatted string, or None if the event should be skipped.
pub fn translate_stream_event(event_type: &str, data: &Value) -> Option<String> {
    match event_type {
        "content_block_delta" => {
            // Extract the text delta
            let delta_text = data
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())?;

            let mut delta = serde_json::Map::new();
            delta.insert("content".to_string(), Value::String(delta_text.to_string()));

            let mut choice = serde_json::Map::new();
            choice.insert(
                "index".to_string(),
                Value::Number(serde_json::Number::from(0)),
            );
            choice.insert("delta".to_string(), Value::Object(delta));

            let mut chunk = serde_json::Map::new();
            chunk.insert(
                "choices".to_string(),
                Value::Array(vec![Value::Object(choice)]),
            );

            Some(format!(
                "data: {}\n\n",
                serde_json::to_string(&chunk).unwrap()
            ))
        }
        "message_delta" => {
            // Final chunk with finish_reason
            let stop_reason = data
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|s| s.as_str())
                .unwrap_or("end_turn");

            let finish_reason = match stop_reason {
                "end_turn" | "stop" => "stop",
                "max_tokens" => "length",
                "tool_use" => "tool_calls",
                _ => "stop",
            };

            let mut delta = serde_json::Map::new();
            delta.insert("content".to_string(), Value::Null);

            let mut choice = serde_json::Map::new();
            choice.insert(
                "index".to_string(),
                Value::Number(serde_json::Number::from(0)),
            );
            choice.insert("delta".to_string(), Value::Object(delta));
            choice.insert(
                "finish_reason".to_string(),
                Value::String(finish_reason.to_string()),
            );

            let mut chunk = serde_json::Map::new();
            chunk.insert(
                "choices".to_string(),
                Value::Array(vec![Value::Object(choice)]),
            );

            Some(format!(
                "data: {}\n\n",
                serde_json::to_string(&chunk).unwrap()
            ))
        }
        "message_stop" => {
            // Signal end of stream
            Some("data: [DONE]\n\n".to_string())
        }
        _ => None, // Skip other event types
    }
}

/// Parse an SSE line and return (event_type, data) if it's a complete event.
///
/// SSE format:
/// ```text
/// event: content_block_delta
/// data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}
///
/// ```
///
/// Returns None for incomplete events (no blank line yet).
#[allow(dead_code)]
fn parse_sse_event(line: &str) -> Option<(String, Value)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    if let Some(event) = line.strip_prefix("event: ") {
        Some((event.trim().to_string(), Value::Null))
    } else if let Some(data) = line.strip_prefix("data: ") {
        let parsed =
            serde_json::from_str(data.trim()).unwrap_or(Value::String(data.trim().to_string()));
        // The event type will be set from the previous line
        Some(("".to_string(), parsed))
    } else {
        None
    }
}

// ────────────────────────────────────────────────────────────────
// AnthropicForwarder
// ────────────────────────────────────────────────────────────────

use crate::providers::Provider;
use axum::body::Body;
use axum::http::request::Parts;

/// Forwards HTTP requests to Anthropic's API, translating between
/// OpenAI and Anthropic formats.
#[derive(Clone)]
pub struct AnthropicForwarder {
    client: reqwest::Client,
}

impl AnthropicForwarder {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Forward an HTTP request to Anthropic's API, translating the request
    /// and response between OpenAI and Anthropic formats.
    ///
    /// # Arguments
    /// * `provider` - The Anthropic provider configuration
    /// * `parts` - The HTTP request parts
    /// * `body` - The request body bytes (OpenAI format)
    ///
    /// # Returns
    /// The provider's response translated to OpenAI format, streamed back to the client.
    pub async fn forward(
        &self,
        provider: &Provider,
        parts: &Parts,
        body: Bytes,
    ) -> anyhow::Result<http::Response<Body>> {
        let api_key = provider.api_key.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "Anthropic provider '{}' has no api_key configured",
                provider.name
            )
        })?;

        // Parse OpenAI request body
        let openai_body: Value =
            serde_json::from_slice(&body).unwrap_or_else(|_| Value::Object(serde_json::Map::new()));

        // Translate to Anthropic format
        let anthropic_body = translate_request_body(&openai_body)?;

        // Determine if streaming
        let is_stream = openai_body
            .get("stream")
            .and_then(|s| s.as_bool())
            .unwrap_or(false);

        // Build target URL
        let base_url = provider
            .base_url
            .as_deref()
            .unwrap_or(ANTHROPIC_BASE_URL)
            .trim_end_matches('/');

        // Anthropic uses /v1/messages endpoint
        let target_url = format!("{}/v1/messages", base_url);

        // Build request
        let mut request = self.client.post(&target_url);

        // Set Anthropic-specific headers
        request = request.header("anthropic-version", ANTHROPIC_VERSION);
        request = request.header("x-api-key", api_key);
        request = request.header(http::header::CONTENT_TYPE, "application/json");

        // Forward other relevant headers (skip host and auth)
        for (name, value) in &parts.headers {
            let should_skip = name.as_str().eq_ignore_ascii_case("host")
                || name.as_str().eq_ignore_ascii_case("authorization")
                || name.as_str().eq_ignore_ascii_case("anthropic-version")
                || name.as_str().eq_ignore_ascii_case("x-api-key");
            if !should_skip {
                request = request.header(name, value);
            }
        }

        let request_body = serde_json::to_string(&anthropic_body)?;
        let request = request.body(request_body).build()?;

        let response = self.client.execute(request).await?;

        // Check for upstream errors before processing response
        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.bytes().await.unwrap_or_default();
            let error_msg = String::from_utf8_lossy(&error_body);
            anyhow::bail!(
                "Anthropic API returned {} {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown"),
                error_msg
            );
        }

        // Handle streaming response
        if is_stream {
            return Ok(Self::translate_stream_response(response).await);
        }

        // Handle non-streaming response
        let response_body = response.bytes().await?;
        let anthropic_response: Value =
            serde_json::from_slice(&response_body).unwrap_or(Value::Object(serde_json::Map::new()));
        let openai_response = translate_response_body(&anthropic_response)?;

        let body_bytes = serde_json::to_vec(&openai_response)?;
        let mut axum_response = http::Response::new(Body::from(body_bytes));
        *axum_response.status_mut() = http::StatusCode::OK;
        axum_response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        Ok(axum_response)
    }

    /// Stream Anthropic SSE response and translate to OpenAI SSE format.
    async fn translate_stream_response(response: reqwest::Response) -> http::Response<Body> {
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<Result<Bytes, std::convert::Infallible>>(64);

        tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut byte_buffer = Vec::new();
            let mut current_event = String::new();
            let mut pending_data = String::new();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        // Accumulate raw bytes to avoid corrupting multi-byte UTF-8
                        // characters split across chunk boundaries
                        byte_buffer.extend_from_slice(&chunk);
                        if let Ok(valid) = std::str::from_utf8(&byte_buffer) {
                            buffer.push_str(valid);
                            byte_buffer.clear();
                        } else {
                            // Find the longest valid UTF-8 prefix by shrinking from the end.
                            // UTF-8 continuation bytes (0x80..=0xBF) are never valid starts,
                            // so we skip past them quickly before trying from_utf8.
                            let len = byte_buffer.len();
                            let mut end = len;
                            while end > 0 && (0x80..=0xBF).contains(&byte_buffer[end - 1]) {
                                end -= 1;
                            }
                            if end > 0 {
                                while end > 0 {
                                    if let Ok(valid) = std::str::from_utf8(&byte_buffer[..end]) {
                                        buffer.push_str(valid);
                                        let remaining = byte_buffer.split_off(end);
                                        byte_buffer = remaining;
                                        break;
                                    }
                                    end -= 1;
                                }
                            }
                            // If end == 0, keep all bytes in byte_buffer for next chunk
                        }

                        // Process complete lines
                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].to_string();
                            buffer.drain(..=newline_pos);

                            let line = line.trim_end_matches('\r').to_string();

                            if line.is_empty() {
                                // Blank line = end of SSE event
                                if !current_event.is_empty() && !pending_data.is_empty() {
                                    if let Ok(data) = serde_json::from_str::<Value>(&pending_data) {
                                        if let Some(translated) =
                                            translate_stream_event(&current_event, &data)
                                        {
                                            let _ = tx.send(Ok(Bytes::from(translated))).await;
                                        }
                                    }
                                }
                                current_event = String::new();
                                pending_data = String::new();
                            } else if let Some(event_name) = line.strip_prefix("event: ") {
                                current_event = event_name.trim().to_string();
                            } else if let Some(data) = line.strip_prefix("data: ") {
                                pending_data = data.trim().to_string();
                            }
                        }
                    }
                    Err(_) => break,
                }
            }

            // Send final DONE marker
            let _ = tx.send(Ok(Bytes::from("data: [DONE]\n\n"))).await;
        });

        let body_stream = async_stream::stream! {
            while let Some(result) = rx.recv().await {
                yield result;
            }
        };

        let mut axum_response = http::Response::new(Body::from_stream(body_stream));
        *axum_response.status_mut() = http::StatusCode::OK;
        axum_response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            "text/event-stream".parse().unwrap(),
        );
        axum_response
            .headers_mut()
            .insert(http::header::CACHE_CONTROL, "no-cache".parse().unwrap());

        axum_response
    }
}

impl Default for AnthropicForwarder {
    fn default() -> Self {
        Self::new()
    }
}

// ────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ──

    fn make_openai_request(
        model: &str,
        messages: Vec<(&str, &str)>,
        temperature: Option<f64>,
        max_tokens: Option<u32>,
        stream: Option<bool>,
    ) -> Value {
        let mut body = serde_json::Map::new();

        body.insert("model".to_string(), Value::String(model.to_string()));

        let messages_array: Vec<Value> = messages
            .into_iter()
            .map(|(role, content)| {
                let mut msg = serde_json::Map::new();
                msg.insert("role".to_string(), Value::String(role.to_string()));
                msg.insert("content".to_string(), Value::String(content.to_string()));
                Value::Object(msg)
            })
            .collect();
        body.insert("messages".to_string(), Value::Array(messages_array));

        if let Some(temp) = temperature {
            body.insert(
                "temperature".to_string(),
                Value::Number(serde_json::Number::from_f64(temp).unwrap()),
            );
        }
        if let Some(max) = max_tokens {
            body.insert(
                "max_tokens".to_string(),
                Value::Number(serde_json::Number::from(max)),
            );
        }
        if let Some(s) = stream {
            body.insert("stream".to_string(), Value::Bool(s));
        }

        Value::Object(body)
    }

    // ── Request translation tests ──

    #[test]
    fn test_translate_request_basic_messages() {
        let openai_body = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![("user", "Hello, how are you?")],
            None,
            None,
            None,
        );

        let anthropic_body = translate_request_body(&openai_body).unwrap();

        // Model should be preserved
        assert_eq!(
            anthropic_body.get("model").and_then(|m| m.as_str()),
            Some("claude-3-5-sonnet-20241022")
        );

        // Messages should be translated correctly
        let messages = anthropic_body
            .get("messages")
            .and_then(|m| m.as_array())
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].get("role").and_then(|r| r.as_str()),
            Some("user")
        );
        assert_eq!(
            messages[0].get("content").and_then(|c| c.as_str()),
            Some("Hello, how are you?")
        );

        // No system field (no system message in input)
        assert!(anthropic_body.get("system").is_none());
    }

    #[test]
    fn test_translate_request_with_system_message() {
        let openai_body = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![
                ("system", "You are a helpful assistant."),
                ("user", "Hello"),
                ("assistant", "Hi there!"),
                ("user", "How are you?"),
            ],
            None,
            None,
            None,
        );

        let anthropic_body = translate_request_body(&openai_body).unwrap();

        // System should be extracted to top-level field
        assert_eq!(
            anthropic_body.get("system").and_then(|s| s.as_str()),
            Some("You are a helpful assistant.")
        );

        // Messages should not include system message
        let messages = anthropic_body
            .get("messages")
            .and_then(|m| m.as_array())
            .unwrap();
        assert_eq!(messages.len(), 3); // user, assistant, user

        // Roles should be preserved
        assert_eq!(
            messages[0].get("role").and_then(|r| r.as_str()),
            Some("user")
        );
        assert_eq!(
            messages[1].get("role").and_then(|r| r.as_str()),
            Some("assistant")
        );
        assert_eq!(
            messages[2].get("role").and_then(|r| r.as_str()),
            Some("user")
        );
    }

    #[test]
    fn test_translate_request_multiple_system_messages() {
        let openai_body = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![
                ("system", "First instruction."),
                ("system", "Second instruction."),
                ("user", "Hello"),
            ],
            None,
            None,
            None,
        );

        let anthropic_body = translate_request_body(&openai_body).unwrap();

        // Multiple system messages should be combined with blank line separator
        assert_eq!(
            anthropic_body.get("system").and_then(|s| s.as_str()),
            Some("First instruction.\n\nSecond instruction.")
        );

        // Only user message remains in messages array
        let messages = anthropic_body
            .get("messages")
            .and_then(|m| m.as_array())
            .unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn test_translate_request_with_temperature() {
        let openai_body = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![("user", "Hello")],
            Some(0.7),
            None,
            None,
        );

        let anthropic_body = translate_request_body(&openai_body).unwrap();

        assert!(anthropic_body.get("temperature").is_some());
        assert_eq!(
            anthropic_body.get("temperature").and_then(|t| t.as_f64()),
            Some(0.7)
        );
    }

    #[test]
    fn test_translate_request_with_max_tokens() {
        let openai_body = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![("user", "Hello")],
            None,
            Some(1000),
            None,
        );

        let anthropic_body = translate_request_body(&openai_body).unwrap();

        assert!(anthropic_body.get("max_tokens").is_some());
        assert_eq!(
            anthropic_body.get("max_tokens").and_then(|t| t.as_u64()),
            Some(1000)
        );
    }

    #[test]
    fn test_translate_request_with_stream() {
        let openai_body = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![("user", "Hello")],
            None,
            None,
            Some(true),
        );

        let anthropic_body = translate_request_body(&openai_body).unwrap();

        assert_eq!(
            anthropic_body.get("stream").and_then(|s| s.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn test_translate_request_empty_messages_fails() {
        let mut body = serde_json::Map::new();
        body.insert("model".to_string(), Value::String("claude-3".to_string()));
        body.insert("messages".to_string(), Value::Array(vec![]));

        let result = translate_request_body(&Value::Object(body));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must not be empty"));
    }

    #[test]
    fn test_translate_request_missing_messages_fails() {
        let body = serde_json::json!({
            "model": "claude-3"
        });
        let result = translate_request_body(&body);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("messages"));
    }

    #[test]
    fn test_translate_request_preserves_message_order() {
        let openai_body = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![
                ("system", "Be nice."),
                ("user", "Hello"),
                ("assistant", "Hi!"),
                ("user", "Tell me a joke"),
                ("assistant", "Why did the..."),
                ("user", "No thanks"),
            ],
            None,
            None,
            None,
        );

        let anthropic_body = translate_request_body(&openai_body).unwrap();

        let messages = anthropic_body
            .get("messages")
            .and_then(|m| m.as_array())
            .unwrap();

        // Verify order is preserved (5 messages, system extracted)
        assert_eq!(messages.len(), 5);

        // Verify the conversation sequence
        let roles: Vec<&str> = messages
            .iter()
            .map(|m| m.get("role").and_then(|r| r.as_str()).unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant", "user"]);
    }

    // ── Response translation tests ──

    #[test]
    fn test_translate_response_basic() {
        let anthropic_body = serde_json::json!({
            "id": "msg_abc123",
            "model": "claude-3-5-sonnet-20241022",
            "content": [
                {"type": "text", "text": "Hello! How can I help you?"}
            ],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "role": "assistant",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 8
            }
        });

        let openai_body = translate_response_body(&anthropic_body).unwrap();

        // Check structure
        assert_eq!(
            openai_body.get("object").and_then(|o| o.as_str()),
            Some("chat.completion")
        );

        // Check model
        assert_eq!(
            openai_body.get("model").and_then(|m| m.as_str()),
            Some("claude-3-5-sonnet-20241022")
        );

        // Check choices
        let choices = openai_body
            .get("choices")
            .and_then(|c| c.as_array())
            .unwrap();
        assert_eq!(choices.len(), 1);

        let choice = &choices[0];
        let message = choice.get("message").unwrap();
        assert_eq!(
            message.get("role").and_then(|r| r.as_str()),
            Some("assistant")
        );
        assert_eq!(
            message.get("content").and_then(|c| c.as_str()),
            Some("Hello! How can I help you?")
        );
        assert_eq!(
            choice.get("finish_reason").and_then(|f| f.as_str()),
            Some("stop")
        );

        // Check usage translation
        let usage = openai_body.get("usage").unwrap();
        assert_eq!(
            usage.get("prompt_tokens").and_then(|t| t.as_i64()),
            Some(10)
        );
        assert_eq!(
            usage.get("completion_tokens").and_then(|t| t.as_i64()),
            Some(8)
        );
        assert_eq!(usage.get("total_tokens").and_then(|t| t.as_i64()), Some(18));
    }

    #[test]
    fn test_translate_response_max_tokens_stop_reason() {
        let anthropic_body = serde_json::json!({
            "id": "msg_xyz",
            "model": "claude-3",
            "content": [
                {"type": "text", "text": "This is a long"}
            ],
            "stop_reason": "max_tokens",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 500
            }
        });

        let openai_body = translate_response_body(&anthropic_body).unwrap();

        let choices = openai_body
            .get("choices")
            .and_then(|c| c.as_array())
            .unwrap();
        assert_eq!(
            choices[0].get("finish_reason").and_then(|f| f.as_str()),
            Some("length")
        );
    }

    #[test]
    fn test_translate_response_empty_content() {
        let anthropic_body = serde_json::json!({
            "id": "msg_empty",
            "model": "claude-3",
            "content": [],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 0
            }
        });

        let openai_body = translate_response_body(&anthropic_body).unwrap();

        let choices = openai_body
            .get("choices")
            .and_then(|c| c.as_array())
            .unwrap();
        let message = choices[0].get("message").unwrap();
        assert_eq!(message.get("content").and_then(|c| c.as_str()), Some(""));
    }

    #[test]
    fn test_translate_response_with_id_format() {
        let anthropic_body = serde_json::json!({
            "id": "msg_test",
            "model": "claude-3",
            "content": [{"type": "text", "text": "Hi"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        });

        let openai_body = translate_response_body(&anthropic_body).unwrap();

        let id = openai_body.get("id").and_then(|i| i.as_str()).unwrap();
        assert!(id.starts_with("chatcmpl-"));
    }

    // ── Streaming translation tests ──

    #[test]
    fn test_translate_stream_content_block_delta() {
        let data = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "text_delta",
                "text": "Hello"
            }
        });

        let result = translate_stream_event("content_block_delta", &data);
        assert!(result.is_some());

        let sse = result.unwrap();
        assert!(sse.starts_with("data: "));
        assert!(sse.ends_with("\n\n"));

        // Parse the JSON from the SSE data
        let json_str = sse.strip_prefix("data: ").unwrap().trim();
        let chunk: Value = serde_json::from_str(json_str).unwrap();

        let choices = chunk.get("choices").and_then(|c| c.as_array()).unwrap();
        assert_eq!(choices.len(), 1);
        let delta = choices[0].get("delta").and_then(|d| d.as_object()).unwrap();
        assert_eq!(delta.get("content").and_then(|c| c.as_str()), Some("Hello"));
    }

    #[test]
    fn test_translate_stream_message_delta() {
        let data = serde_json::json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "end_turn",
                "stop_sequence": null
            },
            "usage": {
                "output_tokens": 42
            }
        });

        let result = translate_stream_event("message_delta", &data);
        assert!(result.is_some());

        let sse = result.unwrap();
        let json_str = sse.strip_prefix("data: ").unwrap().trim();
        let chunk: Value = serde_json::from_str(json_str).unwrap();

        let choices = chunk.get("choices").and_then(|c| c.as_array()).unwrap();
        assert_eq!(
            choices[0].get("finish_reason").and_then(|f| f.as_str()),
            Some("stop")
        );
    }

    #[test]
    fn test_translate_stream_message_delta_max_tokens() {
        let data = serde_json::json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "max_tokens",
                "stop_sequence": null
            }
        });

        let result = translate_stream_event("message_delta", &data);
        let sse = result.unwrap();
        let json_str = sse.strip_prefix("data: ").unwrap().trim();
        let chunk: Value = serde_json::from_str(json_str).unwrap();

        let choices = chunk.get("choices").and_then(|c| c.as_array()).unwrap();
        assert_eq!(
            choices[0].get("finish_reason").and_then(|f| f.as_str()),
            Some("length")
        );
    }

    #[test]
    fn test_translate_stream_message_stop() {
        let result = translate_stream_event("message_stop", &serde_json::json!({}));
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "data: [DONE]\n\n");
    }

    #[test]
    fn test_translate_stream_unknown_event_returns_none() {
        let result = translate_stream_event("ping", &serde_json::json!({}));
        assert!(result.is_none());
    }

    #[test]
    fn test_translate_stream_content_block_delta_empty_text() {
        let data = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "text_delta",
                "text": ""
            }
        });

        // Empty text returns None because of the ? operator on as_str()
        let result = translate_stream_event("content_block_delta", &data);
        // An empty string is not None from as_str(), but ? on Option returns None for empty
        // Actually "" is Some(""), so this should return Some
        assert!(result.is_some());
    }

    #[test]
    fn test_translate_stream_event_message_start_returns_none() {
        let data = serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_123",
                "model": "claude-3",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "role": "assistant",
                "usage": {"input_tokens": 10}
            }
        });

        // message_start should be skipped (we don't forward metadata)
        let result = translate_stream_event("message_start", &data);
        assert!(result.is_none());
    }

    #[test]
    fn test_translate_stream_event_content_block_start_returns_none() {
        let data = serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        });

        let result = translate_stream_event("content_block_start", &data);
        assert!(result.is_none());
    }

    #[test]
    fn test_translate_stream_event_content_block_stop_returns_none() {
        let data = serde_json::json!({
            "type": "content_block_stop",
            "index": 0
        });

        let result = translate_stream_event("content_block_stop", &data);
        assert!(result.is_none());
    }

    // ── SSE parsing tests ──

    #[test]
    fn test_parse_sse_event_line() {
        let result = parse_sse_event("event: content_block_delta");
        assert!(result.is_some());
        let (event, _data) = result.unwrap();
        assert_eq!(event, "content_block_delta");
    }

    #[test]
    fn test_parse_sse_data_line() {
        let result =
            parse_sse_event("data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"Hi\"}}");
        assert!(result.is_some());
        let (_, data) = result.unwrap();
        assert!(data.is_object());
    }

    #[test]
    fn test_parse_sse_empty_line_returns_none() {
        let result = parse_sse_event("");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_whitespace_line_returns_none() {
        let result = parse_sse_event("   ");
        assert!(result.is_none());
    }

    // ── Integration: request roundtrip ──

    #[test]
    fn test_request_response_roundtrip() {
        // Create an OpenAI request
        let openai_request = make_openai_request(
            "claude-3-5-sonnet-20241022",
            vec![
                ("system", "You are a math expert."),
                ("user", "What is 2+2?"),
            ],
            Some(0.0),
            Some(100),
            None,
        );

        // Translate to Anthropic
        let _anthropic_request = translate_request_body(&openai_request).unwrap();

        // Simulate Anthropic response
        let anthropic_response = serde_json::json!({
            "id": "msg_123",
            "model": "claude-3-5-sonnet-20241022",
            "content": [
                {"type": "text", "text": "2+2 equals 4."}
            ],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "role": "assistant",
            "usage": {
                "input_tokens": 15,
                "output_tokens": 6
            }
        });

        // Translate back to OpenAI
        let openai_response = translate_response_body(&anthropic_response).unwrap();

        // Verify the roundtrip
        assert_eq!(
            openai_response.get("object").and_then(|o| o.as_str()),
            Some("chat.completion")
        );

        let choices = openai_response
            .get("choices")
            .and_then(|c| c.as_array())
            .unwrap();
        let content = choices[0]
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(content, "2+2 equals 4.");

        let usage = openai_response.get("usage").unwrap();
        assert_eq!(
            usage.get("prompt_tokens").and_then(|t| t.as_i64()),
            Some(15)
        );
        assert_eq!(usage.get("total_tokens").and_then(|t| t.as_i64()), Some(21));
    }

    // ── AnthropicForwarder constants tests ──

    #[test]
    fn test_anthropic_version_constant() {
        assert_eq!(ANTHROPIC_VERSION, "2023-06-01");
    }

    #[test]
    fn test_anthropic_base_url_constant() {
        assert_eq!(ANTHROPIC_BASE_URL, "https://api.anthropic.com");
    }

    #[test]
    fn test_anthropic_forwarder_new() {
        let _forwarder = AnthropicForwarder::new();
    }

    #[test]
    fn test_anthropic_forwarder_default() {
        let _ = AnthropicForwarder::default();
    }
}
