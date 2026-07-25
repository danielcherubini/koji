# API Handler Boilerplate Plan

**Goal:** Eliminate repeated request-scaffolding in the management API — triplicated benchmark job submission, the 6× model-resolution chain, 4 divergent config-dir resolution variants, blocking SQLite calls on the async executor, and a batch of small verbatim duplications — by extracting one shared helper per pattern and routing every site through it.

**Architecture:** All helpers live in the `tama` binary crate (Axum layer): `api/benchmarks/mod.rs` (job submission), `api/models/info.rs` (model resolution), `api/helpers.rs` (config dir / repository), `api/backends/mod.rs` (traversal guard); two helpers land in `tama-core` library modules (`models/pull/mod.rs` backoff, `bench/mod.rs` stats). Behavior is preserved site-by-site; the only intentional wire changes are (a) config-dir failures uniformly become the canonical nested 404 `{"error":{"message","type"}}` (never a silent `./tama.db` CWD fallback), and (b) the path-traversal guard unifies on one 400 ValidationError body. The flat-error-shape migration for the *other* error sites is plan-161's job — do not "fix" unrelated error bodies while touching these files.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio, Leptos (WASM pages)

---

### Task 1: Route the three benchmark handlers through `submit_benchmark_job` (F13)

**Context:**
`run_benchmark` (`crates/tama/src/api/benchmarks/run.rs:6-53`), `run_mtp_benchmark` (`mtp.rs:52-107`), and `run_spec_benchmark` (`spec.rs:9-96`) each contain an identical ~30-line block: get job manager → `jobs.submit` → build `db_path` with a CWD fallback → read `proxy_base_url`/`client` → `tokio::spawn` an inner fn → `jobs.finish(Succeeded/Failed)`. The intended shared helper `submit_benchmark_job` already exists at `benchmarks/mod.rs:208-260` but is **dead code** (zero call sites — verified with `rg submit_benchmark_job crates/`). Decision: **keep the helper and route all three handlers through it** (the audit's option A), because the helper's body is already verbatim-identical to the triplicated block. Two adjustments are required: (a) the helper's error type changes from `anyhow::Error` to `axum::response::Response` so the two distinct failure responses (503 job-manager-unavailable, 409 job-conflict) survive; (b) the three `run_*_inner` fns are aligned to the helper's existing closure contract — `job: Arc<Job>` **by value** and `db_path: PathBuf` (not `Option<PathBuf>`) — so each handler can pass its inner fn **by name** with no adapter closure. The inner fns have zero external callers (verified: `rg "run_benchmark_inner|run_spec_benchmark_inner|run_mtp_benchmark_inner" crates/` shows only their own files plus the re-exports in `mod.rs:158-159`), so changing their signatures is safe. Pre-submit validation stays in the handlers (mtp: `draft_max_values` non-empty; spec: `spec_types` non-empty + `validate_spec_sweep`) because it must happen **before** `jobs.submit`.

**Files:**
- Modify: `crates/tama/src/api/benchmarks/mod.rs`
- Modify: `crates/tama/src/api/benchmarks/run.rs`
- Modify: `crates/tama/src/api/benchmarks/mtp.rs`
- Modify: `crates/tama/src/api/benchmarks/spec.rs`

**What to implement:**

1. **`benchmarks/mod.rs`** — change the helper signature and error mapping (body otherwise unchanged, including the existing `db_path` CWD-fallback — task 3 migrates it):
   ```rust
   pub async fn submit_benchmark_job<F, Fut, R>(
       state: &tama_core::proxy::ProxyState,
       web_state: &WebState,
       req: R,
       run_inner: F,
   ) -> Result<(String, Arc<JobManager>), axum::response::Response>
   where
       R: Send + 'static,
       F: FnOnce(
               Arc<JobManager>,
               Arc<crate::web_types::Job>,
               R,
               std::path::PathBuf,
               String,
               reqwest::Client,
           ) -> Fut
           + Send
           + 'static,
       Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
   ```
   Inside: `let jobs = match &web_state.jobs { Some(j) => j.clone(), None => return Err(job_manager_unavailable_response()) };` and `let job = jobs.submit(JobKind::Benchmark, None).await.map_err(|_| job_conflict_response())?;`. Remove the now-unused `anyhow::{Context, Result}` imports if nothing else in `mod.rs` uses them (check first — `BenchmarkProgressSink` does not, but verify).

2. **`run.rs` / `mtp.rs` / `spec.rs` inner fns** — change signatures to:
   ```rust
   pub async fn run_benchmark_inner(
       jobs: Arc<JobManager>,
       job: Arc<crate::web_types::Job>,        // was &Arc<Job>
       req: BenchmarkRunRequest,
       db_path: std::path::PathBuf,            // was Option<std::path::PathBuf>
       proxy_base_url: String,
       client: reqwest::Client,
   ) -> Result<()>
   ```
   (same shape for `run_mtp_benchmark_inner`, `run_spec_benchmark_inner`). Delete the `let db_path: std::path::PathBuf = db_path.context("Cannot determine db path")?;` line in all three (the value is already a `PathBuf`). Adjust body references: `&job` → `job` where an owned value is needed (`BenchmarkProgressSink { job: job.clone(), .. }` already clones), `job.id` borrows still work. Nothing else in the inner bodies changes in this task.

3. **Handlers** — replace the whole submit block with the helper call, passing the inner fn **by name**:
   ```rust
   pub async fn run_benchmark(
       Extension(web_state): Extension<WebState>,
       State(state): State<Arc<ProxyState>>,
       Json(req): Json<BenchmarkRunRequest>,
   ) -> impl IntoResponse {
       let (job_id, _jobs) = match submit_benchmark_job(&state, &web_state, req, run_benchmark_inner).await {
           Ok(v) => v,
           Err(resp) => return resp,
       };
       (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
   }
   ```
   `run_mtp_benchmark`: keep the `draft_max_values.is_empty()` 400 check first, then call `submit_benchmark_job(&state, &web_state, req, run_mtp_benchmark_inner)`. `run_spec_benchmark`: keep the `spec_types.is_empty()` check, the `runs.max(1)`/`gen_tokens.max(1)` guards, and the `validate_spec_sweep` 400 check first (note: the validated `runs`/`gen_tokens` locals are only used to build `validation_config` — the request is passed through unmodified, exactly as today), then `submit_benchmark_job(&state, &web_state, req, run_spec_benchmark_inner)`. Update imports: `job_conflict_response`/`job_manager_unavailable_response` are no longer referenced by the three handler files — drop them from the `use crate::api::benchmarks::{...}` lists in `mtp.rs:3-5` and `spec.rs:3-5`, and add `submit_benchmark_job` to the `use super::*;`-adjacent imports as needed (run.rs uses `use super::*;` which already covers it).

4. Do **not** add route tests — benchmark route test coverage is finding F24's scope (different plan). This task is a pure structural refactor guarded by compilation + the existing suite.

**Steps:**
- [ ] Run `cargo nextest run --package tama` — record the green baseline
- [ ] Apply the changes to `benchmarks/mod.rs`, `run.rs`, `mtp.rs`, `spec.rs` per above
- [ ] Run `cargo check --package tama` — compiles; fix only import/signature mistakes (watch for unused imports: `JobKind`, `JobStatus` may become unused in the three handler files — they are still used by the helper in `mod.rs`, and `mtp.rs`/`spec.rs` still need `error_response` for their pre-submit validation)
- [ ] Run `cargo nextest run --package tama` — all pass, same count as baseline
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: route benchmark handlers through submit_benchmark_job"

**Acceptance criteria:**
- [ ] `rg "run_benchmark_inner|run_mtp_benchmark_inner|run_spec_benchmark_inner" crates/tama/src/api/benchmarks/` shows each inner fn referenced exactly twice: its definition and its handler's `submit_benchmark_job` call
- [ ] `rg "submit_benchmark_job" crates/` shows 1 definition + 3 call sites (no longer dead code)
- [ ] The 503/409/400 response bodies are byte-identical to before (helper reuses `job_manager_unavailable_response`/`job_conflict_response`; pre-submit validation untouched)
- [ ] `cargo nextest run --package tama` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 2: Extract `resolve_model_record` for the 6× model-resolution chain (F14)

**Context:**
Six handlers repeat the same chain — `Repository::open(&config_dir)` (500 on error) → `resolve_model_id(&id_str, &repo)` (400 ValidationError on parse error, 404 NotFoundError on `Ok(None)`) → `repo.get_model_config(model_id)` (500 on error, 404 on `None`) — inside `spawn_model_crud`/`spawn_blocking` closures whose error type is `(StatusCode, serde_json::Value)`. Sites: `crates/tama/src/api/models/crud/update.rs:40-72` (`update_model`) and `:123-155` (`patch_model`), `crud/rename.rs:38-56`, `crud/delete.rs:144-184` (`delete_model`), `api/models/files.rs:52-89` (`refresh_model_metadata` step 1) and `:204-241` (`verify_model_files` step 1). Decisions: (a) the helper returns `ModelConfigDto`, **not** the deprecated `ModelConfigRecord` — `Repository::get_model_config` (`crates/tama-core/src/db/repository.rs:256`) returns `ModelConfigDto` and all six sites already consume that; (b) the helper returns the `Repository` in the tuple because `delete.rs` reuses it afterwards (`repo.delete_update_check`, current line ~207) and task 4's `get_model` rewrite needs it — this avoids a second `Repository::open` (each `open()` re-runs the migration suite); (c) placement is `api/models/info.rs` next to `resolve_model_id` (line 29), re-exported through `api/models/mod.rs`'s existing `pub use info::*;`. The `delete_quant` handler (`delete.rs:18-55`) is **not** a site — it takes a typed `Path<(i64, String)>` and skips `resolve_model_id`.

**Files:**
- Modify: `crates/tama/src/api/models/info.rs`
- Modify: `crates/tama/src/api/models/crud/update.rs`
- Modify: `crates/tama/src/api/models/crud/rename.rs`
- Modify: `crates/tama/src/api/models/crud/delete.rs`
- Modify: `crates/tama/src/api/models/files.rs`

**What to implement:**

1. **`info.rs`** — add directly below `resolve_model_id` (after line 37):
   ```rust
   /// Open the Repository at `config_dir`, resolve `id_str` (integer id or
   /// config_key) to a model id, and load its config record.
   ///
   /// The Repository is returned so callers with follow-up queries reuse the
   /// same connection. Error mapping matches the historical per-handler chains:
   /// open failure → 500, unresolvable id → 400 ValidationError,
   /// unknown id → 404 NotFoundError.
   pub(crate) fn resolve_model_record(
       config_dir: &std::path::Path,
       id_str: &str,
   ) -> Result<(Repository, i64, ModelConfigDto), (StatusCode, serde_json::Value)> {
       let repo = Repository::open(config_dir).map_err(|e| {
           (
               StatusCode::INTERNAL_SERVER_ERROR,
               error_body(e.to_string(), None),
           )
       })?;
       let model_id = resolve_model_id(id_str, &repo)
           .map_err(|e| {
               (
                   StatusCode::BAD_REQUEST,
                   error_body(e.to_string(), Some("ValidationError")),
               )
           })?
           .ok_or_else(|| {
               (
                   StatusCode::NOT_FOUND,
                   error_body("Model not found", Some("NotFoundError")),
               )
           })?;
       let record = repo
           .get_model_config(model_id)
           .map_err(|e| {
               (
                   StatusCode::INTERNAL_SERVER_ERROR,
                   error_body(e.to_string(), None),
               )
           })?
           .ok_or_else(|| {
               (
                   StatusCode::NOT_FOUND,
                   error_body("Model not found", Some("NotFoundError")),
               )
           })?;
       Ok((repo, model_id, record))
   }
   ```
   Add `error_body` to the `use crate::api::error::{...}` import at `info.rs:1` (currently imports only `error_response`).

2. **Migrate the six sites.** In each, replace the `Repository::open` + `resolve_model_id` + `get_model_config` block with:
   ```rust
   let (repo, model_id, existing_record) = resolve_model_record(&config_dir, &id_str)?;
   ```
   - `update.rs` ×2 (`update_model`, `patch_model`): the returned `repo` is unused — bind `let (_repo, model_id, existing_record) = ...`. The following `ModelManager::open`, `apply_model_body`/`apply_model_patch`, `config_key` derivation, and `save_model_config` are unchanged. Change `use crate::api::models::resolve_model_id;` (line 17) → `use crate::api::models::resolve_model_record;`.
   - `rename.rs`: same — `_repo` binding; change import at line 14 the same way. The `existing_record.repo_id` uses (`delete_update_check` cleanup, config_key) are unchanged.
   - `delete.rs` (`delete_model`): keep the `repo` binding — it is used later at `repo.delete_update_check("model", &model_id.to_string())` (current line ~207); leave that call and its argument **exactly** as-is (the `model_id.to_string()` argument is deliberate — do not "fix" it to `repo_id`; behavior preservation). Note `delete.rs` opens `ModelManager` BEFORE the resolution today (line 152) — keep that order; only the open+resolve+get chain collapses. Change import at line 13 the same way.
   - `files.rs` ×2 (`refresh_model_metadata`, `verify_model_files`): replace the chain with `let (_repo, model_id, record) = resolve_model_record(&config_dir, &id_str)?;` keeping the surrounding `spawn_blocking` closure and the `models_dir` resolution + `Ok::<_, (StatusCode, serde_json::Value)>` tuple construction unchanged. Change `use super::resolve_model_id;` (line 11) → `use super::resolve_model_record;`.
   - **Do not** touch `info.rs`'s own `get_model`/`list_models` here — they are restructured in task 4 (their error mapping differs: `get_model` currently maps a `get_model_config` DB *error* to 404 via `.ok().flatten()`).

3. **Tests** — add to the existing `#[cfg(test)] mod tests` in `info.rs` (bottom of file). Follow the tempdir pattern: create `let tmp = tempfile::tempdir().unwrap();`, insert one model via `tama_core::models::ModelManager::open(tmp.path()).unwrap().save_model_config("org--test-model", &tama_core::config::ModelConfig { backend: "llama-cpp".into(), model: Some("org/test-model".into()), ..Default::default() }).unwrap();` then:
   - `test_resolve_model_record_by_config_key`: `resolve_model_record(tmp.path(), "org--test-model")` → `Ok((_, id, record))`, `record.repo_id == "org/test-model"`, `id` equals the id returned by `save_model_config`.
   - `test_resolve_model_record_by_integer_id`: same via the numeric id string.
   - `test_resolve_model_record_unknown_config_key_404`: `resolve_model_record(tmp.path(), "no--such-model")` → `Err((StatusCode::NOT_FOUND, _))`.
   - `test_resolve_model_record_unknown_integer_404`: `resolve_model_record(tmp.path(), "999")` → `Err((StatusCode::NOT_FOUND, _))` (integer parse succeeds, record missing).
   (`tempfile` is already a dependency of the `tama` crate — `crates/tama/Cargo.toml`.)

**Steps:**
- [ ] Write the four failing tests in `crates/tama/src/api/models/info.rs` (they fail to compile — `resolve_model_record` doesn't exist yet)
- [ ] Run `cargo nextest run --package tama -- api::models::info` — verify failure
- [ ] Implement `resolve_model_record` in `info.rs`; migrate the six sites per above
- [ ] Run `cargo nextest run --package tama -- api::models` — new tests + all existing model tests pass (watch `crud/tests.rs` — zero test-body edits allowed)
- [ ] Run `cargo nextest run --package tama` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: extract resolve_model_record for model API handlers"

**Acceptance criteria:**
- [ ] `rg "resolve_model_id" crates/tama/src/api/` shows the only remaining uses are inside `info.rs` itself (definition + inside `resolve_model_record`)
- [ ] All six sites call `resolve_model_record`; the ~25-line open/resolve/get chain appears once
- [ ] Response statuses and bodies at every migrated site are identical to before (500/400/404 mapping preserved verbatim)
- [ ] `cargo nextest run --package tama` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 3: Canonical `resolve_config_dir` / `open_repository`; kill the CWD fallback (F15)

**Context:**
Config-dir resolution exists in four variants: (A) silent `./tama.db` CWD fallback — `api/aliases/mod.rs:38,63,93,161,235`, `api/backup.rs:69-73`, `api/backends/list.rs:38-40`, `api/backends/install.rs:524-526` (`remove_backend`), and inside the already-factored `api/helpers.rs::open_backend_manager:18`; (B) hand-rolled 404 with a *flat* `{"error":"..."}` body — `api/updates.rs:83-91,189-197,233-241,424-432,654-662`; (C) `Config::base_dir()` inside a spawned task — `api/updates.rs:543`; (D) the only factored one, `api.rs::load_config_from_state:176-207` (`db_dir → Config::config_dir() → 404` with the canonical nested error). Decision: variant D's resolution order is canonical — `state.db_dir()` then `Config::config_dir()` then a 404 with the canonical nested error body (`error_response(404, "config directory not configured", Some("NotFoundError"))`) — and **no site ever silently uses the CWD**. `Config::config_dir()` is literally `Self::base_dir()` (`crates/tama-core/src/config/loader.rs:33-35`), so variant C's directory is identical whenever the handler didn't already 404. Accepted wire changes, spelled out: variant-B sites in updates.rs switch their 404 body from flat `{"error":"config directory not configured"}` to the canonical nested shape (plan-161's direction); variant-A sites gain a real 404 instead of opening `./tama.db`; `open_backend_manager` failure at its three tolerant callers (`backends/list.rs:42,328,555`) is already handled by warn-and-render-partial, so those endpoints keep working. Explicitly out of scope (do not touch): `backup.rs:128` (restore_preview's upload scratch dir with `temp_dir()` fallback — different semantic) and `backup.rs:266` (`start_restore` — owned by F6's plan); `api.rs::load_config_from_state` itself (already canonical).

**Files:**
- Modify: `crates/tama/src/api/helpers.rs`
- Modify: `crates/tama/src/api/aliases/mod.rs`
- Modify: `crates/tama/src/api/backup.rs`
- Modify: `crates/tama/src/api/backends/list.rs`
- Modify: `crates/tama/src/api/backends/install.rs`
- Modify: `crates/tama/src/api/updates.rs`
- Modify: `crates/tama/src/api/benchmarks/mod.rs`

**What to implement:**

1. **`helpers.rs`** — add two helpers (imports: add `error_response` to the existing `use crate::api::error::error_response_simple;`, add `use tama_core::db::repository::Repository;`):
   ```rust
   /// Resolve the config directory from ProxyState (`db_dir`, set at startup),
   /// falling back to the system default config dir. Never falls back to the
   /// process CWD. Returns the canonical 404 response when unconfigured.
   pub fn resolve_config_dir(
       state: &ProxyState,
   ) -> Result<std::path::PathBuf, axum::response::Response> {
       state
           .db_dir()
           .clone()
           .or_else(|| tama_core::config::Config::config_dir().ok())
           .ok_or_else(|| {
               error_response(
                   StatusCode::NOT_FOUND,
                   "config directory not configured",
                   Some("NotFoundError"),
               )
           })
   }

   /// Resolve the config dir and open a Repository on the blocking pool.
   pub async fn open_repository(
       state: &ProxyState,
   ) -> Result<Repository, axum::response::Response> {
       let config_dir = resolve_config_dir(state)?;
       tokio::task::spawn_blocking(move || Repository::open(&config_dir))
           .await
           .map_err(|e| {
               error_response_simple(
                   StatusCode::INTERNAL_SERVER_ERROR,
                   format!("spawn error: {}", e),
               )
           })?
           .map_err(|e| {
               error_response_simple(
                   StatusCode::INTERNAL_SERVER_ERROR,
                   format!("Database not configured: {}", e),
               )
           })
   }
   ```
   Also rewrite `open_backend_manager`'s first lines (helpers.rs:18-21) to `let config_dir = resolve_config_dir(proxy_state)?;` (its callers: `install.rs:541`, `manage/update.rs:48` return the error response directly — they now 404; `list.rs:42,328,555` log-and-degrade — unchanged code there).

2. **`aliases/mod.rs`** — at all five handlers (`list_aliases:38`, `get_alias:63`, `create_alias:93`, `update_alias:161`, `delete_alias:235`) replace the `let db_dir = state.db_dir().clone().unwrap_or_else(...)` block with:
   ```rust
   let db_dir = match crate::api::helpers::resolve_config_dir(&state) {
       Ok(d) => d,
       Err(resp) => return resp,
   };
   ```
   (Add `use crate::api::helpers::resolve_config_dir;` at the top and call it unqualified.) Leave the `Repository::open(&db_dir)` blocks as-is — task 4 restructures them.

3. **`backup.rs`** — `create_backup` (lines 69-73): replace the variant-A block with the same `resolve_config_dir` match (`let config_dir: std::path::PathBuf = match ...`). Do not touch lines 128 or 266.

4. **`backends/list.rs`** (`list_backends`) — the `config_dir` local at :38-40 feeds **only** the best-effort update-check enrichment at :63-72. Delete the `config_dir` local entirely and rewrite the enrichment to use `open_repository` (best-effort semantics preserved — never 404 this endpoint):
   ```rust
   let update_checks: std::collections::HashMap<
       String,
       tama_core::db::repository::UpdateCheckDto,
   > = match crate::api::helpers::open_repository(&state).await {
       Ok(repo) => repo
           .get_all_update_checks()
           .ok()
           .map(|records| {
               records
                   .into_iter()
                   .filter(|r| r.item_type == "backend")
                   .map(|r| (r.item_id.clone(), r))
                   .collect()
           })
           .unwrap_or_default(),
       Err(resp) => {
           tracing::warn!("update-check lookup unavailable: {}", resp.status());
           std::collections::HashMap::new()
       }
   };
   ```
   (The blocking `get_all_update_checks()` call on the executor is wrapped in `spawn_blocking` in task 4 — leave it inline here to keep this commit minimal.) Add `open_repository` to the `use crate::api::helpers::...` import at line 13.

5. **`backends/install.rs`** (`remove_backend`, lines 524-526) — replace the variant-A block with the `resolve_config_dir` match. `config_dir` is still used later (line ~643 `Repository::open(&config_dir)`), so keep the binding.

6. **`updates.rs`** — at all five variant-B sites (`get_updates:83-91`, `trigger_check:189-197`, `check_single:233-241`, `apply_backend_update:424-432`, `apply_model_update:654-662`) replace the `match state.db_dir().clone() { ... }` block with the `resolve_config_dir` match (wire change: 404 body becomes canonical nested — accepted, see Context). For the spawned task in `apply_backend_update` (currently lines 540-549): before `tokio::spawn(async move {`, add `let config_dir_clone = config_dir.clone();` and inside the task replace the whole `let config_dir = match tama_core::config::Config::base_dir() { ... }` block (lines 543-549) with `let config_dir = config_dir_clone;`. The task currently captures `jobs_clone`, `job_clone`, `name_clone` — add `config_dir_clone` to the captured set (it's `move`, so just referencing it inside suffices).

7. **`benchmarks/mod.rs`** — in `submit_benchmark_job` (rewritten in task 1), replace the `db_path` CWD-fallback block with:
   ```rust
   let db_path = match crate::api::helpers::resolve_config_dir(state) {
       Ok(d) => d.join("tama.db"),
       Err(resp) => return Err(resp),
   };
   ```
   (The helper's `Result<_, axum::response::Response>` error type from task 1 makes this slot in directly.)

8. **Tests** — create `#[cfg(test)] mod tests` at the bottom of `helpers.rs`:
   - `test_resolve_config_dir_prefers_db_dir`: `let state = tama_core::proxy::ProxyState::new(tama_core::config::Config::default(), Some(tmp.path().to_path_buf()));` (tempdir) → `resolve_config_dir(&state).unwrap() == tmp.path()`. Construction pattern per `crates/tama/src/api/backends/manage/tests.rs:23` (`ProxyState::new(config, db_dir)`).
   - `test_resolve_config_dir_falls_back_to_system_dir`: `ProxyState::new(Config::default(), None)` → `Ok` equal to `tama_core::config::Config::config_dir().unwrap()` (documents the fallback; the 404 branch is unreachable while the system config dir resolves, so it gets no unit test — note this in a comment).

**Steps:**
- [ ] Write the two failing tests in `crates/tama/src/api/helpers.rs`
- [ ] Run `cargo nextest run --package tama -- api::helpers` — verify failure (no tests exist / compile error)
- [ ] Implement the helpers in `helpers.rs`; migrate all sites per above
- [ ] Run `cargo check --package tama` — compiles (watch for unused imports: `Config` in aliases/mod.rs, list.rs, install.rs may lose its only use)
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: canonical resolve_config_dir/open_repository; remove CWD fallback"

**Acceptance criteria:**
- [ ] `rg 'PathBuf::from\("\."\)' crates/tama/src/api/` returns zero hits
- [ ] `rg "Config::base_dir\(\)" crates/tama/src/api/` returns zero hits
- [ ] Every former variant-A/B/C site resolves via `resolve_config_dir` or `open_repository`; updates.rs's five 404s use the canonical nested error shape
- [ ] `cargo nextest run --package tama` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 4: Move blocking SQLite off the async executor (F20)

**Context:**
The canonical patterns exist — `api/helpers.rs::spawn_model_crud`/`open_backend_manager` pool the blocking work, and `api/models/files.rs` documents "keep `rusqlite::Connection` off `.await` points" — but ~11 sites still run blocking SQLite directly on the executor. `Repository`/`BackendManager` are `Send` (they wrap `rusqlite::Connection`), so they can move into and out of `spawn_blocking` closures. Worst offender: `updates.rs:550` opens `BackendManager` inside a `tokio::spawn(async …)` task. Scope discipline: fix exactly the sites below; do **not** wrap the many fast `BackendManager` read calls in `backends/list.rs` (`list_versions`/`get_active` — only its `Repository` side is in the audit), do not touch `update_backend_with_progress` (tama-core internals), and do not add `jobs.finish` calls to the update task's error paths (its log-and-return behavior, sloppy as it is, is out of scope). Depends on tasks 1–3: benchmarks handlers are already unified (inner fns remain), `resolve_model_record` exists for `get_model`, and `resolve_config_dir`/`open_repository` exist for aliases/updates.

**Files:**
- Modify: `crates/tama/src/api/aliases/mod.rs`
- Modify: `crates/tama/src/api/updates.rs`
- Modify: `crates/tama/src/api/backends/list.rs`
- Modify: `crates/tama/src/api/backends/install.rs`
- Modify: `crates/tama/src/api/models/info.rs`
- Modify: `crates/tama/src/api/benchmarks/run.rs`
- Modify: `crates/tama/src/api/benchmarks/mtp.rs`
- Modify: `crates/tama/src/api/benchmarks/spec.rs`

**What to implement:**

1. **`aliases/mod.rs`** — in each of the 5 handlers move the `Repository::open` **and all `repo.*` calls that precede any `.await`** into one `tokio::task::spawn_blocking` closure returning `Result<T, (StatusCode, serde_json::Value)>`, then map with:
   ```rust
   let result = tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
       /* open + repo work */
   })
   .await;
   let value = match result {
       Ok(Ok(v)) => v,
       Ok(Err((s, b))) => return (s, Json(b)).into_response(),
       Err(e) => {
           return error_response_simple(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
       }
   };
   ```
   Preserve the exact current error mapping inside the closures (open failure → `error_body(format!("Database not configured: {}", e), None)` at 500 — note today's `error_response_simple` 500 for open failure becomes the tuple equivalent; same wire body). Specifics:
   - `list_aliases`: closure = open + `get_all_aliases`; respond `Json(aliases)` / existing 500.
   - `get_alias`: closure = open + `get_alias_by_id(id)`; 404/500 arms unchanged.
   - `create_alias`: keep `validate_alias_name` (pure) outside; closure = open + `model_exists` + `insert_alias` + `get_alias_by_id(new_id)`; the `state.reload_aliases().await` stays outside, **after** the closure (today it runs before the read-back — reordering is safe: the reload only refreshes ProxyState's cache from the DB, the read-back only reads the DB; response is unchanged). 201 CREATED status unchanged.
   - `update_alias`: keep both `validate_alias_name` checks outside; closure = open + `model_exists` (if provided) + `update_alias` + `get_alias_by_id(id)`; reload after.
   - `delete_alias`: closure = open + `delete_alias(id)`; reload after; `{"deleted": true}` response unchanged.
   Do **not** route these through `spawn_model_crud` — it would fire `trigger_proxy_reload` (these handlers already reload aliases explicitly) and flatten their distinct statuses (201, `{"deleted": true}`).

2. **`updates.rs::get_updates`** — the per-record `Repository::open(&config_dir)` inside the display-name lookup (current lines 127-136) becomes one pooled pre-pass. Before the `for r in records` loop, collect `let model_ids: Vec<i64> = records.iter().filter(|r| r.item_type == "model").filter_map(|r| r.item_id.parse::<i64>().ok()).collect();`, then:
   ```rust
   let config_dir_names = config_dir.clone();
   let display_names: std::collections::HashMap<i64, String> =
       match tokio::task::spawn_blocking(move || {
           let repo = Repository::open(&config_dir_names).ok()?;
           let mut map = std::collections::HashMap::new();
           for id in model_ids {
               if let Ok(Some(m)) = repo.get_model_config(id) {
                   if let Some(name) = m.display_name {
                       map.insert(id, name);
                   }
               }
           }
           Some(map)
       })
       .await
       {
           Ok(Some(m)) => m,
           _ => std::collections::HashMap::new(),
       };
   ```
   In the loop, the lookup becomes `let display_name = if r.item_type == "model" { r.item_id.parse::<i64>().ok().and_then(|id| display_names.get(&id).cloned()) } else { None };` — identical output, including the open-failure → all-`None` degradation.

3. **`updates.rs::apply_backend_update` spawned task** — after task 3, the task starts with `let config_dir = config_dir_clone;` then opens `BackendManager` and calls `list_versions` on the executor. Wrap both:
   ```rust
   let prep = tokio::task::spawn_blocking(move || -> Result<(BackendManager, Option<Vec<_>>), String> {
       let mgr = match BackendManager::open(&config_dir) {
           Ok(m) => m,
           Err(e) => return Err(format!("Failed to open backend manager: {}", e)),
       };
       match mgr.list_versions(&name_clone, None) {
           Ok(v) => Ok((mgr, v)),
           Err(e) => Err(format!(
               "Failed to list versions for backend '{}': {}",
               name_clone, e
           )),
       }
   })
   .await;
   let (mgr, all_versions) = match prep {
       Ok(Ok((m, Some(v)))) => (m, v),
       Ok(Ok((_, None))) => {
           tracing::error!("Backend '{}' not found during update", name_clone2);
           return;
       }
       Ok(Err(msg)) => {
           tracing::error!("{}", msg);
           return;
       }
       Err(e) => {
           tracing::error!("spawn error: {}", e);
           return;
       }
   };
   ```
   (Clone `name_clone` a second time as `name_clone2` for the not-found log, or restructure so the name is still available — the original messages must be reproduced verbatim; the log-and-`return` failure behavior is preserved exactly, including NOT finishing the job.) The subsequent `all_versions.first()`, `backends_dir()`, `InstallOptions` build, and `update_backend_with_progress(mgr, …)` call are untouched.

4. **`updates.rs::apply_model_update`** — wrap Phase 1 (the `Repository::open` at :752 + the `get_active_pull_by_filename` preflight loop) **and** Phase 2 (the `svc.enqueue` loop — same blocking-SQLite class, contiguous lines) in one closure:
   ```rust
   let enqueue_result = tokio::task::spawn_blocking(
       move || -> Result<Vec<String>, (StatusCode, serde_json::Value)> {
           let repo = Repository::open(&config_dir).map_err(|e| {
               (
                   StatusCode::INTERNAL_SERVER_ERROR,
                   serde_json::json!({ "error": format!("Queue check failed: {}", e) }),
               )
           })?;
           // Phase 1 preflight loop, verbatim (409 body with existing_job_id, 500 per-file body)
           // Phase 2 enqueue loop, verbatim (uuid::Uuid::new_v4() per job, 500 body on enqueue error)
           Ok(job_ids)
       },
   )
   .await;
   let job_ids = match enqueue_result {
       Ok(Ok(ids)) => ids,
       Ok(Err((s, b))) => return (s, Json(b)).into_response(),
       Err(e) => {
           return (
               StatusCode::INTERNAL_SERVER_ERROR,
               Json(serde_json::json!({ "error": format!("spawn error: {}", e) })),
           )
               .into_response()
       }
   };
   ```
   Move `unique_files`, `repo_id`, and a cloned `svc` (`let svc = svc.clone();` — `state.pull_queue()` returns `&Option<Arc<PullQueueService>>`, so `svc` is `&Arc<_>`; clone the `Arc`) into the closure. Every response body stays **flat** `{"error": ...}` exactly as today — plan-161 owns shape migration. The final `Json(ModelUpdateResponse { job_ids, total })` is unchanged.

5. **`backends/list.rs`** — wrap the `repo.get_all_update_checks()` call left inline by task 3:
   ```rust
   Ok(repo) => match tokio::task::spawn_blocking(move || repo.get_all_update_checks()).await {
       Ok(Ok(records)) => records
           .into_iter()
           .filter(|r| r.item_type == "backend")
           .map(|r| (r.item_id.clone(), r))
           .collect(),
       _ => std::collections::HashMap::new(),
   },
   ```

6. **`backends/install.rs::remove_backend`** — two pooled blocks, both returning `Result<T, axum::response::Response>` so the exact current error responses propagate (map with `Ok(Ok(v)) => v, Ok(Err(resp)) => return resp, Err(e) => return error_response_simple(500, format!("spawn error: {}", e))`):
   - Block 1 (replaces the two `mgr.list_versions` match arms, current lines 547-586): closure takes `mgr` + `name` + `gpu_variant` by move, runs the variant-specific/all-variants `list_versions` logic verbatim (404 `NotFoundError` / 500 bodies unchanged), returns `(mgr, backends_to_remove)`.
   - Block 2 (after the unchanged `jobs.active().await` conflict check): closure takes `mgr`, `backends_to_remove`, `name`, `gpu_variant`, `config_dir` by move and runs, verbatim: the `safe_remove_installation` loop (409 "outside the managed backends directory" / 500 bodies), `mgr.delete_all_versions(&name, variant_to_remove)`, and the `Repository::open(&config_dir)` update-check cleanup (LIKE-escaped pattern + legacy delete — best-effort, `if let Ok(repo)` preserved).
   - Final `Json(DeleteResponse { removed: true })` unchanged.

7. **`api/models/info.rs`** —
   (a) `build_backend_options` (lines 17-25) becomes async with the body pooled, preserving the silent-empty behavior:
   ```rust
   async fn build_backend_options(
       _cfg: &tama_core::config::Config,
       config_dir: &std::path::Path,
   ) -> Vec<BackendOption> {
       let config_dir = config_dir.to_path_buf();
       tokio::task::spawn_blocking(move || {
           let mgr = tama_core::backends::BackendManager::open(&config_dir).ok()?;
           mgr.available_backends().ok()
       })
       .await
       .ok()
       .flatten()
       .unwrap_or_default()
   }
   ```
   Update both callers (`list_models`, `get_model`) to `.await` it.
   (b) `list_models`: move the entire `let models = match Repository::open(&config_dir) { … }` block (current lines 147-181) into `tokio::task::spawn_blocking(move || { … })` (move clones of `config_dir` and `configs_dir` in), returning the `Vec<serde_json::Value>`; the `Err(e) => { tracing::error!(...); Vec::new() }` degradation is preserved inside the closure, and a `JoinError` maps to the same `tracing::error!` + empty vec.
   (c) `get_model`: replace the inline open + `resolve_model_id` + `.ok().flatten()` + `load_repo_db_meta_from_repo` sequence (current lines 210-262) with one closure:
   ```rust
   let resolved = tokio::task::spawn_blocking(
       move || -> Result<_, (StatusCode, serde_json::Value)> {
           let (repo, _model_id, record) = resolve_model_record(&config_dir, &id_str)?;
           let m = tama_core::config::ModelConfig::from_db_record_for_repo(&record);
           let meta = load_repo_db_meta_from_repo(&repo, record.id);
           Ok((record, m, meta))
       },
   )
   .await;
   let (record, m, meta) = match resolved {
       Ok(Ok(v)) => v,
       Ok(Err((s, b))) => return (s, Json(b)).into_response(),
       Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
   };
   ```
   The quants-population loop, `model_entry_json`, and `backends` injection stay in async context (pure computation). Accepted behavior change (spell out in the commit body): a `get_model_config` **DB error** now maps to 500 (via `resolve_model_record`) instead of 404 — previously `.ok().flatten()` mislabeled errors as "not found"; unknown ids still 404, unresolvable ids still 400 ValidationError.

8. **`benchmarks/{run,mtp,spec}.rs` inner fns** — pool the remaining blocking calls (each inner fn runs inside `tokio::spawn`, so this matters):
   - `run.rs`: wrap `Repository::open(db_dir)?` + `load_model_configs_for_benchmarks()` (current lines 147-152) in `spawn_blocking` returning `(repo, model_configs)` (both `Send`); wrap the final `repo.insert_benchmark(&params)` in a second `spawn_blocking` moving `repo` and the fully-constructed `BenchmarkParams` in (construct `params` in async context exactly as today).
   - `mtp.rs`: pool three segments — (i) `Repository::open` + `load_model_configs_for_benchmarks` + `config.resolve_backend(...)` + `resolve_model_path(&config, db_dir, &repo, …)` + display-name lookup (current lines 142-170), returning `(server_config, model_path, display_name)` (move a `config.clone()` and `db_dir.to_path_buf()` in; `Config` is `Clone`); (ii) `BackendManager::open(db_dir)?` + `config.resolve_backend_path(target_backend, gpu_variant.as_deref(), &manager)` (current lines 195-197) returning `backend_path`; (iii) the second `Repository::open` + `insert_benchmark` (current lines 224-258).
   - `spec.rs`: same three segments (open+load+resolve at :139-165, manager+resolve_backend_path — locate the matching lines after the task-1 edit, insert at :236+).
   - All closures return `anyhow::Result<T>` and are awaited with `.await?` twice (`JoinError` → `anyhow`): `let x = tokio::task::spawn_blocking(move || -> anyhow::Result<_> { … }).await??;` matching the inner fns' existing `Result<()>` error flow.

**Steps:**
- [ ] Run `cargo nextest run --package tama` — record the green baseline
- [ ] Apply the aliases changes; run `cargo check --package tama`
- [ ] Apply the updates.rs changes (get_updates, apply_backend_update task, apply_model_update); run `cargo check --package tama`
- [ ] Apply the list.rs / install.rs / info.rs changes; run `cargo check --package tama`
- [ ] Apply the benchmarks inner-fn changes; run `cargo check --package tama`
- [ ] Run `cargo nextest run --package tama` — all pass, same count as baseline (no test-body edits anywhere)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: move blocking SQLite calls off the async executor"

**Acceptance criteria:**
- [ ] `rg "Repository::open|BackendManager::open" crates/tama/src/api/` — every remaining hit is either inside a `spawn_blocking` closure, inside `open_repository`/`open_backend_manager`/`resolve_model_record` (the pooled helpers), or in tama-core library code
- [ ] `updates.rs` no longer contains `BackendManager::open` outside a `spawn_blocking` (the former `tokio::spawn(async …)` site is fixed)
- [ ] All response statuses/bodies identical to before, except the documented `get_model` DB-error 404→500 correction
- [ ] `cargo nextest run --package tama` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 5: Small duplication batch — backoff, mean/stddev, traversal guard, model editor (F37)

**Context:**
Four independent small dedups, landed as one commit series but specified as separate sub-items (they may be committed separately if preferred — each is self-contained). (a) `jitter()` + `exponential_backoff()` are verbatim in `crates/tama-core/src/models/pull/single.rs:13-24` and `parallel.rs:16-27` (only `exponential_backoff` is called from outside — `jitter` is used solely inside `exponential_backoff`, verified); hoist to `pull/mod.rs`. (b) mean/stddev is computed 4× inline in `crates/tama-core/src/bench/mod.rs:222-258` (`compute_summary` — pp/tg/ttft/total) plus once as `compute_mean_stddev` at `bench/llama_cli_spec/mod.rs:285-300` (callers at :409 and :634); extract one `mean_stddev`. Note `compute_summary` early-returns on empty input (line 191), and the shared fn's `count == 0 → (0.0, 0.0)` branch matches `compute_mean_stddev` exactly — no behavior change. (c) The path-traversal guard `x.contains('/') || x.contains('\\') || x.contains("..")` appears at 12 sites under `crates/tama/src/api/backends/` with divergent messages and two non-canonical flat bodies; add `reject_traversal`. Accepted wire changes: `manage/source.rs:25` and `manage/activate.rs:22` gain the longer message ("Invalid backend name" → "Invalid backend name: path separators or traversal sequences not allowed"); `install.rs:529` and `list.rs:547` switch from flat `{"error": …}` to the canonical nested body with `ValidationError` type; `install.rs:55` ("version must be a single path segment (no slashes or '..')") → "Invalid version: path separators or traversal sequences not allowed". All remain 400s. (d) `crates/tama/src/pages/model_editor/api.rs`: `form_to_sampling_json` (lines 60-119) repeats one parse-insert block ×7 (6× f64 + `top_k` as u64) — table-drive it; the status-check/error-body tail repeats in 6 fns (`save_model:163`, `rename_model:182`, `delete_model_api:197`, `delete_quant_api:212`, `refresh_model_api:225`, `verify_model_api:238` — the audit said ×5 but `verify_model_api` is a 6th identical tail) — extract `expect_status`. `fetch_gpu_devices`/`refresh_gpu_devices` use a different `unwrap_or_default` pattern — leave them.

**Files:**
- Modify: `crates/tama-core/src/models/pull/mod.rs`
- Modify: `crates/tama-core/src/models/pull/single.rs`
- Modify: `crates/tama-core/src/models/pull/parallel.rs`
- Modify: `crates/tama-core/src/bench/mod.rs`
- Modify: `crates/tama-core/src/bench/llama_cli_spec/mod.rs`
- Modify: `crates/tama/src/api/backends/mod.rs`
- Modify: `crates/tama/src/api/backends/manage/{config,update,source,activate,remove}.rs`
- Modify: `crates/tama/src/api/backends/install.rs`
- Modify: `crates/tama/src/api/backends/list.rs`
- Modify: `crates/tama/src/pages/model_editor/api.rs`

**What to implement:**

1. **Pull backoff** — add to `crates/tama-core/src/models/pull/mod.rs` (near `parse_content_length`):
   ```rust
   /// Random jitter in milliseconds (0..=500), adapted from hf_transfer.
   fn jitter() -> u64 {
       rand::rng().random_range(0..=500)
   }

   /// Exponential backoff with jitter, adapted from hf_transfer.
   /// Base: 300ms, max: 10000ms.
   pub(super) fn exponential_backoff(attempt: u32) -> std::time::Duration {
       let base = 300 + (attempt as u64).pow(2) + jitter();
       std::time::Duration::from_millis(base.min(10_000))
   }
   ```
   (`mod.rs` needs `use rand::Rng;` — check it isn't already imported.) Delete both copies from `single.rs:13-24` and `parallel.rs:16-27`; extend their existing `use super::{ProgressCallback, MAX_RETRIES};` to `use super::{exponential_backoff, ProgressCallback, MAX_RETRIES};`. Remove now-unused `use rand::Rng;`/`use std::time::Duration;` from the two files **only if** cargo check reports them unused. Add a unit test in `pull/mod.rs`'s existing `mod tests`: `test_exponential_backoff_bounds` — attempt 0 → between 300ms and 800ms inclusive; attempt 100 → exactly 10_000ms (cap). Do not assert monotonicity (jitter).

2. **Mean/stddev** — add to `crates/tama-core/src/bench/mod.rs` (before `compute_summary`):
   ```rust
   /// Compute mean and population stddev from a slice of f64 values.
   /// Returns (0.0, 0.0) for an empty slice; stddev is 0.0 for a single value.
   pub(crate) fn mean_stddev(values: &[f64]) -> (f64, f64) {
       let count = values.len();
       if count == 0 {
           return (0.0, 0.0);
       }
       let mean = values.iter().sum::<f64>() / count as f64;
       let stddev = if count == 1 {
           0.0
       } else {
           let variance: f64 =
               values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count as f64;
           variance.sqrt()
       };
       (mean, stddev)
   }
   ```
   In `compute_summary` replace the four mean blocks and four variance blocks (current lines 222-258) with:
   ```rust
   let (pp_mean, pp_stddev) = mean_stddev(&pp_values);
   let (tg_mean, tg_stddev) = mean_stddev(&tg_values);
   let (ttft_mean, ttft_stddev) = mean_stddev(&ttft_values);
   let (total_mean, total_stddev) = mean_stddev(&total_values);
   ```
   (The `count` local at line 190 stays — it's used by the early return.) In `llama_cli_spec/mod.rs` delete `compute_mean_stddev` (lines 285-300); change call sites :409 and :634 to `super::mean_stddev(&timings)` / `super::mean_stddev(&baseline_timings)`. **Move** its three tests (`test_compute_mean_stddev_basic`, `_empty`, `_single` at lines 858-881) into `bench/mod.rs`'s existing `mod tests` (line 319), renamed `test_mean_stddev_basic/empty/single`, calling `mean_stddev`.

3. **Traversal guard** — add to `crates/tama/src/api/backends/mod.rs` (after the re-export block):
   ```rust
   /// Returns true if a path parameter contains separators or traversal sequences.
   pub fn is_path_traversal(value: &str) -> bool {
       value.contains('/') || value.contains('\\') || value.contains("..")
   }

   /// Reject path parameters containing separators/traversal with the canonical
   /// 400 ValidationError response. `field` is the human-readable parameter name
   /// (e.g. "backend name", "version", "gpu_variant").
   pub fn reject_traversal(value: &str, field: &str) -> Result<(), axum::response::Response> {
       if is_path_traversal(value) {
           Err(crate::api::error::error_response(
               axum::http::StatusCode::BAD_REQUEST,
               format!(
                   "Invalid {}: path separators or traversal sequences not allowed",
                   field
               ),
               Some("ValidationError"),
           ))
       } else {
           Ok(())
       }
   }
   ```
   Migrate the 12 sites — each `if x.contains(…) { return error_response(…); }` block becomes `if let Err(resp) = super::reject_traversal(&x, "field") { return resp; }` (adjust the path prefix per module depth: `super::` from `manage/*.rs`, `crate::api::backends::reject_traversal` from `install.rs`/`list.rs`): `manage/config.rs:25,77,129` (field "backend name"), `manage/update.rs:24` ("backend name"), `manage/source.rs:25` ("backend name"), `manage/activate.rs:22` ("backend name"), `manage/remove.rs:23` ("backend name") and `:30` ("version"), `install.rs:55` ("version") and `:529` ("backend name"), `list.rs:547` ("backend name"). The 12th site, `manage/source.rs:77`, is inside an `anyhow` closure — keep its message verbatim but use the predicate:
   ```rust
   if super::is_path_traversal(&gpu_variant) {
       return Err(anyhow::anyhow!(
           "Invalid gpu_variant: path separators or traversal sequences not allowed"
       ));
   }
   ```
   Tests: add `#[cfg(test)] mod tests` in `backends/mod.rs` — `test_is_path_traversal` (accepts "llama_cpp", "1.2.3"; rejects "a/b", "a\\b", "..", "a..b"), `test_reject_traversal_returns_400` (`.unwrap_err().status() == StatusCode::BAD_REQUEST` for "../x"; `.is_ok()` for "llama_cpp").

4. **Model editor** — in `crates/tama/src/pages/model_editor/api.rs`:
   (a) Replace the 7 blocks in `form_to_sampling_json` with a table:
   ```rust
   /// Sampling form fields and their JSON value kind. `top_k` is an integer;
   /// all others are floats.
   const SAMPLING_FIELDS: &[(&str, SamplingKind)] = &[
       ("temperature", SamplingKind::Float),
       ("top_k", SamplingKind::Int),
       ("top_p", SamplingKind::Float),
       ("min_p", SamplingKind::Float),
       ("presence_penalty", SamplingKind::Float),
       ("frequency_penalty", SamplingKind::Float),
       ("repeat_penalty", SamplingKind::Float),
   ];

   enum SamplingKind {
       Float,
       Int,
   }
   ```
   and a loop: for each `(key, kind)`, `if let Some(field) = form.sampling.get(*key)` and `field.enabled`, parse per kind (`field.value.parse::<f64>()` / `parse::<u64>()`), and on `Ok(val)` `obj.insert(key.to_string(), serde_json::json!(val))`. The `obj.is_empty() → Value::Null` tail is unchanged. Table order matches the original insertion order.
   (b) Add the fetch-tail helper and use it in 6 fns:
   ```rust
   /// Return the response when its status is one of `ok_statuses`,
   /// otherwise Err with the response body text.
   async fn expect_status(
       resp: gloo_net::http::Response,
       ok_statuses: &[u16],
   ) -> Result<gloo_net::http::Response, String> {
       if ok_statuses.contains(&resp.status()) {
           Ok(resp)
       } else {
           let text = resp.text().await.unwrap_or_else(|_| "Unknown error".into());
           Err(text)
       }
   }
   ```
   `save_model`: `let resp = expect_status(resp, &[200, 201]).await?; Ok(())` (it is the only one accepting 201). `rename_model`/`delete_model_api`/`delete_quant_api`: `expect_status(resp, &[200]).await?; Ok(())`. `refresh_model_api`/`verify_model_api`: `let resp = expect_status(resp, &[200]).await?;` then the existing `.json::<…>()` parse tail. Note the current `!= 200` phrasing in refresh/verify inverts to the same thing — bodies identical.
   (c) Tests: `form_to_sampling_json` is pure and the crate compiles `pages/` natively (default features include `ssr`; existing native tests live in `pages/keys`, `pages/dashboard`, etc.), so add `#[cfg(test)] mod tests` in `model_editor/api.rs`: build forms via `ModelForm { sampling: [("temperature".to_string(), SamplingField { enabled: true, value: "0.7".to_string() })].into_iter().collect(), ..Default::default() }` (`ModelForm` derives `Default`; `SamplingField` is at `pages/model_editor/types.rs:113`). Cases: enabled float inserted as `json!(0.7)`; `top_k` `"40"` inserted as `json!(40u64)` (integer, not `40.0`); disabled field skipped; unparseable value (`"abc"`) skipped; all-empty → `serde_json::Value::Null`. `expect_status` gets **no** unit test (constructing a `gloo_net::http::Response` requires a browser runtime) — compile-checked only; note this in a comment.

**Steps:**
- [ ] Write the failing tests: `pull/mod.rs` backoff bounds; `bench/mod.rs` 3 moved mean_stddev tests; `backends/mod.rs` guard tests; `model_editor/api.rs` form tests
- [ ] Run `cargo nextest run --package tama-core -- models::pull` and `cargo nextest run --package tama-core -- bench` — verify the new tests fail (missing fns)
- [ ] Implement items 1–2 (tama-core); run `cargo nextest run --package tama-core -- models::pull` and `-- bench` — pass
- [ ] Implement item 3 (traversal guard); run `cargo nextest run --package tama -- api::backends` — pass
- [ ] Implement item 4 (model editor); run `cargo nextest run --package tama -- pages::model_editor` — pass
- [ ] Run `cargo nextest run --workspace` — full suite green (tama-core changes have cross-crate reach)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: dedupe backoff, mean/stddev, traversal guard, model editor boilerplate"

**Acceptance criteria:**
- [ ] `rg "fn jitter|fn exponential_backoff" crates/` → one definition (`pull/mod.rs`); `rg "compute_mean_stddev" crates/` → zero hits; `rg "mean_stddev" crates/tama-core/src/bench/` → definition + 6 call/test sites
- [ ] `rg 'contains\("/"\)|contains\('"'"'/'"'"'\)' crates/tama/src/api/backends/` → hits only inside `is_path_traversal`
- [ ] `form_to_sampling_json` contains a single parse-insert loop; the 6 fetch fns share `expect_status`; all 12 traversal sites route through `reject_traversal`/`is_path_traversal`
- [ ] `cargo nextest run --workspace` passes; `cargo clippy --workspace -- -D warnings` clean
