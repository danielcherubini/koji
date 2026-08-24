use super::*;

use crate::proxy::types::LatestInferenceStats;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::http::request::Parts;
use axum::http::StatusCode;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::proxy::{BackendState, ProxyState};

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a `BackendState::Ready` for direct insertion into `state.models()`.
fn make_ready_state(
    backend_url: String,
    pid: u32,
    failures: u32,
    failure_timestamp: Option<SystemTime>,
) -> BackendState {
    BackendState::Ready {
        model_name: "test-model".to_string(),
        backend: "llama_cpp".to_string(),
        backend_pid: pid,
        backend_url,
        load_time: SystemTime::now(),
        last_accessed: Instant::now(),
        consecutive_failures: Arc::new(AtomicU32::new(failures)),
        failure_timestamp,
        is_docker: false,
        restart_count: 0,
    }
}

/// POST variant of the parent's GET-only `make_parts` helper.
fn make_post_parts(path: &str) -> Parts {
    let req = axum::http::Request::post(path).body(()).unwrap();
    let (parts, _) = req.into_parts();
    parts
}

/// Spawn a harmless long-lived process; returns `(child, pid)` so tests can
/// reap it. Needed wherever a live pid is required and `forward_request` may
/// signal it — never use `std::process::id()` in those cases (SIGTERM would
/// kill the test runner).
fn spawn_live_pid() -> (std::process::Child, u32) {
    let child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    (child, pid)
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn test_state() -> Arc<ProxyState> {
    Arc::new(ProxyState::new(
        crate::config::Config::default(),
        None,
        crate::db::pool::test_dummy_pool(),
    ))
}

/// Register a live `ready` wire row for `model_id` on the state's tamad pool
/// (plan-193 T4: `forward_request` reads the endpoint from rows, so tests
/// must seed the live ProcessInfo the same way the tamad stream would). The
/// mirror entry (circuit breaker, failure bookkeeping) is inserted separately.
async fn seed_live_row(state: &Arc<ProxyState>, model_id: &str, endpoint: &str) {
    use crate::tamad::pool::test_support::{handle_with_latest, stats_full};
    let proc = crate::tamad::ProcessInfo {
        model_name: model_id.to_string(),
        provider_name: "llama.cpp".to_string(),
        pid: 1,
        alive: true,
        endpoint_url: endpoint.to_string(),
        status: "ready".to_string(),
        desired: true,
        restart_count: 0,
        max_restarts: 3,
    };
    let stats = stats_full(1.5, vec![], vec![proc]);
    let pool = state.tamad_pool();
    pool.insert_raw_handle(
        "t1",
        Arc::new(handle_with_latest(Instant::now(), stats).await),
    )
    .await;
}

// ── Tests ───────────────────────────────────────────────────────────────

/// Crashed backend (nothing listening) surfaces as a connection error at
/// forward time → 502 + `BadGatewayError`, model removal, inference_stats
/// cleanup. (Plan-191 Task 10: the proxy never pings local pids — liveness
/// converged via connection errors + the reconciler mirror.)
#[tokio::test]
async fn test_forward_request_conn_error_returns_502_and_cleans_up() {
    let state = test_state();

    // Insert a model whose backend URL is not listening (simulates a crash;
    // the recorded pid is irrelevant — the proxy no longer inspects pids).
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state("http://127.0.0.1:1".into(), 99999999, 0, None),
    );
    seed_live_row(&state, "test-model", "http://127.0.0.1:1").await;

    // Pre-seed inference_stats so we can assert it's cleared.
    state
        .metrics
        .record_inference_stats("test-model", LatestInferenceStats::default());

    let resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        b"{}",
        Some("test-model"),
    )
    .await;

    let status = resp.status();
    let body = body_json(resp).await;

    // Status + error type + message
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"]["type"], "BadGatewayError");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Backend error"),);

    // Model removed from state.models()
    assert!(state
        .registry
        .models
        .read()
        .await
        .get("test-model")
        .is_none());

    // Inference stats entry cleared
    assert!(!state
        .metrics
        .inference_stats_snapshot()
        .contains_key("test-model"));
}

/// Connection-error path counts exactly one `failed_request` and one
/// `total_request` (no cooldown counter is involved — the model state is
/// removed immediately instead of accumulating failures).
#[tokio::test]
async fn test_forward_request_conn_error_counts_failed_and_removes_model() {
    let state = test_state();

    // Insert a model whose backend is not listening.
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state("http://127.0.0.1:1".into(), 99999999, 0, None),
    );
    seed_live_row(&state, "test-model", "http://127.0.0.1:1").await;

    let _resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        b"{}",
        Some("test-model"),
    )
    .await;

    // Connection-error path: total=1, successful=0, failed=1.
    assert_eq!(
        state
            .metrics
            .counters
            .total_requests
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .metrics
            .counters
            .successful_requests
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(
        state
            .metrics
            .counters
            .failed_requests
            .load(Ordering::Relaxed),
        1
    );

    // Model state removed immediately (no breaker accumulation).
    assert!(state
        .registry
        .models
        .read()
        .await
        .get("test-model")
        .is_none());
}

/// Model not loaded (no entry in `state.models()`) → 502 + `BackendUrlError`.
#[tokio::test]
async fn test_forward_request_model_not_loaded_returns_502() {
    let state = test_state();
    // Empty state — no models inserted.

    let resp = forward_request(
        &state,
        "nonexistent-model",
        &make_post_parts("/v1/chat/completions"),
        b"{}",
        Some("nonexistent-model"),
    )
    .await;

    let status = resp.status();
    let body = body_json(resp).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"]["type"], "BackendUrlError");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("is not loaded"));
}

// ── Circuit Breaker Tests ───────────────────────────────────────────────

/// Circuit breaker cooldown active (failures == threshold, recent timestamp)
/// → 503 `ServiceUnavailableError` with "in cooldown", model stays loaded,
/// process untouched — no SIGTERM.
#[tokio::test]
async fn test_forward_request_circuit_breaker_cooldown_returns_503_without_unload() {
    let (mut child, pid) = spawn_live_pid();
    let state = test_state();

    let threshold = state.config.read().await.proxy.circuit_breaker_threshold;

    // Insert a model with failures == threshold and a fresh timestamp → cooldown active.
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(
            "http://127.0.0.1:1".into(),
            pid,
            threshold,
            Some(SystemTime::now()),
        ),
    );

    let resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        b"{}",
        Some("test-model"),
    )
    .await;

    let status = resp.status();
    let body = body_json(resp).await;

    // Status + error type + cooldown message
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["type"], "ServiceUnavailableError");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("in cooldown"));

    // Model STILL in state.models() — no unload happened.
    assert!(state
        .registry
        .models
        .read()
        .await
        .get("test-model")
        .is_some());

    // Process still alive — proves no SIGTERM was sent (the proxy must
    // never signal backend pids; they belong to the tamad).
    assert!(
        child.try_wait().expect("poll fixture child").is_none(),
        "fixture process must still be running"
    );

    // failed_requests NOT incremented (short-circuit bypasses failure counters).
    assert_eq!(
        state
            .metrics
            .counters
            .failed_requests
            .load(Ordering::Relaxed),
        0
    );

    child.kill().unwrap();
    let _ = child.wait();
}

/// Circuit breaker trip after cooldown elapsed (failures == threshold,
/// Failures == threshold (with a stale timestamp) → SIGTERMs the backend
/// on its tamad, removes the model, bumps `models_unloaded`, returns a
/// distinct "currently unavailable" 503.
#[tokio::test]
async fn test_forward_request_circuit_breaker_trips_and_unloads_after_cooldown() {
    let (mut child, pid) = spawn_live_pid();
    let state = test_state();

    let threshold = state.config.read().await.proxy.circuit_breaker_threshold;

    // Insert a model with failures == threshold and a timestamp older than the
    // 60s default cooldown → can_reload is true → trip.
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(
            "http://127.0.0.1:1".into(),
            pid,
            threshold,
            Some(SystemTime::now() - Duration::from_secs(120)),
        ),
    );
    seed_live_row(&state, "test-model", "http://127.0.0.1:1").await;

    let resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        b"{}",
        Some("test-model"),
    )
    .await;

    let status = resp.status();
    let body = body_json(resp).await;

    // Status + error type + distinct trip message
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["type"], "ServiceUnavailableError");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("currently unavailable"));

    // Model removed from state.models() by unload_model. The physical
    // kill happens on the model's provider tamad (plan-191 Task 5) — the
    // proxy only clears its local mirror (this unit test has no tamad,
    // so the best-effort RPC is skipped and the mirror is cleared
    // anyway).
    assert!(state
        .registry
        .models
        .read()
        .await
        .get("test-model")
        .is_none());

    // Clean up the fixture process (the proxy must not signal it — pids
    // in the mirror are owned by the tamad, not the proxy).
    let _ = child.kill();
    let _ = child.wait();

    // models_unloaded incremented exactly once.
    assert_eq!(
        state
            .metrics
            .counters
            .models_unloaded
            .load(Ordering::Relaxed),
        1
    );
}

/// Failures below threshold + a backend 5xx (the backend answered) → the
/// response passes through (500), the model stays loaded, and
/// consecutive_failures increments up to the threshold.
///
/// (Plan-191 Task 10: a *connection-level* error instead cleans the model up
/// immediately — see `test_forward_request_conn_error_counts_failed_and_removes_model`.)
#[tokio::test]
async fn test_forward_request_below_threshold_passes_circuit_breaker() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("backend exploded"))
        .mount(&server)
        .await;

    let state = test_state();

    let threshold = state.config.read().await.proxy.circuit_breaker_threshold;

    // Insert a model with failures == threshold - 1 (below the trip point).
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(
            server.uri(),
            std::process::id(), // safe: the below-threshold path never signals pid
            threshold - 1,
            Some(SystemTime::now()),
        ),
    );
    seed_live_row(&state, "test-model", server.uri().as_str()).await;

    let resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        b"{}",
        Some("test-model"),
    )
    .await;

    let status = resp.status();

    // 500 passes through — NOT 503, proving the breaker did NOT short-circuit.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    // Model stays loaded (a 5xx is a transient backend failure, not a crash).
    assert!(state
        .registry
        .models
        .read()
        .await
        .get("test-model")
        .is_some());

    // consecutive_failures incremented to threshold by the 5xx-response path.
    assert_eq!(
        state
            .get_model_state("test-model")
            .await
            .unwrap()
            .consecutive_failures()
            .unwrap()
            .load(Ordering::Relaxed),
        threshold,
    );
}

// ── Metrics and Proxied-Response Tests (wiremock backend) ───────────────

/// Success path: mock returns 200 → successful_requests incremented,
/// consecutive_failures reset to 0, model name rewritten in response body.
#[tokio::test]
async fn test_forward_request_success_increments_metrics_and_rewrites_model() {
    let server = MockServer::start().await;

    // Mock the backend to return a valid chat completion response.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "cmpl-1",
            "model": "backend-model",
            "choices": [{"message": {"role": "assistant", "content": "hi"}}]
        })))
        .mount(&server)
        .await;

    let state = test_state();

    // Insert a model with pre-existing failure count of 2 — success path should reset to 0.
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(server.uri(), std::process::id(), 2, None),
    );
    seed_live_row(&state, "test-model", server.uri().as_str()).await;

    let resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        br#"{"model":"test-model","messages":[]}"#,
        Some("public-name"),
    )
    .await;

    let status = resp.status();
    let json = body_json(resp).await;

    // Status 200 — success proxied through.
    assert_eq!(status, StatusCode::OK);

    // Model name rewritten: backend-model → public-name.
    assert_eq!(json["model"].as_str().unwrap(), "public-name");

    // Response content preserved.
    assert_eq!(
        json["choices"][0]["message"]["content"].as_str().unwrap(),
        "hi"
    );

    // Metrics: total_requests == 1 (fresh state), successful_requests == 1,
    // failed_requests == 0.
    assert_eq!(
        state
            .metrics
            .counters
            .total_requests
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .metrics
            .counters
            .successful_requests
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .metrics
            .counters
            .failed_requests
            .load(Ordering::Relaxed),
        0
    );

    // consecutive_failures reset from 2 → 0 on success.
    assert_eq!(
        state
            .get_model_state("test-model")
            .await
            .unwrap()
            .consecutive_failures()
            .unwrap()
            .load(Ordering::Relaxed),
        0
    );
}

/// Backend returns 500 → failed_requests incremented, consecutive_failures +1,
/// failure_timestamp set, status proxied through unchanged. Model stays loaded.
#[tokio::test]
async fn test_forward_request_backend_500_increments_failures_and_sets_timestamp() {
    let server = MockServer::start().await;

    // Mock the backend to return a 500 error.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let state = test_state();

    // Insert a model with no pre-existing failures.
    state.registry.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(server.uri(), std::process::id(), 0, None),
    );
    seed_live_row(&state, "test-model", server.uri().as_str()).await;

    let resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        br#"{"model":"test-model","messages":[]}"#,
        Some("test-model"),
    )
    .await;

    let status = resp.status();

    // Status 500 — proxied through unchanged.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    // Metrics: failed_requests == 1, successful_requests == 0.
    assert_eq!(
        state
            .metrics
            .counters
            .failed_requests
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        state
            .metrics
            .counters
            .successful_requests
            .load(Ordering::Relaxed),
        0
    );

    // consecutive_failures incremented to 1.
    assert_eq!(
        state
            .get_model_state("test-model")
            .await
            .unwrap()
            .consecutive_failures()
            .unwrap()
            .load(Ordering::Relaxed),
        1
    );

    // failure_timestamp is now Some — the server-error path sets it.
    let model_state = state.get_model_state("test-model").await.unwrap();
    match model_state {
        BackendState::Ready {
            failure_timestamp, ..
        } => {
            assert!(
                failure_timestamp.is_some(),
                "failure_timestamp should be Some after a 5xx response"
            );
        }
        _ => panic!("expected BackendState::Ready, got {:?}", model_state),
    }

    // Model is STILL loaded — proxied 5xx with live pid never unloads.
    assert!(state
        .registry
        .models
        .read()
        .await
        .get("test-model")
        .is_some());
}

/// Alias rewrite: the request body's `model` field is replaced with the resolved
/// name before forwarding, and the backend receives the complete, valid JSON body.
///
/// Regression test for the content-length truncation bug: the rewritten body is a
/// different size than the original, so a stale forwarded content-length header
/// would truncate the body mid-string ("Unterminated string" errors on the backend).
#[tokio::test]
async fn test_forward_request_rewrites_model_in_request_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "cmpl-1",
            "model": "resolved-model",
            "choices": [{"message": {"role": "assistant", "content": "hi"}}]
        })))
        .mount(&server)
        .await;

    let state = test_state();

    state.registry.models.write().await.insert(
        "resolved-model".to_string(),
        make_ready_state(server.uri(), std::process::id(), 0, None),
    );
    seed_live_row(&state, "resolved-model", server.uri().as_str()).await;

    // Body padded large enough that a stale (smaller) content-length header
    // would visibly truncate it. "org/alias" is shorter than "resolved-model"
    // so the rewritten body grows.
    let long_content = "x".repeat(10_000);
    let original_body = serde_json::json!({
        "model": "org/alias",
        "messages": [{"role": "user", "content": long_content}]
    });
    let body_bytes = serde_json::to_vec(&original_body).unwrap();

    let resp = forward_request(
        &state,
        "resolved-model",
        &make_post_parts("/v1/chat/completions"),
        &body_bytes,
        Some("resolved-model"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);

    // The backend must have received exactly one request whose body is complete,
    // valid JSON with the model field rewritten to the resolved name.
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let sent_body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("forwarded body must be valid JSON");
    assert_eq!(sent_body["model"].as_str().unwrap(), "resolved-model");
    assert_eq!(
        sent_body["messages"][0]["content"].as_str().unwrap(),
        long_content
    );
}
