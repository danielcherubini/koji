# Codebase Improvements Plan

**Goal:** Execute quick-win and medium-effort improvements from the 2026-07-06 codebase audit — eliminate duplicated boilerplate, remove dead code, fix domain term violations, and standardize patterns.

**Architecture:** 7 independent tasks, each commitable on its own. No task depends on another. Large architectural efforts (file splits, DB repository layer, ProxyState encapsulation, lifecycle testability) are deferred to follow-up plans.

**Tech Stack:** Rust, Axum, Leptos, SQLite (rusqlite)

---

### Task 1: Error response helper + structured error format

**Context:**
Finding #7 + #23 from the audit. The pattern `(Json(serde_json::json!({"error": e.to_string()}))).into_response()` is repeated 200+ times across the API layer. Some handlers use flat `"error": "string"` while others use structured `{"error": {"message": "...", "type": "..."}}`. This task creates a shared helper and standardizes on the structured format.

**Files:**
- Create: `crates/tama/src/api/error.rs`
- Modify: `crates/tama/src/api/mod.rs` (export new module)
- Modify: All files in `crates/tama/src/api/` that use `serde_json::json!({"error": ...})` pattern (updates.rs, aliases/mod.rs, backends/manage.rs, backends/install.rs, backends/list.rs, models/crud/*.rs, models/files.rs, models/info.rs, benchmarks/*.rs, downloads.rs, backup.rs, pull/*.rs)

**What to implement:**

1. Create `crates/tama/src/api/error.rs` with:
   ```rust
   use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
   use serde::Serialize;

   #[derive(Serialize)]
   pub struct ErrorResponse {
       pub error: ErrorDetail,
   }

   #[derive(Serialize)]
   pub struct ErrorDetail {
       pub message: String,
       #[serde(skip_serializing_if = "Option::is_none")]
       pub r#type: Option<String>,
   }

   /// Create a structured error response.
   /// Usage: `error_response(StatusCode::NOT_FOUND, "Model not found", Some("NotFoundError"))`
   /// or:    `error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)`
   pub fn error_response(status: StatusCode, message: impl Into<String>, error_type: Option<&str>) -> Response {
       let body = ErrorResponse {
           error: ErrorDetail {
               message: message.into(),
               r#type: error_type.map(|s| s.to_string()),
           },
       };
       (status, Json(body)).into_response()
   }

   /// Simple error response without type field (for generic errors).
   pub fn error_response_simple(status: StatusCode, message: impl Into<String>) -> Response {
       error_response(status, message, None)
   }
   ```

2. Add `pub mod error;` to `crates/tama/src/api/mod.rs`.

3. Replace ALL occurrences of `(Json(serde_json::json!({"error": ...}))).into_response()` with calls to `error_response()` or `error_response_simple()`. Use the structured format with error type where the existing code already uses structured errors (models.rs, pull/handlers.rs), and simple format elsewhere.

4. Define consistent error type names:
   - `"NotFoundError"` for 404s
   - `"ValidationError"` for 400s
   - `"ConflictError"` for 409s
   - `"ServiceUnavailableError"` for 503s
   - `"BackendError"` for backend-specific errors
   - `None` for generic internal server errors

**Steps:**
- [ ] Create `crates/tama/src/api/error.rs` with `ErrorResponse`, `ErrorDetail`, `error_response()`, `error_response_simple()`
- [ ] Add `pub mod error;` to `crates/tama/src/api/mod.rs`
- [ ] Run `cargo build --workspace` to verify new module compiles
- [ ] Replace error patterns in `crates/tama/src/api/updates.rs`
- [ ] Replace error patterns in `crates/tama/src/api/aliases/mod.rs`
- [ ] Replace error patterns in `crates/tama/src/api/backends/manage.rs`
- [ ] Replace error patterns in `crates/tama/src/api/backends/install.rs`
- [ ] Replace error patterns in `crates/tama/src/api/backends/list.rs`
- [ ] Replace error patterns in `crates/tama/src/api/models/crud/create.rs`
- [ ] Replace error patterns in `crates/tama/src/api/models/crud/update.rs`
- [ ] Replace error patterns in `crates/tama/src/api/models/crud/rename.rs`
- [ ] Replace error patterns in `crates/tama/src/api/models/crud/delete.rs`
- [ ] Replace error patterns in `crates/tama/src/api/models/files.rs`
- [ ] Replace error patterns in `crates/tama/src/api/models/info.rs`
- [ ] Replace error patterns in `crates/tama/src/api/benchmarks/*.rs`
- [ ] Replace error patterns in `crates/tama/src/api/downloads.rs`
- [ ] Replace error patterns in `crates/tama/src/api/backup.rs`
- [ ] Replace error patterns in `crates/tama/src/api/pull/*.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: extract error_response helper and standardize structured error format"

**Acceptance criteria:**
- [ ] `error_response()` and `error_response_simple()` exist in `api/error.rs`
- [ ] Zero occurrences of `serde_json::json!({"error":` remain in `crates/tama/src/api/`
- [ ] All error responses use the structured `{"error": {"message": "...", "type": "..."}}` format
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 2: Replace `unwrap()` in production code + remove dead code

**Context:**
Finding #24 + #27 + #28 from the audit. Four `unwrap()` calls in production code (SSE event building, Response building) can panic. Two modules (`network.rs`, `logging.rs` functions) are entirely dead. This task is low-risk cleanup.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs` (line 108 — Response::builder().unwrap())
- Modify: `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs` (lines 155, 165 — SSE Event.json_data().unwrap())
- Modify: `crates/tama-core/src/web_types.rs` (line 129 — active.as_ref().unwrap())
- Delete: `crates/tama-core/src/network.rs`
- Modify: `crates/tama-core/src/lib.rs` (remove `pub mod network;`)
- Modify: `crates/tama-core/src/logging.rs` (remove or mark `#[deprecated]` the unused `init()`, `init_with_file()`, `log_path()`)

**What to implement:**

1. In `system.rs:108`, replace `.unwrap()` with `.expect("Response::builder with valid status and body should not fail")`.

2. In `backend_logs.rs:155,165`, replace `.unwrap()` with `.expect("SSE Event json_data serialization should not fail for valid JSON")`.

3. In `web_types.rs:129`, replace `.unwrap()` with `.cloned().unwrap_or_default()` or handle the None case explicitly.

4. Delete `crates/tama-core/src/network.rs` entirely (220 lines, zero consumers). Remove `pub mod network;` from `lib.rs`.

5. In `logging.rs`, either:
   - Remove `init()`, `init_with_file()`, `log_path()` if they serve no purpose, OR
   - Add `#[deprecated(note = "not used — remove in next major version")]` if they might be needed externally

**Steps:**
- [ ] Replace `.unwrap()` with `.expect("descriptive message")` in `system.rs:108`
- [ ] Replace `.unwrap()` with `.expect("descriptive message")` in `backend_logs.rs:155,165`
- [ ] Replace `.unwrap()` with safe alternative in `web_types.rs:129`
- [ ] Delete `crates/tama-core/src/network.rs`
- [ ] Remove `pub mod network;` from `crates/tama-core/src/lib.rs`
- [ ] Remove or deprecate unused functions in `crates/tama-core/src/logging.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: replace unwrap with expect in production code, remove dead network and logging modules"

**Acceptance criteria:**
- [ ] Zero `unwrap()` calls in non-test production code in `tama-core/src/proxy/` and `web_types.rs`
- [ ] `network.rs` deleted and removed from `lib.rs`
- [ ] `logging.rs` unused functions removed or deprecated
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 3: Extract benchmark job submission + ProgressSink DRY

**Context:**
Finding #6 from the audit. Three benchmark handler files (`run.rs`, `spec.rs`, `mtp.rs`) copy-paste ~60 lines of job submission boilerplate and ~45 lines of ProgressSink. This task extracts shared helpers.

**Files:**
- Modify: `crates/tama/src/api/benchmarks/mod.rs` (add shared helpers)
- Modify: `crates/tama/src/api/benchmarks/run.rs` (use shared helpers)
- Modify: `crates/tama/src/api/benchmarks/spec.rs` (use shared helpers)
- Modify: `crates/tama/src/api/benchmarks/mtp.rs` (use shared helpers)

**What to implement:**

1. In `mod.rs`, add a generic `BenchmarkProgressSink` struct:
   ```rust
   pub struct BenchmarkProgressSink {
       pub name: &'static str,
       pub job: Arc<tama_core::web_types::Job>,
       pub jobs: Arc<JobManager>,
   }
   
   impl tama_core::backends::ProgressSink for BenchmarkProgressSink {
       fn log(&self, line: &str) {
           tracing::debug!("[{}] {}", self.name, line);
       }
       fn result(&self, json: &str) {
           tracing::info!("[{}] result: {}", self.name, json);
       }
   }
   ```

2. In `mod.rs`, add a generic job submission helper:
   ```rust
   pub async fn submit_benchmark_job<F, Fut>(
       state: &ProxyState,
       req: axum::http::Request<axum::body::Body>,
       job_kind: JobKind,
       run_inner: F,
   ) -> axum::response::Response
   where
       F: FnOnce(Arc<JobManager>, &Job, &axum::http::Request<axum::body::Body>, PathBuf, &str, reqwest::Client) -> Fut,
       Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
   {
       // ... extract the common boilerplate (jobs.submit, tokio::spawn, jobs.finish)
   }
   ```

3. In each of `run.rs`, `spec.rs`, `mtp.rs`, replace the copy-pasted boilerplate with calls to the shared helpers.

**Steps:**
- [ ] Read all three benchmark files to understand the exact copy-pasted patterns
- [ ] Add `BenchmarkProgressSink` to `mod.rs`
- [ ] Add `submit_benchmark_job()` helper to `mod.rs`
- [ ] Refactor `run.rs` to use shared helpers
- [ ] Refactor `spec.rs` to use shared helpers
- [ ] Refactor `mtp.rs` to use shared helpers
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: extract shared benchmark job submission and ProgressSink helpers"

**Acceptance criteria:**
- [ ] `BenchmarkProgressSink` in `mod.rs` replaces 3 duplicate struct definitions
- [ ] `submit_benchmark_job()` in `mod.rs` replaces 3 duplicate boilerplate blocks
- [ ] Each benchmark file is significantly shorter (net reduction of ~100 lines total)
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 4: Extract `open_backend_manager` + CRUD spawn_blocking helpers

**Context:**
Finding #18 + #19 from the audit. `BackendManager::open` boilerplate repeated 16 times across 4 files. Model CRUD `spawn_blocking` → `match` → `trigger_proxy_reload` → `Json(val)` pattern repeated across 4 files.

**Files:**
- Create: `crates/tama/src/api/helpers.rs`
- Modify: `crates/tama/src/api/mod.rs` (export new module)
- Modify: `crates/tama/src/api/backends/manage.rs` (use helpers)
- Modify: `crates/tama/src/api/backends/list.rs` (use helpers)
- Modify: `crates/tama/src/api/backends/install.rs` (use helpers)
- Modify: `crates/tama/src/api/updates.rs` (use helpers)
- Modify: `crates/tama/src/api/models/crud/create.rs` (use helpers)
- Modify: `crates/tama/src/api/models/crud/update.rs` (use helpers)
- Modify: `crates/tama/src/api/models/crud/rename.rs` (use helpers)
- Modify: `crates/tama/src/api/models/crud/delete.rs` (use helpers)

**What to implement:**

1. Create `crates/tama/src/api/helpers.rs` with:
   ```rust
   /// Open a BackendManager from ProxyState, returning an error response on failure.
   pub async fn open_backend_manager(state: &ProxyState) -> Result<BackendManager, axum::response::Response> {
       let config_dir = state.db_dir.clone().unwrap_or_else(|| {
           tama_core::config::Config::config_dir()
               .unwrap_or_else(|_| std::path::PathBuf::from("."))
       });
       let config_dir_clone = config_dir.clone();
       tokio::task::spawn_blocking(move || BackendManager::open(&config_dir_clone))
           .await
           .map_err(|e| error_response_simple(StatusCode::INTERNAL_SERVER_ERROR, format!("spawn error: {}", e)))?
           .map_err(|e| error_response_simple(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
   }
   
   /// Run a closure in spawn_blocking, handle the Result, trigger proxy reload on success.
   pub async fn spawn_model_crud<F, T>(
       state: &ProxyState,
       f: F,
   ) -> axum::response::Response
   where
       F: FnOnce() -> Result<serde_json::Value, (StatusCode, serde_json::Value)> + Send + 'static,
       T: Send + 'static,
   {
       match tokio::task::spawn_blocking(f).await {
           Ok(Ok(val)) => {
               if let Err(e) = trigger_proxy_reload(state).await {
                   tracing::warn!("failed to trigger proxy reload: {}", e);
               }
               Json(val).into_response()
           }
           Ok(Err((status, body))) => (status, Json(body)).into_response(),
           Err(e) => error_response_simple(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
       }
   }
   ```

2. Replace all 16 occurrences of the `BackendManager::open` pattern with `open_backend_manager(&state)`.

3. Replace all 4 occurrences of the CRUD `spawn_blocking` pattern with `spawn_model_crud(&state, || { ... })`.

**Steps:**
- [ ] Read the exact patterns in each file to understand the boilerplate
- [ ] Create `crates/tama/src/api/helpers.rs` with both helpers
- [ ] Add `pub mod helpers;` to `crates/tama/src/api/mod.rs`
- [ ] Refactor `backends/manage.rs` to use `open_backend_manager()`
- [ ] Refactor `backends/list.rs` to use `open_backend_manager()`
- [ ] Refactor `backends/install.rs` to use `open_backend_manager()`
- [ ] Refactor `updates.rs` to use `open_backend_manager()`
- [ ] Refactor `models/crud/create.rs` to use `spawn_model_crud()`
- [ ] Refactor `models/crud/update.rs` to use `spawn_model_crud()`
- [ ] Refactor `models/crud/rename.rs` to use `spawn_model_crud()`
- [ ] Refactor `models/crud/delete.rs` to use `spawn_model_crud()`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: extract open_backend_manager and spawn_model_crud helpers"

**Acceptance criteria:**
- [ ] `open_backend_manager()` replaces all 16 occurrences of the BackendManager::open pattern
- [ ] `spawn_model_crud()` replaces all 4 occurrences of the CRUD spawn_blocking pattern
- [ ] No direct `BackendManager::open` calls remain in `crates/tama/src/api/`
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 5: Rename `gpu_type` to `gpu_variant` (domain term fix)

**Context:**
Finding #12 from the audit. CONTEXT.md defines **gpu_variant** as the domain term and forbids "GPU type". Yet `gpu_type` is a struct field, DB column, and appears in 10+ files. Additionally, `BackendInfo` has BOTH `gpu_type: Option<GpuType>` and `gpu_variant: String` — structural confusion that needs resolution.

**Files:**
- Modify: `crates/tama-core/src/backends/types.rs` (rename `gpu_type` field, resolve overlap with `gpu_variant`)
- Modify: `crates/tama-core/src/db/backfill/mod.rs` (rename `gpu_type` field)
- Modify: `crates/tama-core/src/backends/installer/mod.rs` (rename `gpu_type` field)
- Modify: `crates/tama-core/src/backends/installer/prebuilt.rs` (update field usage)
- Modify: `crates/tama-core/src/backup/archive.rs` (rename DB column references)
- Modify: `crates/tama-core/src/backup/merge.rs` (rename SQL column references)
- Modify: `crates/tama-core/src/db/backfill/initial_backfill.rs` (rename variable/column)
- Modify: `crates/tama-core/src/config/resolve/tests/path_resolution.rs` (rename test data field)
- Create: `crates/tama-core/src/db/migrations/_00XX_rename_gpu_type_to_gpu_variant.rs` (DB migration)
- Modify: `crates/tama-core/src/db/migrations/mod.rs` (register new migration)

**What to implement:**

1. First, resolve the `BackendInfo` struct overlap: read the struct to understand if `gpu_type` and `gpu_variant` serve different purposes. If `gpu_type` is redundant with `gpu_variant`, remove it. If they serve different purposes, rename `gpu_type` to something more specific (e.g., `gpu_arch` or `gpu_family`).

2. Rename all `gpu_type` fields to `gpu_variant` (or the chosen name) across all structs.

3. Update all usages of the field across the codebase.

4. Create a DB migration that renames the `gpu_type` column to `gpu_variant` in the `backend_installations` table (and any other tables that have it).

5. Update backup/merge SQL to use the new column name.

**Steps:**
- [ ] Read `backends/types.rs` to understand the `gpu_type` vs `gpu_variant` overlap in `BackendInfo`
- [ ] Decide: remove redundant field or rename to distinct name. Document decision in the code.
- [ ] Rename `gpu_type` field in `BackendOptions`, `BackfillInfo`, `BackendInstallerOptions`
- [ ] Update all field usages in `prebuilt.rs`, `initial_backfill.rs`, test files
- [ ] Update DB column references in `archive.rs` and `merge.rs`
- [ ] Create DB migration to rename `gpu_type` → `gpu_variant` column in `backend_installations`
- [ ] Register new migration in `db/migrations/mod.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: rename gpu_type to gpu_variant to match domain terminology"

**Acceptance criteria:**
- [ ] Zero occurrences of `gpu_type` remain in struct fields and variable names (DB column names in migrations are historical)
- [ ] `BackendInfo` struct has no overlapping/confusing GPU fields
- [ ] DB migration renames `gpu_type` column to `gpu_variant`
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 6: Rename `server` to `backend` in proxy/ (domain term fix)

**Context:**
Finding #13 from the audit. CONTEXT.md defines **Backend** = inference engine binary and forbids "server". Yet `server_name`, `resolve_server()`, `get_available_server_for_model()`, `server_ready`, and 20+ comments use the forbidden term.

**Files:**
- Modify: `crates/tama-core/src/proxy/state.rs` (rename `server_name` params, `resolve_server`, `get_available_server_for_model`)
- Modify: `crates/tama-core/src/proxy/status.rs` (update comments)
- Modify: `crates/tama-core/src/proxy/types.rs` (update comments, loop variables)
- Modify: `crates/tama-core/src/config/resolve/mod.rs` (rename `service_name` param)
- Modify: `crates/tama-core/src/process.rs` (rename `server_ready`)
- Modify: All test files that reference the renamed functions

**What to implement:**

1. In `proxy/state.rs`:
   - Rename parameter `server_name: &str` → `backend_name: &str` in all methods
   - Rename `resolve_server` → `resolve_backend`
   - Rename `get_available_server_for_model` → `get_available_backend_for_model`
   - Update doc comments

2. In `proxy/status.rs`:
   - Update all comments: "server" → "backend" (e.g., "resolves its backends", "backend entries", "backend's inference stats")

3. In `proxy/types.rs`:
   - Update comments: "per-server inference stats" → "per-backend inference stats"
   - Rename loop variable `_server` → `_backend`

4. In `config/resolve/mod.rs`:
   - Rename `service_name(server_name: &str)` → `service_name(backend_name: &str)`

5. In `process.rs`:
   - Rename `server_ready` → `backend_ready`

6. Update all callers and test files.

**Steps:**
- [ ] Rename `server_name` → `backend_name` in `proxy/state.rs` methods and all callers
- [ ] Rename `resolve_server` → `resolve_backend` and all callers
- [ ] Rename `get_available_server_for_model` → `get_available_backend_for_model` and all callers
- [ ] Update comments in `proxy/status.rs` ("server" → "backend")
- [ ] Update comments in `proxy/types.rs` ("server" → "backend")
- [ ] Rename `service_name` param in `config/resolve/mod.rs`
- [ ] Rename `server_ready` → `backend_ready` in `process.rs` and all callers
- [ ] Update test files with renamed function calls
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: rename server to backend in proxy/ to match domain terminology"

**Acceptance criteria:**
- [ ] Zero occurrences of `server_name`, `resolve_server`, `get_available_server_for_model`, `server_ready` remain
- [ ] Comments in proxy/ use "backend" not "server" when referring to inference backends
- [ ] All callers updated to use new function names
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 7: Extract config test fixtures into shared helpers

**Context:**
Finding #26 from the audit. Nine test files in `config/resolve/tests/` each repeat ~40 lines of fixture setup (temp dir, model files, BTreeMap, Config, ModelConfig with 20+ fields, BackendConfig). This is ~2,700 lines of total test code with significant duplication.

**Files:**
- Create: `crates/tama-core/src/config/resolve/tests/test_helpers.rs`
- Modify: `crates/tama-core/src/config/resolve/tests/mod.rs` (add `mod test_helpers;`)
- Modify: `crates/tama-core/src/config/resolve/tests/basic.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/gpu_device.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/context_np.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/kv_cache_types.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/unified_slots.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/path_resolution.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/aliases.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/server_resolution.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/spec_decoding/general.rs` (use helpers)
- Modify: `crates/tama-core/src/config/resolve/tests/spec_decoding/mtp.rs` (use helpers)

**What to implement:**

1. Create `test_helpers.rs` with:
   ```rust
   use tempfile::TempDir;
   use std::collections::BTreeMap;
   use std::path::PathBuf;
   
   /// Create a temp dir with a dummy model file at org/repo/model-Q4_K_M.gguf.
   pub fn temp_model_dir() -> (TempDir, PathBuf) {
       let temp_dir = tempdir().expect("Failed to create temp dir");
       let models_dir = temp_dir.path().join("models");
       let org_dir = models_dir.join("org").join("repo");
       let quant_file = org_dir.join("model-Q4_K_M.gguf");
       std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
       std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");
       (temp_dir, models_dir)
   }
   
   /// Create a default Config with models_dir set.
   pub fn sample_config(models_dir: PathBuf) -> Config {
       let mut config = Config::default();
       config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
       config
   }
   
   /// Create a sample ModelConfig with Q4_K_M quant. Accepts a closure to override fields.
   pub fn sample_server<F: FnOnce(&mut ModelConfig)>(overrides: F) -> ModelConfig {
       let mut quants = BTreeMap::new();
       quants.insert("Q4_K_M".to_string(), QuantEntry::default());
       
       let mut server = ModelConfig {
           backend: "llama_cpp".to_string(),
           model: Some("org/repo".to_string()),
           quant: Some("Q4_K_M".to_string()),
           quants,
           ..Default::default()
       };
       overrides(&mut server);
       server
   }
   
   /// Create a default BackendConfig.
   pub fn sample_backend() -> BackendConfig {
       BackendConfig::default()
   }
   ```

2. Add `mod test_helpers;` to `tests/mod.rs`.

3. In each test file, replace the repeated fixture setup with calls to the helpers. Keep test-specific overrides as closures passed to `sample_server()`.

**Steps:**
- [ ] Read 2-3 test files to understand the exact fixture patterns
- [ ] Create `test_helpers.rs` with `temp_model_dir()`, `sample_config()`, `sample_server()`, `sample_backend()`
- [ ] Add `mod test_helpers;` to `tests/mod.rs`
- [ ] Refactor `basic.rs` to use helpers
- [ ] Refactor `gpu_device.rs` to use helpers
- [ ] Refactor `context_np.rs` to use helpers
- [ ] Refactor `kv_cache_types.rs` to use helpers
- [ ] Refactor `unified_slots.rs` to use helpers
- [ ] Refactor `path_resolution.rs` to use helpers
- [ ] Refactor `aliases.rs` to use helpers
- [ ] Refactor `server_resolution.rs` to use helpers
- [ ] Refactor `spec_decoding/general.rs` to use helpers
- [ ] Refactor `spec_decoding/mtp.rs` to use helpers
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: extract shared config test fixtures into test_helpers module"

**Acceptance criteria:**
- [ ] `test_helpers.rs` provides `temp_model_dir()`, `sample_config()`, `sample_server()`, `sample_backend()`
- [ ] All 9+ test files use the shared helpers instead of inline fixture setup
- [ ] Net reduction in test code (each test file is shorter)
- [ ] All tests pass

---

## Deferred to Follow-up Plans

These approved findings are too large for this plan and will be separate plans:

| Finding | Description | Est. Effort |
|---------|-------------|-------------|
| #1 | Split `config/types.rs` (1,407 lines) into 7 modules | Large |
| #2 | Split `proxy/tama_handlers/models.rs` (1,303 lines) into 4 modules | Large |
| #3 | Split `updates/checker.rs` (1,079 lines) into 5 modules | Large |
| #4 | Split `proxy/forward.rs` (1,119 lines) into 5 modules | Large |
| #5 | Split `gpu/system.rs` (1,121 lines) into 4 modules | Large |
| #8 | DB repository layer (35 direct query calls from API) | Large |
| #9 | ProxyState encapsulation (25+ pub fields → pub(crate) + WebState) | Large |
| #10 | GPU Vendor & Model State enums | Medium |
| #11 | DB tuples → typed records | Medium |
| #14 | Backend lifecycle trait abstractions + tests | Large |
| #15-17 | Web UI file splits (types/config.rs, manage.rs, config_editor.rs) | Medium |
| #20 | Move `web_types` from `tama-core` to `tama` | Medium |
| #21 | Rename `download` → `pull` in model subsystem | Medium |
| #22 | Config enums (RestartPolicy, LogLevel, CompactionDevice) | Medium |
| #25 | Update checker + download queue tests | Medium |
| #29-32 | Low severity (handler return types, deprecated fields, rename_legacy, ModelConfig composition) | Small |
