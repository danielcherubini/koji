use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use tower::ServiceExt;

use super::helpers::create_state_with_model;
use crate::config::ModelConfig;
use crate::proxy::tama_handlers::models::handle_tama_cancel_load;
use crate::proxy::BackendState;

/// Cancel returns 200 for a Starting model with a PID.
/// The fake PID (99999) has no real process group, so kill_process_group
/// returns Ok(()) on ESRCH and is_process_group_alive returns false.
#[tokio::test]
async fn test_cancel_returns_200_for_starting_model() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Insert a Starting entry with a fake PID
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Starting {
            model_name: "test-model".into(),
            backend: "llama_cpp".into(),
            backend_url: String::new(),
            backend_pid: 99999,
            last_accessed: Instant::now(),
            start_time: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            is_docker: false,
            failure_timestamp: None,
        },
    );

    // Clone the Arc before moving state into the router
    let state_clone = state.clone();

    let app = Router::new()
        .route(
            "/tama/v1/models/:id/cancel",
            axum::routing::post(handle_tama_cancel_load),
        )
        .with_state(state);

    let request = Request::post("/tama/v1/models/test-model/cancel")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::OK, "Expected 200 OK");
    assert_eq!(json["loaded"], false, "Expected loaded: false");
    assert_eq!(json["id"], "test-model", "Expected id: test-model");

    // Model entry should be removed
    assert!(
        state_clone
            .registry
            .models
            .read()
            .await
            .get("test-model")
            .is_none(),
        "Model entry should be removed after cancel"
    );
}

/// Cancel returns 409 for a Ready (already loaded) model.
#[tokio::test]
async fn test_cancel_returns_409_for_ready_model() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Insert a Ready entry
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Ready {
            model_name: "test-model".to_string(),
            backend: "llama_cpp".to_string(),
            backend_pid: 12345,
            backend_url: "http://127.0.0.1:1234".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            is_docker: false,
            restart_count: 0,
        },
    );

    let app = Router::new()
        .route(
            "/tama/v1/models/:id/cancel",
            axum::routing::post(handle_tama_cancel_load),
        )
        .with_state(state);

    let request = Request::post("/tama/v1/models/test-model/cancel")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "Expected 409 Conflict for Ready model"
    );
    assert_eq!(
        json["error"]["type"], "ModelAlreadyLoadedError",
        "Expected ModelAlreadyLoadedError type"
    );
}

/// Cancel returns 404 for a non-existing model.
#[tokio::test]
async fn test_cancel_returns_404_for_non_existing_model() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // No model entries in the models map

    let app = Router::new()
        .route(
            "/tama/v1/models/:id/cancel",
            axum::routing::post(handle_tama_cancel_load),
        )
        .with_state(state);

    let request = Request::post("/tama/v1/models/nonexistent/cancel")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Expected 404 Not Found for non-existing model"
    );
    assert_eq!(
        json["error"]["type"], "ModelNotLoadingError",
        "Expected ModelNotLoadingError type"
    );
}

/// Cancel returns 404 for a Failed model (not in a loading state).
#[tokio::test]
async fn test_cancel_returns_404_for_failed_model() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Insert a Failed entry
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Failed {
            model_name: "test-model".to_string(),
            backend: "llama_cpp".to_string(),
            error: "Some error".to_string(),
        },
    );

    let app = Router::new()
        .route(
            "/tama/v1/models/:id/cancel",
            axum::routing::post(handle_tama_cancel_load),
        )
        .with_state(state);

    let request = Request::post("/tama/v1/models/test-model/cancel")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Expected 404 Not Found for Failed model"
    );
    assert_eq!(
        json["error"]["type"], "ModelNotLoadingError",
        "Expected ModelNotLoadingError type for Failed model"
    );
}
