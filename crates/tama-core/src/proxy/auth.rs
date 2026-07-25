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
use tracing::{debug, info, warn};

use crate::config::types::OAuth2Config;
use crate::proxy::api_keys::{self, ApiKeyStore, AuthSubject};

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
    let api_keys_enabled = config.proxy.api_keys_enabled;
    drop(config);

    // 1. If no auth configured at all, pass through (open mode)
    let auth_configured =
        auth_url.as_deref().is_some_and(|u| !u.is_empty()) || oauth2_enabled || api_keys_enabled;
    if !auth_configured {
        return next.run(req).await;
    }

    // 2. Check skip_paths (prefix matching: "/health" also matches "/healthcheck")
    //    The OAuth2 login flow routes are ALWAYS skipped — they're the auth entry points.
    let path = req.uri().path().to_string();
    const LOGIN_SKIP_PATHS: &[&str] = &["/login", "/login/callback", "/login/error"];
    if LOGIN_SKIP_PATHS.iter().any(|p| path.starts_with(p))
        || skip_paths.iter().any(|p| path.starts_with(p.as_str()))
    {
        return next.run(req).await;
    }

    // 3. Check session cookie (OIDC login)
    if let Some(claims) = extract_session(&req, &proxy_state) {
        debug!("Authenticated user via session cookie: {}", claims.username);
        let subject = AuthSubject::User {
            username: claims.username,
        };
        let mut req = req;
        req.extensions_mut().insert(subject);
        return next.run(req).await;
    }

    // 4. Check for bearer token
    if let Some(bearer_token) = extract_bearer_token(&req) {
        // 4a. API key bearer token (tama_ prefix, case-sensitive)
        if bearer_token.starts_with("tama_") {
            // If API keys are disabled, reject
            if !api_keys_enabled {
                warn!(
                    remote_addr = ?extract_remote_addr(&req),
                    "API key rejected: api_keys_enabled is false"
                );
                return (StatusCode::UNAUTHORIZED, json_unauthorized_api_keys()).into_response();
            }

            // Validate against database (spawn_blocking for rusqlite)
            let raw_token = bearer_token.clone();
            let raw_token_for_db = raw_token.clone();
            let db_result = tokio::task::spawn_blocking(move || {
                let db = proxy_state.open_db();
                db.map(|conn| ApiKeyStore::new(&conn).validate_key(&raw_token_for_db))
            })
            .await;

            match db_result {
                Ok(Some(Ok(Some((key_id, scopes))))) => {
                    // Successful validation
                    let key_prefix = api_keys::extract_prefix(&raw_token);
                    info!(
                        key_id,
                        key_prefix = %key_prefix,
                        "API key authenticated"
                    );
                    let subject = AuthSubject::Key { key_id, scopes };
                    let mut req = req;
                    req.extensions_mut().insert(subject);
                    return next.run(req).await;
                }
                Ok(Some(Ok(None))) => {
                    // Key not found in database
                    let key_prefix_attempted = api_keys::extract_prefix(&raw_token);
                    warn!(
                        key_prefix_attempted = %key_prefix_attempted,
                        reason = "key not found in database",
                        "API key validation failed"
                    );
                    return (StatusCode::UNAUTHORIZED, json_unauthorized_invalid_key())
                        .into_response();
                }
                Ok(Some(Err(e))) => {
                    warn!(
                        error = %e,
                        reason = "database error during key validation",
                        "API key validation failed"
                    );
                    return (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response();
                }
                Ok(None) => {
                    // No database connection available
                    warn!(
                        reason = "no database connection",
                        "API key validation failed"
                    );
                    return (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response();
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        reason = "spawn_blocking panicked",
                        "API key validation failed"
                    );
                    return (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response();
                }
            }
        } else {
            // 4b. Non-tama_ bearer token — validate against Authentik
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
                    let subject = AuthSubject::User { username };
                    let mut req = req;
                    req.extensions_mut().insert(subject);
                    return next.run(req).await;
                }
                Err(status) => {
                    return (status, json_unauthorized()).into_response();
                }
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
        let subject = AuthSubject::User {
            username: username.to_string(),
        };
        let mut req = req;
        req.extensions_mut().insert(subject);
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

/// JSON 401 response for API key validation failure.
fn json_unauthorized_invalid_key() -> Response {
    let body = serde_json::json!({
        "error": "unauthorized",
        "message": "invalid API key"
    })
    .to_string();
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .expect("build unauthorized response")
}

/// JSON 401 response when API keys are disabled but a tama_ token was provided.
fn json_unauthorized_api_keys() -> Response {
    let body = serde_json::json!({
        "error": "unauthorized",
        "message": "API key authentication is not enabled"
    })
    .to_string();
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .expect("build unauthorized response")
}

/// Extract the remote address from the request for logging.
fn extract_remote_addr(req: &Request) -> Option<String> {
    req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|addr| addr.0.to_string())
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
    // Build a jar from the raw cookie and verify signature.
    // Use parse_encoded to URL-decode the value (it was set via .encoded()),
    // otherwise the HMAC signature won't verify.
    let mut jar = cookie::CookieJar::new();
    jar.add_original(
        cookie::Cookie::parse_encoded(format!("{}={}", SESSION_COOKIE_NAME, cookie_value)).ok()?,
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

/// Minimal HTML-escaper for the login error page.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

// ── OAuth2 handlers ────────────────────────────────────────────────────────

/// Handle GET /login/error — display an OAuth2 login error to the user.
pub async fn handle_login_error(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
    let reason = query.get("reason").map(String::as_str).unwrap_or("unknown");
    let description = query
        .get("description")
        .map(String::as_str)
        .unwrap_or("An unknown error occurred during login.");
    let body = format!(
        "<html><head><title>Login Error</title></head>\n\
         <body style=\"font-family: sans-serif; max-width: 600px; margin: 80px auto; text-align: center;\">\n\
         <h1>Login Error</h1>\n\
         <p><strong>Reason:</strong> {}</p>\n\
         <p>{}</p>\n\
         <p><a href=\"/\">Try again</a></p>\n\
         </body></html>",
        html_escape(reason),
        html_escape(description)
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
}

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
    query: Query<HashMap<String, String>>,
    req: Request,
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
            // Build a jar from the raw cookie and verify HMAC signature.
            // Use parse_encoded to URL-decode the value (it was set via .encoded()),
            // otherwise the HMAC signature won't verify.
            let mut jar = cookie::CookieJar::new();
            jar.add_original(
                cookie::Cookie::parse_encoded(format!(
                    "{}={}",
                    CSRF_STATE_COOKIE_NAME, cookie_value
                ))
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
    use crate::proxy::api_keys::Scope;
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

    /// Helper: ProxyState with OAuth2 enabled and caller-provided endpoint URLs.
    fn make_login_flow_state(
        authorize_url: String,
        token_url: String,
        userinfo_url: Option<String>,
    ) -> Arc<crate::proxy::ProxyState> {
        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                oauth2: crate::config::types::OAuth2Config {
                    enabled: true,
                    client_id: "test-client".to_string(),
                    client_secret: "test-secret".to_string(),
                    authorize_url,
                    token_url,
                    userinfo_url,
                    redirect_uri: "http://localhost:11434/login/callback".to_string(),
                    scopes: vec!["openid".to_string(), "profile".to_string()],
                    session_ttl_secs: 3600,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        Arc::new(crate::proxy::ProxyState::new(config, None))
    }

    /// Helper: minimal router mounting the login-flow handlers directly
    /// (no auth_middleware — these handlers are its skip-path targets).
    fn login_flow_app(state: Arc<crate::proxy::ProxyState>) -> Router {
        Router::new()
            .route("/login", get(handle_login))
            .route("/login/callback", get(handle_login_callback))
            .route("/logout", get(handle_logout))
            .with_state(state)
    }

    /// Helper: GET /login and return (state query param, "tama_oauth2_state=<v>" cookie pair,
    /// full Set-Cookie header).
    async fn start_login(app: &Router) -> (String, String, String) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let url = url::Url::parse(&location).expect("authorize URL must parse");
        let state = url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .expect("authorize URL must contain state param");
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let pair = set_cookie.split(';').next().unwrap().trim().to_string();
        assert!(pair.starts_with(&format!("{}=", CSRF_STATE_COOKIE_NAME)));
        (state, pair, set_cookie)
    }

    /// Test that GET /login redirects to the OAuth2 provider's authorize URL
    /// with all required query parameters.
    #[tokio::test]
    async fn test_handle_login_redirects_to_authorize_url_with_params() {
        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            "https://auth.example.com/token".into(),
            None,
        );
        let app = login_flow_app(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FOUND);

        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        let url = url::Url::parse(location).expect("authorize URL must parse");
        assert!(url
            .as_str()
            .starts_with("https://auth.example.com/authorize?"));

        let params: HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            params.get("response_type").map(|s| s.as_ref()),
            Some("code")
        );
        assert_eq!(
            params.get("client_id").map(|s| s.as_ref()),
            Some("test-client")
        );
        assert_eq!(
            params.get("redirect_uri").map(|s| s.as_ref()),
            Some("http://localhost:11434/login/callback")
        );
        assert_eq!(
            params.get("scope").map(|s| s.as_ref()),
            Some("openid profile")
        );
        assert!(
            params.get("state").is_some_and(|s| !s.is_empty()),
            "state param must be non-empty"
        );
    }

    /// Test that GET /login sets a signed HMAC state cookie with correct attributes.
    #[tokio::test]
    async fn test_handle_login_sets_signed_state_cookie() {
        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            "https://auth.example.com/token".into(),
            None,
        );
        let app = login_flow_app(state.clone());

        let (state_param, pair, set_cookie) = start_login(&app).await;
        assert!(!state_param.is_empty());

        // Verify Set-Cookie header attributes
        assert!(set_cookie.starts_with(&format!("{}=", CSRF_STATE_COOKIE_NAME)));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Path=/login/callback"));
        assert!(set_cookie.contains("Max-Age=300"));

        // Verify the cookie is HMAC-signed by reconstructing a jar and checking signature
        let mut jar = cookie::CookieJar::new();
        jar.add_original(cookie::Cookie::parse_encoded(pair).expect("cookie pair must parse"));
        assert!(
            jar.signed(&state.cookie_key)
                .get(CSRF_STATE_COOKIE_NAME)
                .is_some(),
            "cookie must be HMAC-signed with state.cookie_key"
        );
    }

    /// Test that GET /login returns 503 when OAuth2 is disabled.
    #[tokio::test]
    async fn test_handle_login_disabled_returns_503() {
        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                oauth2: crate::config::types::OAuth2Config {
                    enabled: false,
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
        let state = Arc::new(crate::proxy::ProxyState::new(config, None));
        let app = login_flow_app(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // ── API key authentication tests ────────────────────────────────────

    /// Helper: create a temporary directory with a DB containing an API key.
    /// Returns the temp dir (kept alive by the returned TempDir) and the proxy state.
    fn make_app_with_api_key(
        api_keys_enabled: bool,
    ) -> (
        Router,
        std::sync::Arc<crate::proxy::ProxyState>,
        tempfile::TempDir,
    ) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("tama.db");
        let db_dir = temp_dir.path().to_path_buf();

        // Initialize DB with migrations and seed
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        crate::db::queries::seed_defaults(&conn).unwrap();

        // Create an API key
        let key = api_keys::generate_key();
        let scopes = vec![Scope::Inference];
        ApiKeyStore::new(&conn)
            .create_key("test-key", &key, &scopes, "admin", None)
            .unwrap();

        // Build config with api_keys_enabled
        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_skip_paths: vec![
                    "/health".to_string(),
                    "/metrics".to_string(),
                    "/login".to_string(),
                    "/login/callback".to_string(),
                    "/login/error".to_string(),
                ],
                api_keys_enabled,
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, Some(db_dir)));

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

        // Store the key in a file so the test can read it
        let key_path = temp_dir.path().join("test_key.txt");
        std::fs::write(&key_path, &key).unwrap();

        (app, proxy_state, temp_dir)
    }

    /// Test that a valid tama_ bearer token authenticates successfully.
    #[tokio::test]
    async fn test_tama_key_auth_passes() {
        let (_app, state, temp_dir) = make_app_with_api_key(true);
        let key = std::fs::read_to_string(temp_dir.path().join("test_key.txt")).unwrap();

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ))
            .with_state(state.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", format!("Bearer {}", key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Test that an invalid tama_ bearer token returns 401.
    #[tokio::test]
    async fn test_tama_key_auth_invalid_returns_401() {
        let (_app, state, _temp_dir) = make_app_with_api_key(true);

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ))
            .with_state(state.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(
                        "Authorization",
                        "Bearer tama_invalidtoken1234567890abcdef12345678",
                    )
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
        assert!(body_str.contains("unauthorized"));
    }

    /// Test that tama_ bearer tokens are rejected when api_keys_enabled is false
    /// but another auth method is configured (auth is configured, just not API keys).
    #[tokio::test]
    async fn test_tama_key_disabled_returns_401() {
        // Set up with Authentik configured but api_keys_enabled=false
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("tama.db");
        let db_dir = temp_dir.path().to_path_buf();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        crate::db::queries::seed_defaults(&conn).unwrap();

        let key = api_keys::generate_key();
        ApiKeyStore::new(&conn)
            .create_key("test-key", &key, &[Scope::Inference], "admin", None)
            .unwrap();

        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_url: Some("https://auth.example.com".to_string()),
                authenticator_skip_paths: vec!["/health".to_string(), "/metrics".to_string()],
                api_keys_enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, Some(db_dir)));

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                proxy_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state.clone());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", format!("Bearer {}", key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Test that non-tama_ bearer tokens still go to Authentik.
    #[tokio::test]
    async fn test_non_tama_bearer_still_validates_authentik() {
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

        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_url: Some(format!("http://{}", mock_addr)),
                authenticator_skip_paths: vec!["/health".to_string(), "/metrics".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None));

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                proxy_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state.clone());

        // Non-tama_ token should be validated against Authentik
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", "Bearer some-authentik-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Test that open mode (no auth configured) passes through.
    #[tokio::test]
    async fn test_auth_not_configured_open_mode() {
        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_skip_paths: vec!["/health".to_string(), "/metrics".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None));

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                proxy_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state.clone());

        // No auth header — should pass through (open mode)
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Test that API keys-only auth (no OAuth2, no Authentik) works.
    #[tokio::test]
    async fn test_auth_configured_with_api_keys_only() {
        let (_app, state, temp_dir) = make_app_with_api_key(true);
        let key = std::fs::read_to_string(temp_dir.path().join("test_key.txt")).unwrap();

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ))
            .with_state(state.clone());

        // Valid key should pass
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", format!("Bearer {}", key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // No auth should fail (api_keys_enabled = true means auth is configured)
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Test that session cookie auth still works alongside API keys.
    #[tokio::test]
    async fn test_session_cookie_still_works() {
        let (_app, state, _temp_dir) = make_app_with_api_key(true);

        let claims = SessionClaims::new(
            "cookieuser".to_string(),
            Some("cookie@example.com".to_string()),
            3600,
        );
        let cookie_value = make_signed_session_cookie(&state, &claims);

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ))
            .with_state(state.clone());

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

    /// Test that tama_ prefix is case-sensitive (Tama_, TAMA_ should not match).
    /// A key with uppercase prefix falls through to Authentik validation.
    #[tokio::test]
    async fn test_tama_prefix_case_sensitive() {
        // Start mock Authentik server that returns 401 for any token
        let mock = Router::new().route(
            "/api/v3/core/users/me/",
            axum::routing::get(|| async { StatusCode::UNAUTHORIZED }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = listener.local_addr().unwrap();
        tokio::spawn(async { axum::serve(listener, mock).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("tama.db");
        let db_dir = temp_dir.path().to_path_buf();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        crate::db::queries::seed_defaults(&conn).unwrap();

        let key = api_keys::generate_key();
        ApiKeyStore::new(&conn)
            .create_key("test-key", &key, &[Scope::Inference], "admin", None)
            .unwrap();

        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_url: Some(format!("http://{}", mock_addr)),
                authenticator_skip_paths: vec!["/health".to_string(), "/metrics".to_string()],
                api_keys_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, Some(db_dir)));

        let app = Router::new()
            .route("/", get(test_handler))
            .layer(middleware::from_fn_with_state(
                proxy_state.clone(),
                auth_middleware,
            ))
            .with_state(proxy_state.clone());

        // Uppercase prefix — should NOT match tama_ prefix check,
        // so it falls through to Authentik, which returns 401
        let upper_key = key.to_uppercase();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("Authorization", format!("Bearer {}", upper_key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Should be rejected — uppercase prefix doesn't match tama_,
        // falls through to Authentik which returns 401
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── Login callback CSRF/state validation tests ────────────────────────

    /// Assertion helper: verify a response is a redirect to /login/error
    /// with the expected reason and description fragment.
    fn assert_login_error_redirect(resp: &Response, reason: &str, description_fragment: &str) {
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            location.starts_with(&format!("/login/error?reason={}", reason)),
            "expected /login/error?reason={} redirect, got {}",
            reason,
            location
        );
        assert!(
            location.contains(description_fragment),
            "expected description containing {:?}, got {}",
            description_fragment,
            location
        );
    }

    /// Test that GET /login/callback without a tama_oauth2_state cookie
    /// returns a redirect to /login/error with "CSRF state missing".
    #[tokio::test]
    async fn test_callback_without_state_cookie_errors() {
        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            "https://auth.example.com/token".into(),
            None,
        );
        let app = login_flow_app(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login/callback?code=abc&state=xyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_login_error_redirect(&resp, "state", "CSRF%20state%20missing");
    }

    /// Test that GET /login/callback with a forged (unsigned) tama_oauth2_state cookie
    /// returns a redirect to /login/error — unsigned cookies are indistinguishable
    /// from absent ones.
    #[tokio::test]
    async fn test_callback_with_forged_state_cookie_errors() {
        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            "https://auth.example.com/token".into(),
            None,
        );
        let app = login_flow_app(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login/callback?code=abc&state=xyz")
                    .header("Cookie", "tama_oauth2_state=attacker-controlled")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_login_error_redirect(&resp, "state", "CSRF%20state%20missing");
    }

    /// Test that GET /login/callback without a code query param returns
    /// "Authorization code missing".
    #[tokio::test]
    async fn test_callback_missing_code_errors() {
        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            "https://auth.example.com/token".into(),
            None,
        );
        let app = login_flow_app(state.clone());
        let (_state_query, pair, _set_cookie) = start_login(&app).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login/callback?state=whatever")
                    .header("Cookie", pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_login_error_redirect(&resp, "code", "Authorization%20code%20missing");
    }

    /// Test that GET /login/callback without a state query param returns
    /// "State parameter missing".
    #[tokio::test]
    async fn test_callback_missing_state_param_errors() {
        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            "https://auth.example.com/token".into(),
            None,
        );
        let app = login_flow_app(state.clone());
        let (_state_query, pair, _set_cookie) = start_login(&app).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login/callback?code=abc")
                    .header("Cookie", pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_login_error_redirect(&resp, "state", "State%20parameter%20missing");
    }

    /// Test that GET /login/callback with a state mismatch returns
    /// "CSRF state mismatch". Also asserts the real state differs from
    /// the wrong value so the test can't pass vacuously.
    #[tokio::test]
    async fn test_callback_state_mismatch_errors() {
        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            "https://auth.example.com/token".into(),
            None,
        );
        let app = login_flow_app(state.clone());
        let (real_state, pair, _set_cookie) = start_login(&app).await;

        // Sanity: the real state must not equal our wrong value
        assert_ne!(
            real_state, "wrong-state-value",
            "real_state must differ from wrong-state-value to avoid vacuous pass"
        );

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login/callback?code=abc&state=wrong-state-value")
                    .header("Cookie", pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_login_error_redirect(&resp, "state", "CSRF%20state%20mismatch");
    }

    // ── Login callback token exchange and userinfo tests (wiremock) ───────

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Decode and verify a tama_session Set-Cookie value against the state's signing key.
    fn decode_session_cookie(set_cookie: &str, state: &crate::proxy::ProxyState) -> SessionClaims {
        let pair = set_cookie.split(';').next().unwrap().trim().to_string();
        assert!(pair.starts_with(&format!("{}=", SESSION_COOKIE_NAME)));
        let mut jar = cookie::CookieJar::new();
        jar.add_original(cookie::Cookie::parse_encoded(pair).unwrap());
        let verified = jar
            .signed(&state.cookie_key)
            .get(SESSION_COOKIE_NAME)
            .expect("session cookie must verify against state.cookie_key");
        serde_json::from_str(verified.value()).expect("session claims must be valid JSON")
    }

    /// Run the full login+callback flow against a wiremock provider; returns the callback response.
    async fn run_callback(app: &Router) -> Response {
        let (state, pair, _set_cookie) = start_login(app).await;
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/login/callback?code=test-code&state={}", state))
                    .header(header::COOKIE, pair)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// Test that a successful code exchange + userinfo call issues a signed session cookie
    /// with the correct username and email claims.
    #[tokio::test]
    async fn test_callback_success_issues_signed_session_cookie() {
        let server = MockServer::start().await;

        // Token endpoint — return a valid access token
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-123",
                "token_type": "Bearer",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        // Userinfo endpoint — return user claims
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "preferred_username": "daniel",
                "email": "d@example.com"
            })))
            .mount(&server)
            .await;

        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            format!("{}/token", server.uri()),
            Some(format!("{}/userinfo", server.uri())),
        );
        let app = login_flow_app(state.clone());

        let resp = run_callback(&app).await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "/tama");

        // Verify session cookie attributes
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.starts_with(&format!("{}=", SESSION_COOKIE_NAME)));
        assert!(set_cookie.contains("HttpOnly"));

        // Decode and verify session claims
        let claims = decode_session_cookie(set_cookie, &state);
        assert_eq!(claims.username, "daniel");
        assert_eq!(claims.email, Some("d@example.com".to_string()));
        assert!(claims.is_valid());
    }

    /// Test that a 500 error from the token endpoint redirects to /login/error?reason=token
    /// and no session cookie is issued.
    #[tokio::test]
    async fn test_callback_token_endpoint_500_errors() {
        let server = MockServer::start().await;

        // Token endpoint — return 500 Internal Server Error
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            format!("{}/token", server.uri()),
            Some(format!("{}/userinfo", server.uri())),
        );
        let app = login_flow_app(state.clone());

        let resp = run_callback(&app).await;
        assert_login_error_redirect(&resp, "token", "");

        // No session cookie should be issued
        let set_cookie_headers: Vec<_> = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .into_iter()
            .collect();
        assert!(
            !set_cookie_headers.iter().any(|h| h
                .to_str()
                .is_ok_and(|s| s.starts_with(&format!("{}=", SESSION_COOKIE_NAME)))),
            "no tama_session cookie should be issued on token error"
        );
    }

    /// Test that malformed userinfo JSON degrades gracefully to username "unknown".
    ///
    /// This pins the deliberate fail-soft design of `fetch_userinfo`: when the userinfo
    /// endpoint returns unparseable JSON, the callback still issues a session for
    /// `username = "unknown"` rather than rejecting the login. This ensures that
    /// transient provider issues don't block all users from signing in.
    #[tokio::test]
    async fn test_callback_userinfo_malformed_json_degrades_to_unknown() {
        let server = MockServer::start().await;

        // Token endpoint — OK
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-123",
                "token_type": "Bearer",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        // Userinfo endpoint — return malformed JSON
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json{"))
            .mount(&server)
            .await;

        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            format!("{}/token", server.uri()),
            Some(format!("{}/userinfo", server.uri())),
        );
        let app = login_flow_app(state.clone());

        let resp = run_callback(&app).await;

        // Fail-soft: callback still redirects to /tama (deliberate design)
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "/tama");

        // Session issued with username "unknown"
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        let claims = decode_session_cookie(set_cookie, &state);
        assert_eq!(claims.username, "unknown");
    }

    /// Test that the userinfo claim-alias chain tries `preferred_username` → `nickname`
    /// → `name`, falling back to `name` when neither preferred_username nor nickname
    /// is present.
    #[tokio::test]
    async fn test_callback_userinfo_name_claim_fallback() {
        let server = MockServer::start().await;

        // Token endpoint — OK
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-123",
                "token_type": "Bearer",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        // Userinfo — only `name` (no preferred_username or nickname)
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "fallback-user"
            })))
            .mount(&server)
            .await;

        let state = make_login_flow_state(
            "https://auth.example.com/authorize".into(),
            format!("{}/token", server.uri()),
            Some(format!("{}/userinfo", server.uri())),
        );
        let app = login_flow_app(state.clone());

        let resp = run_callback(&app).await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(location, "/tama");

        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        let claims = decode_session_cookie(set_cookie, &state);
        assert_eq!(claims.username, "fallback-user");
    }
}
