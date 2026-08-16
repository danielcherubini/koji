//! Backend log file reading endpoints.
//!
//! - GET /tama/v1/logs — returns grouped logs (proxied from tama-core)
//! - GET /tama/v1/logs/:backend — returns last N lines of a backend's log file
use crate::api::error::error_response;
use tama_core::proxy::ProxyState;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

/// Maximum number of lines to return (clamp for the `lines` query parameter).
pub const MAX_LINES: usize = 10_000;

/// Query parameters for GET /tama/v1/logs/:backend
#[derive(Deserialize)]
pub struct BackendLogsQuery {
    /// Number of lines to return (default: 200)
    #[serde(default = "default_lines")]
    pub lines: usize,
}

fn default_lines() -> usize {
    200
}

/// Validate a backend name for use in log file paths.
pub fn is_valid_backend_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

/// GET /tama/v1/logs/:backend — return the last N lines of a backend's log file.
pub async fn get_backend_logs(
    State(state): State<Arc<ProxyState>>,
    Path(backend): Path<String>,
    Query(query): Query<BackendLogsQuery>,
) -> impl IntoResponse {
    let dir = match state.with_config(|c| c.logs_dir()).await {
        Ok(d) => d,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    if !is_valid_backend_name(&backend) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid backend name",
            Some("ValidationError"),
        );
    }

    let path = dir.join(format!("{}.log", backend));

    if !path.exists() {
        return error_response(
            StatusCode::NOT_FOUND,
            format!("No logs found for '{}'", backend),
            Some("NotFoundError"),
        );
    }

    let n = query.lines.min(MAX_LINES);
    let path_clone = path.clone();
    let lines =
        tokio::task::spawn_blocking(move || tama_core::logging::tail_lines(&path_clone, n)).await;

    match lines {
        Ok(Ok(result)) => Json(serde_json::json!({ "lines": result })).into_response(),
        Ok(Err(e)) => {
            tracing::warn!("Failed to read backend log {}: {}", path.display(), e);
            Json(serde_json::json!({ "lines": Vec::<String>::new() })).into_response()
        }
        Err(join_err) => {
            tracing::warn!(
                "Failed to read backend log {} (spawn_blocking): {}",
                path.display(),
                join_err
            );
            Json(serde_json::json!({ "lines": Vec::<String>::new() })).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tama_core::config::Config;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    /// `is_valid_backend_name` rejects names with dots.
    #[test]
    fn test_is_valid_backend_name_rejects_invalid() {
        assert!(!is_valid_backend_name("foo..bar"));
        assert!(!is_valid_backend_name(""));
        assert!(is_valid_backend_name("llama_cpp"));
        assert!(is_valid_backend_name("my-backend"));
    }

    /// GET /tama/v1/logs/:backend with invalid name returns 400.
    #[tokio::test]
    async fn test_get_backend_logs_rejects_invalid_name() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_core::db::pool::test_dummy_pool(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_core::db::pool::test_dummy_pool(),
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/logs/foo..bar")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "invalid backend name should return 400"
        );
    }

    /// GET /tama/v1/logs/:backend with a valid name but missing log file returns 404.
    #[tokio::test]
    async fn test_get_backend_logs_missing_file_404() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let config = Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            Some(tmp_dir.path().to_path_buf()),
            tama_core::db::pool::test_dummy_pool(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_core::db::pool::test_dummy_pool(),
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // Valid name but no log file exists.
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/logs/nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "missing log file should return 404"
        );
    }

    /// GET /tama/v1/logs/:backend with ?lines=3 on a 5-line file returns 3 entries.
    #[tokio::test]
    async fn test_get_backend_logs_returns_tail() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");

        // Write logs dir path to config so Config::logs_dir() returns it.
        let logs_dir = tmp_dir.path().join("logs");
        std::fs::create_dir(&logs_dir).unwrap();
        let log_path = logs_dir.join("mybackend.log");
        std::fs::write(&log_path, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let mut config = Config::default();
        config.general.logs_dir = Some(logs_dir.to_string_lossy().to_string());
        let state = Arc::new(ProxyState::new(
            config,
            Some(tmp_dir.path().to_path_buf()),
            tama_core::db::pool::test_dummy_pool(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_core::db::pool::test_dummy_pool(),
        });

        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // Request last 3 lines.
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/logs/mybackend?lines=3")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body should be readable");
        let json: serde_json::Value =
            serde_json::from_slice(&body_str).expect("body should be valid JSON");

        let lines: Vec<&str> = json["lines"]
            .as_array()
            .expect("lines should be an array")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line3");
        assert_eq!(lines[1], "line4");
        assert_eq!(lines[2], "line5");
    }
}
