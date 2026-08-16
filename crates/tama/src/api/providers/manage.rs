//! PATCH /tama/v1/providers/:name — Update a provider.
//! DELETE /tama/v1/providers/:name — Delete a provider.
//! (Postgres, plan-190 Task 5.)

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
    let pool = web_state.db_pool.as_ref();

    // Check provider exists first
    match tama_core::db::queries::get_provider(pool, &name).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                format!("Provider '{}' not found", name),
                Some("NotFoundError"),
            )
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }

    if let Err(e) = tama_core::db::queries::update_provider(
        pool,
        &name,
        req.base_url.as_deref(),
        req.api_key.as_deref(),
    )
    .await
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None);
    }

    match tama_core::db::queries::get_provider(pool, &name).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve updated provider",
            None,
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// DELETE /tama/v1/providers/:name
/// Deletes a provider by name.
pub async fn delete_provider(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

    match tama_core::db::queries::delete_provider(pool, &name).await {
        Ok(true) => Json(serde_json::json!({"deleted": true})).into_response(),
        Ok(false) => error_response(
            StatusCode::NOT_FOUND,
            format!("Provider '{}' not found", name),
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

    fn build_test_state(
        pool: Arc<sqlx::PgPool>,
        tmp_dir: &std::path::Path,
    ) -> (Arc<ProxyState>, Arc<crate::web_types::WebState>) {
        let config = tama_core::config::Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            Some(tmp_dir.to_path_buf()),
            pool.clone(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: pool,
        });

        (state, web_state)
    }

    /// PATCH on non-existent provider → 404.
    #[tokio::test]
    async fn test_update_provider_not_found_returns_404() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
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

        guard.finish().await;
    }

    /// DELETE on non-existent provider → 404.
    #[tokio::test]
    async fn test_delete_provider_not_found_returns_404() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
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

        guard.finish().await;
    }

    /// POST → PATCH → GET verifies updated fields → DELETE → GET 404.
    #[tokio::test]
    async fn test_provider_update_and_delete_round_trip() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
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

        guard.finish().await;
    }
}
