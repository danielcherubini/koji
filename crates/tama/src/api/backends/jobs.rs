use axum::extract::{Extension, Path, State};
use axum::response::sse::Event;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Sse},
    Json,
};
use futures_util::Stream;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::types::*;
use crate::api::error::error_response;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// GET /tama/v1/backends/jobs/:id
pub async fn get_job(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let jobs = match web_state.jobs.as_ref() {
        Some(j) => j,
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Job manager not available",
                None,
            );
        }
    };
    let job = match jobs.get(&job_id).await {
        Some(j) => j,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                "Job not found",
                Some("NotFoundError"),
            );
        }
    };

    let (state, log_head, log_tail, dropped) = tokio::join!(
        job.state.read(),
        job.log_head.read(),
        job.log_tail.read(),
        async { job.log_dropped.load(Ordering::Relaxed) }
    );

    let mut log: Vec<String> = log_head.iter().cloned().collect();
    if dropped > 0 && !log_tail.is_empty() {
        log.push(format!("[... {} lines skipped ...]", dropped));
    }
    log.extend(log_tail.iter().cloned());

    Json(JobSnapshotDto {
        id: job.id.clone(),
        kind: match job.kind {
            crate::web_types::JobKind::Install => "install".to_string(),
            crate::web_types::JobKind::Update => "update".to_string(),
            crate::web_types::JobKind::Restore => "restore".to_string(),
            crate::web_types::JobKind::Benchmark => "benchmark".to_string(),
        },
        status: state.status,
        backend_type: job
            .backend_type
            .as_ref()
            .map(|b| b.to_string())
            .unwrap_or_default(),
        started_at: state.started_at,
        finished_at: state.finished_at,
        error: state.error.clone(),
        log,
    })
    .into_response()
}

/// GET /tama/v1/backends/jobs/:id/events
pub async fn job_events_sse(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(job_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let jobs = web_state
        .jobs
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let job = jobs.get(&job_id).await.ok_or(StatusCode::NOT_FOUND)?;

    let stream = crate::api::sse::job_event_stream(job);
    Ok(Sse::new(stream))
}

#[cfg(test)]
mod tests {
    use crate::api::error::tests::assert_error_shape;
    use crate::web_types::WebState;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tama_core::config::Config;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    fn test_web_state() -> WebState {
        WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            repository: None,
        }
    }

    #[tokio::test]
    async fn test_get_job_not_found_error_shape() {
        let config = Config::default();
        let state = Arc::new(ProxyState::new(config, None));
        let web_state = Arc::new(test_web_state());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/backends/jobs/nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = router.oneshot(req).await.expect("request should complete");

        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "get_job should return 404 for non-existent job"
        );

        let detail = assert_error_shape(resp).await;
        assert_eq!(
            detail.r#type,
            Some("NotFoundError".to_string()),
            "404 error should have NotFoundError type"
        );
    }
}
