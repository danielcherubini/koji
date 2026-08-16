//! POST /tama/v1/tamads — Register a new tamad connection.

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::api::error::{error_body, error_response};
use crate::api::helpers::shared_repository;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// Request body for registering a new tamad.
#[derive(Debug, serde::Deserialize)]
pub struct CreateTamadRequest {
    pub name: String,
    pub url: String,
    pub protocol: String, // "grpc" or "http"
    pub token: Option<String>,
}

/// POST /tama/v1/tamads
/// Registers a new tamad connection. Auto-generates a UUID for the tamad id.
pub async fn create_tamad(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Json(req): Json<CreateTamadRequest>,
) -> impl IntoResponse {
    // Validate name is non-empty
    if req.name.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "name cannot be empty",
            Some("ValidationError"),
        );
    }

    // Validate url is non-empty
    if req.url.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "url cannot be empty",
            Some("ValidationError"),
        );
    }

    // Validate protocol
    let protocol = match req.protocol.as_str() {
        "grpc" => "grpc",
        "http" => "http",
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid protocol '{}': must be 'grpc' or 'http'",
                    req.protocol
                ),
                Some("ValidationError"),
            );
        }
    };

    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    // Auto-generate UUID for tamad id
    let tamad_id = Uuid::new_v4().to_string();

    // Capture fields for spawn_blocking closure
    let name = req.name.clone();
    let url = req.url.clone();
    let token = req.token.clone();
    let repo = repo.clone();

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();

            repo.insert_tamad(&tamad_id, &name, &url, protocol, token.as_deref())
                .map_err(|e: anyhow::Error| {
                    // Walk the error chain to check for UNIQUE constraint violations
                    let is_unique = e
                        .chain()
                        .any(|c| c.to_string().to_lowercase().contains("unique"));
                    let msg = e.to_string();
                    let status = if is_unique {
                        StatusCode::CONFLICT
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR
                    };
                    (status, error_body(msg, Some("DatabaseError")))
                })?;

            // Fetch the created tamad
            repo.get_tamad(&tamad_id).ok().flatten().ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body("Failed to retrieve created tamad", None),
                )
            })
        })
        .await;

    let tamad = match result {
        Ok(Ok(t)) => t,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task panicked: {}", e),
                None,
            )
        }
    };

    // Return the created tamad
    (StatusCode::CREATED, Json(tamad)).into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::{Arc, Mutex};
    use tama_core::db::repository::Repository;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    fn build_test_state(
        tmp_dir: &std::path::Path,
    ) -> (Arc<ProxyState>, Arc<crate::web_types::WebState>) {
        let config = tama_core::config::Config::default();
        let state = Arc::new(ProxyState::new(config, Some(tmp_dir.to_path_buf()), None));

        let repo = Repository::open(tmp_dir).unwrap();

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            repository: Some(Arc::new(Mutex::new(repo))),
            db_pool: None,
        });

        (state, web_state)
    }

    /// POST /tama/v1/tamads with invalid protocol → 400.
    #[tokio::test]
    async fn test_create_tamad_invalid_protocol_returns_400() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "my-tamad",
            "url": "grpc://localhost:50051",
            "protocol": "websocket"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "invalid protocol should return 400"
        );
    }

    /// POST /tama/v1/tamads with empty name → 400.
    #[tokio::test]
    async fn test_create_tamad_empty_name_returns_400() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "",
            "url": "grpc://localhost:50051",
            "protocol": "grpc"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "empty name should return 400"
        );
    }

    /// POST /tama/v1/tamads with empty url → 400.
    #[tokio::test]
    async fn test_create_tamad_empty_url_returns_400() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "my-tamad",
            "url": "",
            "protocol": "grpc"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "empty url should return 400"
        );
    }

    /// POST /tama/v1/tamads with grpc protocol → 201 with auto-generated id.
    #[tokio::test]
    async fn test_create_tamad_grpc_success() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "my-grpc-tamad",
            "url": "grpc://localhost:50051",
            "protocol": "grpc",
            "token": "secret-token"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::CREATED,
            "grpc tamad should return 201"
        );

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["name"], "my-grpc-tamad");
        assert_eq!(json["protocol"], "grpc");
        assert_eq!(json["token"], "secret-token");
        assert!(
            json["id"].is_string(),
            "id should be auto-generated UUID string"
        );
    }

    /// POST /tama/v1/tamads with http protocol → 201.
    #[tokio::test]
    async fn test_create_tamad_http_success() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "my-http-tamad",
            "url": "http://localhost:8080",
            "protocol": "http"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::CREATED,
            "http tamad should return 201"
        );

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["name"], "my-http-tamad");
        assert_eq!(json["protocol"], "http");
    }

    /// POST /tama/v1/tamads with duplicate name → 409.
    #[tokio::test]
    async fn test_create_tamad_duplicate_name_returns_409() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // First creation
        let body = serde_json::json!({
            "name": "my-tamad",
            "url": "grpc://localhost:50051",
            "protocol": "grpc"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.clone()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);

        // Duplicate creation with same name
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::CONFLICT,
            "duplicate name should return 409"
        );
    }
}
