//! Tests for auth middleware, session handling, OAuth2 login flow, and API key auth.

use super::middleware::auth_middleware;
use super::oauth2::CSRF_STATE_COOKIE_NAME;
use super::oauth2::{build_oauth2_client_from_config, fetch_userinfo};
use super::oauth2::{handle_login, handle_login_callback, handle_logout};
use super::session::{SessionClaims, SESSION_COOKIE_NAME};
use crate::proxy::api_keys::{self, Scope};
use axum::body::Body;
use axum::middleware;
use axum::{
    extract::Request,
    http::{header, StatusCode},
    response::Response,
};
use axum::{routing::get, Router};
use std::collections::HashMap;
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
    let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None, None));

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
async fn test_no_auth_url_passes_through() {
    let app = make_app(None, vec![]);
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_skip_path_passes_through() {
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
async fn test_no_auth_returns_401() {
    let app = make_app(Some("https://auth.wizards.town".to_string()), vec![]);
    let resp = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_caddy_forward_auth_header_passes() {
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
async fn test_valid_bearer_token_passes() {
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
async fn test_invalid_bearer_token_returns_401() {
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
async fn test_authentik_unreachable_fails_open() {
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
fn make_signed_session_cookie(state: &crate::proxy::ProxyState, claims: &SessionClaims) -> String {
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
    let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None, None));

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
    Arc::new(crate::proxy::ProxyState::new(config, None, None))
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
    let state = Arc::new(crate::proxy::ProxyState::new(config, None, None));
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

/// Helper: build an app with a seeded Postgres API key (plan-190 Task 6).
/// Returns the router, the proxy state, the schema guard (kept alive),
/// and the raw key.
async fn make_app_with_api_key(
    api_keys_enabled: bool,
) -> (
    Router,
    std::sync::Arc<crate::proxy::ProxyState>,
    crate::testing::postgres::SchemaGuard,
    String,
) {
    let guard = crate::testing::postgres::with_schema().await;

    // Create an API key
    let key = api_keys::generate_key();
    let scopes = vec![Scope::Inference];
    let store = crate::proxy::api_keys::ApiKeyStore::new(Arc::new(guard.pool.clone()));
    store
        .create_key("test-key", &key, &scopes, "admin", None)
        .await
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
    let proxy_state = Arc::new(crate::proxy::ProxyState::new(
        config,
        None,
        Some(Arc::new(guard.pool.clone())),
    ));

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

    (app, proxy_state, guard, key)
}

/// Test that a valid tama_ bearer token authenticates successfully.
#[tokio::test]
async fn test_tama_key_auth_passes() {
    let (_app, state, _guard, key) = make_app_with_api_key(true).await;

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
    let (_app, state, _guard, _key) = make_app_with_api_key(true).await;

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
    let guard = crate::testing::postgres::with_schema().await;

    let key = api_keys::generate_key();
    let store = crate::proxy::api_keys::ApiKeyStore::new(Arc::new(guard.pool.clone()));
    store
        .create_key("test-key", &key, &[Scope::Inference], "admin", None)
        .await
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
    let proxy_state = Arc::new(crate::proxy::ProxyState::new(
        config,
        None,
        Some(Arc::new(guard.pool.clone())),
    ));

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
    let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None, None));

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
    let proxy_state = Arc::new(crate::proxy::ProxyState::new(config, None, None));

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
    let (_app, state, _guard, key) = make_app_with_api_key(true).await;

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
    let (_app, state, _guard, _key) = make_app_with_api_key(true).await;

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

    let guard = crate::testing::postgres::with_schema().await;

    let key = api_keys::generate_key();
    let store = crate::proxy::api_keys::ApiKeyStore::new(Arc::new(guard.pool.clone()));
    store
        .create_key("test-key", &key, &[Scope::Inference], "admin", None)
        .await
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
    let proxy_state = Arc::new(crate::proxy::ProxyState::new(
        config,
        None,
        Some(Arc::new(guard.pool.clone())),
    ));

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
    let encoded_state =
        percent_encoding::utf8_percent_encode(&state, percent_encoding::NON_ALPHANUMERIC);
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/login/callback?code=test-code&state={}",
                    encoded_state
                ))
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
    assert_login_error_redirect(&resp, "token", "returned%20empty");

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

// ── Logout handler tests ────────────────────────────────────────────────

/// Test that GET /logout clears the session cookie (Max-Age=0) and
/// redirects to `/tama` when no logout_url is configured.
#[tokio::test]
async fn test_logout_clears_session_cookie_and_redirects_to_tama() {
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
                .uri("/logout")
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
    assert_eq!(location, "/tama");
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.starts_with(&format!("{}=", SESSION_COOKIE_NAME)));
    assert!(set_cookie.contains("Max-Age=0"));
}

/// Test that GET /logout redirects to the configured logout_url when set.
#[tokio::test]
async fn test_logout_redirects_to_configured_logout_url() {
    let config = crate::config::Config {
        proxy: crate::config::ProxyConfig {
            oauth2: crate::config::types::OAuth2Config {
                enabled: true,
                client_id: "test-client".to_string(),
                client_secret: "test-secret".to_string(),
                authorize_url: "https://auth.example.com/authorize".to_string(),
                token_url: "https://auth.example.com/token".to_string(),
                userinfo_url: None,
                redirect_uri: "http://localhost:11434/login/callback".to_string(),
                scopes: vec!["openid".to_string(), "profile".to_string()],
                session_ttl_secs: 3600,
                logout_url: Some("https://auth.example.com/logout".to_string()),
            },
            ..Default::default()
        },
        ..Default::default()
    };
    let state = Arc::new(crate::proxy::ProxyState::new(config, None, None));
    let app = login_flow_app(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/logout")
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
    assert_eq!(location, "https://auth.example.com/logout");
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.starts_with(&format!("{}=", SESSION_COOKIE_NAME)));
    assert!(set_cookie.contains("Max-Age=0"));
}

// ── OAuth2 helper function tests ────────────────────────────────────────

/// Test that `build_oauth2_client_from_config` rejects invalid URL strings.
#[test]
fn test_build_oauth2_client_rejects_invalid_urls() {
    let config = crate::config::types::OAuth2Config {
        authorize_url: "not a url".to_string(),
        token_url: "http://127.0.0.1/token".to_string(),
        redirect_uri: "http://127.0.0.1/cb".to_string(),
        ..Default::default()
    };
    let result = build_oauth2_client_from_config(&config);
    assert!(result.is_err());
}

/// Test that `fetch_userinfo` returns `("unknown", None)` when the
/// userinfo endpoint is unreachable (connection refused).
#[tokio::test]
async fn test_fetch_userinfo_unreachable_returns_unknown() {
    // Bind a TCP listener to get a free port, then drop it so the
    // subsequent connection is refused immediately (no timeout).
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = tcp_listener.local_addr().unwrap().port();
    drop(tcp_listener);

    let result = fetch_userinfo(
        &reqwest::Client::new(),
        &format!("http://127.0.0.1:{}/userinfo", port),
        "token",
    )
    .await;
    assert_eq!(result, ("unknown".to_string(), None));
}
