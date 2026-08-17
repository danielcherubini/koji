use crate::config::Config;
use crate::proxy::tama_handlers::{handle_hf_list_quants, handle_tama_system_health};
use crate::proxy::BackendState;
use crate::proxy::ProxyState;
use axum::body::Body;
use axum::http::Request;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Handle system health returns 200 with models_loaded matching the registry.
#[tokio::test]
async fn test_handle_tama_system_health() {
    let config = Config::default();
    let state = Arc::new(ProxyState::new(
        config,
        None,
        crate::db::pool::test_dummy_pool(),
    ));

    // Insert one Ready entry into the model registry.
    use std::time::Instant;
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        BackendState::Ready {
            model_name: "test-model".to_string(),
            backend: "llama_cpp".to_string(),
            backend_pid: 1234,
            backend_url: "http://127.0.0.1:12345".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            failure_timestamp: None,
            is_docker: false,
            restart_count: 0,
        },
    );

    let app = axum::Router::new()
        .route(
            "/tama/v1/system/health",
            axum::routing::get(handle_tama_system_health),
        )
        .with_state(state);

    let req = Request::builder()
        .method("GET")
        .uri("/tama/v1/system/health")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.expect("request should complete");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: serde_json::Value =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");

    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "tama");
    assert_eq!(json["models_loaded"], 1);
}

/// handle_hf_list_quants returns sorted quant entries from wiremock.
#[tokio::test]
async fn test_handle_hf_list_quants() {
    let mock_server = MockServer::start().await;

    // Mock the blob metadata endpoint: {endpoint}/api/models/{repo_id}?blobs=true
    let blobs_response = serde_json::json!({
        "siblings": [
            {
                "rfilename": "qwen3-8b-Q4_K_M.gguf",
                "size": 5000000000i64,
                "lfs": { "oid": "abc123", "size": 5000000000i64 }
            },
            {
                "rfilename": "qwen3-8b-Q8_0.gguf",
                "size": 8000000000i64,
                "lfs": { "oid": "def456", "size": 8000000000i64 }
            }
        ]
    });
    Mock::given(method("GET"))
        .and(path_regex("/api/models/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&blobs_response))
        .mount(&mock_server)
        .await;

    std::env::set_var("HF_ENDPOINT", mock_server.uri());

    let req = Request::builder()
        .method("GET")
        .uri("/tama/v1/hf/bartowski/Qwen3-8B-GGUF")
        .body(Body::empty())
        .unwrap();

    let app = axum::Router::new()
        .route(
            "/tama/v1/hf/*repo_id",
            axum::routing::get(handle_hf_list_quants),
        )
        .with_state(());

    let resp = app.oneshot(req).await.expect("request should complete");
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let json: Vec<serde_json::Value> =
        serde_json::from_slice(&body_str).expect("body should be valid JSON");

    // Should have 2 entries sorted by filename.
    assert_eq!(json.len(), 2);
    assert_eq!(json[0]["filename"], "qwen3-8b-Q4_K_M.gguf");
    assert_eq!(json[1]["filename"], "qwen3-8b-Q8_0.gguf");

    std::env::remove_var("HF_ENDPOINT");
}

/// handle_hf_list_quants rejects repo_id with traversal.
#[tokio::test]
async fn test_handle_hf_list_quants_rejects_traversal() {
    let req = Request::builder()
        .method("GET")
        .uri("/tama/v1/hf/evil/../secret")
        .body(Body::empty())
        .unwrap();

    let app = axum::Router::new()
        .route(
            "/tama/v1/hf/*repo_id",
            axum::routing::get(handle_hf_list_quants),
        )
        .with_state(());

    let resp = app.oneshot(req).await.expect("request should complete");
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::BAD_REQUEST,
        "traversal in repo_id should return 400"
    );
}
