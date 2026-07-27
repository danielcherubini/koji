//! API key authentication helpers.

use axum::{extract::Request, response::Response};

/// JSON 401 response for API key validation failure.
pub(super) fn json_unauthorized_invalid_key() -> Response {
    let body = serde_json::json!({
        "error": "unauthorized",
        "message": "invalid API key"
    })
    .to_string();
    Response::builder()
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .expect("build unauthorized response")
}

/// JSON 401 response when API keys are disabled but a tama_ token was provided.
pub(super) fn json_unauthorized_api_keys() -> Response {
    let body = serde_json::json!({
        "error": "unauthorized",
        "message": "API key authentication is not enabled"
    })
    .to_string();
    Response::builder()
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .expect("build unauthorized response")
}

/// Extract the remote address from the request for logging.
pub(super) fn extract_remote_addr(req: &Request) -> Option<String> {
    req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|addr| addr.0.to_string())
}
