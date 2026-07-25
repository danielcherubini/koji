use std::sync::Arc;
use std::time::Duration;

use axum::{body::Body, http::Request};
use tower::ServiceExt;

use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::ProxyState;

use super::helpers::{create_test_state, pull_router};

/// Seed a pull job into the in-memory state map.
fn seed_job(state: &Arc<ProxyState>, job_id: &str, status: PullJobStatus) {
    state.pull_jobs.try_write().unwrap().insert(
        job_id.to_string(),
        PullJob {
            job_id: job_id.to_string(),
            repo_id: "test/repo".to_string(),
            filename: "repo-Q4_K_M.gguf".to_string(),
            status,
            bytes_pulled: 500,
            total_bytes: Some(1000),
            ..Default::default()
        },
    );
}

/// Test that GET /tama/v1/pulls/:job_id returns 404 for an unknown job.
#[tokio::test]
async fn test_get_pull_job_unknown_returns_404() {
    let (state, _tmp) = create_test_state();
    let app = pull_router(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/tama/v1/pulls/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["error"]["type"], "NotFoundError");
}

/// Test that GET /tama/v1/pulls/:job_id returns a snapshot for an existing job.
#[tokio::test]
async fn test_get_pull_job_returns_snapshot() {
    let (state, _tmp) = create_test_state();
    seed_job(&state, "pull-1", PullJobStatus::Running);

    let app = pull_router(state);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/tama/v1/pulls/pull-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(parsed["job_id"], "pull-1");
    assert_eq!(parsed["status"], "running");
    assert_eq!(parsed["repo_id"], "test/repo");
    assert_eq!(parsed["filename"], "repo-Q4_K_M.gguf");
    assert_eq!(parsed["bytes_pulled"], 500);
    assert_eq!(parsed["total_bytes"], 1000);
}

/// Test that the SSE stream emits progress then done events when a job transitions.
#[tokio::test]
async fn test_pull_job_stream_emits_progress_then_done() {
    let (state, _tmp) = create_test_state();
    seed_job(&state, "pull-sse", PullJobStatus::Pending);

    // Schedule status flip: Pending → Completed after 650ms.
    let state_clone = Arc::clone(&state);
    let job_id_for_flip = "pull-sse".to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(650)).await;
        let mut jobs = state_clone.pull_jobs.write().await;
        if let Some(job) = jobs.get_mut(&job_id_for_flip) {
            job.status = PullJobStatus::Completed;
        }
    });

    // GET the SSE stream.
    let app = pull_router(state);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/tama/v1/pulls/pull-sse/stream")
                .header("Accept", "text/event-stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Collect the full body with a generous timeout.
    let body = tokio::time::timeout(
        Duration::from_secs(10),
        axum::body::to_bytes(resp.into_body(), usize::MAX),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let text = String::from_utf8(body.to_vec()).unwrap();

    // The SSE stream should contain both event types.
    assert!(
        text.contains("event: progress"),
        "SSE body missing 'event: progress': {text}"
    );
    assert!(
        text.contains("event: done"),
        "SSE body missing 'event: done': {text}"
    );

    // Collect all data: lines and take the last one (the 'done' event).
    let all_data_lines: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("data:"))
        .map(|l| l.strip_prefix("data:").unwrap().trim())
        .collect();

    assert!(
        !all_data_lines.is_empty(),
        "no data: lines found in SSE response"
    );

    let last_data = all_data_lines
        .last()
        .expect("at least one data line exists");
    let parsed: serde_json::Value =
        serde_json::from_str(last_data).expect("last data line is valid JSON");
    assert_eq!(parsed["status"], "completed", "done event status mismatch");
}

/// Test that the SSE stream for an unknown job closes without emitting events.
#[tokio::test]
async fn test_pull_job_stream_unknown_job_closes_without_events() {
    let (state, _tmp) = create_test_state();

    let app = pull_router(state);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/tama/v1/pulls/ghost/stream")
                .header("Accept", "text/event-stream")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Collect the body with a generous timeout.
    let body = tokio::time::timeout(
        Duration::from_secs(10),
        axum::body::to_bytes(resp.into_body(), usize::MAX),
    )
    .await
    .expect("SSE stream timed out")
    .unwrap();

    let text = String::from_utf8(body.to_vec()).unwrap();

    // Unknown job should close without any events.
    assert!(
        !text.contains("event: progress"),
        "unknown job stream should not emit 'event: progress': {text}"
    );
    assert!(
        !text.contains("event: done"),
        "unknown job stream should not emit 'event: done': {text}"
    );
}
