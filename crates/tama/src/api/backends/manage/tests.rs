use axum::body::Body;
use axum::http::Request;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

/// Create a minimal WebState for tests.
fn test_web_state() -> crate::web_types::WebState {
    crate::web_types::WebState {
        jobs: Some(Arc::new(crate::web_types::JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
    }
}

/// Path traversal in update_backend name should return 400.
#[tokio::test]
async fn test_update_backend_path_traversal_rejected() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state)
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    // Valid CSRF token pair — cookie and header must match.
    let csrf_token = "test-csrf-token-12345";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

    // Test with `\` in name — backslash won't be normalized by Axum.
    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/backends/foo\\bar/update")
        .header(axum::http::header::COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token)
        .body(Body::empty())
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "update_backend should reject names containing '\\' with 400"
    );

    // Test with `..` in name — Axum normalizes `../` segments but not `..`
    // embedded within a segment. The validation catches this.
    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/backends/foo..bar/update")
        .header(axum::http::header::COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token)
        .body(Body::empty())
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "update_backend should reject names containing '..' with 400"
    );
}

/// Path traversal in update_backend_source name should return 400.
#[tokio::test]
async fn test_update_backend_source_path_traversal_rejected() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state)
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    let csrf_token = "test-csrf-token-12345";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

    let body = serde_json::json!({"build_from_source": true}).to_string();

    // Test with `..` in name — Axum normalizes `../` segments but not `..`
    // embedded within a segment. The validation catches this.
    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/backends/foo..bar/source")
        .header(axum::http::header::COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(Body::from(body.clone()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "update_backend_source should reject names containing '..' with 400"
    );

    // Test with `\` in name — backslash won't be normalized by Axum.
    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/backends/foo\\bar/source")
        .header(axum::http::header::COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "update_backend_source should reject names containing '\\' with 400"
    );
}

/// Missing backend in update_backend_source should return 404.
#[tokio::test]
async fn test_update_backend_source_missing_backend() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state)
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    let csrf_token = "test-csrf-token-12345";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

    let body = serde_json::json!({"build_from_source": true}).to_string();

    // POST to a non-existent backend
    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/backends/nonexistent_backend/source")
        .header(axum::http::header::COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::NOT_FOUND,
        "update_backend_source should return 404 for non-existent backend"
    );
}
