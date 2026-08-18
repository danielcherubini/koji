//! Whole-repo `hf` CLI pull endpoints (safetensors / transformers wizard).
//!
//! Thin handlers over the public `ProxyState` delegates
//! (`start_repo_pull` / `get_repo_pull_status` / `cancel_repo_pull`) —
//! no direct access to `tama_core` internals (per ADR-0007).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::api::error::error_response;
use tama_core::proxy::tama_handlers::OkResponse;
use tama_core::proxy::{ProxyState, RepoPullError};

// ── DTO types ────────────────────────────────────────────────────────────────

/// Request body for `POST /tama/v1/pulls/repo`.
#[derive(serde::Deserialize)]
pub struct RepoPullStartBody {
    /// Hugging Face repo id (e.g. `owner/repo`).
    pub repo_id: String,
    /// Pre-created stub model row, if the wizard created one before starting.
    #[serde(default)]
    pub model_id: Option<u32>,
}

/// Response for a successful `POST /tama/v1/pulls/repo`.
#[derive(serde::Serialize)]
pub struct RepoPullStartResponse {
    /// Job id to poll / cancel with.
    pub job_id: String,
    /// Always `"running"` — the job was just created.
    pub status: String,
    /// Expected total size in bytes, if known at start.
    pub total_bytes: Option<u64>,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// POST /tama/v1/pulls/repo — start a whole-repo `hf` CLI pull (executed on
/// the pull host — `proxy.pull_backend`'s tamad — per ADR-0010).
///
/// `model_id` is the pre-created stub model row (the wizard creates it before
/// starting so completion can update the row). Errors map to the canonical
/// shape: invalid repo id / repo not found / no pull host configured / tamad
/// unreachable → 422 `ValidationError` or 502 `UpstreamError`; duplicate →
/// 409 `ConflictError`.
pub async fn start_repo_pull(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<RepoPullStartBody>,
) -> axum::response::Response {
    let model_id = body.model_id.map(|id| id as i64);
    match state.start_repo_pull(&body.repo_id, model_id).await {
        Ok(start) => (
            StatusCode::OK,
            Json(RepoPullStartResponse {
                job_id: start.job_id,
                status: "running".to_string(),
                total_bytes: start.total_bytes,
            }),
        )
            .into_response(),
        Err(e) => {
            let (status, error_type, message) = match &e {
                RepoPullError::InvalidRepoId(_) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "ValidationError",
                    e.to_string(),
                ),
                RepoPullError::DuplicatePull => (
                    StatusCode::CONFLICT,
                    "ConflictError",
                    format!("A repo pull for '{}' is already running", body.repo_id),
                ),
                RepoPullError::RepoNotFound(_) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "ValidationError",
                    e.to_string(),
                ),
                RepoPullError::Upstream(_) => {
                    (StatusCode::BAD_GATEWAY, "UpstreamError", e.to_string())
                }
            };
            error_response(status, message, Some(error_type))
        }
    }
}

/// GET /tama/v1/pulls/repo/:job_id — live status of a whole-repo pull job.
///
/// `bytes_done` is computed server-side (inside `tama_core`) so the client
/// never needs the destination path. Unknown job id → 404 `NotFoundError`.
pub async fn get_repo_pull(
    State(state): State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> axum::response::Response {
    match state.get_repo_pull_status(&job_id).await {
        Some(dto) => (StatusCode::OK, Json(dto)).into_response(),
        None => error_response(
            StatusCode::NOT_FOUND,
            format!("Repo pull '{job_id}' not found"),
            Some("NotFoundError"),
        ),
    }
}

/// DELETE /tama/v1/pulls/repo/:job_id — cancel + kill a running whole-repo pull.
///
/// Unknown job id → 404 `NotFoundError`; already terminal → 409
/// `ConflictError`; success → 200 `{"ok": true}`.
pub async fn delete_repo_pull(
    State(state): State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> axum::response::Response {
    match state.cancel_repo_pull(&job_id).await {
        Ok(()) => (StatusCode::OK, Json(OkResponse::OK)).into_response(),
        Err(msg) if msg == "not found" => error_response(
            StatusCode::NOT_FOUND,
            format!("Repo pull '{job_id}' not found"),
            Some("NotFoundError"),
        ),
        Err(_) => error_response(
            StatusCode::CONFLICT,
            format!("Repo pull '{job_id}' already finished"),
            Some("ConflictError"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::tests::assert_error_shape;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// Build a test router with default config (no DB dir) — mirrors the
    /// `hf.rs` test setup.
    fn test_router() -> axum::Router<()> {
        let config = tama_core::config::Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            None,
            tama_test_support::test_dummy_pool(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: tama_test_support::test_dummy_pool(),
        });

        crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()))
    }

    /// POST /tama/v1/pulls/repo — ADR-0010: with no pull host
    /// (`proxy.pull_backend`) configured, start fails loudly with 502
    /// UpstreamError. The proxy never downloads locally (and there is no
    /// local `hf` binary check anymore — execution is on the pull host).
    #[tokio::test]
    async fn test_start_repo_pull_no_pull_host_502() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/models/foo/bar/revision/main",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": []
                })),
            )
            .mount(&server)
            .await;
        std::env::set_var("HF_ENDPOINT", server.uri());

        let router = test_router();

        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/pulls/repo")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({ "repo_id": "foo/bar" }).to_string(),
            ))
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");
        std::env::remove_var("HF_ENDPOINT");

        assert_eq!(
            resp.status(),
            StatusCode::BAD_GATEWAY,
            "no pull host should return 502"
        );

        let detail = assert_error_shape(resp).await;
        assert_eq!(
            detail.r#type,
            Some("UpstreamError".to_string()),
            "no pull host should return UpstreamError type"
        );
        assert!(
            detail.message.contains("no pull host configured"),
            "message should name the fix: {}",
            detail.message
        );
    }

    /// GET /tama/v1/pulls/repo/:job_id — unknown job id → 404 NotFoundError
    /// (no network involved).
    #[tokio::test]
    async fn test_get_repo_pull_unknown_404() {
        let router = test_router();

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/pulls/repo/hfrepo-does-not-exist")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");

        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "unknown job id should return 404"
        );

        let detail = assert_error_shape(resp).await;
        assert_eq!(
            detail.r#type,
            Some("NotFoundError".to_string()),
            "unknown job id should return NotFoundError type"
        );
    }

    /// DELETE /tama/v1/pulls/repo/:job_id — unknown job id → 404 NotFoundError.
    #[tokio::test]
    async fn test_delete_repo_pull_unknown_404() {
        let router = test_router();

        let req = Request::builder()
            .method("DELETE")
            .uri("/tama/v1/pulls/repo/hfrepo-does-not-exist")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");

        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "unknown job id should return 404"
        );

        let detail = assert_error_shape(resp).await;
        assert_eq!(
            detail.r#type,
            Some("NotFoundError".to_string()),
            "unknown job id should return NotFoundError type"
        );
    }
}
