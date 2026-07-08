//! Authentik auth middleware for Axum.
//!
//! Validates bearer tokens against Authentik's user-info API,
//! falling back to Caddy forward_auth headers for browser sessions.

use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl,
    TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::config::types::OAuth2Config;

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
    let oauth2_enabled = config.proxy.oauth2.enabled;
    drop(config);

    // 1. If no auth configured at all, pass through
    let auth_configured = auth_url.as_deref().is_some_and(|u| !u.is_empty()) || oauth2_enabled;
    if !auth_configured {
        return next.run(req).await;
    }

    // 2. Check skip_paths (prefix matching: "/health" also matches "/healthcheck")
    let path = req.uri().path().to_string();
    if skip_paths.iter().any(|p| path.starts_with(p.as_str())) {
        return next.run(req).await;
    }

    // 3. Check session cookie (OIDC login)
    if let Some(claims) = extract_session(&req, &proxy_state) {
        debug!("Authenticated user via session cookie: {}", claims.username);
        return next.run(req).await;
    }

    // 4. Check for bearer token
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

    // 5. Check for X-Authentik-Username header (Caddy forward_auth)
    if let Some(username) = req
        .headers()
        .get("X-Authentik-Username")
        .and_then(|v| v.to_str().ok())
    {
        debug!("Authenticated user via Caddy forward_auth: {}", username);
        return next.run(req).await;
    }

    // 6. No valid auth — for browser requests, redirect to /login
    // For API requests (Accept: application/json), return 401
    let is_browser = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);

    if is_browser && oauth2_enabled {
        (StatusCode::FOUND, [(header::LOCATION, "/login")]).into_response()
    } else {
        (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response()
    }
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

// ── Session claims and OAuth2 helpers ──────────────────────────────────────

/// Cookie name for the session token.
const SESSION_COOKIE_NAME: &str = "tama_session";
/// Cookie name for the OAuth2 CSRF state token.
const CSRF_STATE_COOKIE_NAME: &str = "tama_oauth2_state";

/// Session claims stored in the signed session cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionClaims {
    /// User identifier from the OAuth2 provider.
    sub: String,
    /// Display name for the user.
    username: String,
    /// User email, if available.
    email: Option<String>,
    /// Issued-at timestamp (Unix seconds).
    iat: i64,
    /// Expiration timestamp (Unix seconds).
    exp: i64,
}

impl SessionClaims {
    /// Create new session claims with the given username, email, and TTL.
    fn new(username: String, email: Option<String>, ttl_secs: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            sub: username.clone(),
            username,
            email,
            iat: now,
            exp: now + ttl_secs as i64,
        }
    }

    /// Returns true if the session has not yet expired.
    fn is_valid(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now < self.exp
    }
}

/// Extract and validate session claims from request cookies (signature verified).
fn extract_session(req: &Request, state: &crate::proxy::ProxyState) -> Option<SessionClaims> {
    let raw = req.headers().get(header::COOKIE)?.to_str().ok()?;
    let cookie_value = raw
        .split(';')
        .map(|p| p.trim())
        .find(|p| p.starts_with(&format!("{}=", SESSION_COOKIE_NAME)))
        .and_then(|p| p.split_once('=').map(|x| x.1))?;
    // Build a jar from the raw cookie and verify signature
    let mut jar = cookie::CookieJar::new();
    jar.add_original(
        cookie::Cookie::parse(format!("{}={}", SESSION_COOKIE_NAME, cookie_value)).ok()?,
    );
    let verified = jar.signed(&state.cookie_key).get(SESSION_COOKIE_NAME)?;
    let claims: SessionClaims = serde_json::from_str(verified.value()).ok()?;
    Some(claims).filter(|c| c.is_valid())
}

/// Build a Set-Cookie header value for the session (HMAC-signed).
fn session_cookie(
    claims: &SessionClaims,
    is_secure: bool,
    state: &crate::proxy::ProxyState,
) -> String {
    let value = serde_json::to_string(claims).unwrap_or_default();
    let mut c = cookie::Cookie::build((SESSION_COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(claims.exp - claims.iat));
    if is_secure {
        c = c.secure(true);
    }
    let finished = c.finish();
    // Sign the cookie so the client cannot forge session claims
    let mut jar = cookie::CookieJar::new();
    jar.signed_mut(&state.cookie_key).add(finished);
    jar.get(SESSION_COOKIE_NAME)
        .map(|c| c.encoded().to_string())
        .unwrap_or_default()
}

/// Build an OAuth2 BasicClient from an OAuth2Config directly (avoids holding config lock).
fn build_oauth2_client_from_config(
    oauth2: &OAuth2Config,
) -> Result<
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>,
    anyhow::Error,
> {
    let client = BasicClient::new(ClientId::new(oauth2.client_id.clone()))
        .set_client_secret(ClientSecret::new(oauth2.client_secret.clone()))
        .set_auth_uri(AuthUrl::new(oauth2.authorize_url.clone())?)
        .set_token_uri(TokenUrl::new(oauth2.token_url.clone())?)
        .set_redirect_uri(RedirectUrl::new(oauth2.redirect_uri.clone())?);
    Ok(client)
}

/// Fetch user info from the provider's userinfo endpoint.
///
/// Returns `(username, email)` extracted from the userinfo response.
async fn fetch_userinfo(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
) -> (String, Option<String>) {
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = match r.json().await {
                Ok(v) => v,
                Err(e) => {
                    warn!(url = %url, error = %e, "Failed to parse userinfo JSON response");
                    return ("unknown".to_string(), None);
                }
            };
            let username = body
                .get("preferred_username")
                .or_else(|| body.get("nickname"))
                .or_else(|| body.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let email = body
                .get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (username, email)
        }
        Ok(r) => {
            warn!(
                url = %url,
                status = ?r.status(),
                "Userinfo endpoint returned non-success status"
            );
            ("unknown".to_string(), None)
        }
        Err(e) => {
            warn!(url = %url, error = %e, "Failed to call userinfo endpoint");
            ("unknown".to_string(), None)
        }
    }
}

/// Redirect to /login with an error query parameter.
fn redirect_to_login_error(reason: &str, description: &str) -> Response {
    let url = format!(
        "/login/error?reason={}&description={}",
        percent_encoding::utf8_percent_encode(reason, percent_encoding::NON_ALPHANUMERIC),
        percent_encoding::utf8_percent_encode(description, percent_encoding::NON_ALPHANUMERIC),
    );
    (StatusCode::FOUND, [(header::LOCATION, url)]).into_response()
}

// ── OAuth2 handlers ────────────────────────────────────────────────────────

/// Handle GET /login — redirect to the OAuth2 provider's authorize endpoint.
pub async fn handle_login(State(state): State<Arc<crate::proxy::ProxyState>>) -> impl IntoResponse {
    let config = state.config.read().await;
    if !config.proxy.oauth2.enabled {
        drop(config);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "OAuth2 login is not configured",
        )
            .into_response();
    }
    let oauth2 = config.proxy.oauth2.clone();
    drop(config);

    let oauth2_client = match build_oauth2_client_from_config(&oauth2) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("OAuth2 misconfigured: {}", e),
            )
                .into_response();
        }
    };

    let mut auth_request = oauth2_client.authorize_url(oauth2::CsrfToken::new_random);
    for scope in &oauth2.scopes {
        auth_request = auth_request.add_scope(oauth2::Scope::new(scope.clone()));
    }
    let (url, csrf_state) = auth_request.url();

    // Store CSRF state in a short-lived signed cookie (5 min TTL)
    let state_value = csrf_state.secret().clone();
    let state_cookie_raw = cookie::Cookie::build((CSRF_STATE_COOKIE_NAME, state_value))
        .path("/login/callback")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(300))
        .finish();
    let mut state_jar = cookie::CookieJar::new();
    state_jar
        .signed_mut(&state.cookie_key)
        .add(state_cookie_raw);
    let state_cookie = state_jar
        .get(CSRF_STATE_COOKIE_NAME)
        .map(|c| c.encoded().to_string())
        .unwrap_or_default();

    let location = url.to_string();
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, location),
            (header::SET_COOKIE, state_cookie.clone()),
        ],
    )
        .into_response()
}

/// Handle GET /login/callback — verify CSRF, exchange code, set session.
pub async fn handle_login_callback(
    State(state): State<Arc<crate::proxy::ProxyState>>,
    req: Request,
    query: Query<HashMap<String, String>>,
) -> impl IntoResponse {
    // Extract and verify CSRF state from signed cookie
    let raw_cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok());
    let expected_state = raw_cookie
        .and_then(|raw| {
            raw.split(';')
                .map(|p| p.trim())
                .find(|p| p.starts_with(&format!("{}=", CSRF_STATE_COOKIE_NAME)))
                .and_then(|p| p.split_once('=').map(|x| x.1))
        })
        .and_then(|cookie_value| {
            // Build a jar from the raw cookie and verify HMAC signature
            let mut jar = cookie::CookieJar::new();
            jar.add_original(
                cookie::Cookie::parse(format!("{}={}", CSRF_STATE_COOKIE_NAME, cookie_value))
                    .ok()?,
            );
            jar.signed(&state.cookie_key)
                .get(CSRF_STATE_COOKIE_NAME)
                .map(|c| c.value().to_string())
        });
    let expected_state = match expected_state {
        Some(s) => s,
        None => return redirect_to_login_error("state", "CSRF state missing"),
    };

    // Extract code and state from query params
    let code = match query.get("code") {
        Some(c) => oauth2::AuthorizationCode::new(c.clone()),
        None => return redirect_to_login_error("code", "Authorization code missing"),
    };
    let returned_state = match query.get("state") {
        Some(s) => s.clone(),
        None => return redirect_to_login_error("state", "State parameter missing"),
    };

    // Verify CSRF state matches
    if returned_state != expected_state {
        return redirect_to_login_error("state", "CSRF state mismatch");
    }

    // Clone all needed config upfront — drop lock before any async I/O
    let config = state.config.read().await;
    let oauth2_config = config.proxy.oauth2.clone();
    drop(config);

    // Exchange code for tokens
    let oauth2_client = match build_oauth2_client_from_config(&oauth2_config) {
        Ok(c) => c,
        Err(e) => return redirect_to_login_error("config", &e.to_string()),
    };
    let http_client = state.client.clone();
    let token_response = match oauth2_client
        .exchange_code(code)
        .request_async(&http_client)
        .await
    {
        Ok(t) => t,
        Err(e) => return redirect_to_login_error("token", &format!("{}", e)),
    };

    // Fetch userinfo if configured
    let (username, email) = if let Some(ref userinfo_url) = oauth2_config.userinfo_url {
        fetch_userinfo(
            &state.client,
            userinfo_url,
            token_response.access_token().secret(),
        )
        .await
    } else {
        ("unknown".to_string(), None)
    };

    // Create signed session claims and cookie
    let claims = SessionClaims::new(username, email, oauth2_config.session_ttl_secs);
    let set_cookie = session_cookie(&claims, false, &state);
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, "/tama"),
            (header::SET_COOKIE, &set_cookie),
        ],
    )
        .into_response()
}

/// Handle GET /logout — clear session cookie and redirect.
pub async fn handle_logout(
    State(state): State<Arc<crate::proxy::ProxyState>>,
) -> impl IntoResponse {
    let config = state.config.read().await;
    let logout_url = config.proxy.oauth2.logout_url.clone();
    drop(config);

    // Create expired session cookie to clear it
    let cleared = cookie::Cookie::build((SESSION_COOKIE_NAME, ""))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(0))
        .finish();

    let cleared_cookie = cleared.encoded().to_string();
    if let Some(url) = logout_url {
        (
            StatusCode::FOUND,
            [
                (header::LOCATION, url.as_str()),
                (header::SET_COOKIE, &cleared_cookie),
            ],
        )
            .into_response()
    } else {
        (
            StatusCode::FOUND,
            [
                (header::LOCATION, "/tama"),
                (header::SET_COOKIE, &cleared_cookie),
            ],
        )
            .into_response()
    }
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

    // ── Session cookie and OAuth2 tests ───────────────────────────────────────

    /// Test that SessionClaims serializes and deserializes correctly with expiry.
    #[test]
    fn test_session_claims_serialization() {
        let claims = SessionClaims::new(
            "testuser".to_string(),
            Some("test@example.com".to_string()),
            3600,
        );

        let json = serde_json::to_string(&claims).expect("serialization failed");
        let parsed: SessionClaims = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(parsed.sub, "testuser");
        assert_eq!(parsed.username, "testuser");
        assert_eq!(parsed.email, Some("test@example.com".to_string()));
        assert!(parsed.is_valid());
    }

    /// Test that SessionClaims::new sets valid expiry.
    #[test]
    fn test_session_claims_expiry_valid() {
        let claims = SessionClaims::new("user".to_string(), None, 3600);
        assert!(claims.is_valid());
        // exp should be roughly iat + 3600
        assert!(claims.exp - claims.iat >= 3599 && claims.exp - claims.iat <= 3601);
    }

    /// Test that SessionClaims with a past expiration is invalid.
    #[test]
    fn test_session_claims_expired() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = SessionClaims {
            sub: "user".to_string(),
            username: "user".to_string(),
            email: None,
            iat: now - 7200,
            exp: now - 3600, // expired 1 hour ago
        };
        assert!(!claims.is_valid());
    }

    /// Create a signed session cookie header value for testing.
    fn make_signed_session_cookie(
        state: &crate::proxy::ProxyState,
        claims: &SessionClaims,
    ) -> String {
        let value = serde_json::to_string(claims).unwrap();
        let c = cookie::Cookie::build((SESSION_COOKIE_NAME, value))
            .path("/")
            .http_only(true)
            .same_site(cookie::SameSite::Lax)
            .finish();
        let mut jar = cookie::CookieJar::new();
        jar.signed_mut(&state.cookie_key).add(c);
        // Return just "name=signed_value" (as it appears in the Cookie request header)
        jar.get(SESSION_COOKIE_NAME)
            .map(|c| format!("{}={}", c.name(), c.value()))
            .unwrap_or_default()
    }

    /// Test that a request with a valid signed session cookie passes auth middleware.
    #[tokio::test]
    async fn test_session_cookie_auth_passes() {
        let claims = SessionClaims::new(
            "cookieuser".to_string(),
            Some("cookie@example.com".to_string()),
            3600,
        );

        let (app, state) = make_app_oauth2(Some("https://auth.example.com".to_string()));
        let cookie_value = make_signed_session_cookie(&state, &claims);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Cookie", cookie_value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Test that an expired signed session cookie is rejected (401).
    #[tokio::test]
    async fn test_session_cookie_expired_rejected() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let claims = SessionClaims {
            sub: "olduser".to_string(),
            username: "olduser".to_string(),
            email: None,
            iat: now - 7200,
            exp: now - 3600,
        };

        let (app, state) = make_app_oauth2(Some("https://auth.example.com".to_string()));
        let cookie_value = make_signed_session_cookie(&state, &claims);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Cookie", cookie_value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Test that a forged (unsigned) session cookie is rejected (401).
    #[tokio::test]
    async fn test_session_cookie_unsigned_rejected() {
        let claims = SessionClaims::new("attacker".to_string(), None, 3600);
        let forged_value = serde_json::to_string(&claims).unwrap();

        let (app, _state) = make_app_oauth2(Some("https://auth.example.com".to_string()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(
                        "Cookie",
                        format!("{}={}", SESSION_COOKIE_NAME, forged_value),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unsigned cookie must be rejected"
        );
    }

    /// Test that a missing session cookie returns 401 when auth is configured.
    #[tokio::test]
    async fn test_session_cookie_missing_rejected() {
        let (app, _state) = make_app_oauth2(Some("https://auth.example.com".to_string()));

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Test that browser requests (Accept: text/html) redirect to /login
    /// when OAuth2 is enabled and no auth is present.
    #[tokio::test]
    async fn test_browser_401_redirects_to_login_when_oauth2_enabled() {
        let (app, _state) = make_app_oauth2(Some("https://auth.example.com".to_string()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Accept", "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp.headers().get("Location").unwrap();
        assert_eq!(location.to_str().unwrap(), "/login");
    }

    /// Test that API requests (Accept: application/json) return JSON 401
    /// even when OAuth2 is enabled.
    #[tokio::test]
    async fn test_api_401_returns_json_when_oauth2_enabled() {
        let (app, _state) = make_app_oauth2(Some("https://auth.example.com".to_string()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("Accept", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains("Authentication required"));
    }

    /// Test that login callback routes are in skip paths.
    #[tokio::test]
    async fn test_login_routes_in_skip_paths() {
        for path in &["/login", "/login/callback", "/login/error"] {
            let (app, _state) = make_app_oauth2(Some("https://auth.example.com".to_string()));
            let resp = app
                .oneshot(Request::builder().uri(*path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            // These should NOT return 401 — they are in skip paths
            assert_ne!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{} should be in skip paths",
                path
            );
        }
    }

    /// Helper: build an app with OAuth2 enabled (auth via OAuth2 config).
    fn make_app_oauth2(auth_url: Option<String>) -> (Router, Arc<crate::proxy::ProxyState>) {
        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_url: auth_url,
                authenticator_skip_paths: vec![
                    "/health".to_string(),
                    "/metrics".to_string(),
                    "/login".to_string(),
                    "/login/callback".to_string(),
                    "/login/error".to_string(),
                ],
                oauth2: crate::config::types::OAuth2Config {
                    enabled: true,
                    client_id: "test-client".to_string(),
                    client_secret: "test-secret".to_string(),
                    authorize_url: "https://auth.example.com/authorize".to_string(),
                    token_url: "https://auth.example.com/token".to_string(),
                    redirect_uri: "http://localhost:11434/login/callback".to_string(),
                    scopes: vec!["openid".to_string(), "profile".to_string()],
                    session_ttl_secs: 3600,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None));

        let app = Router::new()
            .route("/", get(test_handler))
            .route("/health", get(test_handler))
            .route("/metrics", get(test_handler))
            .route("/v1/models", get(test_handler))
            .layer(middleware::from_fn_with_state(
                proxy_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state.clone());
        (app, proxy_state)
    }
}
