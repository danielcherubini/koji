//! POST /tama/v1/tamads — Register (or re-register) a tamad connection.
//!
//! Idempotent upsert keyed by name: a new name creates the row (201), an
//! existing name updates its url/protocol/token (200).

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::api::error::error_response;
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
/// Idempotently registers a tamad connection by name. A new name creates
/// the row and returns 201 with an auto-generated UUID; an existing name
/// updates url/protocol/token and returns 200 with the stored id.
pub async fn create_tamad(
    State(state): State<Arc<ProxyState>>,
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

    let pool = web_state.db_pool.as_ref();

    // Auto-generate candidate UUID for tamad id; on a name conflict the
    // stored id is returned (upsert is idempotent by name).
    let tamad_id = Uuid::new_v4().to_string();

    let name = req.name.clone();
    let url = req.url.clone();
    let token = req.token.clone();

    let (stored_id, created) = match tama_core::db::queries::upsert_tamad_by_name(
        pool,
        &tamad_id,
        &name,
        &url,
        protocol,
        token.as_deref(),
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                e.to_string(),
                Some("DatabaseError"),
            )
        }
    };

    // Fetch the stored tamad
    match tama_core::db::queries::get_tamad(pool, &stored_id).await {
        Ok(Some(tamad)) => {
            // Refresh the pool so the stream task picks up the new or
            // updated connection immediately (plan-191 Task 4).
            if let Err(e) = state.tamad_pool().upsert_connection(&tamad).await {
                tracing::warn!("failed to refresh tamad pool for '{}': {}", tamad.name, e);
            }
            let status = if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (status, Json(tamad)).into_response()
        }
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve created tamad",
            None,
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    async fn build_test_state() -> (
        Arc<ProxyState>,
        Arc<crate::web_types::WebState>,
        crate::testing::postgres::SchemaGuard,
    ) {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let config = tama_core::config::Config::default();
        let state = Arc::new(ProxyState::new(config, None, pool.clone()));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: pool,
        });

        (state, web_state, guard)
    }

    /// POST /tama/v1/tamads with invalid protocol → 400.
    #[tokio::test]
    async fn test_create_tamad_invalid_protocol_returns_400() {
        let (state, web_state, guard) = build_test_state().await;
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
        guard.finish().await;
    }

    /// POST /tama/v1/tamads with empty name → 400.
    #[tokio::test]
    async fn test_create_tamad_empty_name_returns_400() {
        let (state, web_state, guard) = build_test_state().await;
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
        guard.finish().await;
    }

    /// POST /tama/v1/tamads with empty url → 400.
    #[tokio::test]
    async fn test_create_tamad_empty_url_returns_400() {
        let (state, web_state, guard) = build_test_state().await;
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
        guard.finish().await;
    }

    /// POST /tama/v1/tamads with grpc protocol → 201 with auto-generated id.
    #[tokio::test]
    async fn test_create_tamad_grpc_success() {
        let (state, web_state, guard) = build_test_state().await;
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
        guard.finish().await;
    }

    /// POST /tama/v1/tamads with http protocol → 201.
    #[tokio::test]
    async fn test_create_tamad_http_success() {
        let (state, web_state, guard) = build_test_state().await;
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
        guard.finish().await;
    }

    /// POST /tama/v1/tamads with duplicate name → 200 (idempotent upsert)
    /// returning the same id as the first registration.
    #[tokio::test]
    async fn test_create_tamad_duplicate_name_upserts() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let first_body = serde_json::json!({
            "name": "my-tamad",
            "url": "grpc://localhost:50051",
            "protocol": "grpc",
            "token": "tok-old"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(first_body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let first: serde_json::Value = serde_json::from_slice(&body_str).unwrap();

        // Same name, different url/token → 200 with the same id, updated values.
        let dup_body = serde_json::json!({
            "name": "my-tamad",
            "url": "grpc://localhost:50052",
            "protocol": "grpc",
            "token": "tok-new"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(dup_body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::OK,
            "duplicate name should upsert and return 200"
        );
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let second: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(
            second["id"], first["id"],
            "upsert must return the originally stored id"
        );
        assert_eq!(second["url"], "grpc://localhost:50052");
        assert_eq!(second["token"], "tok-new");
        guard.finish().await;
    }
}
