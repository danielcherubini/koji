pub mod chat;
pub mod compaction;
pub mod forward;
pub mod metrics;
pub mod models;
pub mod status;
pub mod tts;

pub(crate) mod helpers;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};

/// Build a structured JSON error response:
/// `{"error": {"message": "...", "type": "..."}}` (type omitted when None).
///
/// This is the canonical error wire shape used across both the management
/// API (`tama::api::error::error_response`) and the proxy handlers. The
/// tama-core crate cannot depend on the `tama` crate, so this helper
/// duplicates the shape intentionally.
pub fn json_error(
    status: StatusCode,
    message: impl Into<String>,
    error_type: Option<&str>,
) -> Response {
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
    let mut body = serde_json::Map::new();
    body.insert("error".to_string(), serde_json::Value::Object(detail));
    (status, Json(serde_json::Value::Object(body))).into_response()
}

/// Backwards-compatible helper for the "request body too large" error.
///
/// Implemented in terms of [`json_error`] so the wire shape stays identical:
/// `{"error":{"message":"Request body too large","type":"BadRequestError"}}`
/// at HTTP 400.
pub fn json_error_response() -> Response {
    json_error(
        StatusCode::BAD_REQUEST,
        "Request body too large",
        Some("BadRequestError"),
    )
}

#[cfg(test)]
mod json_error_tests {
    use super::*;

    /// `json_error_response()` must still yield the exact same wire shape
    /// that chat.rs/compaction.rs depend on (backward compatibility).
    #[tokio::test]
    async fn test_json_error_response_backward_compat() {
        let resp = json_error_response();
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"]["message"], "Request body too large");
        assert_eq!(parsed["error"]["type"], "BadRequestError");
    }

    /// `json_error` with `Some(type)` nests both message and type.
    #[tokio::test]
    async fn test_json_error_with_type() {
        let resp = json_error(
            StatusCode::NOT_FOUND,
            "key not found",
            Some("NotFoundError"),
        );
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"]["message"], "key not found");
        assert_eq!(parsed["error"]["type"], "NotFoundError");
    }

    /// `json_error` with `None` type must omit the `type` key entirely
    /// (not emit `null`).
    #[tokio::test]
    async fn test_json_error_without_type_omits_type_key() {
        let resp = json_error(StatusCode::INTERNAL_SERVER_ERROR, "x", None);
        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"]["message"], "x");
        assert!(
            parsed["error"]["type"].is_null(),
            "type key should be absent (null), got: {}",
            parsed["error"]["type"]
        );
    }

    /// The response status code must match the `status` argument.
    #[tokio::test]
    async fn test_json_error_status_code() {
        let resp = json_error(StatusCode::CONFLICT, "conflict", Some("ConflictError"));
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}

#[cfg(test)]
mod alias_tests;
#[cfg(test)]
mod forward_tests;
#[cfg(test)]
mod get_model_tests;
#[cfg(test)]
mod list_models_tests;
#[cfg(test)]
pub(crate) mod tests;
