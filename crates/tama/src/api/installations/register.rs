//! POST /tama/v1/backends — Register a backend directly (bypasses binary install).
//!
//! Used for docker-based backends and other non-binary installations.

use std::str::FromStr;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::*;
use crate::api::error::error_response;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends — Register a backend directly.
///
/// For docker backends: requires `docker_config` in the request body.
/// Validates docker availability and docker_config before inserting into DB.
pub async fn register_installation(
    Extension(_web_state): Extension<WebState>,
    state: State<Arc<ProxyState>>,
    Json(req): Json<RegisterBackendRequest>,
) -> impl IntoResponse {
    // Validate name (non-empty, no path traversal)
    if req.name.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "name cannot be empty",
            Some("ValidationError"),
        );
    }
    if let Err(resp) = crate::api::installations::reject_traversal(&req.name, "backend name") {
        return resp;
    }

    // Validate version (non-empty, no path traversal)
    if req.version.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "version cannot be empty",
            Some("ValidationError"),
        );
    }
    if let Err(resp) = crate::api::installations::reject_traversal(&req.version, "version") {
        return resp;
    }

    // Validate backend_type consistency with docker_config
    let is_docker = req.backend_type.to_lowercase() == "docker";

    if is_docker && req.docker_config.is_none() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "docker backend requires docker_config",
            Some("ValidationError"),
        );
    }

    if !is_docker && req.docker_config.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "docker_config only valid for docker backend type",
            Some("ValidationError"),
        );
    }

    // Validate docker_config if present
    if let Some(ref cfg) = req.docker_config {
        if let Err(e) = cfg.validate() {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Invalid docker_config: {}", e),
                Some("ValidationError"),
            );
        }
    }

    // Check docker availability for docker backends
    if is_docker {
        if let Err(e) = tama_core::installations::docker::docker_available().await {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Docker is not available: {}", e),
                Some("ValidationError"),
            );
        }
    }

    // Parse backend_type string to InstallationType enum
    let backend_type = match tama_core::installations::InstallationType::from_str(&req.backend_type)
    {
        Ok(bt) => bt,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Invalid backend type: {}", e),
                Some("ValidationError"),
            );
        }
    };

    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool);

    let info = tama_core::installations::InstallationInfo {
        name: req.name.clone(),
        backend_type: backend_type.clone(),
        version: req.version.clone(),
        // For docker backends, use the image as the path
        path: if is_docker {
            std::path::PathBuf::from(
                req.docker_config
                    .as_ref()
                    .map(|c| c.image.clone())
                    .unwrap_or_default(),
            )
        } else {
            std::path::PathBuf::new()
        },
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        gpu_variant: req.gpu_variant,
        source: None, // docker backends and non-docker direct registration have no source
        docker_config: req.docker_config,
    };

    let add_result = mgr.add_installation(&info).await;

    match add_result {
        Ok(()) => {
            let response = RegisterBackendResponse::from(info);
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to register backend: {}", e),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tama_core::config::Config;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    fn test_web_state() -> crate::web_types::WebState {
        crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            db_pool: tama_test_support::test_dummy_pool(),
        }
    }

    /// POST /tama/v1/backends with docker type but no docker_config → 400.
    #[tokio::test]
    async fn test_register_docker_without_config_returns_400() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "my_docker_backend",
                    "backend_type": "docker",
                    "version": "1.0.0"
                })
                .to_string(),
            ))
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "docker type without docker_config should return 400"
        );

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");

        assert_eq!(
            json["error"]["message"],
            "docker backend requires docker_config"
        );
    }

    /// POST /tama/v1/backends with non-docker type and docker_config → 400.
    #[tokio::test]
    async fn test_register_non_docker_with_config_returns_400() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "my_backend",
                    "backend_type": "llama_cpp",
                    "version": "1.0.0",
                    "docker_config": {
                        "image": "test:latest",
                        "model_mount": {
                            "host_path": "/models",
                            "container_path": "/models"
                        }
                    }
                })
                .to_string(),
            ))
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "non-docker type with docker_config should return 400"
        );

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");

        assert_eq!(
            json["error"]["message"],
            "docker_config only valid for docker backend type"
        );
    }

    /// POST /tama/v1/backends with empty name → 400.
    #[tokio::test]
    async fn test_register_empty_name_returns_400() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "",
                    "backend_type": "docker",
                    "version": "1.0.0",
                    "docker_config": {
                        "image": "test:latest",
                        "model_mount": {
                            "host_path": "/models",
                            "container_path": "/models"
                        }
                    }
                })
                .to_string(),
            ))
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "empty name should return 400"
        );
    }

    /// POST /tama/v1/backends with invalid docker_config (non-absolute path) → 400.
    #[tokio::test]
    async fn test_register_invalid_docker_config_returns_400() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/backends")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "my_docker_backend",
                    "backend_type": "docker",
                    "version": "1.0.0",
                    "docker_config": {
                        "image": "test:latest",
                        "model_mount": {
                            "host_path": "/models",
                            "container_path": "models"
                        }
                    }
                })
                .to_string(),
            ))
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "invalid docker_config should return 400"
        );

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");

        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("absolute path"));
    }
}
