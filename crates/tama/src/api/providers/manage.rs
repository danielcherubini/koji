//! PATCH /tama/v1/providers/:name — Update a provider.
//! DELETE /tama/v1/providers/:name — Delete a provider.

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

/// Request body for updating a provider.
#[derive(Debug, serde::Deserialize)]
pub struct UpdateProviderRequest {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

/// PATCH /tama/v1/providers/:name
/// Updates a provider's base_url and/or api_key.
pub async fn update_provider(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(name): Path<String>,
    Json(req): Json<UpdateProviderRequest>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    // Check provider exists first
    let exists = match &web_state.repository {
        Some(repo_arc) => {
            let repo = repo_arc.lock().unwrap();
            repo.get_provider(&name)
                .map(|p| p.is_some())
                .unwrap_or(false)
        }
        None => false,
    };

    if !exists {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("Provider '{}' not found", name),
            Some("NotFoundError"),
        );
    }

    let repo = repo.clone();

    // Capture fields for spawn_blocking closure
    let base_url = req.base_url.clone();
    let api_key = req.api_key.clone();

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();

            repo.update_provider(&name, base_url.as_deref(), api_key.as_deref())
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        error_body(e.to_string(), None),
                    )
                })?;

            repo.get_provider(&name).ok().flatten().ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body("Failed to retrieve updated provider", None),
                )
            })
        })
        .await;

    let provider = match result {
        Ok(Ok(p)) => p,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task panicked: {}", e),
                None,
            )
        }
    };

    Json(provider).into_response()
}

/// DELETE /tama/v1/providers/:name
/// Deletes a provider by name.
pub async fn delete_provider(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();
            let deleted = repo.delete_provider(&name).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?;
            if !deleted {
                return Err((
                    StatusCode::NOT_FOUND,
                    error_body(
                        format!("Provider '{}' not found", name),
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
        let state = Arc::new(ProxyState::new(config, Some(tmp_dir.to_path_buf())));

        let repo = Repository::open(tmp_dir).unwrap();

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            repository: Some(Arc::new(Mutex::new(repo))),
        });

        (state, web_state)
    }

    /// PATCH on non-existent provider → 404.
    #[tokio::test]
    async fn test_update_provider_not_found_returns_404() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "base_url": "https://new.api/v1"
        })
        .to_string();
        let req = Request::builder()
            .method("PATCH")
            .uri("/tama/v1/providers/nonexistent")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "update non-existent provider should return 404"
        );
    }

    /// DELETE on non-existent provider → 404.
    #[tokio::test]
    async fn test_delete_provider_not_found_returns_404() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("DELETE")
            .uri("/tama/v1/providers/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "delete non-existent provider should return 404"
        );
    }

    /// POST → PATCH → GET verifies updated fields → DELETE → GET 404.
    #[tokio::test]
    async fn test_provider_update_and_delete_round_trip() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create remote provider
        let body = serde_json::json!({
            "name": "my-remote",
            "provider_type": "remote",
            "engine": "openai",
            "base_url": "https://old.api/v1",
            "api_key": "sk-old"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/providers")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);

        // PATCH update base_url
        let body = serde_json::json!({
            "base_url": "https://new.api/v1"
        })
        .to_string();
        let req = Request::builder()
            .method("PATCH")
            .uri("/tama/v1/providers/my-remote")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["base_url"], "https://new.api/v1");

        // DELETE
        let req = Request::builder()
            .method("DELETE")
            .uri("/tama/v1/providers/my-remote")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // GET should now 404
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/providers/my-remote")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }
}
