# Backup & Restore Implementation Plan

**Goal:** Make the backup feature real: route the unrouted `create_backup` handler at `GET /tama/v1/backup`, replace the no-op `start_restore` stub with the existing, tested `tama_core::backup` merge machinery driven by a job with progress events, and fix `docs/api/backup.md` to match reality.

**Architecture:** All merge logic already exists in `crates/tama-core/src/backup/` (`create_backup` archive.rs:92, `extract_backup` archive.rs:325, `merge_config` merge.rs:55, `merge_model_cards` merge.rs:84, `merge_database` merge.rs:135 — 16 tests) but has no live callers. This plan only wires it up in `crates/tama`: one route registration in `router.rs`, one new sync function `run_restore` plus a rewritten task body in `api/backup.rs`, route-level tests, and a docs rewrite. No changes to `tama-core` are needed.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio, tower (tests)

---

### Task 1: Route `create_backup` at `GET /tama/v1/backup`

**Context:**
The handler `create_backup` (`crates/tama/src/api/backup.rs:68`) is fully implemented — it resolves the config dir from `state.db_dir()` (falling back to `tama_core::config::Config::config_dir()`), calls `tama_core::backup::create_backup` inside `tokio::task::spawn_blocking`, and streams the archive bytes back with `Content-Type: application/gzip` and `Content-Disposition: attachment; filename="backup.tar.gz"` — but `crates/tama/src/router.rs:22` imports only `restore_preview` and `start_restore`, so the handler is unreachable even though `docs/api/backup.md` documents the endpoint. Decision: mount it in `backend_routes` (the sub-router wrapped in `middleware::from_fn(api::middleware::enforce_same_origin)` + CORS at `crates/tama/src/router.rs:100-204`), right next to the restore routes. GET requests pass through the CSRF middleware's token-issuance branch, so no token is required for the download; putting it in this group keeps all backup/restore management routes in one place.

**Files:**
- Modify: `crates/tama/src/router.rs`
- Modify: `crates/tama/src/api/backup.rs` (tests only)

**What to implement:**

1. **`crates/tama/src/router.rs`** — change line 22 from `use crate::api::backup::{restore_preview, start_restore};` to `use crate::api::backup::{create_backup, restore_preview, start_restore};`. In `build_web_routes`, immediately before the `// Restore routes (CSRF-protected)` comment inside `backend_routes` (before the `/tama/v1/restore/preview` route, ~line 157), add:
   ```rust
   // Backup download (GET passes through CSRF token issuance)
   .route("/tama/v1/backup", get(create_backup)),
   ```
   Do NOT touch any other route group.

2. **Route test** in the existing `#[cfg(test)] mod tests` at the bottom of `crates/tama/src/api/backup.rs` (line 310). Add the shared helpers and the first route test here (later tasks reuse the helpers):
   - Helper `fn test_web_state() -> crate::web_types::WebState` — copy the exact body of the helper at `crates/tama/src/api/backends/manage/tests.rs:8-18` (`jobs: Some(Arc::new(JobManager::new()))`, `capabilities: None`, `update_checker: Arc::new(tama_core::updates::UpdateChecker::default())`, `binary_version: "test".to_string()`, `update_tx`, `upload_lock`).
   - Helper `fn seed_config_dir(dir: &std::path::Path)` — create `dir/configs/` and a `dir/tama.db` whose schema is the three-table DDL copied verbatim from the roundtrip test in `crates/tama-core/src/backup/archive.rs` (the `conn.execute_batch("CREATE TABLE model_pulls ... CREATE TABLE model_files ... CREATE TABLE backend_installations ...")` block, ~lines 487-490), then insert one `model_pulls` row (`'test/repo'`). Do NOT call `tama_core::db::open` for the seed — the restore path runs migrations via `Config::to_db`, and the seed DB must look like a real pre-migration config dir; raw DDL keeps the test independent of migration details.
   - Helper to build the app: `fn test_app(state: Arc<ProxyState>, web_state: &Arc<crate::web_types::WebState>) -> Router` following `crates/tama/src/api/backends/manage/tests.rs:26-31`: `crate::router::build_web_routes(web_state.clone()).with_state(state).layer(axum::extract::Extension(web_state.as_ref().clone()))`. (`build_web_routes` already adds the Extension layer internally, but adding it again is harmless and matches the existing test pattern.)
   - `#[tokio::test] async fn test_create_backup_route_returns_gzip_download`: tempdir + `seed_config_dir`; `let state = Arc::new(ProxyState::new(tama_core::config::Config::default(), Some(tempdir.path().to_path_buf())));` (signature: `ProxyState::new(config: Config, db_dir: Option<PathBuf>)`, `crates/tama-core/src/proxy/state.rs:9`). GET `/tama/v1/backup` via `tower::ServiceExt::oneshot` with `Body::empty()`. Assert: status `200 OK`; `content-type` header is `application/gzip`; `content-disposition` header starts with `attachment; filename=`; body bytes start with the gzip magic `0x1f 0x8b`; and `tama_core::backup::extract_manifest` can parse a manifest out of the body (write the body to a temp file first — `extract_manifest(archive_path: &Path)` at `crates/tama-core/src/backup/archive.rs:303` takes a path) and the manifest's `models` contains `test/repo`.

**Steps:**
- [ ] Write the helpers + failing test `test_create_backup_route_returns_gzip_download` in `crates/tama/src/api/backup.rs` (fails with 404 before the route exists)
- [ ] Run `cargo nextest run --package tama -- api::backup` — verify the new test fails with 404
- [ ] Add the import and route in `crates/tama/src/router.rs` per above
- [ ] Run `cargo nextest run --package tama -- api::backup` — all pass
- [ ] Run `cargo nextest run --package tama` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "feat: route GET /tama/v1/backup for archive download"

**Acceptance criteria:**
- [ ] `rg '"/tama/v1/backup"' crates/tama/src/router.rs` — one hit inside `backend_routes`
- [ ] GET `/tama/v1/backup` returns 200 with a parseable gzip archive — proven by the new test
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 2: Implement the restore merge in `start_restore`

**Context:**
`start_restore` (`crates/tama/src/api/backup.rs:227`) validates the `upload_id` against `WebState.upload_lock`, submits a `JobKind::Restore` job, and spawns a task whose body is `// TODO: Implement actual restore logic` + `let _ = (config_dir, temp_dir, job);` — it reports success while doing nothing (or returns a 501 stopgap if plan-159 task 2 landed first; this task replaces either version). The full merge machinery exists and is tested in `tama-core`. Decisions: (1) the merge runs as a new private sync function `run_restore` so it can be unit-tested without a router; (2) `selected_models`/`skip_backends`/`skip_models` on `RestoreRequest` are accepted but NOT applied — restore v1 always performs the full additive merge, because the merge steps in `tama-core` are monolithic (e.g. `merge_database` merges all three tables in one call) and filtering is a follow-up; the job log says so explicitly; (3) atomicity is what `merge.rs` already guarantees: extraction+validation completes in a temp dir before ANY mutation, each `INSERT OR IGNORE` in `merge_database` is a single atomic statement, `merge_model_cards` copies per-file (local wins), and `merge_config` is pure in-memory until `Config::to_db` persists it — do NOT wrap merges in an explicit transaction (SQLite `DETACH` inside `merge_database`'s `DetachGuard` interacts badly with an open transaction); (4) on success the running proxy's in-memory config is refreshed via `state.config()` so the merged backends take effect without restart.

**Files:**
- Modify: `crates/tama/src/api/backup.rs`

**What to implement:**

1. **New private function** in `crates/tama/src/api/backup.rs` (above `start_restore`):
   ```rust
   /// Apply a validated backup archive to `config_dir`.
   ///
   /// Order of operations (all additive; local data always wins):
   /// 1. Extract + validate the archive into a temp dir (manifest parse,
   ///    `BACKUP_FORMAT_VERSION` check, SHA-256 integrity — any failure here
   ///    means zero mutations to `config_dir`).
   /// 2. `merge_model_cards` — copy new `configs/*.toml` cards (per-file).
   /// 3. `merge_database` — INSERT OR IGNORE model_pulls, model_files,
   ///    backend_installations (each statement individually atomic).
   /// 4. `merge_config` — in-memory merge, persisted with `Config::to_db`.
   ///
   /// Returns the merged `Config` (for the in-memory refresh) and a
   /// human-readable summary for the job log.
   fn run_restore(
       config_dir: &std::path::Path,
       archive_path: &std::path::Path,
   ) -> anyhow::Result<(tama_core::config::Config, String)>
   ```
   Body:
   - `let extract_dir = tempfile::tempdir().context("Failed to create restore temp dir")?;`
   - `let extracted = tama_core::backup::extract_backup(archive_path, extract_dir.path()).context("Failed to extract backup archive")?;` — `extract_backup` (`crates/tama-core/src/backup/archive.rs:325`) already does manifest parse, `manifest.validate_version()`, and the SHA-256 check that wipes the target dir on mismatch. Returns `ExtractResult { manifest, db_path, card_paths }`.
   - `let copied_cards = tama_core::backup::merge_model_cards(&config_dir.join("configs"), &extract_dir.path().join("configs")).context("Failed to merge model cards")?;`
   - `let open = tama_core::db::open(config_dir).context("Failed to open local database")?;` then `let db_stats = tama_core::backup::merge_database(&open.conn, &extracted.db_path).context("Failed to merge database")?;` (`db::open` at `crates/tama-core/src/db/mod.rs:95` returns `OpenResult { conn, .. }`; `merge_database` at merge.rs:135 takes `&rusqlite::Connection` and attaches the backup DB with an RAII `DetachGuard`).
   - `let db_path = config_dir.join("tama.db");` `let mut local = tama_core::config::Config::load_from(&db_path).context("Failed to load local config")?;` `let backup_cfg = tama_core::config::Config::load_from(&extracted.db_path).context("Failed to load backup config")?;` (`Config::load_from` at `crates/tama-core/src/config/loader.rs:63`). `let cfg_stats = tama_core::backup::merge_config(&mut local, &backup_cfg);` then `local.to_db(&db_path).context("Failed to save merged config")?;`
   - Build the summary string from `copied_cards.len()`, `db_stats.new_model_pulls` / `new_model_files` / `new_backend_installations`, `cfg_stats.new_backends.len()` / `skipped_backends.len()`, and `extracted.manifest.models.len()`; include the literal sentence `Full merge performed; selected_models/skip_backends/skip_models are accepted but not yet applied.`
   - `Ok((local, summary))`

2. **Rewrite `start_restore`'s spawned task.** Keep everything up to and including the `jobs.submit(crate::web_types::JobKind::Restore, None).await` + `match job` structure and the 404/`NOT_FOUND` upload lookup unchanged (if the plan-159 501 stopgap is present, delete it entirely). Two changes:
   - Handler signature: `State(_state): State<Arc<ProxyState>>` → `State(state): State<Arc<ProxyState>>` (the state is now used for the in-memory config refresh).
   - Config-dir resolution: replace the current `tama_core::config::Config::config_dir()`-only block with the same resolution `create_backup` uses (backup.rs:70-75): `state.db_dir().clone().unwrap_or_else(|| tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))` — keep the existing 500 `error_response` when `Config::config_dir()` errors and there is no `db_dir` (structure the match so both fallbacks are tried before failing).
   - Inside `tokio::spawn`, on the `Ok(job)` arm (delete the TODO block and the `let _ = (config_dir, temp_dir, job);`):
   ```rust
   let jobs_for_spawn = jobs.clone();
   let job_for_spawn = job.clone();
   let state_for_spawn = state.clone();
   let cleanup_path = upload_path.clone();
   tokio::spawn(async move {
       jobs_for_spawn
           .append_log(&job_for_spawn, "Extracting and validating backup archive".to_string())
           .await;
       let config_dir_for_blocking = config_dir.clone();
       let archive_for_blocking = upload_path.clone();
       let result = tokio::task::spawn_blocking(move || {
           run_restore(&config_dir_for_blocking, &archive_for_blocking)
       })
       .await;

       match result {
           Ok(Ok((merged_config, summary))) => {
               // Refresh the running proxy's in-memory config so merged
               // backends/templates take effect without a restart.
               *state_for_spawn.config().write().await = merged_config;
               jobs_for_spawn.append_log(&job_for_spawn, summary).await;
               jobs_for_spawn
                   .finish(&job_for_spawn, crate::web_types::JobStatus::Succeeded, None)
                   .await;
           }
           Ok(Err(e)) => {
               tracing::error!("Restore job {} failed: {:#}", job_for_spawn.id, e);
               jobs_for_spawn
                   .finish(
                       &job_for_spawn,
                       crate::web_types::JobStatus::Failed,
                       Some(format!("{:#}", e)),
                   )
                   .await;
           }
           Err(join_err) => {
               tracing::error!("Restore task panicked: {:?}", join_err);
               jobs_for_spawn
                   .finish(
                       &job_for_spawn,
                       crate::web_types::JobStatus::Failed,
                       Some(format!("Restore task panicked: {}", join_err)),
                   )
                   .await;
           }
       }

       // Clean up the uploaded archive after the restore completes (success or failure).
       if let Err(e) = std::fs::remove_file(&cleanup_path) {
           tracing::warn!("Failed to delete upload file: {}", e);
       }
   });
   ```
   Note `jobs` is currently bound as `&Arc<JobManager>` via `let Some(jobs) = web_state.jobs.as_ref() else {...}` — clone it into an owned `Arc` before the spawn (`jobs_for_spawn` above) exactly as `submit_benchmark_job` does at `crates/tama/src/api/benchmarks/mod.rs:231-264`. Keep returning `Json(RestoreResponse { job_id })` on submission and the `409 CONFLICT` arm unchanged.

3. Do NOT change: `RestoreRequest`/`RestoreResponse` DTOs, `restore_preview`, `create_backup`, the `UploadEntry` re-export, or `merge.rs`/`archive.rs` in `tama-core`.

**Steps:**
- [ ] Implement `run_restore` and the `start_restore` rewrite in `crates/tama/src/api/backup.rs`
- [ ] Run `cargo check --package tama` — compiles (remove now-unused imports only if clippy/compiler flags them, e.g. `temp_dir` local)
- [ ] Run `cargo nextest run --package tama -- api::backup` — existing 13 DTO tests + task-1 route test still pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "feat: implement backup restore merge behind the restore job"

**Acceptance criteria:**
- [ ] `rg "TODO: Implement actual restore logic" crates/` — zero hits; `start_restore` contains no `let _ =` discard
- [ ] `run_restore` calls exactly `extract_backup`, `merge_model_cards`, `merge_database`, `merge_config`, `Config::to_db` in the documented order
- [ ] The spawned task always calls `jobs.finish` (Succeeded or Failed) and always attempts upload cleanup
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 3: Route-level restore tests (happy path, corrupt archive, 404)

**Context:**
The existing `#[cfg(test)] mod tests` in `crates/tama/src/api/backup.rs` (13 tests, line 310) covers only DTO field assertions — no test has ever exercised `start_restore` over HTTP. These tests pin the failure semantics decided in Task 2: a corrupt or tampered archive must fail the job with the extraction error and leave the local `tama.db` and `configs/` untouched, and a successful restore must actually merge rows and refresh the in-memory config. The archive fixture is built with the real `tama_core::backup::create_backup` (`crates/tama-core/src/backup/archive.rs:92`) against a second seeded config dir — never a hand-crafted tar — so the fixture can never drift from the format.

**Files:**
- Modify: `crates/tama/src/api/backup.rs` (tests only)

**What to implement:**

Add to the existing test module, reusing Task 1's `test_web_state`, `seed_config_dir`, and `test_app` helpers. Additional helpers:

- `fn seed_source_and_make_archive(source_dir: &Path, archive_path: &Path)` — call `seed_config_dir(source_dir)`, then insert one extra row that the local dir does NOT have (`INSERT INTO model_pulls (repo_id, commit_sha, pulled_at) VALUES ('source/only', 'def456', ...)`) and one active `backend_installations` row (use the INSERT from `crates/tama-core/src/backup/archive.rs` roundtrip test, ~lines 496-505), then `tama_core::backup::create_backup(source_dir, archive_path).expect("create fixture archive")`.
- `async fn wait_for_job(jobs: &Arc<crate::web_types::JobManager>, job_id: &str) -> crate::web_types::JobState` — poll `jobs.get(&job_id.to_string()).await` up to 100 × 20 ms (`tokio::time::sleep`) until `job.state.read().await.status != JobStatus::Running`; panic with the job's `error` on timeout. Note `JobState` is not `Clone` — read `status`/`error` out under the lock and return them as a tuple `(JobStatus, Option<String>)` instead.
- Seeding the upload: build `web_state`, then `web_state.upload_lock.write().await.insert("up-1".to_string(), crate::web_types::UploadEntry { path: archive_path.clone(), created_at: chrono::Utc::now() });` (`UploadEntry` fields at `crates/tama/src/web_types.rs:395-399`).

Tests:

1. `test_start_restore_unknown_upload_returns_404` — POST `/tama/v1/restore` with JSON `{"upload_id": "nope"}` and a valid CSRF pair (cookie `tama_csrf_token=t` + header `x-csrf-token: t`, pattern from `crates/tama/src/api/backends/manage/tests.rs:33-42`) → assert `404 NOT_FOUND` and `body["error"]["type"] == "NotFoundError"`.
2. `test_start_restore_merges_archive_and_completes_job` — tempdirs `local/` (seeded) and `source/` (fixture archive at `source/backup.tar.gz`); `ProxyState::new(Config::default(), Some(local.path().to_path_buf()))`; seed upload; POST → `200 OK`, body has `job_id` string; `wait_for_job` → `(JobStatus::Succeeded, None)`; then assert the merge actually happened:
   - open `local/tama.db` with `rusqlite` and assert `SELECT COUNT(*) FROM model_pulls WHERE repo_id = 'source/only'` is 1;
   - `state.config().read().await.backends` contains the backend name inserted into the source fixture (in-memory refresh);
   - the uploaded archive file no longer exists (cleanup ran).
3. `test_start_restore_corrupt_archive_fails_job_and_leaves_config_untouched` — same setup but the "archive" is a temp file containing `b"not a gzip"`; POST → 200 (job accepted); `wait_for_job` → `JobStatus::Failed` with `error` containing `extract` (matches the `context("Failed to extract backup archive")` wrapper); assert `local/tama.db` still has exactly the one seeded `model_pulls` row and `local/configs/` has no new files.
4. `test_start_restore_tampered_sha_fails_job` — build a valid archive, then flip a byte near the end of the file (read into `Vec<u8>`, XOR the 8th-from-last byte, rewrite — this corrupts a data block, not just the gzip trailer, so the SHA-256 check trips rather than only the gzip CRC; if the gzip decoder rejects it first, that is also an acceptable failure — assert only `Failed`, not a specific message); `wait_for_job` → `JobStatus::Failed`; local DB unchanged.

**Steps:**
- [ ] Write the four tests in `crates/tama/src/api/backup.rs` (they fail against the pre-Task-2 stub: happy path reports `Succeeded` without merging / or 501 with the stopgap)
- [ ] Run `cargo nextest run --package tama -- api::backup` — confirm failures for the right reasons, then confirm all pass against the Task-2 implementation
- [ ] Run `cargo nextest run --package tama` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: route-level tests for backup restore success and failure semantics"

**Acceptance criteria:**
- [ ] Four new route tests pass, proving: 404 unknown upload; happy path merges `model_pulls`/backends + refreshes in-memory config + deletes upload; corrupt and tampered archives fail the job and leave the local DB/`configs/` byte-identical
- [ ] No test touches the real user config dir (all paths under `tempfile::tempdir()`; `ProxyState::new` gets `Some(<tempdir>)` as `db_dir`)
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 4: Fix `docs/api/backup.md` to match reality

**Context:**
The current doc documents `GET /tama/v1/backup` (which only became real in Task 1), and claims the restore response is `{ "jobId": "uuid-string" }` — but `RestoreResponse` (`crates/tama/src/api/backup.rs:46-49`) serializes as `{"job_id": "..."}` (snake_case, no serde rename). It also omits the error shapes, the conflict case, the job-tracking endpoints, and the full-merge semantics decided in Task 2. Docs-only task; no code changes.

**Files:**
- Modify: `docs/api/backup.md`

**What to implement:**

Rewrite the file with four sections, matching the style of `docs/api/jobs.md`:

1. **`GET /tama/v1/backup`** — returns the archive as `application/gzip` with `Content-Disposition: attachment; filename="backup.tar.gz"`; errors: `500` nested error shape (`{"error":{"message":...}}`) when the archive cannot be created.
2. **`POST /tama/v1/restore/preview`** — keep the existing content (it is accurate); add the error cases that exist in code: `400 ValidationError` for missing/unparseable file, and note the upload is stored under `<config_dir>/uploads/<upload_id>.tar.gz` until consumed by restore.
3. **`POST /tama/v1/restore`** — request-body table (keep the four fields, but mark `selected_models`, `skip_backends`, `skip_models` as "accepted for forward compatibility; restore currently always performs the full additive merge (local data wins)"); success response corrected to:
   ```json
   { "job_id": "j_<uuid>" }
   ```
   Errors: `404 NotFoundError` (unknown/expired `upload_id`), `409 ConflictError` (another job already running — from `JobError::AlreadyRunning` in `crates/tama/src/web_types.rs`). Note that the uploaded archive is deleted after the job finishes, on success or failure.
4. **Tracking the restore job** — point at `GET /tama/v1/backends/jobs/:id` and the SSE stream `GET /tama/v1/backends/jobs/:id/events` (see `docs/api/jobs.md`); restore emits `log` events for each merge step and a terminal `status` event of `Succeeded` or `Failed` (with `error` set on failure). State the failure guarantee: an archive that fails validation (bad manifest, unsupported `version`, SHA-256 mismatch) fails the job without modifying the local config or database.

**Steps:**
- [ ] Rewrite `docs/api/backup.md` per above
- [ ] Run `rg -n "jobId" docs/api/` — zero hits outside historical plans
- [ ] Commit with message: "docs: document real backup/restore behavior and job tracking"

**Acceptance criteria:**
- [ ] Every documented field name matches the Rust DTOs (`job_id`, `upload_id`, `selected_models`, `skip_backends`, `skip_models`)
- [ ] The full-merge caveat and the job-tracking endpoints are documented
- [ ] No code changed in this commit
