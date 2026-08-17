//! PATCH /tama/v1/tamads/:id — Update a tamad connection.
//! DELETE /tama/v1/tamads/:id — Unregister a tamad connection.
//! POST /tama/v1/tamads/:id/health — Trigger health check (stub).

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

/// Request body for updating a tamad connection.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateTamadRequest {
    pub url: Option<String>,
    pub token: Option<String>,
}

/// PATCH /tama/v1/tamads/:id
/// Updates a tamad's url and/or token. At least one field must be provided.
pub async fn update_tamad(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateTamadRequest>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

    // Check tamad exists first
    let current = match tama_core::db::queries::get_tamad(pool, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("Tamad '{}' not found", id),
                Some("NotFoundError"),
            )
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    // Validate at least one field is provided
    if req.url.is_none() && req.token.is_none() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "At least one of 'url' or 'token' must be provided",
            Some("ValidationError"),
        );
    }

    let new_url = req.url.as_deref().unwrap_or(current.url.as_str());
    let new_token = req.token.as_deref().or(current.token.as_deref());

    if let Err(e) = tama_core::db::queries::update_tamad(pool, &id, new_url, new_token).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None);
    }

    match tama_core::db::queries::get_tamad(pool, &id).await {
        Ok(Some(tamad)) => Json(tamad).into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve updated tamad",
            None,
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// DELETE /tama/v1/tamads/:id
/// Unregisters a tamad connection.
pub async fn delete_tamad(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

    let deleted = match tama_core::db::queries::delete_tamad(pool, &id).await {
        Ok(d) => d,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    if !deleted {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("Tamad '{}' not found", id),
            Some("NotFoundError"),
        );
    }

    Json(serde_json::json!({"deleted": true})).into_response()
}

/// POST /tama/v1/tamads/:id/health
/// Performs a health check against the tamad instance.
pub async fn trigger_health_check(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Check tamad exists
    let pool = web_state.db_pool.as_ref();
    let exists = match tama_core::db::queries::get_tamad(pool, &id).await {
        Ok(t) => t.is_some(),
        Err(_) => false,
    };

    if !exists {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("Tamad '{}' not found", id),
            Some("NotFoundError"),
        );
    }

    match state.tamad_health_check(&id).await {
        Ok(healthy) => Json(serde_json::json!({
            "status": if healthy { "ok" } else { "unhealthy" },
            "healthy": healthy,
        }))
        .into_response(),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "error": e.to_string(),
        }))
        .into_response(),
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

    /// PATCH on non-existent tamad → 404.
    #[tokio::test]
    async fn test_update_tamad_not_found_returns_404() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "url": "grpc://newhost:50051"
        })
        .to_string();
        let req = Request::builder()
            .method("PATCH")
            .uri("/tama/v1/tamads/nonexistent-id")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "update non-existent tamad should return 404"
        );

        guard.finish().await;
    }

    /// DELETE on non-existent tamad → 404.
    #[tokio::test]
    async fn test_delete_tamad_not_found_returns_404() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("DELETE")
            .uri("/tama/v1/tamads/nonexistent-id")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "delete non-existent tamad should return 404"
        );

        guard.finish().await;
    }

    /// POST create → PATCH update → GET verify → DELETE → GET 404.
    #[tokio::test]
    async fn test_tamad_update_and_delete_round_trip() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create tamad
        let body = serde_json::json!({
            "name": "my-tamad",
            "url": "grpc://localhost:50051",
            "protocol": "grpc",
            "token": "old-token"
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

        // Read back the created tamad to get its id
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        let tamad_id = created["id"].as_str().unwrap();

        // PATCH update url and token
        let body = serde_json::json!({
            "url": "grpc://newhost:50051",
            "token": "new-token"
        })
        .to_string();
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/tama/v1/tamads/{}", tamad_id))
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["url"], "grpc://newhost:50051");
        assert_eq!(json["token"], "new-token");
        assert_eq!(json["name"], "my-tamad");

        // DELETE
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/tama/v1/tamads/{}", tamad_id))
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["deleted"], true);

        // GET should now 404
        let req = Request::builder()
            .method("GET")
            .uri(format!("/tama/v1/tamads/{}", tamad_id))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);

        guard.finish().await;
    }

    /// PATCH update only url preserves token.
    #[tokio::test]
    async fn test_update_tamad_partial_preserves_other_fields() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create tamad with token
        let body = serde_json::json!({
            "name": "partial-tamad",
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
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        let tamad_id = created["id"].as_str().unwrap();

        // PATCH only url (no token)
        let body = serde_json::json!({
            "url": "grpc://newhost:50051"
        })
        .to_string();
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/tama/v1/tamads/{}", tamad_id))
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["url"], "grpc://newhost:50051");
        assert_eq!(json["token"], "secret-token"); // preserved
        assert_eq!(json["name"], "partial-tamad"); // preserved

        guard.finish().await;
    }

    /// POST /tama/v1/tamads/:id/health for non-existent tamad → 404.
    #[tokio::test]
    async fn test_health_check_not_found_returns_404() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/tamads/nonexistent-id/health")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "health check for non-existent tamad should return 404"
        );

        guard.finish().await;
    }

    /// POST /tama/v1/tamads/:id/health for existing tamad → 200.
    /// The status will be "error" since no actual tamad server is running.
    #[tokio::test]
    async fn test_health_check_existing_tamad() {
        let (state, web_state, guard) = build_test_state().await;
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create tamad
        let body = serde_json::json!({
            "name": "health-tamad",
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

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        let tamad_id = created["id"].as_str().unwrap();

        // POST health check
        let req = Request::builder()
            .method("POST")
            .uri(format!("/tama/v1/tamads/{}/health", tamad_id))
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        // Status is "error" because no actual tamad server is listening
        assert_eq!(json["status"], "error");
        assert!(json.get("error").is_some(), "should include error message");

        guard.finish().await;
    }
}
