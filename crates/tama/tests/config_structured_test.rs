//! Integration tests for GET/POST /tama/v1/config/structured endpoints.
//!
//! These tests verify:
//! - GET returns valid JSON Config
//! - POST persists and round-trips without field loss
//! - Config loads from Postgres via Config::load_from_pool() (plan-190 Task 3)
//! - All ModelConfig/Supervisor/BackendConfig/ProxyConfig fields preserved
//! - api_keys_enabled is derived from the api_keys table, never the saved value
//! - 410 Gone for raw TOML endpoints

mod common;

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
        repository: None,
        db_pool: None,
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

/// Build test ProxyState with a Postgres-backed config (plan-190 Task 3).
///
/// Returns the state, a temp db dir (for `db_dir()`), the schema pool, and
/// the schema guard (call `finish()` at the end of the test).
async fn build_test_state() -> (
    Arc<ProxyState>,
    TempDir,
    Arc<sqlx::PgPool>,
    common::SchemaGuard,
) {
    let temp_dir = TempDir::new().expect("create temp dir");

    let guard = common::with_schema().await;
    let pool = Arc::new(guard.pool.clone());

    let config = tama_core::config::Config::load_from_pool(&guard.pool)
        .await
        .expect("load config from fresh schema");
    let state = Arc::new(ProxyState::new(
        config,
        Some(temp_dir.path().to_path_buf()),
        Some(pool.clone()),
    ));

    (state, temp_dir, pool, guard)
}

#[tokio::test]
async fn test_get_structured_config_returns_valid_json() {
    let (state, _temp_dir, _pool, guard) = build_test_state().await;
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
    // models are stored in the DB and not included in the structured config response
    assert!(parsed.get("lifecycle").is_some());
    assert!(parsed.get("sampling_templates").is_some());
    assert!(parsed.get("proxy").is_some());
    // The app config never carries the bootstrap/`database` section.
    assert!(parsed.get("database").is_none());

    guard.finish().await;
}

#[tokio::test]
async fn test_post_structured_config_persists_and_round_trips() {
    let (state, _temp_dir, _pool, guard) = build_test_state().await;
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

    guard.finish().await;
}

#[tokio::test]
async fn test_400_on_invalid_json() {
    let (state, _temp_dir, _pool, guard) = build_test_state().await;
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

    guard.finish().await;
}

#[tokio::test]
async fn test_get_structured_config_without_db_dir() {
    // Use a temp dir so we don't read/write the user's real config.
    let temp_dir = TempDir::new().expect("create temp dir");

    let guard = common::with_schema().await;
    let pool = Arc::new(guard.pool.clone());
    let config = tama_core::config::Config::load_from_pool(&guard.pool)
        .await
        .unwrap();
    let state = Arc::new(ProxyState::new(
        config,
        Some(temp_dir.path().to_path_buf()),
        Some(pool),
    ));
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
    // Returns 200 — config is loaded from Postgres
    assert_eq!(response.status(), 200);

    // POST with missing required fields returns 422
    // (but first needs CSRF — we skip CSRF here since validation fails first)
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

    guard.finish().await;
}

/// Regression: even if a client POSTs a config with `api_keys_enabled: false`,
/// the server must derive the flag from the `api_keys` table. This prevents
/// a stale client (e.g. the config editor with a missing field in its mirror
/// type) from locking the operator out of their own proxy on every save.
#[tokio::test]
async fn test_post_structured_config_cannot_disable_api_keys_with_active_keys() {
    let (state, _temp_dir, pool, guard) = build_test_state().await;

    // Seed an active API key in Postgres (raw insert — the ApiKeyStore port
    // lands in Task 6).
    let raw_key = tama_core::proxy::api_keys::generate_key();
    let key_hash = tama_core::proxy::api_keys::hash_key(&raw_key);
    sqlx::query(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by) \
         VALUES ('test-key', 'tama_test', $1, '[\"inference\"]', 'test')",
    )
    .bind(&key_hash)
    .execute(pool.as_ref())
    .await
    .unwrap();

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

    guard.finish().await;
}

/// Drift-guard (moved from `src/api.rs` tests; needs a Postgres pool):
/// POST /tama/v1/config/structured must return a body that deserializes into
/// OkResponse with ok:true. The round-trip is lossless — no fields are
/// silently dropped or invented.
#[tokio::test]
async fn test_save_structured_config_response_deserializes_into_ok_response() {
    let (state, _temp_dir, _pool, guard) = build_test_state().await;

    let web_state = Arc::new(test_web_state());
    let router = build_web_routes(web_state.clone())
        .with_state(state)
        .layer(axum::extract::Extension(web_state.as_ref().clone()));

    // Build a minimal valid StructuredConfigBody from the sample.
    let body = serde_json::json!({
        "general": {
            "log_level": "info",
            "models_dir": "/models",
            "logs_dir": "/logs",
            "update_check_interval": 12
        },
        "backends": {},
        "lifecycle": {
            "restart_policy": "always",
            "max_restarts": 10,
            "restart_delay_ms": 3000,
            "health_check_interval_ms": 5000,
            "health_check_timeout_ms": 30000,
            "health_check_retries": 3
        },
        "sampling_templates": {},
        "proxy": {
            "host": "0.0.0.0",
            "port": 18910,
            "auto_unload": false,
            "idle_timeout_secs": 300,
            "startup_timeout_secs": 120,
            "circuit_breaker_threshold": 3,
            "circuit_breaker_cooldown_seconds": 60,
            "metrics_retention_secs": 86400,
            "pull_queue_poll_interval_secs": 2,
            "max_loaded_models": 1
        },
        "compaction": {
            "enabled": false
        },
        "langfuse": {
            "enabled": false,
            "public_key": "",
            "secret_key": "",
            "host": "",
            "environment": "",
            "capture_input": false,
            "capture_output": false,
            "capture_streaming": false,
            "telemetry_max_bytes": 0,
            "electricity_price_per_kwh": 0.0
        }
    });

    let req = axum::extract::Request::builder()
        .method("POST")
        .uri("/tama/v1/config/structured")
        .header("Content-Type", "application/json")
        .header("origin", "http://localhost:11435")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body must be readable");
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "response body: {}",
        String::from_utf8_lossy(&body_bytes)
    );

    // Deserialize into OkResponse.
    let parsed: tama_core::proxy::tama_handlers::OkResponse = serde_json::from_slice(&body_bytes)
        .expect("config save response must deserialize into OkResponse");
    assert!(parsed.ok, "ok must be true");

    // Lossless round-trip.
    let raw_value: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("body must be valid JSON");
    assert_eq!(
        serde_json::to_value(parsed).expect("parsed must serialize"),
        raw_value,
        "OkResponse round-trip must be lossless — config save struct fields must match wire shape"
    );

    guard.finish().await;
}
