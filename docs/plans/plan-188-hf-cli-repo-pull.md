# hf CLI Repo Pull (Safetensors Wizard Support) Plan

**Goal:** Let the model download wizard handle safetensors (transformers) repos end-to-end: detect the format, pull the whole repo via the `hf` CLI as a tracked subprocess, and set the model up as a vLLM model — while leaving the GGUF flow completely unchanged.

**Architecture:** The wizard's search callback already fetches HF metadata (which carries `hf_format`: `"gguf"` / `"transformers"`, GGUF wins). It now branches on it: GGUF repos keep today's flow byte-for-byte; transformers-only repos get a Confirm → `hf`-CLI download → vLLM-settings flow. The server gains a small in-memory repo-pull job registry on `PullState` that spawns `hf download <repo> --local-dir <models_dir>/<org>/<repo>` (with `HF_TOKEN` injected from Tama's existing token resolution), tracks progress by scanning the destination directory, and on success parses `<dest>/config.json` and updates the model row's HF metadata. Per ADR-0007, this path deliberately skips SHA-256 verification and `model_files` rows.

**Tech Stack:** Rust (tama-core, tama SSR/CSR), axum, tokio (process feature already enabled in workspace), Leptos, serde, the `hf` CLI (huggingface_hub ≥1.x) as an external subprocess.

**Key prior art (read these before starting):**
- ADR-0007: `docs/adr/0007-hf-cli-for-safetensors-pulls.md` — why `hf` CLI subprocess instead of the built-in downloader
- `crates/tama-core/src/models/pull/api.rs` — `lookup_hf_metadata`, `lookup_blob_metadata`, `hf_api_model_blobs_url`, `detect_hf_format`
- `crates/tama-core/src/models/pull/mod.rs` — `HfModelMetadata` struct, `get_hf_token()` (pub(crate)), `hf_api()` (pub(crate))
- `crates/tama-core/src/models/transformers.rs` — `parse_transformers_metadata(model_dir) -> Result<TransformersMetadata>` (reads `<dir>/config.json`)
- `crates/tama-core/src/proxy/state/pull.rs` — `PullState` (add the registry here)
- `crates/tama-core/src/models/update.rs` — `update_model_config_hf_metadata(conn, model_id, meta)` (COALESCE semantics)
- `crates/tama/src/components/pull_quant_wizard.rs` — the wizard being modified
- `crates/tama/src/components/pull_wizard/mod.rs` — shared wizard types (both SSR and CSR via `core_shared`)
- Existing web tests pattern: `crates/tama/src/api/hf.rs` tests use wiremock + `HF_ENDPOINT` env override + `crate::router::build_web_routes`

**Global invariants — do not violate in any task:**
1. The GGUF wizard flow (select-quants, per-file pulls, llama.cpp stub, context step) must remain behavior-identical. All GGUF-related code paths are only *relocated*, never altered.
2. `parse_blob_siblings`, `list_gguf_files`, and `POST /tama/v1/pulls` validation are NOT touched.
3. New endpoints are web routes only (`crates/tama/src/router.rs`) — do NOT register them on the proxy port router (`crates/tama-core/src/proxy/server/router.rs`).
4. Every task ends with: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy --package tama --features ssr --all-targets -- -D warnings`, and targeted nextest runs passing.
5. **Cross-crate boundary:** the `tama` (web) crate cannot touch `tama_core` internals — `ProxyState` fields and the `proxy::state` module are all `pub(crate)`. ALL repo-pull state access from web handlers goes through public `ProxyState` delegate methods (defined in Tasks 2–3) returning public DTOs. Never write `state.pull.*` or `scan_dir_bytes(...)` in web-crate code.

---

### Task 1: Repo-pull job core (spawn, progress, cancel, registry)

**Context:**
This task builds the server-side job machinery for whole-repo `hf` CLI downloads. Tama currently has no subprocess-based download path — all GGUF pulls are in-process. Per ADR-0007, safetensors repos are pulled by shelling out to the `hf` CLI because a transformers repo is all-or-nothing (no meaningful per-file selection). The job state lives on `PullState` next to the existing per-file `pull_jobs` map, and the subprocess is tracked in memory (no DB rows, not in the Downloads Center — deliberate YAGNI decision).

Progress is computed by scanning the destination directory's total file size against the expected total (from HF sibling sizes) — we do NOT parse the `hf` CLI's progress bar output (fragile). On failure, the last ~2 KB of the child's stderr is surfaced as the error (gated-repo 401s, network errors, etc. appear there).

The spawn function takes the binary path as a parameter (dependency injection) so unit tests can pass a stub executable instead of a real `hf` install.

**Files:**
- Create: `crates/tama-core/src/proxy/state/repo_pull.rs`
- Modify: `crates/tama-core/src/proxy/state/pull.rs` (add `repo_pulls` field + accessor methods)
- Modify: `crates/tama-core/src/proxy/state/mod.rs` (declare + re-export `repo_pull` module)
- Test: tests inside `crates/tama-core/src/proxy/state/repo_pull.rs` `#[cfg(test)]`

**What to implement:**

1. In `crates/tama-core/src/proxy/state/repo_pull.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RepoPullStatus { Running, Completed, Failed, Cancelled }

pub(crate) struct RepoPullJob {
    pub job_id: String,
    pub repo_id: String,
    pub model_id: Option<i64>,
    pub dest: std::path::PathBuf,
    pub total_bytes: Option<u64>,
    pub status: RepoPullStatus,
    pub error: Option<String>,
    /// Set by cancel_repo_pull BEFORE killing, so the wait-loop's final
    /// status decision can distinguish "killed by user" from "crashed".
    pub cancel_requested: bool,
    /// From config.json max_position_embeddings, populated on completion.
    pub context_length: Option<u32>,
    /// Capped tail of the child's stderr (last 4096 bytes), updated by the reader task.
    pub(crate) stderr_tail: std::sync::Arc<tokio::sync::Mutex<String>>,
    /// Shared child handle (see the concurrency model below the struct).
    pub(crate) child: std::sync::Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
}
```

**Concurrency model (read before implementing — this is the crux):**
The wait-loop and `cancel_repo_pull` both use the shared `Arc<Mutex<Option<Child>>>` with BRIEF lock holds only — no code path holds the lock (or the job-map write lock) across a long `.await`:
- **Wait-loop tick:** `let got = { let mut g = child_arc.lock().await; g.as_mut().and_then(|c| c.try_wait()) };` — `try_wait` is NON-BLOCKING; then, OUTSIDE the lock, `tokio::time::sleep(500ms)` and repeat until `got` is `Some(status)`.
- **Cancel:** brief lock on the child handle, `if let Some(c) = g.as_mut() { c.kill().await; }` (kill error is ignored — process may have just died), then in the job map (brief write lock) set `cancel_requested = true` and `status = Cancelled`.
- **Final status decision** (in `finish_repo_pull`): if `cancel_requested` → `Cancelled` (a killed process exits non-zero via signal — the flag takes precedence); else exit code 0 → `Completed`, non-zero → `Failed`.

2. Functions in the same file (all `pub(crate)`):

- `pub(crate) fn scan_dir_bytes(dir: &std::path::Path) -> u64` — recursive sum of regular file sizes using plain recursive `std::fs` (`walkdir` is NOT a workspace dependency — do not add it). Missing dir → 0.
- `pub(crate) async fn check_hf_binary() -> Result<(), String>` — `tokio::process::Command::new("hf").arg("--version").output().await` (tokio Command, NOT the blocking std one); Ok if exit status success, `Err("hf CLI not found. Install with: pip install -U huggingface_hub")` otherwise (spawn error or non-zero exit).
- `pub(crate) async fn spawn_hf_download(binary: &str, repo_id: &str, dest: &std::path::Path, hf_token: Option<&str>) -> Result<tokio::process::Child, String>` — `tokio::process::Command::new(binary)` with args `["download", repo_id, "--local-dir", dest.to_string_lossy().as_ref()]`; if `hf_token` is `Some(t)`, set env `HF_TOKEN=t`; `stdout(Stdio::null())`, `stderr(Stdio::piped())`. Return the spawned child.
- `pub(crate) fn start_stderr_reader(stderr: std::process::ChildStderr, sink: std::sync::Arc<tokio::sync::Mutex<String>>)` — spawn a tokio task that reads stderr lines and keeps only the last 4096 bytes in `sink` (e.g. `sink.push(chunk)` then truncate to the tail). The task self-terminates when stderr hits EOF. Return the `JoinHandle`.
- `pub(crate) fn stderr_tail_str(sink: &std::sync::Arc<tokio::sync::Mutex<String>>) -> Option<String>` — helper returning `Some(tail)` when non-empty (strip trailing newlines), else `None`.

3. In `crates/tama-core/src/proxy/state/pull.rs` — add to `PullState`:
```rust
/// Whole-repo `hf` CLI pull jobs, keyed by job_id.
pub(crate) repo_pulls: Arc<tokio::sync::Mutex<HashMap<String, RepoPullJob>>>,
```
- Initialize empty in `new()`. `PullState` currently derives `Default`; the new field would also be `Default`-compatible, but REMOVE the derive and rely on `new()` for explicit init anyway (verify there are no `PullState::default()` call sites first with `rg -n "PullState::default" crates/` — there are none).
- Accessor methods on `PullState`:
  - `pub(crate) async fn upsert_repo_pull(&self, job: RepoPullJob)`
  - `pub(crate) async fn get_repo_pull(&self, job_id: &str) -> Option<RepoPullJob>` — clone the job; the `child` field is cloned as the `Arc` (cheap — both sides keep accessing the same mutex), `stderr_tail` likewise.
  - `pub(crate) async fn repo_pull_running_for(&self, repo_id: &str) -> bool` — any job with `status == Running` and same `repo_id`.
  - `pub(crate) async fn cancel_repo_pull(&self, job_id: &str) -> Result<(), String>` — per the concurrency model above: brief job-map read to check state; if `Running` → set `cancel_requested = true` (brief write lock), take a brief lock on the shared child handle and `kill().await` (ignored if it fails), set `status = Cancelled`, return Ok; if not found → `Err("not found")`; if already terminal → `Err("already finished")`.
  - Extend `clear()` to also clear `repo_pulls`: iterate the map; for each job take a brief lock on the child handle and `kill().await` if present (best-effort, ignore errors); then clear the map.

**Steps:**
- [ ] Create the module with the types + `scan_dir_bytes`, `check_hf_binary`, `spawn_hf_download`, `start_stderr_reader`, `stderr_tail_str` (no tests yet). Wire it into `mod.rs` (`pub(crate) mod repo_pull;` + `pub(crate) use repo_pull::*;` matching the existing style in `crates/tama-core/src/proxy/state/mod.rs`).
- [ ] Run `cargo check --package tama-core` — did it compile? Fix any errors before continuing.
- [ ] Write tests in `repo_pull.rs` `#[cfg(test)]` (all `#[cfg(unix)]` — CI is Linux; add a `#[cfg(unix)]` gate on the whole test module):
  - `test_scan_dir_bytes_nested` — tempdir with files of known sizes (incl. a nested dir); assert the sum; missing dir → 0.
  - `test_check_hf_binary_missing` — if a real `hf` is on PATH in the test env, this test would be wrong, so: test the spawn-error branch by calling `spawn_hf_download("definitely-not-a-real-binary-xyz", ...)` and asserting `Err`. Name it `test_spawn_hf_download_missing_binary` and do NOT test `check_hf_binary` (it depends on host PATH — covered by manual E2E instead).
  - `test_spawn_hf_download_runs_stub_and_captures_stderr` — write a stub to `tempfile::tempdir()`: file `hf-stub` with `#!/bin/sh\necho "line1" \necho "err" 1>&2\nexit 0`, `chmod +x` (via `std::os::unix::fs::PermissionsExt`). Call `spawn_hf_download(&stub, "foo/bar", dest, Some("tok123"))`, start the stderr reader, `child.wait()` → success. Assert stderr sink contains "err" and NOT "line1". Also assert the `HF_TOKEN` env was passed: use a stub variant whose last line is `echo "$HF_TOKEN" > "$4/.hf_token_check"` (the CLI argv is `hf download <repo> --local-dir <dest>`, so the `--local-dir` value is positional arg `$4`), then assert the marker file in `dest` contains `tok123`.
  - `test_spawn_hf_download_nonzero_exit` — stub with `exit 3`; assert `wait()` status code 3 and stderr tail captured.
  - `test_pull_state_repo_pull_lifecycle` — build a `PullState::new(None)`, upsert a job with `child: Arc::new(Mutex::new(None))` (childless), assert `get_repo_pull` returns it, assert `repo_pull_running_for` true while Running, `cancel_repo_pull` on a childless Running job → tolerates an empty child handle (still marks `cancel_requested` + Cancelled), second cancel → `Err("already finished")`, unknown id → `Err("not found")`.
  - `test_wait_loop_cancel_race` — spawn a stub that `sleep 30`; wrap the child in the shared `Arc<Mutex<Option<Child>>>`; run the wait-loop logic as a task; after 500 ms, call the cancel path (brief lock + `kill().await` + flag); assert the wait-loop task terminates promptly (< 5 s) with the `cancel_requested` flag set, and that the loop observed the exit via `try_wait` (not by holding the lock — the cancel's brief lock acquired successfully, which it would not if the loop held it).
  - [ ] Run `cargo nextest run --package tama-core -- state::repo_pull` — do all tests pass?
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --package tama-core` (full crate — ensures the `PullState` derive change broke nothing)
- [ ] Commit with message: `feat: in-memory repo-pull job core for hf CLI downloads`

**Acceptance criteria:**
- [ ] `PullState` exposes the four repo-pull accessors; removing the `Default` derive broke nothing (verified: zero `PullState::default()` call sites)
- [ ] Stub-binary tests prove: spawn args include `--local-dir`, `HF_TOKEN` env is injected only when provided, stderr tail is captured (stdout is not), non-zero exit is observable
- [ ] `scan_dir_bytes` is recursive and returns 0 for missing dirs
- [ ] `cargo nextest run --package tama-core` fully green

---

### Task 2: Completion handling (config.json → model row)

**Context:**
When the `hf` child exits 0, the downloaded repo needs to be reflected on the model's DB row: HF metadata (format, architecture, context length, layers) and the quant name (from `config.json`'s `quantization_config.quant_method`, e.g. `"fp8"`, `"awq"`). The wizard always creates a stub model row *before* starting the pull (Task 5), so completion updates an existing row identified by `model_id`. Scope decision (simpler than duplicating web row-creation code in proxy code): if `model_id` is `None` (API-only caller), the files still download but no DB update happens — the job completes normally and the wizard flow is unaffected because it always supplies `model_id`.

`TransformersMetadata` (from `crate::models::transformers::parse_transformers_metadata`, which reads `<dest>/config.json`) feeds into an `HfModelMetadata` value, reusing the existing COALESCE update `crate::models::update::update_model_config_hf_metadata` (fills NULLs only — safe to run after the wizard's stub already stored README-derived metadata). The quant update is a new small function with direct SET semantics (the per-file GGUF path sets `quant` unconditionally from `quantization_method` — mirror that).

The completion core takes a `rusqlite::Connection` parameter (testability seam, same pattern as `_setup_model_after_pull_with_config`) so unit tests can use a temp SQLite repo without a live `hf`.

**Files:**
- Modify: `crates/tama-core/src/proxy/state/repo_pull.rs` (completion functions + orchestration + public DTO)
- Modify: `crates/tama-core/src/models/update.rs` (add `update_model_config_quant`)
- Modify: `crates/tama-core/src/proxy/types.rs` (three public `ProxyState` delegates — see step 3)
- Modify: `crates/tama-core/src/proxy/mod.rs` (explicit `pub use state::repo_pull::{RepoPullError, RepoPullStart, RepoPullStatusDto};` — the `state` module is private and currently only `pub(crate)`-re-exports, so this `pub use` is mandatory for the web crate to see the types)
- Test: tests in `repo_pull.rs` + `crates/tama-core/src/models/update.rs` (existing test module)

**What to implement:**

1. In `crates/tama-core/src/models/pull/api.rs` (needed by `start_repo_pull` below — created in THIS task, not later):
```rust
/// Total size (bytes) and file count across ALL files in the repo (any extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoStats { pub total_bytes: u64, pub file_count: u32 }

/// Pure: sum `size` (default 0 when absent) over `siblings[]` and count entries.
pub fn parse_siblings_stats(value: &serde_json::Value) -> RepoStats { /* ... */ }

/// Hit the same blobs endpoint as lookup_blob_metadata, sum ALL siblings.
pub async fn lookup_repo_stats(repo_id: &str) -> Result<RepoStats> { /* client.get(hf_api_model_blobs_url). ... .json::<Value>() ... parse_siblings_stats */ }
```
Add `parse_siblings_stats, lookup_repo_stats, RepoStats` to the `pub use api::{...}` re-export list in `models/pull/mod.rs`. Tests here:
- `test_parse_siblings_stats_basic` — JSON with 3 siblings of sizes 100/200/300 (one missing `size` → counts as 0) → `total_bytes=600, file_count=3`.
- `test_parse_siblings_stats_empty` — `{"siblings": []}` → zeros; `{}` → zeros.

2. In `crates/tama-core/src/models/update.rs`:
```rust
/// Set the selected_quant column directly (no COALESCE) — used by the repo-pull
/// completion path where config.json quantization_method is authoritative.
pub(crate) fn update_model_config_quant(
    conn: &rusqlite::Connection,
    model_id: i64,
    quant: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE model_configs SET selected_quant = ?1 WHERE id = ?2",
        rusqlite::params![quant, model_id],
    )?;
    Ok(())
}
```
NOTE: the DB column is **`selected_quant`** (see migration `_0026_rebuild_model_configs_auto_parallel.rs`), mapped to `ModelConfig.quant` — do NOT write `SET quant` (that column does not exist and would be a runtime SQL error).
Plus a test: use the same connection setup the existing tests in that file use (`crate::db::open_in_memory()` — see how `update_model_config_hf_metadata`'s tests obtain their `conn`; the `test_repo()` helper in `db/repository.rs` is private to that module and NOT reusable), insert a `model_configs` row, call with `quant="fp8"`, assert `selected_quant` changed.

2. In `repo_pull.rs` — completion + orchestration. **Critical constraint: `rusqlite::Connection` is `!Sync`, so no `&Connection` may be held across an `.await`** (the wait-loop is `tokio::spawn`ed and requires a `Send` future — the codebase documents this exact trap on `refresh_metadata`). Structure the completion as one async part (no DB) + one fully-sync part (DB only):

- `pub(crate) async fn fetch_completion_metadata(repo_id: &str) -> HfModelMetadata` — network part only: `crate::models::pull::lookup_hf_metadata(repo_id).await`, soft-failing to `HfModelMetadata::default()`. No DB access.
- `pub(crate) fn apply_repo_pull_completion_with_meta(conn: &rusqlite::Connection, model_id: i64, base: &HfModelMetadata, meta_tf: Option<&crate::models::transformers::TransformersMetadata>, dest: &std::path::Path) -> anyhow::Result<Option<u32>>` — **synchronous** (no `.await` anywhere):
  1. Fill `base`'s gaps from `meta_tf`: if `hf_architecture_type` is None → `architectures.first()`; if `hf_context_length` is None → `max_position_embeddings`; if `hf_num_layers` is None → `num_hidden_layers`; if `hf_format` is None → `Some("transformers")`. (`dest` is only used by callers that pre-parse; keep the param if useful for logging, else drop it — the parsed `meta_tf` is already passed.)
  2. `crate::models::update::update_model_config_hf_metadata(conn, model_id, &meta)?;`
  3. If `meta_tf` has `quantization_method` → `update_model_config_quant(conn, model_id, qm)?;`
  4. Return `meta_tf.and_then(|m| m.max_position_embeddings)`.
- `pub(crate) async fn finish_repo_pull(state: &crate::proxy::ProxyState, job_id: &str, exit_status: Option<i32>)` — called from the wait-loop when the child exits. Order matters (all awaits BEFORE any DB work):
  1. Re-read the job from the map (brief lock); read `repo_id`, `dest`, `model_id`, `cancel_requested`, clone the `stderr_tail` Arc.
  2. `let meta_tf = crate::models::transformers::parse_transformers_metadata(&dest).ok();` (sync fs, soft-fail: a repo without config.json still completes).
  3. `let base = fetch_completion_metadata(&repo_id).await;` (network — no connection open).
  4. `let context_length = if let (Some(mid), Some(conn)) = (model_id, state.open_db()) { let _ = apply_repo_pull_completion_with_meta(&conn, mid, &base, meta_tf.as_ref(), &dest); context_length } else { None }` — `conn` is owned, used synchronously, and dropped at end of scope; `tracing::warn!` on DB errors (job still completes — metadata is informational). Do NOT hold the job-map write lock or the connection across this block.
  5. Under a brief job-map write lock, set the final state: if `cancel_requested` → `status = Cancelled`; else if `exit_status == Some(0)` → `status = Completed` + `context_length = step 4's value`; else → `status = Failed`, `context_length = step 4's value` (metadata may still have been written — fine), `error = Some(stderr tail)` (use `stderr_tail_str`; fallback `"hf download exited with code {n}"` when tail is empty). Upsert back.
  6. The stderr reader task needs no explicit abort — it self-terminates when the child's stderr hits EOF. Do NOT add a `JoinHandle` field to `RepoPullJob`.
- `pub(crate) async fn start_repo_pull(state: &Arc<crate::proxy::ProxyState>, repo_id: &str, model_id: Option<i64>) -> Result<RepoPullStart, RepoPullError>` — the orchestration entry point (takes `&Arc` so the wait-loop can clone it for `tokio::spawn`):
  - `RepoPullError` enum (this file): `HfBinaryMissing(String)`, `DuplicatePull`, `RepoNotFound(String)`, `Upstream(String)`, `InvalidRepoId(String)`.
  - Validate `crate::models::is_valid_repo_id(repo_id)` → `InvalidRepoId`.
  - `state.pull.repo_pull_running_for(repo_id)` → `DuplicatePull` (payload-free — the web handler formats its 409 message from `body.repo_id`, which it has in scope).
  - `check_hf_binary()` → `HfBinaryMissing`.
  - Repo existence + totals: `crate::models::pull::hf_api().await?` then `api.model(repo_id.clone()).info().await` — 404/err → `RepoNotFound(format!("'{}' not found on HuggingFace", repo_id))` (distinguish a missing-repo error from a network error: check the error string for "404" / "not found", else `Upstream`). For the byte total use `crate::models::pull::lookup_repo_stats(repo_id).await` (added in step 1 of this task) — soft-fail to `None` totals on error (progress bar then shows indeterminate; the download still proceeds).
  - Resolve dest: `let cfg = state.config.read().await; let models_dir = cfg.models_dir().map_err(|e| RepoPullError::Upstream(e.to_string()))?;` then `let dest = crate::models::repo_path(models_dir, repo_id);` `tokio::fs::create_dir_all(&dest).await.map_err(|e| RepoPullError::Upstream(e.to_string()))?`.
  - Spawn via `spawn_hf_download("hf", repo_id, &dest, crate::models::pull::get_hf_token().as_deref())` — note `get_hf_token` is `pub(crate)` in `crate::models::pull`. Start the stderr reader. Wrap the child in `let child_handle = std::sync::Arc::new(tokio::sync::Mutex::new(Some(child)));` and insert `RepoPullJob { job_id: format!("hfrepo-{}", uuid::Uuid::new_v4().hyphenated()), status: Running, cancel_requested: false, child: child_handle.clone(), .. }` via `upsert_repo_pull`.
  - Spawn the wait-loop task: `tokio::spawn` an async block holding a CLONE of the `Arc<ProxyState>`, a CLONE of `child_handle`, the job_id, and a clone of the stderr sink Arc. Per the concurrency model: loop `{ brief lock → try_wait; if Some(status) → break; 500 ms sleep outside the lock }`, then `finish_repo_pull(&state_clone, &job_id, Some(status.code())).await` (map a signal-killed exit to `None`/non-zero — the `cancel_requested` flag decides the final label).
3. Public web boundary in `crates/tama-core/src/proxy/types.rs` (three `pub` methods on `ProxyState`; the `tama` web crate can ONLY see these):
```rust
/// Start a whole-repo hf CLI pull. `model_id` is the pre-created stub row (None = no DB update on completion).
pub async fn start_repo_pull(self: &Arc<Self>, repo_id: &str, model_id: Option<i64>) -> Result<RepoPullStart, RepoPullError> { /* delegates to state::repo_pull::start_repo_pull(self, ...) */ }

/// Live status snapshot; `bytes_done` is computed here (inside tama-core) via `scan_dir_bytes` wrapped in `tokio::task::spawn_blocking`.
pub async fn get_repo_pull_status(&self, job_id: &str) -> Option<RepoPullStatusDto> { /* read job; compute bytes_done; build DTO */ }

/// Cancel + kill. Err message is user-facing: "not found" / "already finished".
pub async fn cancel_repo_pull(&self, job_id: &str) -> Result<(), String> { /* state.pull.cancel_repo_pull(job_id) */ }
```
With a public DTO (in `repo_pull.rs`, re-exported from `tama_core::proxy` next to the other proxy re-exports — check `crates/tama-core/src/proxy/mod.rs` for where `ProxyState`-adjacent public types are re-exported):
```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoPullStatusDto {
    pub job_id: String,
    pub status: String,              // lowercase: running|completed|failed|cancelled
    pub bytes_done: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    pub context_length: Option<u32>,
}
```
Also make `RepoPullError` + `RepoPullStart` `pub` (they cross the crate boundary in the return type) and re-export all three (`RepoPullError`, `RepoPullStart`, `RepoPullStatusDto`) from `tama_core::proxy`.

**Steps:**
- [ ] Add `RepoStats`/`parse_siblings_stats`/`lookup_repo_stats` to `models/pull/api.rs` with the two `parse_siblings_stats` tests. Run `cargo nextest run --package tama-core -- pull::api` — failing (functions missing) → implement → green.
- [ ] Add `update_model_config_quant` + its test to `crates/tama-core/src/models/update.rs`. Run `cargo nextest run --package tama-core -- models::update` — does the new test fail first (function missing)? Implement, re-run, green.
- [ ] Add `fetch_completion_metadata`, `apply_repo_pull_completion_with_meta`, `finish_repo_pull`, `start_repo_pull`, `RepoPullError`, `RepoPullStart`, `RepoPullStatusDto` to `repo_pull.rs`, plus the three `ProxyState` delegates in `types.rs` and the `pub use` re-export in `proxy/mod.rs`.
  - Write `test_apply_repo_pull_completion_with_meta` FIRST (it exercises the SYNC seam — no network, no async): get a migrated in-memory connection via `crate::db::open_in_memory()` (the same pattern the existing `update_model_config_hf_metadata` tests in `models/update.rs` use — do NOT try to reuse the private `test_repo()` from `db/repository.rs`), insert a `model_configs` row, build a tempdir dest with a `config.json` fixture:
    ```json
    {"architectures": ["Qwen3ForCausalLM"], "max_position_embeddings": 32768, "num_hidden_layers": 48, "quantization_config": {"quant_method": "fp8"}}
    ```
    Parse it in the test with `parse_transformers_metadata(&dest)`, then call `apply_repo_pull_completion_with_meta(&conn, model_id, &HfModelMetadata::default(), meta_tf.as_ref(), &dest)`. Assert: `hf_format='transformers'`, `hf_architecture_type='Qwen3ForCausalLM'`, `hf_context_length=32768`, `hf_num_layers=48`, `selected_quant='fp8'`, returned context length `Some(32768)`.
  - Run `cargo nextest run --package tama-core -- state::repo_pull` — failing (missing fns)? Implement, green.
  - Add `test_apply_repo_pull_completion_with_meta_no_config_json` — call the seam with `meta_tf = None` and an empty base: returns `Ok(None)`, writes only `hf_format='transformers'` — assert no error.
- [ ] Run `cargo fmt --all`, both clippy commands, `cargo nextest run --package tama-core`
- [ ] Commit with message: `feat: repo-pull completion handling (config.json metadata + quant)`

**Acceptance criteria:**
- [ ] `update_model_config_quant` exists, is direct-set (no COALESCE), and has a unit test
- [ ] Completion writes hf_format/architecture/context/layers with COALESCE semantics (existing non-NULL values preserved — covered by the existing `test_update_hf_metadata` pattern) and quant unconditionally from `quantization_method`
- [ ] `start_repo_pull` enforces: invalid repo id → `InvalidRepoId`; running duplicate → `DuplicatePull`; missing `hf` → `HfBinaryMissing`; unknown repo → `RepoNotFound`
- [ ] Non-zero child exit → job `Failed` with stderr tail as `error`; exit 0 → `Completed` with `context_length` populated
- [ ] `cargo nextest run --package tama-core` fully green

---

### Task 3: Web API endpoints + routing

**Context:**
The wizard (Task 6) talks to the repo-pull machinery through three web endpoints, sitting next to the existing `/tama/v1/pulls/*` routes in `crates/tama/src/router.rs`. These handlers are thin: they translate HTTP bodies/paths into `start_repo_pull` / `PullState` accessor calls and map `RepoPullError` variants to the project's canonical error shape (see `docs/api/errors.md`: `{"error": {"message", "type"}}` via `crate::api::error::error_response`).

Route placement: `POST /tama/v1/pulls/repo` and `DELETE /tama/v1/pulls/repo/:job_id` go in the **write group** (the router section that already carries `.layer(json_body_limit)` and CSRF/auth — around the existing `/tama/v1/pulls/:job_id/cancel` route); `GET /tama/v1/pulls/repo/:job_id` goes in the **read group** (around the existing `/tama/v1/pulls/active` route). matchit resolves the static `repo` segment and the `:job_id` parameter without conflict, but keep the static `repo` routes and `:job_id` routes distinct as specified.

Progress endpoint returns `bytes_done` (live `scan_dir_bytes`) so the client doesn't have to know the destination path.

**Files:**
- Create: `crates/tama/src/api/repo_pulls.rs`
- Modify: `crates/tama/src/api.rs` (add `pub mod repo_pulls;` to the existing module list — NOTE: the module file is `crates/tama/src/api.rs`; there is no `api/mod.rs`)
- Modify: `crates/tama/src/router.rs` (3 route registrations)
- Modify: `docs/api/pulls.md` (document the three endpoints — follow the existing file's format)
- Test: tests inside `crates/tama/src/api/repo_pulls.rs` `#[cfg(test)]`

**What to implement:**

1. DTOs + handlers in `crates/tama/src/api/repo_pulls.rs`. The handlers use ONLY the public `ProxyState` delegates from Task 2 (`start_repo_pull`, `get_repo_pull_status`, `cancel_repo_pull`) — never `state.pull.*` or core internals:

```rust
#[derive(serde::Deserialize)]
pub struct RepoPullStartBody {
    pub repo_id: String,
    #[serde(default)]
    pub model_id: Option<u32>,
}

#[derive(serde::Serialize)]
pub struct RepoPullStartResponse { pub job_id: String, pub status: String, pub total_bytes: Option<u64> }
```

- `pub async fn start_repo_pull(State(state): State<Arc<ProxyState>>, Json(body): Json<RepoPullStartBody>) -> Response`:
  - Convert `body.model_id.map(|id| id as i64)` before calling `state.start_repo_pull(&body.repo_id, ...)` (core uses `i64`).
  - Map errors: `InvalidRepoId` → 422 `ValidationError`; `HfBinaryMissing` → 422 `ValidationError` (message already contains the pip install hint); `DuplicatePull` → 409 `ConflictError` (message: "A repo pull for '{repo_id}' is already running"); `RepoNotFound` → 422 `ValidationError`; `Upstream` → 502 `UpstreamError`.
  - Success → 200 `RepoPullStartResponse { job_id, status: "running", total_bytes }`.
- `pub async fn get_repo_pull(State, Path(job_id)) -> Response` — `state.get_repo_pull_status(&job_id)`; `None` → 404 `NotFoundError`. Else 200 the `RepoPullStatusDto` JSON as-is (it already serializes in the exact shape the wizard needs).
- `pub async fn delete_repo_pull(State, Path(job_id)) -> Response` — `state.cancel_repo_pull(&job_id)`; `Err("not found")` → 404 `NotFoundError`; `Err("already finished")` → 409 `ConflictError`; Ok → 200 `{"ok": true}`.

2. `crates/tama/src/router.rs`:
- Write group (near `"/tama/v1/pulls/:job_id/cancel"`):
  ```rust
  .route("/tama/v1/pulls/repo", post(api::repo_pulls::start_repo_pull).layer(json_body_limit))
  .route("/tama/v1/pulls/repo/:job_id", delete(api::repo_pulls::delete_repo_pull))
  ```
- Read group (near `"/tama/v1/pulls/active"`):
  ```rust
  .route("/tama/v1/pulls/repo/:job_id", get(api::repo_pulls::get_repo_pull))
  ```

3. `docs/api/pulls.md`: add a "## Repo pulls (safetensors / transformers)" section documenting all three endpoints with request/response examples and the error table, matching the doc's existing style.

**Steps:**
- [ ] Write a failing handler test in `repo_pulls.rs` (follow the `crates/tama/src/api/hf.rs` test setup: `Config::default()`, `ProxyState::new(config, Some(tempdir))`, `build_web_routes`, `tower::ServiceExt::oneshot`):
  - `test_start_repo_pull_missing_hf_binary_422` — a real `hf` on the host PATH would make this test wrong, so guard it with an INLINE probe (do NOT call `tama_core`'s `pub(crate)` helper): `let probe = std::process::Command::new("hf").arg("--version").output(); if probe.map(|o| o.status.success()).unwrap_or(false) { return; }` — then POST `/tama/v1/pulls/repo` `{"repo_id": "foo/bar"}` → expect 422 + error type `ValidationError`.
  - `test_get_repo_pull_unknown_404` — GET `/tama/v1/pulls/repo/hfrepo-does-not-exist` → 404 `NotFoundError` (no network involved).
  - `test_delete_repo_pull_unknown_404` — DELETE same → 404.
  - Run `cargo nextest run --package tama -- api::repo_pulls` — the 404 tests pass once handlers+routes exist; the 422 test is guard-skipped on hosts with `hf`.
- [ ] Add the two write-group routes + one read-group route to `router.rs`; declare the module in `crates/tama/src/api.rs` (NOT `api/mod.rs` — that file does not exist).
- [ ] Update `docs/api/pulls.md`.
- [ ] Run `cargo fmt --all`, both clippy commands, `cargo nextest run --package tama -- api::repo_pulls`, `cargo nextest run --package tama -- router`
- [ ] Commit with message: `feat: repo pull endpoints (start/status/cancel)`

**Acceptance criteria:**
- [ ] `POST /tama/v1/pulls/repo` validates + starts (or 422/409/502 with the canonical error shape); `GET` returns live `bytes_done` (computed server-side inside `tama_core`, off the async runtime via `spawn_blocking`); `DELETE` kills the child
- [ ] Web handlers compile against public `ProxyState` delegates only — no `state.pull.*` access in the `tama` crate
- [ ] Routes sit in the correct auth groups (write routes behind the CSRF/CSRF-token layer, GET in the read group)
- [ ] `docs/api/pulls.md` documents the three endpoints with examples
- [ ] All tests in `crates/tama -- api::repo_pulls` pass; clippy clean

---

### Task 4: Metadata response gains repo size + file count

**Context:**
The wizard's Confirm step (Task 6) shows the total download size and file count *before* the user starts a 10–100 GB pull. The wizard already fetches `GET /tama/v1/hf/{repo}/metadata` in parallel with the quant listing, so we extend `HfModelMetadata` with two optional fields computed from the HF blobs API. The plumbing (`RepoStats`, `parse_siblings_stats`, `lookup_repo_stats`) already exists after Task 2 — this task only wires it into the metadata response and the frontend mirror. Both fields are optional and soft-fail to `None` (the Confirm step renders without a size when unavailable). Note: this adds a second blobs-API call per metadata fetch (the quant listing makes its own) — accepted as-is; both are cheap metadata calls.

**Files:**
- Modify: `crates/tama-core/src/models/pull/api.rs` (wire `lookup_repo_stats` into `lookup_hf_metadata`)
- Modify: `crates/tama-core/src/models/pull/mod.rs` (`HfModelMetadata` fields)
- Modify: `crates/tama/src/components/pull_wizard/mod.rs` (frontend mirror struct — same two fields with `#[serde(default)]`)
- Test: existing `api.rs` + `api::hf` test modules (extend if needed)

**What to implement:**

1. In `lookup_hf_metadata` (`api.rs`), after the README merge block, add:
```rust
match lookup_repo_stats(repo_id).await {
    Ok(stats) => { meta.hf_total_size_bytes = Some(stats.total_bytes); meta.hf_file_count = Some(stats.file_count); }
    Err(e) => tracing::debug!(repo_id = %repo_id, error = %e, "repo stats lookup failed — leaving size/count unset"),
}
```
2. `HfModelMetadata` (tama-core `models/pull/mod.rs` line ~540): add
```rust
#[serde(default)] pub hf_total_size_bytes: Option<u64>,
#[serde(default)] pub hf_file_count: Option<u32>,
```
(Check the derive list of the struct first — if it derives `Deserialize`, the `#[serde(default)]` is required for backward compat with old serialized values; if it also derives `Default`, the new `Option` fields keep that working.)
3. Frontend mirror in `crates/tama/src/components/pull_wizard/mod.rs` — the `HfModelMetadata` struct there (CSR-only copy): add the same two fields with `#[serde(default)]`.

**Steps:**
- [ ] Add the two fields to both structs + wire into `lookup_hf_metadata`.
- [ ] Run `cargo nextest run --package tama-core -- pull::api`, `cargo nextest run --package tama -- api::hf` (existing hf metadata tests must still pass — they assert specific JSON fields; new optional fields serialize as `null` and must not break those assertions — if a test does an exact-object comparison, extend its expected JSON).
- [ ] Run `cargo fmt --all`, both clippy commands.
- [ ] Commit with message: `feat: hf metadata carries repo total size and file count`

**Acceptance criteria:**
- [ ] `GET /tama/v1/hf/{repo}/metadata` returns `hf_total_size_bytes` / `hf_file_count` for a real repo (verify manually or via the wiremock test if you add one); soft-fails to absent/null when the blobs API errors
- [ ] Existing `pull::api` and `api::hf` tests green; clippy clean
- [ ] CSR (WASM) build compiles: `cargo check --package tama` (default features)

---

### Task 5: Wizard format branching + stub reordering (frontend logic)

**Context:**
The wizard's `on_search` callback (`crates/tama/src/components/pull_quant_wizard.rs`) today: (1) fetches quants + metadata in parallel, (2) **immediately creates a stub model with hardcoded `backend: "llama_cpp"`**, (3) if the quant list is empty shows "No GGUF files found…" — which is both misleading for safetensors repos and leaves an orphaned stub behind. This task restructures the callback so the format decision (from `hf_format`, already in the fetched metadata) happens **before** stub creation, the stub gets the branch-correct backend (`llama_cpp` vs `vllm`), and a new `branch` signal drives which step-2/step-4 components render (Task 6 builds those components).

The branching rule (matches `detect_hf_format`'s GGUF-wins semantics):
- `hf_format == Some("transformers")` → **Transformers** (by construction this means no GGUF files are present)
- `hf_format == Some("gguf")` OR (`hf_format == None` AND the quant listing is non-empty) → **Gguf**
- everything else → **no model files** (error)

The `WizardStep` enum is NOT changed: the transformers flow reuses `SelectQuants` (renders the Confirm component) and `SetContext` (renders the Vllm config component), with the step-header label ("2. Select") made branch-aware. This avoids touching the `step_class` index math.

**Files:**
- Modify: `crates/tama/src/components/pull_wizard/mod.rs` (`WizardBranch` + `resolve_branch` + tests)
- Modify: `crates/tama/src/components/pull_quant_wizard.rs` (callback restructure, `branch` signal, branch-aware step header, error text)
- Test: tests in `pull_wizard/mod.rs` test module (create one if the file has none)

**What to implement:**

1. In `pull_wizard/mod.rs`:
```rust
/// Which download flow the wizard is running. Decided once per search,
/// from `hf_format` + the quant listing. GGUF wins when both are present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WizardBranch {
    #[default]
    Gguf,
    Transformers,
}

/// Returns the wizard branch for a search result, or `None` when the repo
/// contains no recognizable model files.
///
/// Exact semantics:
/// - `hf_format == Some("transformers")` → `Some(Transformers)`
///   (the server's `detect_hf_format` only reports "transformers" when no
///   GGUF files exist, so this implies a safetensors-only repo)
/// - `hf_format == Some("gguf")` → `Some(Gguf)` (mixed repos report gguf)
/// - `hf_format == None` (metadata fetch failed) → `Some(Gguf)` iff
///   `has_gguf_files`, else `None` (degrade to today's flow when the listing
///   says GGUF files exist)
/// - any other `hf_format` value → `None`
pub fn resolve_branch(hf_format: Option<&str>, has_gguf_files: bool) -> Option<WizardBranch> { /* implement per the semantics above — a plain match, no clever arm-punning */ }
Tests (pure, no DOM):
- `test_resolve_branch_transformers` — `("transformers", false)` → `Some(Transformers)`
- `test_resolve_branch_gguf` — `("gguf", true)` → `Some(Gguf)`; `("gguf", false)` → `Some(Gguf)` (a mixed repo: GGUF wins — `detect_hf_format` already reports gguf)
- `test_resolve_branch_none_with_files` — `(None, true)` → `Some(Gguf)` (metadata fetch failed, listing says gguf files exist — degrade to today's flow)
- `test_resolve_branch_none_empty` — `(None, false)` → `None`; `("transformers", true)` → `Some(Transformers)` (document the GGUF-wins guarantee comes from the server: if GGUF files existed, format would be "gguf")

2. In `pull_quant_wizard.rs`:
- Add signal: `let branch = RwSignal::new(WizardBranch::Gguf);`
- Restructure the `on_search` closure:
  1. Fetch quants + metadata (unchanged).
  2. `let is_transformers = matches!(resolve_branch(metadata.as_ref().and_then(|m| m.hf_format.as_deref()), quants_parsed_nonempty), Some(WizardBranch::Transformers));` — compute the branch and handle three outcomes:
     - `Transformers` → create stub with body `{"repo_id": rid, "backend": "vllm", "metadata": metadata}` (same POST as today, different backend); store `model_id`; `branch.set(Transformers)`; `wizard_step.set(SelectQuants)` (renders Confirm via Task 6).
     - `Gguf` → create stub with `"backend": "llama_cpp"` (EXACTLY today's code, moved after the branch decision); `branch.set(Gguf)`; existing select flow.
     - `None` (no files) → **no stub created**; `error_msg.set(Some("No model files found in this repo (no .gguf or .safetensors files). Check the repo ID and try again."))`; stay on `RepoInput`.
  3. Also update the `is_open` reset path's inline fetch (the modal-lifecycle duplicate of the search logic, top of the file) to apply the same branch decision — it currently fetches ONLY the quant listing (no metadata), so it must also fetch `/metadata` to branch. **The two code paths MUST share one helper to avoid drift**: extract `#[cfg(not(feature = "ssr"))] async fn fetch_repo_listing(rid: &str) -> (Vec<QuantEntry>, Option<HfModelMetadata>)` in the same file (fetch both in parallel with `futures_util::join!`, same logic the search callback has today), and have BOTH the search callback and the reset Effect call it, then both run the same `resolve_branch` + stub-creation + step-routing code (extract that too if needed — e.g. a `handle_search_result` closure). Behavior of the GGUF path must stay identical.
- Step header: the "2. Select" label becomes `move || if branch.get() == WizardBranch::Transformers { "2. Confirm" } else { "2. Select" }` and "4. Configure" stays. Keep the `step_class(...)` calls unchanged.
- The GGUF rendering arms are untouched; the `SelectQuants` and `SetContext` and `Downloading` arms gain a branch check that Task 6 fills in (leave a clearly-marked `// Task 6: transformers variant` TODO and keep rendering the GGUF components when `branch == Gguf` so this task alone compiles and behaves exactly as today).

**Steps:**
- [ ] Write the failing `resolve_branch` tests in `pull_wizard/mod.rs`. Run `cargo nextest run --package tama -- pull_wizard` — failing (function missing)?
- [ ] Implement `WizardBranch` + `resolve_branch`. Tests green.
- [ ] Restructure `on_search` + reset path per the steps above (stub moved after branch decision; backend per branch; new error text; `branch` signal; branch-aware "2." label).
- [ ] Run `cargo nextest run --package tama -- pull_wizard` + `cargo nextest run --package tama -- components` — all green.
- [ ] Manual smoke (dev server, `make dev`): search a known GGUF repo (e.g. a small one) — confirm the stub is created with `llama_cpp` and the select step is unchanged; search a repo with no model files — confirm the new error text and **no new model row appears** in the DB/models page (this was the orphan-stub bug).
- [ ] Run `cargo fmt --all`, both clippy commands.
- [ ] Commit with message: `feat: wizard branches on hf_format; stub created after decision`

**Acceptance criteria:**
- [ ] `resolve_branch` is pure, documented, and fully unit-tested (4+ cases)
- [ ] No stub model row is created for repos with no model files (verified in the manual smoke)
- [ ] Transformers branch creates the stub with `backend: "vllm"` and sets `hf_format` via metadata
- [ ] GGUF flow behavior-identical (select step, context step, pull request bodies unchanged)
- [ ] CSR and SSR builds compile (`cargo check --package tama` and `cargo check --package tama --features ssr`)

---

### Task 6: Transformers wizard UI (Confirm, repo-pull download, vLLM configure)

**Context:**
This task builds the three transformers-flow components and wires them into the wizard's dispatch. The GGUF flow is untouched. The download step reuses the wizard's existing signals/step machinery but polls the repo-pull status endpoint (the per-file SSE listener is NOT reused — repo pulls are a single job, polled every 1.5 s with `gloo_timers::future::sleep`, the same timer crate the model editor uses).

The vLLM configure step saves via `PUT /tama/v1/models/{id}` with body `{"backend": "vllm", "vllm": {...}}` — the existing `ModelPatchBody` accepts both `backend` and `vllm` (`tama_core::config::VllmConfig` field names: `max_model_len`, `kv_cache_dtype`, `tensor_parallel_size`, `gpu_memory_utilization`, `trust_remote_code`). The wizard sends ONLY the five fields it exposes (all others stay at defaults); the JSON is built with `serde_json::json!` so no new shared type is needed. "Max model length" is prefilled from the completed job's `context_length` (fallback: the pre-fetched metadata's `hf_context_length`).

**Files:**
- Create: `crates/tama/src/components/pull_wizard/components/confirm_step.rs`
- Create: `crates/tama/src/components/pull_wizard/components/repo_pull_step.rs`
- Create: `crates/tama/src/components/pull_wizard/components/vllm_config_step.rs`
- Modify: `crates/tama/src/components/pull_wizard/components/mod.rs` (module declarations)
- Modify: `crates/tama/src/components/pull_quant_wizard.rs` (signals, dispatch arms, polling loop, cancel/retry)
- Modify: `crates/tama/src/components/pull_wizard/mod.rs` (DTOs: `RepoPullStartRequest`, `RepoPullStatus`, `VllmWizardSettings`, `vllm_patch_body` + tests; **change `format_bytes` signature from `i64` to `u64`**)
- Modify: `crates/tama/src/components/pull_wizard/components/selection_step.rs` + `pull_step.rs` (update `format_bytes` call sites for the `u64` signature — see step 1)
- Modify: `crates/tama/src/pages/model_editor/hardware_form.rs` (make `KV_CACHE_DTYPE_OPTIONS` `pub(crate)` so the wizard imports it instead of duplicating)

**What to implement:**

1. **`format_bytes` → `u64`** (in `pull_wizard/mod.rs`): change `pub fn format_bytes(bytes: i64)` to `pub fn format_bytes(bytes: u64)` (the body's comparisons already work as-is with u64). Then fix the existing call sites:
   - `selection_step.rs` (3 places): `.map(format_bytes)` on `Option<i64>` → `.map(|b| format_bytes(b as u64))`
   - `pull_step.rs` (2 places): `format_bytes(job.bytes_pulled as i64)` → `format_bytes(job.bytes_pulled)`; `.map(|b| format_bytes(b as i64))` → `.map(|b| format_bytes(b))`
   Run `cargo check --package tama` after — all call sites compile.
2. `confirm_step.rs` — `#[component] pub fn ConfirmStep(repo_id: Signal<String>, metadata: Signal<HfModelMetadata>, on_start: Callback<()>, on_back: Callback<()>) -> impl IntoView`:
   - Header "Confirm Download" + description.
   - Info banner (reuse `alert` classes, informational variant if one exists, else `form-card__desc`): "This repo contains safetensors (transformers) weights. Tama will download the whole repo with the hf CLI and set it up as a vLLM model."
   - Summary rows: repo id, architecture (`metadata.hf_architecture_type`), total params (`hf_total_params`), **total size** (`hf_total_size_bytes.map(format_bytes)` — render `—` when None), **file count** (`hf_file_count` — render `—` when None).
   - Buttons: Back (secondary), "Start Download" (primary, calls `on_start`).
2. `repo_pull_step.rs` — `#[component] pub fn RepoPullStep(status: Signal<RepoPullStatus>, on_retry: Callback<()>, on_cancel: Callback<()>, on_back: Callback<()>) -> impl IntoView`:
   - `RepoPullStatus` (defined in `pull_wizard/mod.rs`, `Deserialize`): `{ job_id, status, bytes_done, total_bytes, error, context_length }`.
   - Running: progress bar — reuse the same progress markup/classes `pull_step.rs` uses for its per-file bars (read that file first and mirror the CSS classes) showing `format_bytes(bytes_done)` / `format_bytes(total_bytes)` (indeterminate state when `total_bytes` is None) + a Cancel button (secondary).
   - Failed: error alert with `error` text + Retry (primary) + Back.
   - Cancelled: info text + Retry + Back.
   - Completed: brief "Download complete" state (the wizard auto-advances to SetContext on terminal status, so this is transient).
3. `vllm_config_step.rs` — `#[component] pub fn VllmConfigStep(settings: Signal<VllmWizardSettings>, initial_max_model_len: Signal<Option<u32>>, on_next: Callback<()>, on_back: Callback<()>) -> impl IntoView`:
   - `VllmWizardSettings` (in `pull_wizard/mod.rs`, `Clone + Default`): `{ max_model_len: Option<u32>, kv_cache_dtype: Option<String>, tensor_parallel_size: Option<u32>, gpu_memory_utilization: Option<f64>, trust_remote_code: bool }`.
   - On mount, if `initial_max_model_len` is Some and `max_model_len` is None, prefill it (use a one-shot `Effect` or initialize in the parent before rendering — parent-init is simpler: the wizard sets the settings signal when entering SetContext).
   - Fields (reuse `form-group`/`form-input`/`form-label` classes and the editor's field ids pattern `field-vllm-*`): Max model length (number, placeholder "e.g. 32768"), KV cache dtype (select over `KV_CACHE_DTYPE_OPTIONS` from `hardware_form` — it ALREADY contains `"auto"`, so just map the list directly; no dedupe needed), Tensor parallel size (number, placeholder "1"), GPU memory utilization (number, placeholder "0.9"), Trust remote code (checkbox).
   - Buttons: Back (to the Downloading step — informational), "Save & Finish" (primary → `on_next`), "Skip for now" (secondary → also calls `on_next` with a flag OR a second callback `on_skip` — add `on_skip: Callback<()>`; both advance to Done, only Save persists).
   - Keep it minimal: no validation beyond parse-failure-ignore (mirror the editor's `val.parse::<u32>().ok()` pattern — empty → None).
4. `pull_wizard/mod.rs` additions:
   ```rust
   #[derive(Serialize)] pub struct RepoPullStartRequest { pub repo_id: String, #[serde(skip_serializing_if = "Option::is_none")] pub model_id: Option<u32> }
   #[derive(Deserialize, Clone, Debug)] pub struct RepoPullStatus { pub job_id: String, pub status: String, #[serde(default)] pub bytes_done: u64, #[serde(default)] pub total_bytes: Option<u64>, #[serde(default)] pub error: Option<String>, #[serde(default)] pub context_length: Option<u32> }
   impl RepoPullStatus { pub fn is_terminal(&self) -> bool { matches!(self.status.as_str(), "completed" | "failed" | "cancelled") } pub fn is_completed(&self) -> bool { self.status == "completed" } }
   ```
   Plus pure helper `pub fn vllm_patch_body(s: &VllmWizardSettings) -> serde_json::Value` (builds `{"backend":"vllm","vllm":{...}}` with `kv_cache_dtype`/`max_model_len` etc. only when Some, `trust_remote_code` always, `tensor_parallel_size`/`gpu_memory_utilization` when Some) + a unit test asserting the exact JSON (field presence/absence for a half-filled and a fully-default settings).
   NOTE on the save endpoint: `PUT /tama/v1/models/:id` deserializes **`ModelBody`** (not `ModelPatchBody` — `backend: String` is required, `vllm: Option<VllmConfig>`). The body `{"backend":"vllm","vllm":{...}}` deserializes correctly for `ModelBody`, and `apply_model_body` merges with the existing row field-by-field via `.or(existing)` so the stub's `model`/`quant`/`hf_*` fields are preserved. Do not send other fields.
5. `pull_quant_wizard.rs` wiring:
   - New signals: `repo_pull_status: RwSignal<Option<RepoPullStatus>>`, `repo_pull_job_id: RwSignal<Option<String>>`, `vllm_settings: RwSignal<VllmWizardSettings>`.
   - `SelectQuants` arm: `branch == Transformers` → render `ConfirmStep`; `on_start` → `spawn_local`: POST `/tama/v1/pulls/repo` with `RepoPullStartRequest { repo_id, model_id: model_id.get_untracked() }` (via `post_request(...).json(...)`); 422/other → `error_msg` + back to RepoInput; success → store job_id, seed `repo_pull_status` with `{status:"running", total_bytes from response}`, `wizard_step.set(Downloading)`, start the poll loop.
   - Poll loop (one `spawn_local` fn `spawn_repo_poll(status_sig, wizard_step, cancelled, es-free)`): `loop { if cancelled.get() { break } let st = get_request(&format!("/tama/v1/pulls/repo/{}", job_id)).send().await; if ok { status_sig.set(Some(st)) } gloo_timers::future::sleep(Duration::from_millis(1500)).await; if st.is_terminal() { break } }` then advance: completed → set vllm prefill (`vllm_settings` max_model_len from `st.context_length` or `hf_metadata` fallback) + `wizard_step.set(SetContext)`; failed/cancelled → stay on Downloading (RepoPullStep shows the error/retry UI).
   - `Downloading` arm: `branch == Transformers` → `RepoPullStep(status, on_retry=|_| { re-run on_start logic (extract a `start_repo_pull_job()` closure) }, on_cancel=|_| spawn DELETE, on_back=|_| wizard_step.set(SelectQuants))`; GGUF → existing `PullStep` unchanged.
   - `SetContext` arm: `branch == Transformers` → `VllmConfigStep`; `on_next`/`on_skip`: next → `spawn_local` PUT `/tama/v1/models/{id}` with `vllm_patch_body(&settings)` (id = `model_id` else `config_key_from_repo_id`), success → Done, failure → `error_msg`; skip → straight to Done.
   - `Done` arm unchanged. `on_complete` Effect unchanged — verified: the models page handler (`crates/tama/src/pages/models/mod.rs` ~line 414) ignores the `CompletedQuant` vec and increments its refresh signal, so the empty-vec case already triggers a full model list re-fetch. No models-page change is needed.
   - Reset paths: reset the new signals wherever the existing signals are reset (search callback + is_open reset Effect).

**Steps:**
- [ ] Do step 1 first (`format_bytes` → u64 + call-site fixes); `cargo check --package tama` compiles.
- [ ] Add `RepoPullStartRequest`, `RepoPullStatus`, `VllmWizardSettings`, `vllm_patch_body` to `pull_wizard/mod.rs` with a failing test for `vllm_patch_body` first (half-filled: only `max_model_len` Some → JSON has exactly `backend`, `vllm.max_model_len`, `vllm.trust_remote_code`; all-default → `vllm` object has only `trust_remote_code: false`). Run `cargo nextest run --package tama -- pull_wizard` — failing → implement → green.
- [ ] Make `KV_CACHE_DTYPE_OPTIONS` in `hardware_form.rs` `pub(crate)` (update the const line only; confirm no other visibility-dependent usage).
- [ ] Create the three step components + module declarations. `cargo check --package tama` compiles.
- [ ] Wire the wizard per item 5. `cargo check --package tama` + `cargo check --package tama --features ssr` both compile.
- [ ] Manual E2E (dev server, needs `hf` on PATH and a small public safetensors repo — e.g. `HuggingFaceTB/SmolLM2-135M-Instruct` or any <1 GB transformers repo):
  1. Open the wizard, search the repo → Confirm step shows size + file count (matches `hf`'s own report within tolerance).
  2. Start Download → progress advances; Cancel works (process actually dies — `ps aux | grep "hf download"`); Retry resumes.
  3. Let it finish → Configure step prefills max model length from config.json; Save & Finish → Done.
  4. Models page shows the model with backend `vllm`, `hf_format` transformers label (model card format badge), and the editor's vLLM form shows the saved settings.
  5. Search a GGUF repo → flow unchanged (spot-check select + context steps).
- [ ] Run `cargo nextest run --package tama -- pull_wizard`, `cargo nextest run --package tama -- components`, `cargo nextest run --workspace`.
- [ ] Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy --package tama --features ssr --all-targets -- -D warnings`.
- [ ] Commit with message: `feat: transformers wizard flow (confirm, hf repo pull, vLLM configure)`

**Acceptance criteria:**
- [ ] A safetensors repo pulled through the wizard results in: all files under `<models_dir>/<org>/<repo>`, a model row with `backend=vllm`, `hf_format=transformers`, `hf_context_length` from config.json, and (when `quantization_method` present) `quant` set
- [ ] Cancel kills the subprocess; retry resumes without re-downloading completed files
- [ ] "Skip for now" lands on Done with a launchable model; "Save & Finish" persists the five vLLM settings (visible in the editor afterward)
- [ ] GGUF flow spot-check passes; all workspace tests green; both clippy gates clean
