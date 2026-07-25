use super::*;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::http::request::Parts;
use axum::http::StatusCode;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::proxy::{BackendState, ProxyState};

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build a `BackendState::Ready` for direct insertion into `state.models`.
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
    Arc::new(ProxyState::new(crate::config::Config::default(), None))
}

// ── Tests ───────────────────────────────────────────────────────────────

/// Dead PID at request entry → 502 + `BackendCrashedError`, model removal, inference_stats cleanup.
#[tokio::test]
async fn test_forward_request_dead_pid_returns_502_and_cleans_up() {
    let state = test_state();

    // Insert a model with a definitely-dead PID (99999999 is the repo convention).
    state.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state("http://127.0.0.1:1".into(), 99999999, 0, None),
    );

    // Pre-seed inference_stats so we can assert it's cleared.
    state.inference_stats.send_modify(|map| {
        map.insert("test-model".to_string(), LatestInferenceStats::default());
    });

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
    assert_eq!(body["error"]["type"], "BackendCrashedError");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("has crashed, reloading"));

    // Model removed from state.models
    assert!(state.models.read().await.get("test-model").is_none());

    // Inference stats entry cleared
    assert!(state.inference_stats.borrow().get("test-model").is_none());
}

/// Dead PID crash path increments only `total_requests`, not `failed_requests`.
#[tokio::test]
async fn test_forward_request_dead_pid_increments_only_total_requests() {
    let state = test_state();

    // Insert model with dead PID.
    state.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state("http://127.0.0.1:1".into(), 99999999, 0, None),
    );

    let _resp = forward_request(
        &state,
        "test-model",
        &make_post_parts("/v1/chat/completions"),
        b"{}",
        Some("test-model"),
    )
    .await;

    // Crash-detection short-circuit increments total_requests only.
    assert_eq!(state.metrics.total_requests.load(Ordering::Relaxed), 1);
    assert_eq!(state.metrics.successful_requests.load(Ordering::Relaxed), 0);
    assert_eq!(state.metrics.failed_requests.load(Ordering::Relaxed), 0);
}

/// Model not loaded (no entry in `state.models`) → 502 + `BackendUrlError`.
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
    state.models.write().await.insert(
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

    // Model STILL in state.models — no unload happened.
    assert!(state.models.read().await.get("test-model").is_some());

    // Process still alive — proves no SIGTERM was sent.
    assert!(crate::process::is_process_alive(pid));

    // failed_requests NOT incremented (short-circuit bypasses failure counters).
    assert_eq!(state.metrics.failed_requests.load(Ordering::Relaxed), 0);

    child.kill().unwrap();
    let _ = child.wait();
}

/// Circuit breaker trip after cooldown elapsed (failures == threshold,
/// stale timestamp) → SIGTERMs the backend, removes the model,
/// bumps `models_unloaded`, returns distinct "currently unavailable" 503.
#[tokio::test]
async fn test_forward_request_circuit_breaker_trips_and_unloads_after_cooldown() {
    let (mut child, pid) = spawn_live_pid();
    let state = test_state();

    let threshold = state.config.read().await.proxy.circuit_breaker_threshold;

    // Insert a model with failures == threshold and a timestamp older than the
    // 60s default cooldown → can_reload is true → trip.
    state.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(
            "http://127.0.0.1:1".into(),
            pid,
            threshold,
            Some(SystemTime::now() - Duration::from_secs(120)),
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

    // Status + error type + distinct trip message
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["type"], "ServiceUnavailableError");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("currently unavailable"));

    // Model removed from state.models by unload_model.
    assert!(state.models.read().await.get("test-model").is_none());

    // Process should be dead — SIGTERM/SIGKILL landed via unload_model.
    // Use child.try_wait() instead of is_process_alive() because some test
    // environments (tokio/nextest) have signal delivery quirks with libc::kill.
    let mut killed_by_test = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait().unwrap() {
            Some(_) => break, // Process exited (from unload_model's signals)
            None => {
                if std::time::Instant::now() >= deadline {
                    killed_by_test = true;
                    let _ = child.kill();
                    // Wait for the fallback kill to take effect
                    let _ = child.wait();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    assert!(
        !killed_by_test,
        "unload_model should have killed the process (it was still alive after 5s)"
    );

    // models_unloaded incremented exactly once.
    assert_eq!(state.metrics.models_unloaded.load(Ordering::Relaxed), 1);
}

/// Failures below threshold → request passes through to backend,
/// gets a transport error (502 BadGatewayError), and consecutive_failures
/// increments to the threshold.
#[tokio::test]
async fn test_forward_request_below_threshold_passes_circuit_breaker() {
    let state = test_state();

    let threshold = state.config.read().await.proxy.circuit_breaker_threshold;

    // Insert a model with failures == threshold - 1 (below the trip point).
    state.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(
            "http://127.0.0.1:1".into(), // nothing listening → transport error
            std::process::id(),          // safe: below-threshold path never signals pid
            threshold - 1,
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

    // 502 (transport error) — NOT 503, proving the breaker did NOT short-circuit.
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"]["type"], "BadGatewayError");

    // consecutive_failures incremented to threshold by the transport-error path.
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
    state.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(server.uri(), std::process::id(), 2, None),
    );

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
    assert_eq!(state.metrics.total_requests.load(Ordering::Relaxed), 1);
    assert_eq!(state.metrics.successful_requests.load(Ordering::Relaxed), 1);
    assert_eq!(state.metrics.failed_requests.load(Ordering::Relaxed), 0);

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
    state.models.write().await.insert(
        "test-model".to_string(),
        make_ready_state(server.uri(), std::process::id(), 0, None),
    );

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
    assert_eq!(state.metrics.failed_requests.load(Ordering::Relaxed), 1);
    assert_eq!(state.metrics.successful_requests.load(Ordering::Relaxed), 0);

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
    assert!(state.models.read().await.get("test-model").is_some());
}
