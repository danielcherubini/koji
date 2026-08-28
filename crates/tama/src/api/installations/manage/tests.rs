use crate::api::error::tests::assert_error_shape;
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
        db_pool: tama_test_support::test_dummy_pool(),
        log_filter: None,
        log_status: None,
        log_read: None,
        log_tail: None,
        log_events_tx: Arc::new(tokio::sync::Mutex::new(None)),
    }
}

/// Path traversal in update_installation name should return 400.
#[tokio::test]
async fn test_update_installation_path_traversal_rejected() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        None,
        tama_test_support::test_dummy_pool(),
    ));

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
        "update_installation should reject names containing '\\' with 400"
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
        "update_installation should reject names containing '..' with 400"
    );
}

/// Path traversal in update_installation_source name should return 400.
#[tokio::test]
async fn test_update_installation_source_path_traversal_rejected() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        None,
        tama_test_support::test_dummy_pool(),
    ));

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
        "update_installation_source should reject names containing '..' with 400"
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
        "update_installation_source should reject names containing '\\' with 400"
    );
}

/// Missing backend in update_installation_source should return 404.
#[tokio::test]
async fn test_update_installation_source_missing_backend() {
    let config = tama_core::config::Config::default();
    let db_dir = tempfile::tempdir().unwrap();
    let guard = crate::testing::postgres::with_schema().await;
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        Some(db_dir.path().to_path_buf()),
        Arc::new(guard.pool.clone()),
    ));

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
        "update_installation_source should return 404 for non-existent backend"
    );
}

/// Path traversal in patch_installation name should return 400.
/// Note: `/` cannot be tested via HTTP since it's a path separator.
/// The handler still checks for it as defense-in-depth.
#[tokio::test]
async fn test_patch_installation_path_traversal_rejected() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        None,
        tama_test_support::test_dummy_pool(),
    ));

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state)
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    let csrf_token = "test-csrf-token-12345";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);
    let body = serde_json::json!({}).to_string();

    // Test with `\` in name — backslash won't be normalized by Axum.
    let req = Request::builder()
        .method("PATCH")
        .uri("/tama/v1/backends/foo\\bar?gpu_variant=cpu")
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
        "patch_installation should reject names containing '\\' with 400"
    );

    // Test with `..` in name — Axum normalizes `../` segments but not `..`
    // embedded within a segment. The validation catches this.
    let req = Request::builder()
        .method("PATCH")
        .uri("/tama/v1/backends/foo..bar?gpu_variant=cpu")
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
        "patch_installation should reject names containing '..' with 400"
    );
}

/// PATCH with all-None body preserves all backend config fields (no-op).
#[tokio::test]
async fn test_patch_installation_all_none_preserves() {
    let config = tama_core::config::Config::default();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let guard = crate::testing::postgres::with_schema().await;
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        Some(tmp_dir.path().to_path_buf()),
        Arc::new(guard.pool.clone()),
    ));

    // Seed backend config via InstallationManager
    {
        let mgr = tama_core::installations::InstallationManager::new(Arc::new(guard.pool.clone()));
        mgr.save_config(
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string(), "-b 2048".to_string()],
            &["RADV_PERFTEST=nogttspill".to_string()],
            Some("http://localhost:8080/health"),
        )
        .await
        .unwrap();
    }

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state.clone())
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    let csrf_token = "test-csrf-token-67890";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

    // PATCH with empty body (all fields None/present as null)
    let body = serde_json::json!({}).to_string();
    let req = Request::builder()
        .method("PATCH")
        .uri("/tama/v1/backends/llama_cpp?gpu_variant=cpu")
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
        axum::http::StatusCode::OK,
        "patch_installation with all-None body should succeed"
    );

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");
    assert_eq!(json["success"], true);

    // Verify fields were preserved
    let mgr = tama_core::installations::InstallationManager::new(Arc::new(guard.pool.clone()));
    let args = mgr.get_default_args("llama_cpp", "cpu").await;
    assert_eq!(args, vec!["-fa 1", "-b 2048"]);

    let env = mgr.get_default_env("llama_cpp", "cpu").await;
    assert_eq!(env, vec!["RADV_PERFTEST=nogttspill"]);

    let health = mgr.get_health_check_url("llama_cpp", "cpu").await;
    assert_eq!(health, Some("http://localhost:8080/health".to_string()));
}

/// PATCH default_args only changes args, preserves env.
#[tokio::test]
async fn test_patch_installation_default_args_only() {
    let config = tama_core::config::Config::default();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let guard = crate::testing::postgres::with_schema().await;
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        Some(tmp_dir.path().to_path_buf()),
        Arc::new(guard.pool.clone()),
    ));

    // Seed backend config
    {
        let mgr = tama_core::installations::InstallationManager::new(Arc::new(guard.pool.clone()));
        mgr.save_config(
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string(), "-b 2048".to_string()],
            &["RADV_PERFTEST=nogttspill".to_string()],
            Some("http://localhost:8080/health"),
        )
        .await
        .unwrap();
    }

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state.clone())
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    let csrf_token = "test-csrf-token-abcde";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

    // PATCH with only default_args changed
    let body = serde_json::json!({
        "default_args": ["-fa 2", "-b 4096"]
    })
    .to_string();
    let req = Request::builder()
        .method("PATCH")
        .uri("/tama/v1/backends/llama_cpp?gpu_variant=cpu")
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
        axum::http::StatusCode::OK,
        "patch_installation should succeed"
    );

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");
    assert_eq!(json["success"], true);

    // Verify args changed
    let mgr = tama_core::installations::InstallationManager::new(Arc::new(guard.pool.clone()));
    let args = mgr.get_default_args("llama_cpp", "cpu").await;
    assert_eq!(args, vec!["-fa 2", "-b 4096"]);

    // Verify env preserved
    let env = mgr.get_default_env("llama_cpp", "cpu").await;
    assert_eq!(env, vec!["RADV_PERFTEST=nogttspill"]);

    // Verify health_check_url preserved
    let health = mgr.get_health_check_url("llama_cpp", "cpu").await;
    assert_eq!(health, Some("http://localhost:8080/health".to_string()));
}

/// DELETE /tama/v1/backends/:name with path traversal in name should return
/// 400 with canonical error shape.
#[tokio::test]
async fn test_remove_installation_error_shape() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        None,
        tama_test_support::test_dummy_pool(),
    ));

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state)
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    // Path traversal in name — `..` embedded within a segment.
    let req = Request::builder()
        .method("DELETE")
        .uri("/tama/v1/backends/foo..bar")
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "remove_installation should reject names containing '..' with 400"
    );

    let detail = assert_error_shape(resp).await;
    assert_eq!(
        detail.r#type,
        Some("ValidationError".to_string()),
        "path traversal should return ValidationError type"
    );
}

/// POST /tama/v1/backends/:name/activate with path traversal in name should
/// return 400 with canonical error shape.
#[tokio::test]
async fn test_activate_backend_error_shape() {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        None,
        tama_test_support::test_dummy_pool(),
    ));

    let web_state_for_test = Arc::new(test_web_state());
    let router = crate::router::build_web_routes(web_state_for_test.clone())
        .with_state(state)
        .layer(axum::extract::Extension(
            web_state_for_test.as_ref().clone(),
        ));

    let csrf_token = "test-csrf-token-12345";
    let cookie_header = format!("{}={}", "tama_csrf_token", csrf_token);

    let body = serde_json::json!({ "version": "1.0.0" }).to_string();

    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/backends/foo..bar/activate")
        .header(axum::http::header::COOKIE, cookie_header.as_str())
        .header("X-CSRF-Token", csrf_token)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let resp = router.oneshot(req).await.expect("request should complete");

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "activate_installation_version should reject names containing '..' with 400"
    );

    let detail = assert_error_shape(resp).await;
    assert_eq!(
        detail.r#type,
        Some("ValidationError".to_string()),
        "path traversal should return ValidationError type"
    );
}
