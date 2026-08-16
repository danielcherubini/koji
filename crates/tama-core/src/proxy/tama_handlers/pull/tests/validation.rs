use axum::{
    body::Body,
    http::{Method, Request},
};
use tower::ServiceExt;

use super::helpers::{create_test_state, mount_listing, pull_router, ENV_GUARD};
use crate::proxy::tama_handlers::types::max_concurrent_pulls;

const PULLS_ROUTE: &str = "/tama/v1/pulls";
const CT_JSON: &str = "application/json";

/// Malformed JSON body returns 400 (axum JsonSyntaxError)
#[tokio::test]
async fn test_pull_model_malformed_json_returns_400() {
    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(r#"{"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
}

/// Missing repo_id returns 422 (JsonDataError)
#[tokio::test]
async fn test_pull_model_missing_repo_id_returns_422() {
    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from("{}"))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 422);
}

/// Too many filenames (> max_concurrent_pulls) returns 400
#[tokio::test]
async fn test_pull_model_too_many_files_returns_400() {
    // Isolate TAMA_MAX_CONCURRENT_PULLS so the test uses the default limit.
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::remove_var("TAMA_MAX_CONCURRENT_PULLS");
    }

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let max = max_concurrent_pulls();
    let filenames: Vec<String> = (1..=max as u32 + 1)
        .map(|i| format!("f{}.gguf", i))
        .collect();
    let body = serde_json::json!({
        "repo_id": "test/repo",
        "filenames": filenames
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains(&format!("Too many files requested. Maximum is {}.", max)),
        "Response: {}",
        text
    );
}

/// Too many quants (> max_concurrent_pulls) returns 400
#[tokio::test]
async fn test_pull_model_too_many_quants_returns_400() {
    // Isolate TAMA_MAX_CONCURRENT_PULLS so the test uses the default limit.
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::remove_var("TAMA_MAX_CONCURRENT_PULLS");
    }

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let max = max_concurrent_pulls();
    let quants: Vec<serde_json::Value> = (1..=max as u32 + 1)
        .map(|i| {
            serde_json::json!({
                "filename": format!("f{}.gguf", i),
                "quant": "Q4_K_M"
            })
        })
        .collect();
    let body = serde_json::json!({
        "repo_id": "test/repo",
        "quants": quants
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("Too many quants requested"),
        "Response: {}",
        text
    );
}

// ── Listing-backed validation tests (wiremock) ──────────────────────────

/// Unknown filename in request returns 400 with ValidationError.
#[tokio::test]
async fn test_pull_model_unknown_filename_returns_400() {
    let server = wiremock::MockServer::start().await;
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    mount_listing(&server, "test/repo", &["repo-Q4_K_M.gguf"]).await;

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let body = serde_json::json!({
        "repo_id": "test/repo",
        "filenames": ["nope.gguf"]
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);

    std::env::remove_var("HF_ENDPOINT");

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "ValidationError");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("is not a valid GGUF file for repo 'test/repo'"),
        "Response: {}",
        json
    );
}

/// Duplicate filenames in request returns 400 with ValidationError.
#[tokio::test]
async fn test_pull_model_duplicate_filename_returns_400() {
    let server = wiremock::MockServer::start().await;
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    mount_listing(&server, "test/repo", &["repo-Q4_K_M.gguf"]).await;

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let body = serde_json::json!({
        "repo_id": "test/repo",
        "filenames": ["repo-Q4_K_M.gguf", "repo-Q4_K_M.gguf"]
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);

    std::env::remove_var("HF_ENDPOINT");

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "ValidationError");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Duplicate filename"),
        "Response: {}",
        json
    );
}

/// Missing quant with no filenames returns 422 with available_quants.
#[tokio::test]
async fn test_pull_model_missing_quant_returns_422_with_available() {
    let server = wiremock::MockServer::start().await;
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    mount_listing(&server, "test/repo", &["repo-Q4_K_M.gguf"]).await;

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    // Send repo_id but no filenames/quant — triggers listing fetch
    // which returns available quants in the error response.
    let body = serde_json::json!({
        "repo_id": "test/repo"
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 422);

    std::env::remove_var("HF_ENDPOINT");

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "ValidationError");
    // available_quants is at top level (sibling of "error")
    assert!(json["available_quants"].is_array());
    let available: Vec<&serde_json::Value> = json["available_quants"]
        .as_array()
        .unwrap()
        .iter()
        .collect();
    assert_eq!(available.len(), 1);
    assert_eq!(available[0]["filename"], "repo-Q4_K_M.gguf");
}

/// Unknown quant returns 422 with available_quants.
#[tokio::test]
async fn test_pull_model_unknown_quant_returns_422() {
    let server = wiremock::MockServer::start().await;
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    mount_listing(&server, "test/repo", &["repo-Q4_K_M.gguf"]).await;

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let body = serde_json::json!({
        "repo_id": "test/repo",
        "quant": "Q9_XX"
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 422);

    std::env::remove_var("HF_ENDPOINT");

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "ValidationError");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Quant 'Q9_XX' not found in repo 'test/repo'"),
        "Response: {}",
        json
    );
}

/// Listing fetch failure returns 502 with UpstreamError.
#[tokio::test]
async fn test_pull_model_listing_failure_returns_502() {
    let server = wiremock::MockServer::start().await;
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    // Mount a raw 500 response instead of a listing.
    // hf-hub calls /api/models/{repo_id}/revision/{revision}
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/api/models/test/repo/revision/main",
        ))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state);

    let body = serde_json::json!({
        "repo_id": "test/repo",
        "filenames": ["x.gguf"]
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 502);

    std::env::remove_var("HF_ENDPOINT");

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["error"]["type"], "UpstreamError");
}

/// Happy path: listing validation passes, job is created and enqueued.
#[tokio::test]
async fn test_pull_model_enqueues_job_and_returns_pending() {
    let server = wiremock::MockServer::start().await;
    {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", server.uri());
    }

    mount_listing(&server, "test/repo", &["repo-Q4_K_M.gguf"]).await;

    let (state, _guard) = create_test_state().await;
    let app = pull_router(state.clone());

    let body = serde_json::json!({
        "repo_id": "test/repo",
        "filenames": ["repo-Q4_K_M.gguf"]
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(PULLS_ROUTE)
        .header("content-type", CT_JSON)
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    std::env::remove_var("HF_ENDPOINT");

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["filename"], "repo-Q4_K_M.gguf");
    assert_eq!(entry["status"], "pending");

    // Extract job_id from the response for further assertions.
    let job_id = entry["job_id"].as_str().unwrap().to_string();

    // Verify the pull job exists in memory with Pending status.
    assert!(
        state.pull.pull_jobs.read().await.contains_key(&job_id),
        "pull_jobs should contain {}",
        job_id
    );
    let jobs = state.pull.pull_jobs.read().await;
    let job = jobs.get(&job_id).unwrap();
    use crate::proxy::pull_jobs::PullJobStatus;
    assert_eq!(job.status, PullJobStatus::Pending);
    assert_eq!(job.repo_id, "test/repo");
    assert_eq!(job.filename, "repo-Q4_K_M.gguf");

    // Verify the DB queue row exists.
    let svc = state.pull_queue().as_ref().unwrap();
    let db_row = svc.get_queue_item(&job_id).await.unwrap();
    assert!(
        db_row.is_some(),
        "pull_queue DB row should exist for job {}",
        job_id
    );
}
