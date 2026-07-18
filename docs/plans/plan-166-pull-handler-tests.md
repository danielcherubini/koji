# Pull Pipeline Handler & Orchestration Tests Plan

**Goal:** Add the first tests for the pull HTTP surface — `handle_tama_pull_model`, `handle_tama_get_pull_job`, `handle_pull_job_stream` (`crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs`) and the download+verify orchestration `start_pull_from_queue` (download.rs:12) / `run_verification` (verify.rs:39) — which today have no `#[cfg(test)]` at all.

**Architecture:** Tests live in-crate in a new `pull/tests/` tree mirroring the `models/tests/` convention (`crates/tama-core/src/proxy/tama_handlers/models/mod.rs:20-25`): `pull/tests.rs` declaring `mod helpers; mod validation; mod jobs_stream; mod orchestration;`. Handlers are mounted on a minimal router (`/tama/v1/pulls` routes as in `crates/tama-core/src/proxy/server/router.rs:67-69`) and driven with `tower::ServiceExt::oneshot`; state is built with the `crates/tama/tests/downloads_api.rs:16-29` pattern (`ModelManager::open` on a tempdir + `PullQueueService::new(mgr, 2)` + `ProxyState::new(config, Some(db_dir))` + `set_pull_queue`). HuggingFace is faked with `wiremock` via the `HF_ENDPOINT` env var. One production seam fix is required (Task 1).

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio, wiremock, tower

**Key facts discovered while reading the code (tests must pin THESE):**
- Status codes: malformed JSON body → **400** (axum 0.7.9 `JsonSyntaxError`); missing `repo_id` → **422** (`JsonDataError`); too many files/quants → **400**; unknown filename / duplicate filename → **400** `ValidationError`; missing `quant` → **422** with `available_quants`; unknown quant → **422**; HF listing failure → **502** `UpstreamError`. There is no HTTP-level "queue full"/"duplicate job" rejection — `enqueue_pull` errors are discarded best-effort (handlers.rs:139,260,390: `let _ = enqueue_pull(...)`). The real rejection paths are the request limit (`max_concurrent_pulls()`, default 8 via `TAMA_MAX_CONCURRENT_PULLS`, types.rs:9-14) and the in-flight duplicate guard in `start_pull_from_queue` (download.rs:151-171). `PullQueueService`-level enqueue conflicts are already covered by pull_queue.rs's 25 tests — do not re-test them.
- `list_gguf_files` (models/pull/api.rs:17) goes through the `hf-hub` client built by `hf_api()` (models/pull/mod.rs:452) which uses `ApiBuilder::new()` — that constructor hardcodes `https://huggingface.co`; only `ApiBuilder::from_env()` reads `HF_ENDPOINT` (hf-hub 0.5 tokio.rs:245-252 vs :276-299). Task 1 fixes this. `fetch_blob_metadata` (api.rs:80-104) and the download HEAD/GET (download.rs:174-180) read `HF_ENDPOINT` per call and are already redirectable.
- `hf_api()` caches in a `static HF_API: OnceCell<Api>` (mod.rs:406) — first init wins for the process. Nextest runs each test in its own process (repo-mandated runner), so setting `HF_ENDPOINT` at test start is deterministic; under plain `cargo test` these tests would interfere. Document this in the test file header; also take the repo's `static ENV_GUARD: Mutex<()>` convention (models/pull/mod.rs:525-526, types.rs:136) — the `serial` crate is NOT a dependency and must not be added.
- Env redirection for writes: `Config::models_dir()` honors `general.models_dir` (types/mod.rs:59-64) — always set it to a tempdir. `Config::configs_dir()` is `Config::config_dir()/configs` (types/mod.rs:53-55) with no override — redirect it by setting BOTH `XDG_CONFIG_HOME` and `HOME` to tempdirs (Linux uses XDG, macOS uses `$HOME/Library/Application Support`), and compute the expected path in-test via `Config::config_dir()` rather than hardcoding.

---

### Task 1: Seam fix — `hf_api()` must honor `HF_ENDPOINT`

**Context:**
Every listing-backed validation path in `handle_tama_pull_model` calls `list_gguf_files`, which builds its URLs inside the `hf-hub` crate from the endpoint baked into the cached `Api`. Because `hf_api()` uses `ApiBuilder::new()`, `HF_ENDPOINT` is ignored for listings (it is honored everywhere else in the pull pipeline via manual `std::env::var` reads — see audit F17). Without this one-line change, the Task 3 tests would need real network access. Decision: switch to `ApiBuilder::from_env()` (hf-hub 0.5, tokio.rs:245) — the smallest possible change, no refactor; keep the explicit `.with_token(get_hf_token())` and `.with_max_files(8)` calls so token/cache behavior is unchanged (`with_token` overrides whatever `from_env` read).

**Files:**
- Modify: `crates/tama-core/src/models/pull/mod.rs`

**What to implement:**

1. In `hf_api()` (models/pull/mod.rs:452-463), change `ApiBuilder::new()` to `ApiBuilder::from_env()` and extend the doc comment above (:447-451) with: `HF_ENDPOINT` is honored via `from_env` so tests and mirrors can redirect the API; the `Api` is still cached process-wide in `HF_API`, so the first initialization wins.
2. Add a proof test in the existing `#[cfg(test)] mod tests` of `models/pull/mod.rs` (it already has the `ENV_GUARD` at :526):
   ```rust
   /// hf_api() must respect HF_ENDPOINT (regression: ApiBuilder::new ignored it)
   #[tokio::test]
   async fn test_list_gguf_files_respects_hf_endpoint() {
       let _guard = ENV_GUARD.lock().unwrap();
       let server = wiremock::MockServer::start().await;
       wiremock::Mock::given(wiremock::matchers::method("GET"))
           .and(wiremock::matchers::path("/api/models/test/repo"))
           .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
               "sha": "abc123",
               "siblings": [{"rfilename": "repo-Q4_K_M.gguf"}]
           })))
           .mount(&server)
           .await;
       std::env::set_var("HF_ENDPOINT", server.uri());
       let listing = list_gguf_files("test/repo").await.expect("listing from mock");
       std::env::remove_var("HF_ENDPOINT");
       assert_eq!(listing.files.len(), 1);
       assert_eq!(listing.files[0].filename, "repo-Q4_K_M.gguf");
       assert_eq!(listing.files[0].quant.as_deref(), Some("Q4_K_M"));
   }
   ```
   (`RepoInfo` deserializes from `{"siblings": [{"rfilename": ...}], "sha": ...}` — hf-hub 0.5 api/mod.rs:64-77.) Note: this test initializes `HF_API` under nextest's per-test process isolation; under plain `cargo test` it must run before any other `hf_api()` user — another reason AGENTS.md mandates nextest.

**Steps:**
- [ ] Change `ApiBuilder::new()` → `ApiBuilder::from_env()` in `crates/tama-core/src/models/pull/mod.rs:456` and update the doc comment
- [ ] Add the proof test
- [ ] Run `cargo nextest run --package tama-core -- models::pull` — all pass
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (guards against any caller depending on the old endpoint behavior)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: honor HF_ENDPOINT in hf_api via ApiBuilder::from_env"

**Acceptance criteria:**
- [ ] `list_gguf_files` provably fetches from the `HF_ENDPOINT` URL (wiremock) — the test fails before the change (connection to real huggingface.co or sandbox refusal) and passes after
- [ ] No other production code touched

---

### Task 2: Test scaffolding + no-listing validation tests

**Context:**
Four rejection paths in `handle_tama_pull_model` execute BEFORE any HF call: axum's `Json` extractor rejections (malformed body → 400, missing `repo_id` → 422) and the `max_concurrent_pulls()` count checks (handlers.rs:31-44 simplified path, :155-168 legacy quants path). These are the cheapest tests and establish the shared scaffolding. `PullRequest` (types.rs:40-63) has all-`#[serde(default)]` fields except `repo_id`.

**Files:**
- Create: `crates/tama-core/src/proxy/tama_handlers/pull/tests.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/pull/tests/helpers.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/pull/tests/validation.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/mod.rs` (add `#[cfg(test)] mod tests;` after the `mod verify;` line)

**What to implement:**

1. `pull/tests.rs`:
   ```rust
   //! Tests for the pull HTTP surface and download/verify orchestration.
   //!
   //! NOTE: tests that set HF_ENDPOINT rely on nextest's process-per-test
   //! isolation (hf_api caches its endpoint in a process-wide OnceCell).
   //! Run with `cargo nextest run`, never plain `cargo test`.
   mod helpers;
   mod jobs_stream;
   mod orchestration;
   mod validation;
   ```

2. `pull/tests/helpers.rs`:
   ```rust
   use std::sync::Arc;
   use axum::Router;
   use crate::proxy::pull_queue::PullQueueService;
   use crate::proxy::ProxyState;

   /// Serializes env-var-mutating tests (repo convention — see
   /// models/pull/mod.rs ENV_GUARD).
   pub(crate) static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

   /// ProxyState on a tempdir DB with a PullQueueService, mirroring
   /// crates/tama/tests/downloads_api.rs create_test_state.
   /// Returns (state, db TempDir — keep alive).
   pub(crate) fn create_test_state() -> (Arc<ProxyState>, tempfile::TempDir) {
       let tmp = tempfile::tempdir().unwrap();
       let db_dir = tmp.path().to_path_buf();
       let mgr = crate::models::ModelManager::open(&db_dir).unwrap();
       let svc = PullQueueService::new(mgr, 2);
       let config = crate::config::Config::default();
       let mut state = ProxyState::new(config, Some(db_dir));
       state.set_pull_queue(Some(Arc::new(svc)));
       (Arc::new(state), tmp)
   }

   /// Minimal router mounting the three pull handlers (routes mirror
   /// crates/tama-core/src/proxy/server/router.rs:67-69).
   pub(crate) fn pull_router(state: Arc<ProxyState>) -> Router {
       use axum::routing::{get, post};
       Router::new()
           .route("/tama/v1/pulls", post(super::super::handlers::handle_tama_pull_model))
           .route("/tama/v1/pulls/:job_id", get(super::super::handlers::handle_tama_get_pull_job))
           .route("/tama/v1/pulls/:job_id/stream", get(super::super::handlers::handle_pull_job_stream))
           .with_state(state)
   }
   ```
   (Adjust the `super::super::` paths so they resolve to `crate::proxy::tama_handlers::pull::handlers::*` — using the full `crate::` path is fine and clearer.)

3. `pull/tests/validation.rs` — four `#[tokio::test]`s using `create_test_state()` + `pull_router(state)` + `oneshot` (imports: `axum::body::Body`, `axum::http::Request`, `tower::ServiceExt`):
   - `test_pull_model_malformed_json_returns_400` — POST `/tama/v1/pulls`, header `content-type: application/json`, body `"{"` → status `400`.
   - `test_pull_model_missing_repo_id_returns_422` — POST with JSON `{}` → status `422`.
   - `test_pull_model_too_many_files_returns_400` — JSON `{"repo_id": "test/repo", "filenames": ["f1.gguf", ... nine entries ...]}` (default `max_concurrent_pulls` is 8 — do NOT set `TAMA_MAX_CONCURRENT_PULLS`) → status `400`, body `json["error"]` contains `"Too many files requested. Maximum is 8."` (this path uses the flat `{"error": "..."}` shape — handlers.rs:37-43 — assert on the string, not a nested shape).
   - `test_pull_model_too_many_quants_returns_400` — JSON with nine `quants` entries (`{"filename": "fN.gguf", "quant": "Q4_K_M"}`) → `400`, body contains `"Too many quants requested"`.

**Steps:**
- [ ] Create the three files and register `#[cfg(test)] mod tests;` in `pull/mod.rs`
- [ ] Run `cargo nextest run --package tama-core -- tama_handlers::pull` — 4 tests pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: pull handler scaffolding and pre-listing validation"

**Acceptance criteria:**
- [ ] 4 validation tests pass without any network access (no wiremock needed for these — verify by running with the sandbox offline if convenient; the code paths provably precede `list_gguf_files`)
- [ ] Scaffolding compiles once and is reused by Tasks 3–5
- [ ] No production code changed

---

### Task 3: Listing-backed validation and happy-path enqueue tests (wiremock)

**Context:**
The remaining `handle_tama_pull_model` branches all fetch the HF listing first (handlers.rs:47, :170, :284, :305), so they need Task 1's seam plus wiremock. The listing fixture shape is hf-hub's `RepoInfo`: `{"sha": "abc123", "siblings": [{"rfilename": "..."}]}`; `list_gguf_files` filters siblings to `.gguf` and infers quants (api.rs:34-45). Each test sets `HF_ENDPOINT` at the start under `ENV_GUARD` (nextest isolates processes; the guard keeps plain-`cargo test` runs honest).

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/tests/validation.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/tests/helpers.rs` (add mock helpers)

**What to implement:**

1. Add to `helpers.rs`:
   ```rust
   /// Mount a RepoInfo listing for `GET /api/models/<owner>/<name>` with the given GGUF filenames.
   pub(crate) async fn mount_listing(server: &wiremock::MockServer, repo_path: &str, gguf_files: &[&str]) {
       let siblings: Vec<serde_json::Value> = gguf_files
           .iter()
           .map(|f| serde_json::json!({"rfilename": f}))
           .collect();
       wiremock::Mock::given(wiremock::matchers::method("GET"))
           .and(wiremock::matchers::path(format!("/api/models/{}", repo_path)))
           .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
               "sha": "abc123",
               "siblings": siblings,
           })))
           .mount(server)
           .await;
   }
   ```
   Use repo_id `"test/repo"` everywhere (it does not end in `-GGUF`, so `list_gguf_files` tries `test/repo` first and succeeds — api.rs:21-25 — the fallback candidate is never requested).

2. Six tests in `validation.rs` (each: `let _guard = ENV_GUARD.lock().unwrap();` → `MockServer::start()` → `std::env::set_var("HF_ENDPOINT", server.uri())` → run request → `std::env::remove_var("HF_ENDPOINT")` BEFORE asserting panics can leak the var — set/remove around the oneshot, and keep assertions after removal):
   - `test_pull_model_unknown_filename_returns_400` — listing has `repo-Q4_K_M.gguf`; POST `{"repo_id": "test/repo", "filenames": ["nope.gguf"]}` → `400`, `body["error"]["type"] == "ValidationError"`, message contains `"is not a valid GGUF file for repo 'test/repo'"`.
   - `test_pull_model_duplicate_filename_returns_400` — POST `{"repo_id": "test/repo", "filenames": ["repo-Q4_K_M.gguf", "repo-Q4_K_M.gguf"]}` → `400`, message contains `"Duplicate filename"`.
   - `test_pull_model_missing_quant_returns_422_with_available` — POST `{"repo_id": "test/repo"}` → `422`, `body["error"]["type"] == "ValidationError"`, `body["error"]["available_quants"]` is an array containing an entry with `"filename": "repo-Q4_K_M.gguf"`.
   - `test_pull_model_unknown_quant_returns_422` — POST `{"repo_id": "test/repo", "quant": "Q9_XX"}` → `422`, message contains `"Quant 'Q9_XX' not found in repo 'test/repo'"`.
   - `test_pull_model_listing_failure_returns_502` — mock `GET /api/models/test/repo` → 500; POST `{"repo_id": "test/repo", "filenames": ["x.gguf"]}` → `502`, `body["error"]["type"] == "UpstreamError"`.
   - `test_pull_model_enqueues_job_and_returns_pending` — listing has `repo-Q4_K_M.gguf`; POST `{"repo_id": "test/repo", "filenames": ["repo-Q4_K_M.gguf"]}` → `200`; body is a JSON array of length 1 with `filename == "repo-Q4_K_M.gguf"` and `status == "pending"`; capture `job_id`; assert `state.pull_jobs().read().await` contains the job with `PullJobStatus::Pending`; assert the DB queue row exists via `state.pull_queue().as_ref().unwrap().test_model_mgr().queue_get_by_job_id(&job_id).unwrap().is_some()` (`test_model_mgr` is `#[cfg(test)]`, pull_queue.rs:82-89; `queue_get_by_job_id` at manager.rs:314). NOTE: this asserts the job was created + enqueued; it does NOT wait for the pull itself (no queue processor is running in tests).

**Steps:**
- [ ] Add `mount_listing` + the six tests
- [ ] Run `cargo nextest run --package tama-core -- tama_handlers::pull` — all pass (10 total)
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: pull model listing validation and enqueue paths"

**Acceptance criteria:**
- [ ] All six listing-backed behaviors pinned: 400 unknown filename, 400 duplicate, 422 missing quant (with `available_quants`), 422 unknown quant, 502 upstream failure, 200 happy path with in-memory job + DB queue row
- [ ] Every test cleans up `HF_ENDPOINT` before assertions
- [ ] No production code changed

---

### Task 4: `handle_tama_get_pull_job` and `handle_pull_job_stream` SSE tests

**Context:**
`handle_tama_get_pull_job` (handlers.rs:413) reads only the in-memory `state.pull_jobs` map — 404 `NotFoundError` for unknown ids, otherwise a JSON snapshot (`job_id`, snake_case `status`, `repo_id`, `filename`, `bytes_pulled`, `total_bytes`, `error`, `gguf_context_length`). `handle_pull_job_stream` (handlers.rs:478) polls the same map every 500 ms, emits `progress` events while pending/running, one terminal `done` event, then closes after a 100 ms flush sleep; an unknown job closes the stream with zero events. The SSE data payload is the serialized `PullJob` (`serde_json::to_string(&job)`). No DB or network involved.

**Files:**
- Create: `crates/tama-core/src/proxy/tama_handlers/pull/tests/jobs_stream.rs`

**What to implement:**

Helper to seed a job:
```rust
fn seed_job(state: &ProxyState, job_id: &str, status: PullJobStatus) {
    // pull_jobs is pub(crate) — tests insert directly.
    state.pull_jobs.try_write().unwrap().insert(
        job_id.to_string(),
        crate::proxy::pull_jobs::PullJob {
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
```

Tests:
1. `test_get_pull_job_unknown_returns_404` — GET `/tama/v1/pulls/nope` → `404`, `body["error"]["type"] == "NotFoundError"`.
2. `test_get_pull_job_returns_snapshot` — seed `pull-1` `Running`; GET → `200`; assert `job_id`, `status == "running"` (snake_case serde, pull_jobs.rs:5-7), `repo_id`, `filename`, `bytes_pulled == 500`, `total_bytes == 1000`.
3. `test_pull_job_stream_emits_progress_then_done` — seed `pull-sse` `Pending`; spawn a task that sleeps 650 ms then sets the job to `Completed` (write-lock `state.pull_jobs`, mutate status); oneshot GET `/tama/v1/pulls/pull-sse/stream`; collect the body with `tokio::time::timeout(Duration::from_secs(10), axum::body::to_bytes(resp.into_body(), usize::MAX))`. Assert the decoded text contains `event: progress` AND `event: done`, and that the `done` event's `data:` line parses as JSON with `"status": "completed"`. (Timeline: first poll at ~500 ms sees Pending → progress; flip at 650 ms; poll at 1000 ms → done; 100 ms flush → close.)
4. `test_pull_job_stream_unknown_job_closes_without_events` — GET `/tama/v1/pulls/ghost/stream` with the same 10 s timeout → body contains neither `event: progress` nor `event: done` (stream closes after the first 500 ms poll finds nothing; keep-alive comment lines are fine).

**Steps:**
- [ ] Add the four tests
- [ ] Run `cargo nextest run --package tama-core -- tama_handlers::pull` — all pass (14 total)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: pull job status endpoint and SSE stream"

**Acceptance criteria:**
- [ ] 404 + snapshot shape of the GET endpoint pinned
- [ ] SSE stream proven to emit `progress` → `done` and close; unknown job closes silently
- [ ] No production code changed

---

### Task 5: `start_pull_from_queue` download + verify orchestration tests (wiremock)

**Context:**
This is the corrupt-GGUF-admission guard the audit cares about: after the chunked download, `run_verification` hashes the file and compares against the LFS SHA-256 from `fetch_blob_metadata` (`GET {HF_ENDPOINT}/api/models/{repo}?blobs=true` → `parse_blob_siblings` expects `{"siblings": [{"rfilename", "blobId", "size", "lfs": {"sha256"}}]}` — api.rs:292-322). Mismatch → job `Failed` with `"hash mismatch: expected ... got ..."` and the file deleted (verify.rs:129-142, :180-202); match → job `Completed`, `setup_model_after_pull` writes the model card + `model_configs` row, and `download.rs:439-476` upserts the `model_files` row. Both paths are fully redirectable: download HEAD/GET read `HF_ENDPOINT` per call (download.rs:174-180), and `pull_chunked_with_progress` falls back to a single-stream GET when the HEAD response lacks `accept-ranges` (models/pull/mod.rs:131-141). The test serves tiny files so single-stream is always used. Two unavoidable best-effort network calls remain on the success path: `fetch_community_card` (hardcoded GitHub raw URL, 10 s timeout, fails fast to `None` offline / 404 for `test/repo` online — metadata.rs:336-361) and `fetch_model_pipeline_tag` (hits the wiremock, which 404s unmatched routes → `None`). Both are soft-fail by design; document them in the test comments.

**Files:**
- Create: `crates/tama-core/src/proxy/tama_handlers/pull/tests/orchestration.rs`

**What to implement:**

Setup helper (file-local):
```rust
/// State wired for a real pull: models_dir + config dir redirected to tempdirs,
/// HF_ENDPOINT pointing at `server`. Returns (state, models_dir TempDir guard, xdg TempDir guard).
async fn create_pull_state(server: &wiremock::MockServer) -> (Arc<ProxyState>, tempfile::TempDir, tempfile::TempDir) {
    let _guard = super::helpers::ENV_GUARD.lock().unwrap(); // NOTE: guard must live in the TEST fn, not here — see below
    ...
}
```
IMPORTANT: a `MutexGuard` cannot be returned from a helper across `.await`s cleanly — instead each test takes `let _guard = ENV_GUARD.lock().unwrap();` itself first, then calls helpers. The helper body:
- `let models_tmp = tempfile::tempdir().unwrap(); let xdg_tmp = tempfile::tempdir().unwrap();`
- `std::env::set_var("HF_ENDPOINT", server.uri());`
- `std::env::set_var("XDG_CONFIG_HOME", xdg_tmp.path());` and `std::env::set_var("HOME", xdg_tmp.path());` (macOS `directories` uses `$HOME`; Linux uses `XDG_CONFIG_HOME` — set both so the model card never lands in a real home dir; compute expectations via `crate::config::Config::config_dir()` AFTER setting).
- Build config: `let mut config = crate::config::Config::default(); config.general.models_dir = Some(models_tmp.path().to_string_lossy().to_string());`
- DB + queue as in `create_test_state`, but pass this `config`: `ProxyState::new(config, Some(db_dir))` + `set_pull_queue` (create the `ModelManager` on a SEPARATE tempdir or reuse `models_tmp` — reuse is fine; `ModelManager::open` creates `tama.db` inside it).
- Seed the in-memory job: `state.pull_jobs.write().await.insert(job_id, PullJob { job_id, repo_id: "test/repo", filename: FILE, ..Default::default() })` — `start_pull_from_queue` early-returns if the job is absent (download.rs:79-88).

Mocks (constants: `REPO = "test/repo"`, `FILE = "repo-Q4_K_M.gguf"`):
- `method("HEAD")` + `path("/test/repo/resolve/main/repo-Q4_K_M.gguf")` → `ResponseTemplate::new(200).insert_header("content-length", body.len().to_string())` (NO `accept-ranges` header → single-stream path).
- `method("GET")` + same path → `ResponseTemplate::new(200).set_body_bytes(body.clone())`.
- `method("GET")` + `path("/api/models/test/repo")` + `query_param("blobs", "true")` → `200` `{"siblings": [{"rfilename": FILE, "blobId": "b1", "size": body.len(), "lfs": {"sha256": sha}}]}`. (`query_param` matcher: `wiremock::matchers::query_param`.)

Tests:
1. `test_pull_hash_mismatch_fails_job_and_deletes_file` — `body = b"corrupt gguf bytes"`, `sha = sha256 of b"different content"` (compute with `sha2::Sha256` — `sha2` is already a tama-core dependency, used in backup/archive.rs). Call `start_pull_from_queue(state.clone(), job_id.into(), REPO.into(), FILE.into(), crate::proxy::tama_handlers::types::QuantDownloadSpec { filename: FILE.into(), quant: Some("Q4_K_M".into()), context_length: None }).await` directly (it's `pub`, re-exported at pull/mod.rs:10). Assert: in-memory job `status == PullJobStatus::Failed`, `error`/`verify_error` contains `"hash mismatch"`; `models_tmp.path()/test/repo/repo-Q4_K_M.gguf` does NOT exist (deleted); `svc.test_model_mgr().queue_get_by_job_id(&job_id)` has `status == "failed"` (PullQueueItem field name — check `PullQueueItem` in manager.rs and use the right accessor). Clean up env vars.
2. `test_pull_success_completes_and_records_model_files` — `body = b"fake but consistent gguf bytes"` (GGUF parse of non-GGUF content soft-fails to `None` — download.rs:398-425 — fine); `sha = sha256 of body`. After the call assert: job `status == Completed`, `verified_ok == Some(true)`; the file EXISTS at `models_tmp/test/repo/...`; `state.model_mgr().unwrap().get_all_files().unwrap()` has exactly 1 row with `filename == FILE` (manager.rs:141); a model card was written under the redirected config dir — `crate::config::Config::config_dir().unwrap().join("configs").join("test--repo.toml")` exists; the DB queue row is `completed`. Clean up env vars.

Both tests: remove `HF_ENDPOINT`/`XDG_CONFIG_HOME`/`HOME` after `start_pull_from_queue` returns, BEFORE asserting (restore `HOME` to its original value captured at test start with `std::env::var("HOME").ok()`).

**Steps:**
- [ ] Write the two orchestration tests
- [ ] Run `cargo nextest run --package tama-core -- tama_handlers::pull` — all pass (16 total)
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: pull download+verify orchestration with mock HF endpoint"

**Acceptance criteria:**
- [ ] Hash mismatch proven to fail the job AND delete the corrupt file (the audit's corrupt-GGUF admission guard)
- [ ] Successful pull proven to complete the job, keep the file, write the `model_files` row, the model card, and mark the queue row completed
- [ ] Nothing is written outside tempdirs (models dir, config dir, DB all redirected; env vars restored)
- [ ] `cargo nextest run --package tama-core` passes; clippy clean
