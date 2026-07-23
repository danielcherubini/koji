//! TTS (Text-to-Speech) API handlers.
//!
//! Implements OpenAI-compatible `/v1/audio/*` endpoints for speech synthesis.
//! The TTS backend runs as a subprocess (Kokoro-FastAPI uvicorn server).

use crate::proxy::ProxyState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use serde::Deserialize;
use std::sync::Arc;

use super::helpers::get_backend_url;
use super::json_error;

/// Request body for speech synthesis.
#[derive(Debug, Deserialize)]
pub struct AudioRequest {
    /// Model/engine name (e.g., "kokoro", "tts_kokoro").
    pub model: String,
    /// Text to synthesize.
    pub input: String,
    /// Voice ID to use.
    #[serde(default)]
    pub voice: Option<String>,
    /// Output format: "mp3", "wav", or "ogg". Defaults to "mp3".
    #[serde(default = "default_response_format")]
    pub response_format: String,
    /// Whether to stream the output.
    #[serde(default)]
    pub stream: bool,
    /// Speech speed (0.5 = half speed, 2.0 = double speed). Defaults to 1.0.
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_response_format() -> String {
    "mp3".to_string()
}

fn default_speed() -> f32 {
    1.0
}

/// Resolve a model name to the backend-specific model identifier.
/// "kokoro" and "tts_kokoro" both map to "kokoro" for the backend.
fn resolve_model_name(model: &str) -> &str {
    if model.to_lowercase() == "kokoro" || model.to_lowercase() == "tts_kokoro" {
        "kokoro"
    } else {
        model
    }
}

/// Ensure a TTS backend is loaded and return its server URL.
async fn ensure_tts_server(state: &ProxyState, model_name: &str) -> anyhow::Result<String> {
    // Resolve backend name from model name
    let backend_name = match model_name.to_lowercase().as_str() {
        "kokoro" | "tts_kokoro" => "tts_kokoro",
        other => {
            return Err(anyhow::anyhow!(
                "Unknown TTS engine '{}'. Supported: kokoro, tts_kokoro",
                other
            ))
        }
    };

    // Check if already loaded and get the actual URL from BackendState
    if let Some(url) = get_backend_url(state, backend_name).await? {
        return Ok(url);
    }

    // Not loaded — try to load it
    state.load_tts_backend(backend_name).await?;

    // After loading, get the server URL from models map
    get_backend_url(state, backend_name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("TTS backend '{}' loaded but URL not set", backend_name))
}

/// GET /v1/audio/voices - List available voices.
pub async fn handle_audio_voices(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Try to lazy-load the default TTS backend (Kokoro) if not already loaded,
    // and get its actual URL from BackendState
    let server_url = match ensure_tts_server(&state, "kokoro").await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                return json_error(StatusCode::NOT_FOUND, err_msg, Some("NotFoundError"));
            }
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load TTS backend: {}", e),
                Some("ServerError"),
            );
        }
    };

    let url = format!("{}/v1/audio/voices", server_url);
    match state.client.get(&url).send().await {
        Ok(response) => {
            let body = match response.text().await {
                Ok(text) => text,
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to read backend response: {}", e),
                        Some("ServerError"),
                    );
                }
            };

            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(parsed) => Json(parsed).into_response(),
                Err(e) => json_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Backend returned invalid JSON: {}", e),
                    Some("ServerError"),
                ),
            }
        }
        Err(e) => json_error(
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach TTS backend: {}", e),
            Some("ServerError"),
        ),
    }
}

/// GET /v1/audio/models - List available audio models.
pub async fn handle_audio_models(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Try to lazy-load the default TTS backend (Kokoro) if not already loaded
    let server_url = match ensure_tts_server(&state, "kokoro").await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                return json_error(StatusCode::NOT_FOUND, err_msg, Some("NotFoundError"));
            }
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load TTS backend: {}", e),
                Some("ServerError"),
            );
        }
    };

    // Forward to the backend's /v1/audio/models endpoint
    let url = format!("{}/v1/audio/models", server_url);
    match state.client.get(&url).send().await {
        Ok(response) => {
            let body = match response.text().await {
                Ok(text) => text,
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to read backend response: {}", e),
                        Some("ServerError"),
                    );
                }
            };

            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(parsed) => Json(parsed).into_response(),
                Err(e) => json_error(
                    StatusCode::BAD_GATEWAY,
                    format!("Backend returned invalid JSON: {}", e),
                    Some("ServerError"),
                ),
            }
        }
        Err(e) => json_error(
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach TTS backend: {}", e),
            Some("ServerError"),
        ),
    }
}

/// POST /v1/audio/speech - Synthesize speech (non-streaming).
pub async fn handle_audio_speech(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<AudioRequest>,
) -> Response {
    let server_url = match ensure_tts_server(&state, &req.model).await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                return json_error(StatusCode::NOT_FOUND, err_msg, Some("NotFoundError"));
            }
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load TTS backend: {}", e),
                Some("ServerError"),
            );
        }
    };

    // Build the request body for Kokoro-FastAPI (OpenAI-compatible format)
    let voice = req.voice.unwrap_or_default();
    let model_name = resolve_model_name(&req.model);

    let speech_req = serde_json::json!({
        "model": model_name,
        "input": req.input,
        "voice": voice,
        "response_format": req.response_format.to_lowercase(),
        "speed": req.speed.clamp(0.5, 2.0),
    });

    let url = format!("{}/v1/audio/speech", server_url);
    match state.client.post(&url).json(&speech_req).send().await {
        Ok(response) => {
            let status = response.status();
            let content_type = content_type_for_format(&req.response_format);
            let bytes = match response.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    return json_error(
                        StatusCode::BAD_GATEWAY,
                        format!("Failed to read backend response: {}", e),
                        Some("ServerError"),
                    );
                }
            };
            Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
                })
        }
        Err(e) => json_error(
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach TTS backend: {}", e),
            Some("ServerError"),
        ),
    }
}

/// POST /v1/audio/speech/stream - Synthesize speech (streaming via SSE).
pub async fn handle_audio_stream(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<AudioRequest>,
) -> Response {
    let server_url = match ensure_tts_server(&state, &req.model).await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                return json_error(StatusCode::NOT_FOUND, err_msg, Some("NotFoundError"));
            }
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load TTS backend: {}", e),
                Some("ServerError"),
            );
        }
    };

    let voice = req.voice.unwrap_or_default();
    let model_name = resolve_model_name(&req.model);

    let speech_req = serde_json::json!({
        "model": model_name,
        "input": req.input,
        "voice": voice,
        "response_format": req.response_format.to_lowercase(),
        "speed": req.speed.clamp(0.5, 2.0),
        "stream": true,
    });

    let url = format!("{}/v1/audio/speech", server_url);
    match state.client.post(&url).json(&speech_req).send().await {
        Ok(response) => {
            let status = response.status();
            let content_type = content_type_for_format(&req.response_format);
            // Forward raw binary audio stream as-is (no base64 encoding)
            let body = axum::body::Body::from_stream(response.bytes_stream());
            Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .body(body)
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
                })
        }
        Err(e) => json_error(
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach TTS backend: {}", e),
            Some("ServerError"),
        ),
    }
}

fn content_type_for_format(format: &str) -> &'static str {
    match format.to_lowercase().as_str() {
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        _ => "audio/mpeg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that content_type_for_format returns correct MIME types.
    #[test]
    fn test_content_type_for_format_mp3() {
        assert_eq!(content_type_for_format("mp3"), "audio/mpeg");
    }

    #[test]
    fn test_content_type_for_format_wav() {
        assert_eq!(content_type_for_format("wav"), "audio/wav");
    }

    #[test]
    fn test_content_type_for_format_ogg() {
        assert_eq!(content_type_for_format("ogg"), "audio/ogg");
    }

    /// Test that content_type_for_format handles edge cases.
    #[test]
    fn test_content_type_edge_cases() {
        // Case insensitive
        assert_eq!(content_type_for_format("MP3"), "audio/mpeg");
        assert_eq!(content_type_for_format("WAV"), "audio/wav");
        assert_eq!(content_type_for_format("OGG"), "audio/ogg");
    }

    /// Test that default_response_format returns mp3.
    #[test]
    fn test_default_response_format() {
        assert_eq!(default_response_format(), "mp3");
    }

    /// Test that default_speed returns 1.0.
    #[test]
    fn test_default_speed() {
        assert_eq!(default_speed(), 1.0);
    }
}
