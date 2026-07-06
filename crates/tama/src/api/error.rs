use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Structured error response body: `{"error": {"message": "...", "type": "..."}}`
#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Serialize)]
pub struct ErrorDetail {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

/// Create a structured error response.
///
/// # Usage
/// ```ignore
/// error_response(StatusCode::NOT_FOUND, "Model not found", Some("NotFoundError"))
/// error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
/// ```
pub fn error_response(
    status: StatusCode,
    message: impl Into<String>,
    error_type: Option<&str>,
) -> Response {
    let body = ErrorResponse {
        error: ErrorDetail {
            message: message.into(),
            r#type: error_type.map(|s| s.to_string()),
        },
    };
    (status, Json(body)).into_response()
}

/// Simple error response without type field (for generic errors).
pub fn error_response_simple(status: StatusCode, message: impl Into<String>) -> Response {
    error_response(status, message, None)
}

/// Create a structured error body as `serde_json::Value`.
///
/// Used in closures that return `(StatusCode, serde_json::Value)` tuples
/// (e.g. in `spawn_blocking` closures with `map_err`).
pub fn error_body(message: impl Into<String>, error_type: Option<&str>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    let mut detail = serde_json::Map::new();
    detail.insert(
        "message".to_string(),
        serde_json::Value::String(message.into()),
    );
    if let Some(ty) = error_type {
        detail.insert(
            "type".to_string(),
            serde_json::Value::String(ty.to_string()),
        );
    }
    map.insert("error".to_string(), serde_json::Value::Object(detail));
    serde_json::Value::Object(map)
}
