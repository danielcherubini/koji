//! GET /tama/v1/tamads — List all tamad connections.
//! GET /tama/v1/tamads/:id — Get a tamad by id.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::api::error::error_response;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// GET /tama/v1/tamads
/// Returns list of all registered tamad connections.
pub async fn list_tamads(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

    match tama_core::db::queries::list_tamads(pool).await {
        Ok(tamads) => Json(tamads).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// GET /tama/v1/tamads/:id
/// Returns a single tamad by id.
pub async fn get_tamad(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

    match tama_core::db::queries::get_tamad(pool, &id).await {
        Ok(Some(tamad)) => Json(tamad).into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            format!("Tamad '{}' not found", id),
            Some("NotFoundError"),
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
            log_filter: None,
            log_status: None,
            log_read: None,
            log_tail: None,
            log_events_tx: Arc::new(tokio::sync::Mutex::new(None)),
        });

        (state, web_state, guard)
    }

    /// GET /tama/v1/tamads on empty DB → 200 with empty array.
    #[tokio::test]
    async fn test_list_tamads_empty() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/tamads")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);

        guard.finish().await;
    }

    /// GET /tama/v1/tamads/:id for unknown tamad → 404.
    #[tokio::test]
    async fn test_get_tamad_not_found() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/tamads/nonexistent-id")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "unknown tamad should return 404"
        );

        guard.finish().await;
    }

    /// POST → GET list → GET single round trip.
    #[tokio::test]
    async fn test_tamad_crud_round_trip() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create tamad
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
            .body(Body::from(body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);

        // Read back the created tamad to get its auto-generated id
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        let tamad_id = created["connection"]["id"].as_str().unwrap();

        // GET list — should contain one tamad
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/tamads")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);

        // GET single tamad by id
        let req = Request::builder()
            .method("GET")
            .uri(format!("/tama/v1/tamads/{}", tamad_id))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["name"], "my-tamad");
        assert_eq!(json["id"], tamad_id);

        guard.finish().await;
    }
}
