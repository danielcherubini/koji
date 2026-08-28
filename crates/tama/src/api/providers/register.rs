//! POST /tama/v1/providers — Create a new provider.
//! (Postgres, plan-190 Task 5.)

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::api::error::{error_body, error_response};
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// Request body for creating a new provider.
#[derive(Debug, serde::Deserialize)]
pub struct CreateProviderRequest {
    pub name: String,
    pub provider_type: String, // "local" or "remote"
    pub engine: String,
    pub tamad_id: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

/// POST /tama/v1/providers
/// Creates a new provider. Validates that local providers have tamad_id and remote have base_url.
pub async fn create_provider(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Json(req): Json<CreateProviderRequest>,
) -> impl IntoResponse {
    // Validate name is non-empty
    if req.name.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "name cannot be empty",
            Some("ValidationError"),
        );
    }

    // Validate provider_type
    let provider_type = match req.provider_type.as_str() {
        "local" => "local",
        "remote" => "remote",
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!(
                    "Invalid provider_type '{}': must be 'local' or 'remote'",
                    req.provider_type
                ),
                Some("ValidationError"),
            );
        }
    };

    // Validate engine is non-empty
    if req.engine.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "engine cannot be empty",
            Some("ValidationError"),
        );
    }

    // Validate: local needs tamad_id, remote needs base_url
    if provider_type == "local" {
        if req.tamad_id.is_none() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Local providers require tamad_id",
                Some("ValidationError"),
            );
        }
    } else {
        if req.base_url.is_none() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Remote providers require base_url",
                Some("ValidationError"),
            );
        }
    }

    let pool = web_state.db_pool.as_ref();

    let name = req.name;
    let engine = req.engine;
    let tamad_id = req.tamad_id;
    let base_url = req.base_url;
    let api_key = req.api_key;

    match tama_core::db::queries::insert_provider(
        pool,
        &name,
        provider_type,
        &engine,
        tamad_id.as_deref(),
        base_url.as_deref(),
        api_key.as_deref(),
    )
    .await
    {
        Ok(_) => {}
        Err(e) => {
            // Walk the error chain to check for UNIQUE constraint violations
            let is_unique = e
                .chain()
                .any(|c| c.to_string().to_lowercase().contains("unique"));
            let status = if is_unique {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return (
                status,
                Json(error_body(e.to_string(), Some("DatabaseError"))),
            )
                .into_response();
        }
    }

    // Fetch the created provider
    match tama_core::db::queries::get_provider(pool, &name).await {
        Ok(Some(p)) => (StatusCode::CREATED, Json(p)).into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve created provider",
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
            log_filter: None,
            log_status: None,
            log_read: None,
            log_tail: None,
            log_events_tx: Arc::new(tokio::sync::Mutex::new(None)),
        });

        (state, web_state)
    }

    /// POST /tama/v1/providers with local type but no tamad_id → 400.
    #[tokio::test]
    async fn test_create_local_without_tamad_id_returns_400() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "my-local",
            "provider_type": "local",
            "engine": "llama_cpp"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/providers")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "local provider without tamad_id should return 400"
        );

        guard.finish().await;
    }

    /// POST /tama/v1/providers with remote type but no base_url → 400.
    #[tokio::test]
    async fn test_create_remote_without_base_url_returns_400() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "my-remote",
            "provider_type": "remote",
            "engine": "openai"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/providers")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "remote provider without base_url should return 400"
        );

        guard.finish().await;
    }

    /// POST /tama/v1/providers with invalid provider_type → 400.
    #[tokio::test]
    async fn test_create_invalid_provider_type_returns_400() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "my-provider",
            "provider_type": "invalid",
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
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "invalid provider_type should return 400"
        );

        guard.finish().await;
    }

    /// POST /tama/v1/providers with empty name → 400.
    #[tokio::test]
    async fn test_create_empty_name_returns_400() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let body = serde_json::json!({
            "name": "",
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
        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "empty name should return 400"
        );

        guard.finish().await;
    }

    /// POST /tama/v1/providers with duplicate name → 409.
    #[tokio::test]
    async fn test_create_duplicate_name_returns_409() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // First creation
        let body = serde_json::json!({
            "name": "my-provider",
            "provider_type": "local",
            "engine": "llama_cpp",
            "tamad_id": "uuid-123"
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/providers")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.clone()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);

        // Duplicate creation
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/providers")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::CONFLICT,
            "duplicate name should return 409"
        );

        guard.finish().await;
    }
}
