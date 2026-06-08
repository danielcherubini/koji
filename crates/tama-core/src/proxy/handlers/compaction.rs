//! Compaction endpoint handler.
//!
//! Handles POST /v1/compaction — compresses prompts using LLMLingua-2.

use crate::config::MAX_REQUEST_BODY_SIZE;
use crate::proxy::ProxyState;
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Request for raw text compression.
#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum CompactionRequest {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default = "default_rate")]
        rate: f64,
        #[serde(default = "default_force_tokens")]
        force_tokens: Vec<String>,
        #[serde(default = "default_chunk_end_tokens")]
        chunk_end_tokens: Vec<String>,
    },
    #[serde(rename = "messages")]
    Messages {
        messages: Vec<serde_json::Value>,
        #[serde(default = "default_rates")]
        rates: HashMap<String, f64>,
        #[serde(default = "default_force_tokens")]
        force_tokens: Vec<String>,
        #[serde(default = "default_chunk_end_tokens")]
        chunk_end_tokens: Vec<String>,
    },
}

fn default_rate() -> f64 {
    0.3
}

fn default_force_tokens() -> Vec<String> {
    vec!["\n".to_string()]
}

fn default_chunk_end_tokens() -> Vec<String> {
    vec![".".to_string(), "\n".to_string()]
}

fn default_rates() -> HashMap<String, f64> {
    let mut map = HashMap::new();
    map.insert("system".to_string(), 0.8);
    map.insert("user".to_string(), 0.3);
    map.insert("assistant".to_string(), 0.3);
    map.insert("default".to_string(), 0.3);
    map
}

/// Response from the compaction endpoint.
#[derive(Debug, Serialize)]
pub struct CompactionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_messages: Option<Vec<serde_json::Value>>,
    pub original_tokens: u64,
    pub compressed_tokens: u64,
    pub compression_ratio: f64,
    pub latency_ms: u64,
    pub status: String,
}

impl CompactionResponse {
    /// Create a fallback response when compaction is unavailable.
    fn skipped(text: Option<String>, messages: Option<Vec<serde_json::Value>>) -> Self {
        Self {
            compressed_text: text,
            compressed_messages: messages,
            original_tokens: 0,
            compressed_tokens: 0,
            compression_ratio: 1.0,
            latency_ms: 0,
            status: "skipped".to_string(),
        }
    }
}

#[axum::debug_handler]
pub async fn handle_compaction(state: State<Arc<ProxyState>>, req: Request<Body>) -> Response {
    // Read and parse request body
    let (_parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return super::json_error_response(),
    };

    let request: CompactionRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Failed to parse compaction request: {}", e);
            return super::json_error_response();
        }
    };

    // Check if compaction is enabled
    let timeout_ms = {
        let config = state.config.read().await;
        if !config.compaction.enabled {
            drop(config);
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({
                    "error": {
                        "message": "Compaction is not enabled. Add [compaction] section to config.toml.",
                        "type": "NotImplementedError"
                    }
                })),
            )
                .into_response();
        }
        config.compaction.timeout_ms
    };

    // Ensure server is running
    let server_url = match state.ensure_compaction_server().await {
        Ok(url) => url,
        Err(e) => {
            tracing::warn!("Compaction server unavailable: {}", e);
            // Fallback: return original text
            let response = match &request {
                CompactionRequest::Text { text, .. } => {
                    CompactionResponse::skipped(Some(text.clone()), None)
                }
                CompactionRequest::Messages { messages, .. } => {
                    CompactionResponse::skipped(None, Some(messages.clone()))
                }
            };
            return (StatusCode::SERVICE_UNAVAILABLE, Json(response)).into_response();
        }
    };

    // Build the forward request body
    let (forward_body, original_text, original_messages) = match &request {
        CompactionRequest::Text {
            text,
            rate,
            force_tokens,
            chunk_end_tokens,
        } => {
            let body = serde_json::json!({
                "mode": "text",
                "text": text,
                "rate": rate,
                "force_tokens": force_tokens,
                "chunk_end_tokens": chunk_end_tokens,
            });
            (body, Some(text.clone()), None)
        }
        CompactionRequest::Messages {
            messages,
            rates,
            force_tokens,
            chunk_end_tokens,
        } => {
            let body = serde_json::json!({
                "mode": "messages",
                "messages": messages,
                "rates": rates,
                "force_tokens": force_tokens,
                "chunk_end_tokens": chunk_end_tokens,
            });
            (body, None, Some(messages.clone()))
        }
    };

    // Forward to compaction server with timeout
    let url = format!("{}/compress", server_url);
    let timeout = Duration::from_millis(timeout_ms);

    let result = tokio::time::timeout(timeout, async {
        match state.client.post(&url).json(&forward_body).send().await {
            Ok(resp) => resp.json::<serde_json::Value>().await,
            Err(e) => Err(e),
        }
    })
    .await;

    match result {
        Ok(Ok(response)) => {
            // Parse and return the server response
            let compressed_text = response
                .get("compressed_text")
                .and_then(|v| v.as_str())
                .map(String::from);
            let compressed_messages = response
                .get("compressed_messages")
                .and_then(|v| v.as_array())
                .map(|arr| arr.to_vec());
            let original_tokens = response
                .get("original_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let compressed_tokens = response
                .get("compressed_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let compression_ratio = response
                .get("compression_ratio")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0);
            let latency_ms = response
                .get("latency_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let status = response
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("compressed")
                .to_string();

            (
                StatusCode::OK,
                Json(CompactionResponse {
                    compressed_text,
                    compressed_messages,
                    original_tokens,
                    compressed_tokens,
                    compression_ratio,
                    latency_ms,
                    status,
                }),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!("Compaction server returned error: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(CompactionResponse::skipped(
                    original_text,
                    original_messages,
                )),
            )
                .into_response()
        }
        Err(_) => {
            tracing::warn!("Compaction request timed out after {}ms", timeout_ms);
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(CompactionResponse::skipped(
                    original_text,
                    original_messages,
                )),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rate() {
        assert_eq!(default_rate(), 0.3);
    }

    #[test]
    fn test_default_force_tokens() {
        let tokens = default_force_tokens();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], "\n");
    }

    #[test]
    fn test_default_chunk_end_tokens() {
        let tokens = default_chunk_end_tokens();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0], ".");
        assert_eq!(tokens[1], "\n");
    }

    #[test]
    fn test_default_rates() {
        let rates = default_rates();
        assert_eq!(rates.get("system"), Some(&0.8));
        assert_eq!(rates.get("user"), Some(&0.3));
        assert_eq!(rates.get("assistant"), Some(&0.3));
        assert_eq!(rates.get("default"), Some(&0.3));
    }

    #[test]
    fn test_compaction_response_skipped() {
        let resp = CompactionResponse::skipped(Some("original text".to_string()), None);
        assert_eq!(resp.status, "skipped");
        assert_eq!(resp.compression_ratio, 1.0);
        assert_eq!(resp.compressed_text, Some("original text".to_string()));
        assert!(resp.compressed_messages.is_none());
    }

    #[test]
    fn test_compaction_request_text_mode_deserialize() {
        let json = r#"{"mode": "text", "text": "hello world", "rate": 0.5}"#;
        let req: CompactionRequest = serde_json::from_str(json).unwrap();
        match req {
            CompactionRequest::Text { text, rate, .. } => {
                assert_eq!(text, "hello world");
                assert_eq!(rate, 0.5);
            }
            _ => panic!("Expected Text mode"),
        }
    }

    #[test]
    fn test_compaction_request_messages_mode_deserialize() {
        let json = r#"{"mode": "messages", "messages": [{"role": "user", "content": "hello"}]}"#;
        let req: CompactionRequest = serde_json::from_str(json).unwrap();
        match req {
            CompactionRequest::Messages { messages, .. } => {
                assert_eq!(messages.len(), 1);
            }
            _ => panic!("Expected Messages mode"),
        }
    }

    /// Integration test: 501 when compaction is disabled.
    #[tokio::test]
    async fn test_compaction_disabled_returns_501() {
        let config = crate::config::Config::default();
        let state = Arc::new(ProxyState::new(config, None));
        let app = crate::proxy::server::router::build_router(state.clone()).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{}/v1/compaction", addr))
            .json(&serde_json::json!({
                "mode": "text",
                "text": "test"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    /// Integration test: compaction request body size limit.
    /// `to_bytes(body, MAX_REQUEST_BODY_SIZE)` errors on oversized bodies.
    #[tokio::test]
    async fn test_compaction_body_size_limit() {
        let config = crate::config::Config::default();
        let state = Arc::new(ProxyState::new(config, None));
        let app = crate::proxy::server::router::build_router(state.clone()).await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        // Send a body larger than MAX_REQUEST_BODY_SIZE (16MB)
        let large_text = "x".repeat(17 * 1024 * 1024);
        let resp = client
            .post(format!("http://{}/v1/compaction", addr))
            .body(large_text)
            .send()
            .await
            .unwrap();
        // to_bytes errors on oversized bodies → handler returns 400
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "Oversized body should return 400 Bad Request"
        );
    }
}
