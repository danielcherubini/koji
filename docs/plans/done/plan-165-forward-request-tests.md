# `forward_request` Core Routing Tests Plan

**Goal:** Add the first direct tests for `forward_request` (`crates/tama-core/src/proxy/forward/request.rs:19`) — dead-PID detection and cleanup, circuit-breaker cooldown and trip behavior, metric increments, and the proxied success path.

**Architecture:** Tests live in a new file `crates/tama-core/src/proxy/forward/tests/request.rs`, registered via `mod request;` in `crates/tama-core/src/proxy/forward/tests.rs` (:21-25 — sibling files `headers.rs`, `json.rs`, `sse.rs`, `extract_stats.rs`, `integration.rs` follow the same pattern, each starting with `use super::*;`). `forward_request(&Arc<ProxyState>, &str, &Parts, &[u8], Option<&str>)` is called directly — no router, no middleware. `ProxyState` internals (`models`, `metrics`, `inference_stats`) are `pub(crate)`, so tests insert `BackendState::Ready` values by hand. `wiremock` 0.6 (dev-dependency) plays the backend for the success path. No production code changes.

**Tech Stack:** Rust, Axum, tokio, wiremock, tower (not needed — handlers called directly)

**Key facts discovered while reading the code (the tests must pin THESE, not imagined behavior):**
- Dead-PID check happens at request entry (request.rs:31-63): `backend_pid()` + `!is_process_alive(pid)` → `502` with `error.type == "BackendCrashedError"`, and the model is removed from `state.models`, its `inference_stats` entry cleared, and `model_mgr().remove_active` called best-effort (`model_mgr()` is `None` when `db_dir` is `None` — state.rs:277 — so tests need no DB).
- Circuit breaker (request.rs:65-104): with `failures >= config.proxy.circuit_breaker_threshold` (default `3`, `crates/tama-core/src/config/types/proxy.rs:185-187`):
  - `can_reload(cooldown)` false (recent `failure_timestamp`; default cooldown `60s`, proxy.rs:190-192) → `503` "Server {} is in cooldown due to repeated failures", `ServiceUnavailableError`, **no unload, no kill**.
  - `can_reload(cooldown)` true (stale or `None` `failure_timestamp`) → the breaker TRIPS: `state.unload_model(backend_name)` is awaited (SIGTERMs the pid, removes the model, bumps `metrics.models_unloaded` — lifecycle/mod.rs:597-694) and `503` "Server {} is currently unavailable due to repeated failures" is returned. **There is no "cooldown elapsed → pass-through" path** — pass-through only happens on the *next* request via the caller's auto-load logic (`crates/tama-core/src/proxy/handlers/forward.rs`), outside `forward_request`. Tests pin the actual trip behavior.
- Metrics: `total_requests` incremented unconditionally at entry (request.rs:25); `successful_requests` on 2xx (:251-258, also resets `consecutive_failures` to 0); `failed_requests` on non-2xx/transport error (:261-264, :530-533); `consecutive_failures` +1 only on 5xx responses (:266-270) or transport errors with a live pid (:549-554). The 502/503 early returns increment ONLY `total_requests`.
- No backend URL / model not loaded → `502` `BackendUrlError` (request.rs:108-122).

---

### Task 1: Scaffolding + dead-PID crash-path tests

**Context:**
The dead-PID branch (request.rs:31-63) is the highest-risk untested path: it mutates shared state (`models`, `inference_stats`) and returns a distinctive 502 that the auto-load caller depends on. Scaffolding decision: helpers are defined locally in the new test file rather than imported from `crates/tama-core/src/proxy/tama_handlers/models/tests/helpers.rs` (`create_state_with_model`) — that module is `#[cfg(test)] mod tests` private to the `models` module (`models/mod.rs:20-25`) and inserts into `model_configs`, not the `models` map these tests need.

**Files:**
- Create: `crates/tama-core/src/proxy/forward/tests/request.rs`
- Modify: `crates/tama-core/src/proxy/forward/tests.rs` (add `mod request;`)

**What to implement:**

1. Register the module: add `mod request;` to `crates/tama-core/src/proxy/forward/tests.rs` after `mod json;` (keep alphabetical-ish grouping with the existing five).

2. New file `crates/tama-core/src/proxy/forward/tests/request.rs`, starting with `use super::*;` (gives `forward_request`, `make_parts`, `Parts`, `HashMap`, `LatestInferenceStats` via the parent module's imports) plus:
   ```rust
   use crate::proxy::{BackendState, ProxyState};
   use axum::http::StatusCode;
   use std::sync::atomic::{AtomicU32, Ordering};
   use std::sync::Arc;
   use std::time::{Duration, Instant, SystemTime};
   ```

3. Helpers (file-local):
   ```rust
   /// Build a BackendState::Ready for direct insertion into state.models.
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

   /// POST variant of the parent's GET-only make_parts helper.
   fn make_post_parts(path: &str) -> Parts {
       let req = axum::http::Request::post(path).body(()).unwrap();
       let (parts, _) = req.into_parts();
       parts
   }

   /// Spawn a harmless long-lived process; returns (child, pid) so tests can
   /// reap it. Needed wherever a live pid is required AND forward_request may
   /// signal it — never use std::process::id() in those cases (SIGTERM would
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
   ```

4. Tests:
   - `test_forward_request_dead_pid_returns_502_and_cleans_up` — `test_state()`; insert `make_ready_state("http://127.0.0.1:1".into(), 99999999, 0, None)` under key `"test-model"` (99999999 is the repo's definitely-dead PID convention, `crates/tama-core/src/proxy/process.rs:131,145`); pre-seed `state.inference_stats.send_modify(|m| { m.insert("test-model".to_string(), LatestInferenceStats::default()); });`. Call `forward_request(&state, "test-model", &make_post_parts("/v1/chat/completions"), b"{}", Some("test-model")).await`. Assert: status `502`; `body["error"]["type"] == "BackendCrashedError"`; `body["error"]["message"]` contains `"has crashed, reloading"`; `state.models.read().await.get("test-model")` is `None`; `state.inference_stats.borrow().get("test-model")` is `None`.
   - `test_forward_request_dead_pid_increments_only_total_requests` — same setup; assert `state.metrics.total_requests.load(Ordering::Relaxed) == 1`, `successful_requests == 0`, `failed_requests == 0` (documents that crash-detection bypasses the failure counters).
   - `test_forward_request_model_not_loaded_returns_502` — empty `test_state()` (no models inserted) → status `502`, `body["error"]["type"] == "BackendUrlError"`, message contains `"is not loaded"`.

**Steps:**
- [ ] Create the test file with helpers and the three tests; register `mod request;` in `crates/tama-core/src/proxy/forward/tests.rs`
- [ ] Run `cargo nextest run --package tama-core -- proxy::forward` — new tests pass (they pin existing behavior; if one fails, re-read the branch in request.rs before changing anything)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: cover forward_request dead-PID detection and cleanup"

**Acceptance criteria:**
- [ ] 502 + `BackendCrashedError` on dead PID proven, including removal from `state.models` and `inference_stats`
- [ ] Not-loaded model → 502 `BackendUrlError` proven
- [ ] Crash path shown to increment only `total_requests`
- [ ] No production code changed

---

### Task 2: Circuit-breaker tests (cooldown 503, trip-and-unload after cooldown)

**Context:**
Both circuit-breaker exits return 503 but differ critically: the cooldown branch leaves the model loaded and the process untouched; the trip branch SIGTERMs the pid and removes the model so the NEXT request auto-loads fresh. SAFETY: any test whose branch can reach `unload_model` MUST use a spawned `sleep` pid (`spawn_live_pid`) — `unload_model` sends SIGTERM (lifecycle/mod.rs:644) and would kill the test runner if given `std::process::id()`. Both tests use `Config::default()` (threshold 3, cooldown 60s).

**Files:**
- Modify: `crates/tama-core/src/proxy/forward/tests/request.rs`

**What to implement:**

1. `test_forward_request_circuit_breaker_cooldown_returns_503_without_unload` — `let (mut child, pid) = spawn_live_pid();` insert `make_ready_state("http://127.0.0.1:1".into(), pid, 3, Some(SystemTime::now()))` (failures == threshold, timestamp fresh → cooldown active). Call forward_request. Assert: status `503`; `body["error"]["type"] == "ServiceUnavailableError"`; message contains `"in cooldown"`; model STILL in `state.models`; `crate::proxy::process::is_process_alive(pid)` is `true` (proves no SIGTERM); `failed_requests == 0`. Cleanup: `child.kill().unwrap(); let _ = child.wait();`.
2. `test_forward_request_circuit_breaker_trips_and_unloads_after_cooldown` — `let (mut child, pid) = spawn_live_pid();` insert `make_ready_state("http://127.0.0.1:1".into(), pid, 3, Some(SystemTime::now() - Duration::from_secs(120)))` (timestamp older than the 60s cooldown → `can_reload` true → trip). Call forward_request. Assert: status `503`; `body["error"]["type"] == "ServiceUnavailableError"`; message contains `"currently unavailable"` (distinct from the cooldown message); `state.models.read().await.get("test-model")` is `None` (unloaded by `unload_model`); `is_process_alive(pid)` is `false` (SIGTERM landed — `unload_model` polls until exit, lifecycle/mod.rs:646-664, so no sleep needed in the test); `state.metrics.models_unloaded.load(Ordering::Relaxed) == 1`. Cleanup: `let _ = child.wait();` (reap the zombie).
3. `test_forward_request_below_threshold_passes_circuit_breaker` — guard against off-by-one: failures `2` (threshold − 1), fresh `failure_timestamp`, pid = `std::process::id()` (safe: this path never signals the pid — assert that by the request reaching the backend-URL stage). Use `backend_url = "http://127.0.0.1:1"` (nothing listening → transport error) → status `502` with `body["error"]["type"] == "BadGatewayError"` — proving the request was NOT short-circuited by the breaker (which would have been 503) and DID attempt the backend; `consecutive_failures` becomes `3` afterward (transport-error increment at request.rs:549-554).

**Steps:**
- [ ] Add the three tests to `crates/tama-core/src/proxy/forward/tests/request.rs`
- [ ] Run `cargo nextest run --package tama-core -- proxy::forward` — all pass
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (guards against helper-name collisions across the crate's test modules)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: cover forward_request circuit breaker cooldown and trip"

**Acceptance criteria:**
- [ ] Cooldown-active 503 proven to leave model + process untouched
- [ ] Post-cooldown trip proven to SIGTERM the backend, remove the model, bump `models_unloaded`, and return the distinct "currently unavailable" 503
- [ ] Threshold − 1 proven to pass through to the backend (502 transport error, failure counter increments)
- [ ] No test ever passes the test runner's own pid into a killable path
- [ ] No production code changed

---

### Task 3: Metrics and proxied-response tests (wiremock backend)

**Context:**
The remaining untested behavior is the pass-through itself: successful response → `successful_requests` + failure-counter reset + model-name rewrite; backend 5xx → `failed_requests` + `consecutive_failures` +1 + `failure_timestamp` set. `wiremock` supplies the backend (`BackendState::Ready.backend_url = server.uri()`); the pattern follows `crates/tama-core/src/backends/updater.rs:397-418` (`MockServer::start()`, `Mock::given(method(..)).and(path(..))`, `.mount(&server).await`). Pids here are `std::process::id()` — safe because neither the 2xx nor the proxied-5xx path ever signals the pid (verified in request.rs: the kill only happens in the trip branch, which requires failures ≥ threshold on ENTRY, and these tests enter below threshold).

**Files:**
- Modify: `crates/tama-core/src/proxy/forward/tests/request.rs`

**What to implement:**

Add imports: `use wiremock::matchers::{method, path};` `use wiremock::{Mock, MockServer, ResponseTemplate};`

1. `test_forward_request_success_increments_metrics_and_rewrites_model` — mock `method("POST")` + `path("/v1/chat/completions")` → 200 JSON `{"id": "cmpl-1", "model": "backend-model", "choices": [{"message": {"role": "assistant", "content": "hi"}}]}`. Insert `make_ready_state(server.uri(), std::process::id(), 2, None)` (pre-existing failure count 2). Call `forward_request(&state, "test-model", &make_post_parts("/v1/chat/completions"), br#"{"model":"test-model","messages":[]}"#, Some("public-name")).await`. Assert: status `200`; body `json["model"] == "public-name"` (rewrite via `rewrite_json_model_name`, request.rs:489); `json["choices"][0]["message"]["content"] == "hi"`; `total_requests == 1`; `successful_requests == 1`; `failed_requests == 0`; `state.get_model_state("test-model").await.unwrap().consecutive_failures().unwrap().load(Ordering::Relaxed) == 0` (reset on success, request.rs:253-257).
2. `test_forward_request_backend_500_increments_failures_and_sets_timestamp` — mock returns `ResponseTemplate::new(500).set_body_string("boom")`. Insert `make_ready_state(server.uri(), std::process::id(), 0, None)`. Call forward_request. Assert: response status `500` (proxied through unchanged, request.rs:274); `failed_requests == 1`; `successful_requests == 0`; `consecutive_failures == 1`; `failure_timestamp` is now `Some` — read it back via `state.get_model_state("test-model").await.unwrap()` and match `BackendState::Ready { failure_timestamp, .. }` (timestamp-set logic at request.rs:271-287); the model is STILL loaded (proxied 5xx with a live pid never unloads).

**Steps:**
- [ ] Add the two tests
- [ ] Run `cargo nextest run --package tama-core -- proxy::forward` — all pass
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: cover forward_request success path and failure metrics"

**Acceptance criteria:**
- [ ] Success path proven end-to-end against a mock backend: 200, rewritten model name, `successful_requests` incremented, failure counter reset
- [ ] Backend 5xx proven to increment `failed_requests` + `consecutive_failures`, set `failure_timestamp`, and proxy the status through
- [ ] All ~8 `forward_request` tests pass with `cargo nextest run --package tama-core -- proxy::forward` (8 direct tests + the existing 33 helper tests)
- [ ] No production code changed; clippy clean
