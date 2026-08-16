use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use std::sync::{atomic::AtomicU32, Arc};
use std::time::Instant;
use tower::ServiceExt;

use crate::config::{Config, ModelConfig};
use crate::proxy::tama_handlers::models::{
    handle_tama_cancel_load, handle_tama_get_model, handle_tama_list_models,
    handle_tama_load_model, handle_tama_unload_model,
};
use crate::proxy::{BackendState, ProxyState};

use super::helpers::create_state_with_model;

/// Two model configs, one Ready → loaded has state=="ready", other has "idle".
#[tokio::test]
async fn test_handle_tama_list_models_states() {
    let config = Config::default();
    let state = Arc::new(ProxyState::new(config, None, None));

    // Insert two model configs.
    {
        let mut mc = state.registry.model_configs.write().await;
        mc.insert(
            "ready-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("ready-model".to_string()),
                model: Some("test/ready".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
        mc.insert(
            "idle-model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                api_name: Some("idle-model".to_string()),
                model: Some("test/idle".to_string()),
                enabled: true,
                ..Default::default()
            },
        );
    }

    // Insert a Ready entry for "ready-model" (must match the config key).
    state.registry.models.write().await.insert(
        "ready-model".to_string(),
        BackendState::Ready {
            model_name: "ready-model".to_string(),
            backend: "llama_cpp".to_string(),
            backend_pid: 1234,
            backend_url: "http://127.0.0.1:1234".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            is_docker: false,
            restart_count: 0,
        },
    );

    let app = axum::Router::new()
        .route(
            "/tama/v1/models",
            axum::routing::get(handle_tama_list_models),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("GET")
        .uri("/tama/v1/models")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let models: Vec<&serde_json::Value> = json["models"]
        .as_array()
        .expect("models should be an array")
        .iter()
        .collect();
    assert_eq!(models.len(), 2);

    // Find the ready model and idle model by api_name.
    let ready_model = models
        .iter()
        .find(|m| m["api_name"].as_str() == Some("ready-model"));
    let idle_model = models
        .iter()
        .find(|m| m["api_name"].as_str() == Some("idle-model"));

    assert!(ready_model.is_some(), "ready-model should exist");
    assert!(idle_model.is_some(), "idle-model should exist");

    assert_eq!(ready_model.unwrap()["state"], "ready");
    assert_eq!(idle_model.unwrap()["state"], "idle");
}

/// GET /tama/v1/models/:id for a Ready model → 200 with ready==true.
#[tokio::test]
async fn test_handle_tama_get_model_loaded() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Insert a Ready entry.
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Ready {
            model_name: "test-model".to_string(),
            backend: "llama_cpp".to_string(),
            backend_pid: 1234,
            backend_url: "http://127.0.0.1:1234".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            is_docker: false,
            restart_count: 0,
        },
    );

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id",
            axum::routing::get(handle_tama_get_model),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("GET")
        .uri("/tama/v1/models/test-model")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["ready"], true);
    assert_eq!(json["owned_by"], "llama_cpp");
}

/// GET /tama/v1/models/:id for a configured but not loaded model → 200 with ready==false.
#[tokio::test]
async fn test_handle_tama_get_model_configured_not_loaded() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("configured-model".to_string()),
        model: Some("test/configured".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // No runtime entry — model is configured but not loaded.

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id",
            axum::routing::get(handle_tama_get_model),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("GET")
        .uri("/tama/v1/models/configured-model")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["ready"], false);
}

/// GET /tama/v1/models/:id for an unknown model → 404.
#[tokio::test]
async fn test_handle_tama_get_model_unknown_404() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("other-model".to_string()),
        model: Some("test/other".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // No entry for "unknown-model".

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id",
            axum::routing::get(handle_tama_get_model),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("GET")
        .uri("/tama/v1/models/unknown-model")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// POST /tama/v1/models/:id/load when backend has no binary → 500 LoadModelError.
#[tokio::test]
async fn test_handle_tama_load_model_failure_returns_500() {
    let state = create_state_with_model(ModelConfig {
        backend: "nonexistent_backend".to_string(), // This backend won't have a binary
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id/load",
            axum::routing::post(handle_tama_load_model),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/tama/v1/models/test-model/load")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["type"], "LoadModelError");
}

/// POST /tama/v1/models/:id/cancel for a Starting model → 200, removed from models.
#[tokio::test]
async fn test_handle_tama_cancel_load_starting() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Insert a Starting entry with PID 0 (no real process to kill).
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Starting {
            model_name: "test-model".into(),
            backend: "llama_cpp".into(),
            backend_url: String::new(),
            backend_pid: 0,
            last_accessed: Instant::now(),
            start_time: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            is_docker: false,
            failure_timestamp: None,
        },
    );

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id/cancel",
            axum::routing::post(handle_tama_cancel_load),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/tama/v1/models/test-model/cancel")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Model entry should be removed.
    assert!(
        state
            .registry
            .models
            .read()
            .await
            .get("test-model")
            .is_none(),
        "Model entry should be removed after cancel"
    );
}

/// POST /tama/v1/models/:id/cancel for a Ready model → 409 ModelAlreadyLoadedError.
#[tokio::test]
async fn test_handle_tama_cancel_load_ready_conflict() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Insert a Ready entry.
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Ready {
            model_name: "test-model".to_string(),
            backend: "llama_cpp".to_string(),
            backend_pid: 1234,
            backend_url: "http://127.0.0.1:1234".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            is_docker: false,
            restart_count: 0,
        },
    );

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id/cancel",
            axum::routing::post(handle_tama_cancel_load),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/tama/v1/models/test-model/cancel")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["type"], "ModelAlreadyLoadedError");
}

/// POST /tama/v1/models/:id/cancel for an unknown model → 404 ModelNotLoadingError.
#[tokio::test]
async fn test_handle_tama_cancel_load_unknown_404() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("other-model".to_string()),
        model: Some("test/other".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // No entry for "unknown-model".

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id/cancel",
            axum::routing::post(handle_tama_cancel_load),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/tama/v1/models/unknown-model/cancel")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["type"], "ModelNotLoadingError");
}

/// POST /tama/v1/models/:id/unload for a Ready model → 200, entry gone.
#[tokio::test]
async fn test_handle_tama_unload_model_ready() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("test-model".to_string()),
        model: Some("test/model".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // Insert a Ready entry with a bogus PID (won't find a real process).
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Ready {
            model_name: "test-model".to_string(),
            backend: "llama_cpp".to_string(),
            backend_pid: 99999,
            backend_url: "http://127.0.0.1:1234".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            is_docker: false,
            restart_count: 0,
        },
    );

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id/unload",
            axum::routing::post(handle_tama_unload_model),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/tama/v1/models/test-model/unload")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Model entry should be removed.
    assert!(
        state
            .registry
            .models
            .read()
            .await
            .get("test-model")
            .is_none(),
        "Model entry should be removed after unload"
    );
}

/// POST /tama/v1/models/:id/unload for an unknown model → 404 NotFoundError.
#[tokio::test]
async fn test_handle_tama_unload_model_unknown_404() {
    let state = create_state_with_model(ModelConfig {
        backend: "llama_cpp".to_string(),
        api_name: Some("other-model".to_string()),
        model: Some("test/other".to_string()),
        enabled: true,
        ..Default::default()
    })
    .await;

    // No entry for "unknown-model".

    let app = axum::Router::new()
        .route(
            "/tama/v1/models/:id/unload",
            axum::routing::post(handle_tama_unload_model),
        )
        .with_state(state.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/tama/v1/models/unknown-model/unload")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["error"]["type"], "NotFoundError");
}
