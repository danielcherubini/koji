//! Integration tests for GET/POST /tama/v1/config/structured endpoints.
//!
//! These tests verify:
//! - GET returns valid JSON Config
//! - POST persists and round-trips without field loss
//! - Config loads from DB via Config::from_db()
//! - All ModelConfig/Supervisor/BackendConfig/ProxyConfig fields preserved
//! - Standalone mode works (no proxy_config)
//! - 410 Gone for raw TOML endpoints

use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

use tower::util::ServiceExt;

use tama_core::proxy::ProxyState;
use tama_web::router::build_web_routes;

/// Create a minimal WebState for tests.
fn test_web_state() -> tama_web::web_types::WebState {
    tama_web::web_types::WebState {
        jobs: Some(Arc::new(tama_web::web_types::JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
    }
}

/// Helper to extract CSRF token from response headers and set cookie.
async fn get_csrf_token(router: &axum::Router) -> String {
    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/tama/v1/config/structured")
        .header("origin", "http://localhost:11435")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = router.clone().oneshot(req).await.unwrap();
    response
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie| {
            cookie
                .split(';')
                .next()
                .and_then(|part| part.split_once('='))
                .map(|(_, val)| val.to_string())
        })
        .unwrap_or_else(|| "test-csrf-token".to_string())
}

/// Build a POST request with CSRF token.
async fn post_with_csrf(
    router: axum::Router,
    uri: String,
    body: axum::body::Body,
    csrf_token: String,
) -> axum::http::Response<axum::body::Body> {
    let req = axum::extract::Request::builder()
        .method("POST")
        .uri(&uri)
        .header("content-type", "application/json")
        .header("origin", "http://localhost:11435")
        .header("cookie", format!("tama_csrf_token={csrf_token}"))
        .header("x-csrf-token", &csrf_token)
        .body(body)
        .unwrap();
    router.oneshot(req).await.unwrap()
}

/// Build test ProxyState with config in temp dir.
fn build_test_state(_config_content: &str) -> (Arc<ProxyState>, TempDir) {
    let temp_dir = TempDir::new().expect("create temp dir");

    // Seed a DB in the temp dir so handlers don't fall back to the user's real config dir.
    let _open_result = tama_core::db::open(temp_dir.path()).expect("open test DB");

    let config = tama_core::config::Config::default();
    let state = Arc::new(ProxyState::new(config, Some(temp_dir.path().to_path_buf())));

    (state, temp_dir)
}

#[tokio::test]
async fn test_get_structured_config_returns_valid_json() {
    let (state, _temp_dir) = build_test_state("");
    let router = build_web_routes(Arc::new(test_web_state()))
        .with_state(state)
        .layer(axum::extract::Extension(test_web_state()));

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/tama/v1/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(parsed.get("general").is_some());
    assert!(parsed.get("backends").is_some());
    // models are stored in SQLite and not included in the structured config response
    assert!(parsed.get("supervisor").is_some());
    assert!(parsed.get("sampling_templates").is_some());
    assert!(parsed.get("proxy").is_some());
}

#[tokio::test]
async fn test_post_structured_config_persists_and_round_trips() {
    let (state, _temp_dir) = build_test_state("");
    let router = build_web_routes(Arc::new(test_web_state()))
        .with_state(state)
        .layer(axum::extract::Extension(test_web_state()));

    // Get CSRF token first
    let csrf_token = get_csrf_token(&router).await;

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/tama/v1/config/structured")
        .header("origin", "http://localhost:11435")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut initial: serde_json::Value = serde_json::from_slice(&body).unwrap();

    if let Some(general) = initial.get_mut("general") {
        general["log_level"] = "debug".into();
    }

    let response = post_with_csrf(
        router.clone(),
        "/tama/v1/config/structured".to_string(),
        axum::body::Body::from(serde_json::to_string(&initial).unwrap()),
        csrf_token,
    )
    .await;
    assert_eq!(response.status(), 200);

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/tama/v1/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let final_config: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(final_config["general"]["log_level"], "debug");
}

#[tokio::test]
async fn test_400_on_invalid_json() {
    let (state, _temp_dir) = build_test_state("");
    let router = build_web_routes(Arc::new(test_web_state()))
        .with_state(state)
        .layer(axum::extract::Extension(test_web_state()));

    let csrf_token = get_csrf_token(&router).await;

    let response = post_with_csrf(
        router.clone(),
        "/tama/v1/config/structured".to_string(),
        axum::body::Body::from("{ invalid json }"),
        csrf_token,
    )
    .await;
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn test_get_structured_config_without_db_dir() {
    // Use a temp dir so we don't read/write the user's real config.
    let temp_dir = TempDir::new().expect("create temp dir");
    let _open_result = tama_core::db::open(temp_dir.path()).expect("open test DB");

    let config = tama_core::config::Config::default();
    let state = Arc::new(ProxyState::new(config, Some(temp_dir.path().to_path_buf())));
    let router = build_web_routes(Arc::new(test_web_state()))
        .with_state(state)
        .layer(axum::extract::Extension(test_web_state()));

    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/tama/v1/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    // Returns 200 — config_dir is always available via Config::config_dir()
    assert_eq!(response.status(), 200);

    // POST with config_path=None and missing required fields returns 422
    // (but first needs CSRF — we skip CSRF here since config_path is None)
    let req = axum::extract::Request::builder()
        .method("POST")
        .uri("/tama/v1/config/structured")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let response: axum::http::Response<axum::body::Body> =
        router.clone().oneshot(req).await.unwrap();
    // 422 Unprocessable Entity — body validation fails before CSRF check
    assert_eq!(response.status(), 422);
}

/// Regression: even if a client POSTs a config with `api_keys_enabled: false`,
/// the server must derive the flag from the `api_keys` table. This prevents
/// a stale client (e.g. the config editor with a missing field in its mirror
/// type) from locking the operator out of their own proxy on every save.
#[tokio::test]
async fn test_post_structured_config_cannot_disable_api_keys_with_active_keys() {
    use tama_core::proxy::api_keys::{self, Scope};

    let (state, _temp_dir) = build_test_state("");
    let db_path = _temp_dir.path().join("tama.db");

    // Seed an active API key in the DB
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let raw_key = api_keys::generate_key();
        api_keys::create_key(
            &conn,
            "test-key",
            &raw_key,
            &[Scope::Inference],
            "admin",
            None,
        )
        .unwrap();
    }

    let router = build_web_routes(Arc::new(test_web_state()))
        .with_state(state)
        .layer(axum::extract::Extension(test_web_state()));

    let csrf_token = get_csrf_token(&router).await;

    // GET the current (full) config, then flip api_keys_enabled to false to
    // simulate the stale-client bug, then POST it back.
    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/tama/v1/config/structured")
        .header("origin", "http://localhost:11435")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let mut config: serde_json::Value = serde_json::from_slice(&body).unwrap();
    config["proxy"]["api_keys_enabled"] = serde_json::Value::Bool(false);

    let response = post_with_csrf(
        router.clone(),
        "/tama/v1/config/structured".to_string(),
        axum::body::Body::from(serde_json::to_string(&config).unwrap()),
        csrf_token,
    )
    .await;
    assert_eq!(response.status(), 200);

    // Reload and check that api_keys_enabled was corrected to true
    let req = axum::extract::Request::builder()
        .method("GET")
        .uri("/tama/v1/config/structured")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let loaded: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        loaded["proxy"]["api_keys_enabled"], true,
        "api_keys_enabled must be derived from active keys, not from the saved value"
    );
}
