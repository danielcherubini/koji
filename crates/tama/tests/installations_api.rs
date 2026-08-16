use axum::body::Body;
use axum::http::Request;
use std::collections::HashMap;
use std::sync::Arc;
use tama_core::config::Config;
use tama_core::installations::{InstallationInfo, InstallationType};
use tama_core::proxy::ProxyState;
use tama_web::web_types::{CapabilitiesCache, JobManager, WebState};
use tower::ServiceExt;

mod common;

/// Create a minimal WebState for tests.
fn test_web_state() -> WebState {
    WebState {
        jobs: Some(Arc::new(JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        db_pool: tama_core::db::pool::test_dummy_pool(),
    }
}

/// Build the full web router with the given ProxyState and WebState.
fn build_web_routes(state: Arc<ProxyState>, web_state: Arc<WebState>) -> axum::Router {
    tama_web::router::build_web_routes(web_state.clone())
        .with_state(state)
        .layer(axum::extract::Extension(web_state.as_ref().clone()))
}

/// Seed a llama_cpp backend installation into Postgres (cpu variant).
async fn seed_llama_cpp_backend(pool: &sqlx::PgPool) {
    use tama_core::installations::InstallationManager;
    let mgr = InstallationManager::new(Arc::new(pool.clone()));
    // Save config so list_configs returns it
    mgr.save_config("llama_cpp", "cpu", &["-fa 1".to_string()], &[], None)
        .await
        .unwrap();
    // Add an installation record so list_versions returns it
    let info = InstallationInfo {
        name: "llama_cpp".to_string(),
        backend_type: InstallationType::LlamaCpp,
        version: "b8407".to_string(),
        path: std::path::PathBuf::from("/tmp/test/llama-server"),
        installed_at: 1000,
        gpu_variant: "cpu".to_string(),
        source: None,
        docker_config: None,
    };
    mgr.add_installation(&info).await.unwrap();
}

/// Seed a custom backend installation into Postgres.
async fn seed_custom_backend(pool: &sqlx::PgPool) {
    use tama_core::installations::InstallationManager;
    let mgr = InstallationManager::new(Arc::new(pool.clone()));
    mgr.save_config("my_custom", "cpu", &[], &[], None)
        .await
        .unwrap();
    let info = InstallationInfo {
        name: "my_custom".to_string(),
        backend_type: InstallationType::Custom,
        version: "1.0.0".to_string(),
        path: std::path::PathBuf::from("/tmp/test/custom-server"),
        installed_at: 2000,
        gpu_variant: "cpu".to_string(),
        source: None,
        docker_config: None,
    };
    mgr.add_installation(&info).await.unwrap();
}

/// GET /tama/v1/installations on an empty registry returns 200 with backends=[],
/// custom=[], available containing known types, and compaction.enabled==false.
#[tokio::test]
async fn test_get_backends_empty_registry_matches_snapshot() {
    let guard = common::with_schema().await;
    let config = Config::default();
    let state = Arc::new(ProxyState::new(config, None, Arc::new(guard.pool.clone())));

    let web_state_for_test = Arc::new(test_web_state());
    let router = build_web_routes(state.clone(), web_state_for_test);

    let req = Request::builder()
        .method("GET")
        .uri("/tama/v1/installations")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");

    assert_eq!(json["backends"], serde_json::Value::Array(Vec::new()));
    assert_eq!(json["custom"], serde_json::Value::Array(Vec::new()));

    // available should contain "llama_cpp" since no installation exists
    let available: Vec<&str> = json["available"]
        .as_array()
        .expect("available should be an array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        available.contains(&"llama_cpp"),
        "available should contain 'llama_cpp', got: {:?}",
        available
    );

    // compaction.enabled should be false (Config default)
    assert_eq!(json["compaction"]["enabled"], false);
}

/// GET /tama/v1/installations includes an installed llama_cpp entry.
#[tokio::test]
async fn test_get_backends_includes_installed_entry() {
    let guard = common::with_schema().await;
    seed_llama_cpp_backend(&guard.pool).await;

    let config = Config::default();
    let state = Arc::new(ProxyState::new(config, None, Arc::new(guard.pool.clone())));

    let web_state_for_test = Arc::new(test_web_state());
    let router = build_web_routes(state.clone(), web_state_for_test);

    let req = Request::builder()
        .method("GET")
        .uri("/tama/v1/installations")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");

    // backends array should contain an entry with type "llama_cpp"
    let backend_entries: Vec<&serde_json::Value> = json["backends"]
        .as_array()
        .expect("backends should be an array")
        .iter()
        .filter(|b| b["type"].as_str() == Some("llama_cpp"))
        .collect();
    assert!(
        !backend_entries.is_empty(),
        "backends should contain llama_cpp entry, got: {:?}",
        json["backends"]
    );

    // The installed field should be true
    assert_eq!(backend_entries[0]["installed"], true);
}

/// Custom backend entries appear in the custom array, not in backends.
#[tokio::test]
async fn test_get_backends_custom_entry_appears_in_custom_array() {
    let guard = common::with_schema().await;
    seed_custom_backend(&guard.pool).await;

    let config = Config::default();
    let state = Arc::new(ProxyState::new(config, None, Arc::new(guard.pool.clone())));

    let web_state_for_test = Arc::new(test_web_state());
    let router = build_web_routes(state.clone(), web_state_for_test);

    let req = Request::builder()
        .method("GET")
        .uri("/tama/v1/installations")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");

    // custom array should contain an entry for my_custom
    let custom_entries: Vec<&serde_json::Value> = json["custom"]
        .as_array()
        .expect("custom should be an array")
        .iter()
        .filter(|b| b["type"].as_str() == Some("custom"))
        .collect();
    assert!(
        !custom_entries.is_empty(),
        "custom should contain my_custom entry, got: {:?}",
        json["custom"]
    );

    // The type should be "custom"
    assert_eq!(custom_entries[0]["type"], "custom");
}

/// GET /tama/v1/system/capabilities returns a body with cuda_versions array.
#[tokio::test]
async fn test_get_capabilities_returns_supported_cuda_versions() {
    let config = Config::default();
    let state = Arc::new(ProxyState::new(
        config,
        None,
        tama_core::db::pool::test_dummy_pool(),
    ));

    // Build WebState with a CapabilitiesCache so the endpoint can compute
    let web_state = Arc::new(WebState {
        jobs: Some(Arc::new(JobManager::new())),
        capabilities: Some(Arc::new(CapabilitiesCache::new())),
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        db_pool: tama_core::db::pool::test_dummy_pool(),
    });

    let router = build_web_routes(state.clone(), web_state);

    let req = Request::builder()
        .method("GET")
        .uri("/tama/v1/system/capabilities")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");

    // cuda_versions should be an array
    assert!(
        json["supported_cuda_versions"].is_array(),
        "supported_cuda_versions should be an array, got: {:?}",
        json["supported_cuda_versions"]
    );
}

/// POST /tama/v1/installations/compaction without matching CSRF pair → 403;
/// with matching cookie+header → not 403.
#[tokio::test]
async fn test_origin_enforcement_blocks_cross_origin_post() {
    let config = Config::default();
    let state = Arc::new(ProxyState::new(
        config,
        None,
        tama_core::db::pool::test_dummy_pool(),
    ));

    let web_state_for_test = Arc::new(test_web_state());
    let router = build_web_routes(state.clone(), web_state_for_test);

    // POST with only a cookie (no header) → 403
    let token = "test-csrf-token-abcde";
    // Use enabled=false to avoid triggering load_compaction_backend which can be slow
    let body = serde_json::json!({"enabled": false}).to_string();

    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/installations/compaction")
        .header(
            axum::http::header::COOKIE,
            format!("tama_csrf_token={}", token),
        )
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.clone()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::FORBIDDEN,
        "POST with cookie-only should return 403"
    );

    // POST with matching cookie + header → not 403
    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/installations/compaction")
        .header(
            axum::http::header::COOKIE,
            format!("tama_csrf_token={}", token),
        )
        .header("X-CSRF-Token", token)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");
    assert_ne!(
        resp.status(),
        axum::http::StatusCode::FORBIDDEN,
        "POST with matching CSRF pair should not return 403"
    );
}
