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

use crate::api::error::{error_body, error_response};
use crate::api::helpers::shared_repository;
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
    // Check tamad exists first
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let exists = match &web_state.repository {
        Some(repo_arc) => {
            let repo = repo_arc.lock().unwrap();
            repo.get_tamad(&id).map(|t| t.is_some()).unwrap_or(false)
        }
        None => false,
    };

    if !exists {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("Tamad '{}' not found", id),
            Some("NotFoundError"),
        );
    }

    // Validate at least one field is provided
    if req.url.is_none() && req.token.is_none() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "At least one of 'url' or 'token' must be provided",
            Some("ValidationError"),
        );
    }

    let repo = repo.clone();

    // Capture fields for spawn_blocking closure
    let url = req.url.clone();
    let token = req.token.clone();

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();

            // Get the current tamad to preserve fields not being updated
            let current = repo.get_tamad(&id).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;

            let current = current.ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body(format!("Tamad '{}' not found", id), Some("NotFoundError")),
                )
            })?;

            let new_url = url.as_deref().unwrap_or(current.url.as_str());
            let new_token = token.as_deref().or(current.token.as_deref());

            repo.update_tamad(&id, new_url, new_token).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;

            repo.get_tamad(&id).ok().flatten().ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body("Failed to retrieve updated tamad", None),
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

    Json(tamad).into_response()
}

/// DELETE /tama/v1/tamads/:id
/// Unregisters a tamad connection.
pub async fn delete_tamad(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let id_clone = id.clone();
    let repo = repo.clone();

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();
            let deleted = repo.delete_tamad(&id).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;
            if !deleted {
                return Err((
                    StatusCode::NOT_FOUND,
                    error_body(
                        format!("Tamad '{}' not found", id_clone),
                        Some("NotFoundError"),
                    ),
                ));
            }
            Ok(())
        })
        .await;

    match result {
        Ok(Ok(())) => Json(serde_json::json!({"deleted": true})).into_response(),
        Ok(Err((s, b))) => (s, Json(b)).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task panicked: {}", e),
            None,
        ),
    }
}

/// POST /tama/v1/tamads/:id/health
/// Performs a health check against the tamad instance.
pub async fn trigger_health_check(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Check tamad exists
    let exists = match &web_state.repository {
        Some(repo_arc) => {
            let repo = repo_arc.lock().unwrap();
            repo.get_tamad(&id).map(|t| t.is_some()).unwrap_or(false)
        }
        None => false,
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

    /// PATCH on non-existent tamad → 404.
    #[tokio::test]
    async fn test_update_tamad_not_found_returns_404() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
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
    }

    /// DELETE on non-existent tamad → 404.
    #[tokio::test]
    async fn test_delete_tamad_not_found_returns_404() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
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
    }

    /// POST create → PATCH update → GET verify → DELETE → GET 404.
    #[tokio::test]
    async fn test_tamad_update_and_delete_round_trip() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
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
    }

    /// PATCH update only url preserves token.
    #[tokio::test]
    async fn test_update_tamad_partial_preserves_other_fields() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
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
    }

    /// POST /tama/v1/tamads/:id/health for non-existent tamad → 404.
    #[tokio::test]
    async fn test_health_check_not_found_returns_404() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
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
    }

    /// POST /tama/v1/tamads/:id/health for existing tamad → 200.
    /// The status will be "error" since no actual tamad server is running.
    #[tokio::test]
    async fn test_health_check_existing_tamad() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
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
    }
}
