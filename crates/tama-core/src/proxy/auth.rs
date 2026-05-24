//! Authentik auth middleware for Axum.
//!
//! Validates bearer tokens against Authentik's user-info API,
//! falling back to Caddy forward_auth headers for browser sessions.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, warn};

// --- Auth config ---
// AuthConfig is no longer used; auth settings are read live from ProxyState.
// The type is retained for backward compatibility but is empty.
#[derive(Clone, Debug)]
pub struct AuthConfig;

// --- Authentik user-info response ---

#[derive(Debug, Deserialize)]
struct AuthentikUserResponse {
    user: AuthentikUser,
}

#[derive(Debug, Deserialize)]
struct AuthentikUser {
    username: String,
}

// --- Middleware function ---

pub async fn auth_middleware(
    State(proxy_state): State<Arc<crate::proxy::ProxyState>>,
    req: Request,
    next: Next,
) -> Response {
    // Read auth settings from shared state (live, not snapshot)
    let config = proxy_state.config.read().await;
    let auth_url = config.proxy.authenticator_url.clone();
    let skip_paths = config.proxy.authenticator_skip_paths.clone();
    drop(config);

    // 1. If no auth URL configured, pass through
    if auth_url.as_deref().is_none_or(|u| u.is_empty()) {
        return next.run(req).await;
    }

    // 2. Check skip_paths (prefix matching: "/health" also matches "/healthcheck")
    let path = req.uri().path().to_string();
    if skip_paths.iter().any(|p| path.starts_with(p.as_str())) {
        return next.run(req).await;
    }

    // 3. Check for bearer token
    if let Some(bearer_token) = extract_bearer_token(&req) {
        let authenticator_url = auth_url.as_deref().unwrap_or("");
        match validate_token_against_authentik(
            &proxy_state.client,
            authenticator_url,
            &bearer_token,
        )
        .await
        {
            Ok(username) => {
                debug!("Authenticated user via bearer token: {}", username);
                return next.run(req).await;
            }
            Err(status) => {
                return (status, json_unauthorized()).into_response();
            }
        }
    }

    // 4. Check for X-Authentik-Username header (Caddy forward_auth)
    if let Some(username) = req
        .headers()
        .get("X-Authentik-Username")
        .and_then(|v| v.to_str().ok())
    {
        debug!("Authenticated user via Caddy forward_auth: {}", username);
        return next.run(req).await;
    }

    // 5. No valid auth
    (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response()
}

// --- Helper functions ---

fn extract_bearer_token(req: &Request) -> Option<String> {
    let header = req.headers().get(header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    value.strip_prefix("Bearer ").map(|s| s.to_string())
}

async fn validate_token_against_authentik(
    client: &reqwest::Client,
    authenticator_url: &str,
    token: &str,
) -> Result<String, StatusCode> {
    let url = format!("{}/api/v3/core/users/me/", authenticator_url);
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: AuthentikUserResponse = r.json().await.map_err(|e| {
                warn!("Failed to parse Authentik user response: {}", e);
                StatusCode::UNAUTHORIZED
            })?;
            Ok(body.user.username)
        }
        Ok(r) if r.status() == StatusCode::UNAUTHORIZED => Err(StatusCode::UNAUTHORIZED),
        // Any non-success non-401 (403, 500, 429) is treated as unauthorized.
        // This is intentional: if Authentik signals the token is bad, deny access.
        Ok(_) => Err(StatusCode::UNAUTHORIZED),
        Err(e) => {
            // Fail-open: if Authentik is unreachable (connection timeout, DNS failure),
            // allow the request through with a warning log.
            warn!("Authentik auth API unreachable ({}), allowing request", e);
            Ok("unknown".to_string())
        }
    }
}

fn json_unauthorized() -> Response {
    let body = serde_json::json!({
        "error": "Authentication required",
        "detail": "Provide a valid Authorization: Bearer <token> header"
    })
    .to_string();
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .expect("build unauthorized response")
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use axum::middleware;
    use axum::{routing::get, Router};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::util::ServiceExt;

    async fn test_handler() -> &'static str {
        "ok"
    }

    fn make_app(auth_url: Option<String>, skip_paths: Vec<String>) -> Router {
        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_url: auth_url,
                authenticator_skip_paths: skip_paths,
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None));

        Router::new()
            .route("/", get(test_handler))
            .route("/health", get(test_handler))
            .layer(middleware::from_fn_with_state(
                proxy_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state)
    }

    #[tokio::test]
    async fn no_auth_url_passes_through() {
        let app = make_app(None, vec![]);
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn skip_path_passes_through() {
        let app = make_app(
            Some("https://auth.wizards.town".to_string()),
            vec!["/health".to_string()],
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_auth_returns_401() {
        let app = make_app(Some("https://auth.wizards.town".to_string()), vec![]);
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn caddy_forward_auth_header_passes() {
        let app = make_app(Some("https://auth.wizards.town".to_string()), vec![]);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("X-Authentik-Username", "daniel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn valid_bearer_token_passes() {
        // Start mock Authentik server that returns a valid user response
        let mock = Router::new().route(
            "/api/v3/core/users/me/",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "user": {"username": "daniel", "pk": 7, "is_active": true}
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = listener.local_addr().unwrap();
        tokio::spawn(async { axum::serve(listener, mock).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = make_app(Some(format!("http://{}", mock_addr)), vec![]);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", "Bearer valid-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_bearer_token_returns_401() {
        // Mock server that returns 401 for any token
        let mock = Router::new().route(
            "/api/v3/core/users/me/",
            axum::routing::get(|| async { StatusCode::UNAUTHORIZED }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = listener.local_addr().unwrap();
        tokio::spawn(async { axum::serve(listener, mock).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let app = make_app(Some(format!("http://{}", mock_addr)), vec![]);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", "Bearer bad-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Test fail-open behavior: when Authentik is unreachable, the request
    /// passes through with a warning log. This is a critical security trade-off.
    #[tokio::test]
    async fn authentik_unreachable_fails_open() {
        // Bind a UDP socket to get a free port. Nothing TCP will be listening
        // on this port, so the connection is refused immediately (no timeout).
        let udp_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = udp_socket.local_addr().unwrap().port();
        drop(udp_socket);

        let app = make_app(Some(format!("http://127.0.0.1:{}", port)), vec![]);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", "Bearer some-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Fail-open: request should pass through even though Authentik is unreachable
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "fail-open should allow request when Authentik is unreachable"
        );
    }
}
