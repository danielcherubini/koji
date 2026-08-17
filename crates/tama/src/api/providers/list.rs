//! GET /tama/v1/providers — List all providers.
//! GET /tama/v1/providers/:name — Get a provider by name.
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

/// GET /tama/v1/providers
/// Returns list of all registered providers.
pub async fn list_providers(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();
    match tama_core::db::queries::list_providers(pool).await {
        Ok(providers) => Json(providers).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// GET /tama/v1/providers/:name
/// Returns a single provider by name.
pub async fn get_provider(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();
    let name_clone = name.clone();
    match tama_core::db::queries::get_provider(pool, &name).await {
        Ok(Some(provider)) => Json(provider).into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            format!("Provider '{}' not found", name_clone),
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

    /// GET /tama/v1/providers on empty DB → 200 with empty array.
    #[tokio::test]
    async fn test_list_providers_empty() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/providers")
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

    /// GET /tama/v1/providers/:name for unknown provider → 404.
    #[tokio::test]
    async fn test_get_provider_not_found() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/providers/nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "unknown provider should return 404"
        );

        guard.finish().await;
    }

    /// POST → GET list → GET single round trip.
    #[tokio::test]
    async fn test_provider_crud_round_trip() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create provider
        let body = serde_json::json!({
            "name": "my-local",
            "provider_type": "local",
            "engine": "llama_cpp",
            "tamad_id": "uuid-123"
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

        // GET list — should contain one provider
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/providers")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);

        // GET single provider
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/providers/my-local")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json["name"], "my-local");

        guard.finish().await;
    }
}
