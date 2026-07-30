# Test Coverage Wave 2 Plan

**Goal:** Close the second wave of test gaps from the 2026-07-18 audit: route compaction/TTS spawn paths through the plan-148 lifecycle traits and test their failure modes (F22), test the update-check fetch→compare→persist orchestration (F23), test the bench/pull execution engines and wire `tama-mock` into a real integration test (F24), and fill the remaining route-handler gaps including the 5 permanently ignored backend tests (F36).

**Architecture:** All work is test-first against existing seams: the lifecycle traits in `crates/tama-core/src/proxy/lifecycle/traits.rs` (only `HealthChecker` is currently used by the main LLM path), the `HF_ENDPOINT` env seam honored by `hf-hub` 0.5 and by `models/pull/*`, the `LLAMA_BENCH_PATH`/`LLAMA_SERVER_TIMEOUT_SECS` env seams in `bench/`, wiremock for HTTP stubs (established in `backends/updater.rs` tests), tempdir-backed `ProxyState::new(config, Some(dir))` (established in `crates/tama/src/api/backends/manage/tests.rs`), and `env!("CARGO_BIN_EXE_tama-mock")` from `crates/tama-mock/tests/`. Two small production changes are required to make failure modes testable: compaction/TTS spawn-error cleanup (currently leaves a stuck `Starting` entry) and a `run_llama_bench` `db_dir` extraction. cargo-nextest runs each test in its own process, which makes `std::env::set_var` per test safe — no serial guards needed.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio, wiremock, tower (tests), tempfile

---

### Task 1: Route compaction & TTS spawn paths through lifecycle traits (F22)

**Context:**
`ProcessSpawner`/`PortAllocator`/`HealthChecker` in `crates/tama-core/src/proxy/lifecycle/traits.rs` were built in plan-148 to make spawn failure, port allocation, and health-timeout paths testable, but only `HealthChecker` is used (by `ProxyState::load_model` in `crates/tama-core/src/proxy/lifecycle/mod.rs:64`). Compaction (`lifecycle/compaction.rs`) and TTS (`lifecycle/tts.rs`) spawn directly via `tokio::process::Command`, bind ports with raw `TcpListener`, and health-poll via `process::check_health` — zero tests cover them. Two real bugs surface when writing the tests: both functions insert a `BackendState::Starting` reservation BEFORE spawning, and on spawn failure they return early without removing it (stuck `Starting` entry forever); and `ProxyState::load_compaction_backend`/`load_tts_backend` resolve the base directory via `Config::base_dir()` (real `~/.config/tama`) instead of the test-seamable `self.db_dir`. Decisions (already made): (a) both functions become generic over `<H: HealthChecker, S: ProcessSpawner, P: PortAllocator>` and production callers pass `&(), &(), &()`, exactly mirroring `load_model<H: HealthChecker>`; (b) base dir resolves from `self.db_dir` first, falling back to `Config::base_dir()` — production passes `Some(db_dir)` (`crates/tama/src/main.rs:97`), tests pass a tempdir; (c) the per-process "reaper" `tokio::spawn(child.wait())` logging task is deleted — tokio's runtime reaps dropped `Child` handles via its orphan queue, so no zombie regression; dead-PID detection in the idle checker continues to cover crash cleanup; (d) `proxy/server/mod.rs`'s orphan-kill stops shelling out to `kill` and uses the existing `process::kill_process` helper (no trait — it is a startup reconciliation loop, not a spawn path); (e) `MockProcessSpawner` gains a spawn-failure mode so spawn-error cleanup is testable.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/traits.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/compaction.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/tts.rs`
- Modify: `crates/tama-core/src/proxy/server/mod.rs`
- Modify: `crates/tama-core/src/proxy/handlers/compaction.rs`
- Modify: `crates/tama-core/src/proxy/handlers/tts.rs`
- Modify: `crates/tama/src/api/backends/compaction.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/tests.rs`

**What to implement:**

1. **`traits.rs` — add spawn-failure mode to `MockProcessSpawner`.** Add one field to the struct (keep the existing three):
   ```rust
   pub fail_spawn: std::sync::Arc<std::sync::atomic::AtomicBool>,
   ```
   Add to the existing `#[allow(dead_code)] impl MockProcessSpawner` block:
   ```rust
   /// Configure the mock to fail the next (and every subsequent) spawn.
   pub fn set_fail_spawn(&self, fail: bool) {
       self.fail_spawn
           .store(fail, std::sync::atomic::Ordering::SeqCst);
   }
   ```
   In the `ProcessSpawner for MockProcessSpawner` `spawn` impl, before incrementing `spawn_count`, return early when the flag is set:
   ```rust
   if self.fail_spawn.load(std::sync::atomic::Ordering::SeqCst) {
       return Err(anyhow::anyhow!("Mock spawn error for '{}'", _cmd));
   }
   ```
   Note: the parameter is currently named `_cmd`; rename it to `cmd` and use it in the error message. Do NOT touch `SpawnedProcess`, the `()` impls, `MockHealthChecker`, `MockPortAllocator`, `MockProcessChecker`, or the existing tests.

2. **`compaction.rs` — generic `load_compaction_backend`.** Change the signature to:
   ```rust
   pub async fn load_compaction_backend<H: HealthChecker, S: ProcessSpawner, P: PortAllocator>(
       &self,
       health_checker: &H,
       spawner: &S,
       port_allocator: &P,
   ) -> Result<()>
   ```
   Add `use super::traits::{HealthChecker, ProcessSpawner, PortAllocator};` at the top. Then:
   - Step 4 (base dir): replace `let base_dir = crate::config::Config::base_dir().with_context(...)?;` with:
     ```rust
     let base_dir = match self.db_dir.clone() {
         Some(dir) => dir,
         None => crate::config::Config::base_dir()
             .with_context(|| "Failed to get config directory")?,
     };
     ```
   - Step 6 (port): keep the `if let Some(p) = compaction.port` arm; replace the `else` TcpListener block with `port_allocator.allocate_port().with_context(|| "Failed to allocate port for compaction backend")?`.
   - Step 9 (spawn): build
     ```rust
     let args: Vec<String> = vec![
         "run".into(), "--project".into(), server_dir.to_string_lossy().into_owned(),
         "uvicorn".into(), uvicorn_target.clone(),
         "--host".into(), "127.0.0.1".into(), "--port".into(), port.to_string(),
     ];
     let env: Vec<(&str, String)> = vec![
         ("COMPACTION_PORT", port.to_string()),
         ("COMPACTION_DEVICE", compaction.device.as_str().to_string()),
     ];
     let spawned = spawner.spawn("uv", &args, &env, Some(&server_dir)).await
     ```
     On spawn error: remove the `"compaction"` entry from `self.models` (write lock) and clear `self.inference_stats` for that key (same `send_modify` pattern used in `tts.rs`'s timeout cleanup), THEN return the error with the existing context `"Failed to spawn compaction server via uv run (install with: pipx install uv)"`. This is the stuck-Starting fix.
     Delete the `child.id()` block (use `spawned.pid`), the `configure_process_group` call, and the entire step-11 reaper `tokio::spawn(async move { match child.wait() ... })` (see Context decision (c)).
   - Step 12 (health loop): replace the `if let Ok(response) = check_health(&health_url, Some(5)).await { if response.status().is_success() { ... break; } }` block with `if health_checker.check_health(&health_url, Some(5)).await { debug!("Health check passed for compaction backend"); break; }`. In the timeout arm, replace `kill_process_group(pid).await` / `is_process_group_alive(pid)` / `force_kill_process_group(pid).await` with the `spawner` trait methods of the same names. Keep the `Failed`-state transition and error message exactly as they are.
   - Imports: remove `check_health`, `configure_process_group`, `is_process_group_alive` from the `crate::proxy::process` use (keep `kill_process_group`/`force_kill_process_group` ONLY if still referenced — after this edit they are not; drop them). Keep `Context, Result` imports.

3. **`tts.rs` — generic `load_tts_backend`.** Same treatment with TTS specifics:
   ```rust
   pub async fn load_tts_backend<H: HealthChecker, S: ProcessSpawner, P: PortAllocator>(
       &self,
       backend_name: &str,
       health_checker: &H,
       spawner: &S,
       port_allocator: &P,
   ) -> Result<String>
   ```
   - Base dir: same `self.db_dir`-first match as compaction (currently `Config::base_dir()` at the top of the function).
   - Port: replace the `tokio::net::TcpListener::bind("127.0.0.1:0")` block with `let port = port_allocator.allocate_port()?;`.
   - Spawn: `python_bin` is a `PathBuf`; pass `&python_bin.to_string_lossy()` as `cmd`. args: `["-m", "uvicorn", "api.src.main:app", "--host", "127.0.0.1", "--port", &port.to_string()]` as `Vec<String>`; env: `[("PYTHONPATH", repo_root.to_string_lossy().into_owned()), ("MODEL_DIR", "api/src/models".into()), ("VOICES_DIR", "api/src/voices/v1_0".into())]`; cwd `Some(repo_root.as_path())`. On spawn error: remove `backend_name` from `self.models` and from `self.inference_stats` (same cleanup as the existing timeout path), then return the existing context error `format!("Failed to spawn Kokoro-FastAPI process: {}", python_bin.display())`.
   - Delete the PID re-extraction (use `spawned.pid`), `configure_process_group`, and the reaper task.
   - Health loop: `if health_checker.check_health(&health_url, Some(5)).await { ... health_ok = true; break; }`; timeout arm uses `spawner.kill_process_group` / `spawner.force_kill_process_group`; replace the `is_process_group_alive(pid)` call with `spawner.is_process_group_alive(pid)`... — STOP: `ProcessSpawner` has no `is_process_group_alive`. Use `crate::proxy::process::is_process_group_alive(pid)` for the liveness probe (keep that import); only the kill calls route through `spawner`. Rationale: liveness checking is `ProcessChecker`'s job and threading a fourth trait through is out of scope; the kill behavior is what the tests assert.
   - `unload_tts_backend` and `get_tts_server` stay exactly as they are.

4. **`server/mod.rs` — stop shelling out to `kill`.** In `cleanup_stale_processes` (the `else` arm after the unhealthy reconnect attempt, currently around line 161), replace:
   ```rust
   let _ = tokio::process::Command::new("kill")
       .arg(pid.to_string())
       .status()
       .await;
   ```
   with `let _ = super::process::kill_process(pid).await;`. `kill_process` already exists in `crate::proxy::process` (imported by `lifecycle/tts.rs` today).

5. **Callers.** `crates/tama-core/src/proxy/handlers/compaction.rs:106` → `state.load_compaction_backend(&(), &(), &()).await?;`. `crates/tama/src/api/backends/compaction.rs:56` → `state.load_compaction_backend(&(), &(), &()).await`. `crates/tama-core/src/proxy/handlers/tts.rs:77` → `state.load_tts_backend(backend_name, &(), &(), &()).await?;`. No other callers exist (verified by `rg`).

6. **Tests** in `crates/tama-core/src/proxy/lifecycle/tests.rs` (append; the file already imports `MockHealthChecker`, add `MockPortAllocator, MockProcessSpawner` to the existing `use crate::proxy::lifecycle::traits::{...}` at line 747):
   - `test_load_compaction_health_timeout_marks_failed`: `Config::default()` with `config.compaction.enabled = true`, `config.proxy.startup_timeout_secs = 1`; `ProxyState::new(config, Some(tempdir.path().to_path_buf()))`; `MockHealthChecker` left at `false`; `MockPortAllocator` set to `18962`; `MockProcessSpawner::new()`. Call `state.load_compaction_backend(&hc, &sp, &pa).await` → assert `is_err()`; assert `spawn_count == 1`; assert `models` map contains `"compaction"` in `BackendState::Failed` with error containing `"Startup timeout"`. (Compaction's embedded server files extract into the tempdir via `compaction_server::get_server_dir` — no network, no real spawn.)
   - `test_load_compaction_spawn_failure_cleans_up`: same setup but `sp.set_fail_spawn(true)` → `is_err()`; assert `!state.models.read().await.contains_key("compaction")` (FAILS before the stuck-Starting fix — expected TDD failure).
   - `test_load_tts_health_timeout_cleans_up`: tempdir; `BackendManager::open(tempdir)` + `add_installation(&BackendInfo { name: "tts_kokoro".into(), backend_type: crate::backends::BackendType::TtsKokoro, version: "1.0.0".into(), path: tempdir.path().join("tts_kokoro"), installed_at: 0, gpu_variant: "cpu".into(), source: None })`; `startup_timeout_secs = 1`; health mock `false`, port mock `18963` → `state.load_tts_backend("tts_kokoro", &hc, &sp, &pa).await` → `is_err()`; assert `models` map does not contain `"tts_kokoro"` and `inference_stats.borrow()` has no `"tts_kokoro"` key.
   - `test_load_tts_spawn_failure_cleans_up`: same + `set_fail_spawn(true)` → `is_err()`; assert `models` map empty of the key (FAILS before the fix).
   - `test_load_tts_success_marks_ready`: same seed, health mock `true` → `Ok(_)`; assert the `models` entry is `BackendState::Ready` with `backend_url == "http://127.0.0.1:18963"` and `backend_pid == 12345` (MockProcessSpawner default `return_pid` — call `sp.set_return_pid(12345)` explicitly to be safe).

**Steps:**
- [ ] Add the 5 failing tests to `crates/tama-core/src/proxy/lifecycle/tests.rs` (they fail to compile until the signatures change — that is the expected first failure)
- [ ] Run `cargo nextest run --package tama-core -- proxy::lifecycle` — verify compile failure / red tests
- [ ] Implement the `MockProcessSpawner.set_fail_spawn` extension in `crates/tama-core/src/proxy/lifecycle/traits.rs`
- [ ] Implement the `compaction.rs`, `tts.rs`, `server/mod.rs` changes and update the 3 call sites
- [ ] Run `cargo nextest run --package tama-core -- proxy::lifecycle` — all pass, including the two cleanup tests
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo nextest run --package tama` — `api::backends::compaction` callers still compile/pass
- [ ] Run `rg "Command::new" crates/tama-core/src/proxy/lifecycle/compaction.rs crates/tama-core/src/proxy/lifecycle/tts.rs crates/tama-core/src/proxy/server/mod.rs` — zero hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: route compaction/TTS spawn paths through lifecycle traits"

**Acceptance criteria:**
- [ ] `rg "tokio::process::Command" crates/tama-core/src/proxy/lifecycle/ crates/tama-core/src/proxy/server/mod.rs` — zero hits outside `lifecycle/mod.rs`
- [ ] Spawn failure in compaction/TTS leaves no `Starting` entry in the models map — proven by the two new cleanup tests
- [ ] `load_compaction_backend`/`load_tts_backend` are generic over `HealthChecker`/`ProcessSpawner`/`PortAllocator`; production callers pass `&(), &(), &()`
- [ ] `cargo nextest run --package tama-core` passes; clippy clean

---

### Task 2: Test update-check orchestration `run_check`/`check_model`/`check_backend` (F23)

**Context:**
`UpdateChecker::run_check` (`crates/tama-core/src/updates/checker/mod.rs:101`), `check_model` (`checker/model.rs`), and `check_backend` (`checker/backend.rs`) — the fetch→compare→persist pipeline — have no test; only the pure decision table is covered (~32 tests in `checker/tests.rs`). Seams: `hf-hub` 0.5's `ApiBuilder::new()` reads `HF_ENDPOINT` at build time (verified in the vendored source, `api/tokio.rs:248`), and `fetch_blob_metadata` builds its URL from `HF_ENDPOINT` too — so wiremock covers both network calls in `check_model`. `hf_api()` is a per-process static (`OnceCell`), and nextest gives every test its own process, so `std::env::set_var("HF_ENDPOINT", ...)` at test start is race-free. `check_backend` calls `check_latest_version(backend_type, None, None)` with no URL seam — decision: do NOT add one; instead register only `BackendType::TtsKokoro` backends in these tests (its arm returns `None` without network), which still exercises the full DB read→decide→persist path. `UpdateEvent` is `#[cfg(feature = "web-ui")]`, so the whole new test module is gated on that feature and the test command passes `--features web-ui`. Tests may access the private `gguf_listing_cache` field (child module of `checker`), which lets the cache-hit path run without any listing HTTP call.

**Files:**
- Create: `crates/tama-core/src/updates/checker/orchestration_tests.rs`
- Modify: `crates/tama-core/src/updates/checker/mod.rs`

**What to implement:**

1. **`checker/mod.rs`** — after the existing `#[cfg(test)] mod tests;` (line ~16) add:
   ```rust
   #[cfg(all(test, feature = "web-ui"))]
   mod orchestration_tests;
   ```

2. **`orchestration_tests.rs`** — new file. Contents:
   - Imports: `super::UpdateChecker`, `crate::backends::{BackendInfo, BackendManager, BackendType}`, `crate::db::{self, queries}`, `wiremock::{Mock, MockServer, ResponseTemplate}`, `wiremock::matchers::{method, path, query_param}`, `tempfile::tempdir`.
   - Helper `fn seed_backend(config_dir: &std::path::Path)` — `BackendManager::open(config_dir)` + `add_installation(&BackendInfo { name: "tts_kokoro".into(), backend_type: BackendType::TtsKokoro, version: "1.0.0".into(), path: config_dir.join("tts_kokoro"), installed_at: 0, gpu_variant: "cpu".into(), source: None })`.
   - Helper `fn seed_model(config_dir: &std::path::Path, repo_id: &str, commit_sha: &str, lfs_oid: &str) -> i64` — `let db::OpenResult { conn, .. } = db::open(config_dir).unwrap();` then `queries::upsert_model_config(&conn, &record)` returning the id (build the `ModelConfigRecord` by copying the field list from the existing construction in `crates/tama-core/src/db/queries/tests.rs` — read it first; set `repo_id`, everything else minimal), then `queries::upsert_model_pull(&conn, ...)` with `commit_sha` (read `upsert_model_pull`'s params struct at `db/queries/model_queries.rs:11` before writing), and `queries::upsert_model_file(&conn, ...)` with `filename: "Test-Q4_K_M.gguf"`, `quant: Some("Q4_K_M")`, `lfs_oid: Some(lfs_oid)` (params at `model_queries.rs:57`).
   - `#[tokio::test] async fn test_run_check_persists_backend_and_model_rows_and_emits_events`:
     1. `let tmp = tempdir()`; `seed_backend(tmp.path())`; `let model_id = seed_model(tmp.path(), "unsloth/Test-GGUF", "sha-old", "lfs-old");`
     2. `let server = MockServer::start().await;` mount:
        - `GET /api/models/unsloth/Test-GGUF` → 200 `{"sha": "sha-new", "siblings": [{"rfilename": "Test-Q4_K_M.gguf"}]}` (hf-hub `RepoInfo` shape — verify against `hf-hub` 0.5 `RepoInfo` before finalizing; add `"lastModified": null`-style optional fields only if deserialization demands them).
        - `GET /api/models/unsloth/Test-GGUF` with `query_param("blobs", "true")` → 200 `{"siblings": [{"rfilename": "Test-Q4_K_M.gguf", "blobId": "b1", "size": 123, "lfs": {"sha256": "lfs-new", "size": 123}}]}` (parsed by `parse_blob_siblings`, `models/pull/api.rs:292`).
        Mount the `?blobs=true` mock AFTER the plain one (wiremock matches most-recently-mounted first; the query_param mock only matches blob requests, the plain one catches the listing).
     3. `std::env::set_var("HF_ENDPOINT", server.uri());` BEFORE any `UpdateChecker` call.
     4. `let mut checker = UpdateChecker::new(); let (tx, mut rx) = tokio::sync::broadcast::channel(16); checker.set_update_events_tx(tx);`
     5. `checker.run_check(tmp.path()).await.unwrap();`
     6. DB assertions (open a fresh `db::open`): `queries::get_update_check(&conn, "backend", "tts_kokoro:cpu")` → `status == "up_to_date"`, `current_version == Some("1.0.0")`, `latest_version == None`, `update_available == false`. `queries::get_update_check(&conn, "model", &model_id.to_string())` → `status == "update_available"`, `update_available == true`, `current_version == Some("sha-old")`, `latest_version == Some("sha-new")`, `details_json` contains `"lfs-new"` and `"sha-new"`.
     7. Event assertions: drain `rx` with `while let Ok(ev) = rx.try_recv()`; assert the sequence contains a `CheckStarted { item_type: "backend", .. }`, a `CheckStarted { item_type: "model", item_id, .. }` with `item_id == format!("model-{}", model_id)`, a `CheckCompleted { item_type: "model", .. }` whose `dto["status"] == "update_available"`, and a `CheckCompleted { item_type: "backend", .. }`. No `CheckError` events.
   - `#[tokio::test] async fn test_run_check_model_up_to_date_via_cache_without_http`: same seed but `commit_sha == "sha-same"` and pre-seed the cache: `checker.gguf_listing_cache.insert("unsloth/Test-GGUF".to_string(), "sha-same".to_string(), vec![crate::models::pull::RemoteGguf { filename: "Test-Q4_K_M.gguf".into(), quant: Some("Q4_K_M".into()) }], None).await;` — do NOT set `HF_ENDPOINT` and do NOT start wiremock (a network call would fail, proving the cache short-circuit... the listing is skipped, but note the tier-1 SHA match then returns `up_to_date` WITHOUT calling `fetch_blob_metadata` — so zero HTTP happens). Assert the model row: `status == "up_to_date"`, `update_available == false`, `latest_version == Some("sha-same")`.
   - `#[tokio::test] async fn test_run_check_model_without_repo_records_unknown`: seed a model via `seed_model` but call `checker.check_model(tmp.path(), model_id, None).await` directly → row `status == "unknown"`, `error_message == Some("Model has no source repo configured")`, and a `CheckError { item_type: "model", .. }` event is emitted.
   - `#[tokio::test] async fn test_run_check_concurrent_invocation_skips`: acquire the checker lock manually (`checker.lock.try_lock().unwrap()` — field is private but visible to this child module), then `run_check` returns `Ok(())` immediately and emits `CheckSkipped { item_type: "all", reason }` with `reason` containing `"already in progress"`. Hold the guard for the duration.
   - IMPORTANT hygiene: every test that sets `HF_ENDPOINT` must be self-sufficient (nextest process isolation handles cleanup; no `remove_var` needed, but add it at test end anyway for `cargo test` compatibility).

**Steps:**
- [ ] Write `orchestration_tests.rs` with the 4 tests and wire the `#[cfg(all(test, feature = "web-ui"))] mod orchestration_tests;` declaration
- [ ] Run `cargo nextest run --package tama-core --features web-ui -- updates::checker` — verify the new tests run and any wiring mistakes fail (iterate on mock JSON shapes until `hf-hub` parses them)
- [ ] Run `cargo nextest run --package tama-core` (no features) — existing tests unaffected, new module correctly compiled out
- [ ] Run `cargo nextest run --package tama-core --features web-ui` — full crate passes with the feature on
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean (also `cargo clippy --package tama-core --features web-ui -- -D warnings`)
- [ ] Commit with message: "test: cover update-check orchestration (run_check/check_model/check_backend)"

**Acceptance criteria:**
- [ ] `run_check` persists `update_checks` rows for a seeded backend (`tts_kokoro:cpu`) and model with correct statuses — proven by tests
- [ ] `UpdateEvent::CheckStarted/CheckCompleted/CheckError/CheckSkipped` emissions asserted via broadcast receiver
- [ ] No test hits the network: all HF traffic goes through wiremock or is pre-empted by the cache seed
- [ ] `cargo nextest run --package tama-core --features web-ui` and without features both pass

---

### Task 3: Test bench execution, pull engines, and wire in `tama-mock` (F24)

**Context:**
`run_llama_bench` (`crates/tama-core/src/bench/llama_bench/mod.rs`, 284 lines), the llama-server lifecycle in `bench/llama_cli_spec/server.rs` (415 lines: `spawn_server`, `wait_ready`, `complete`, `chat_complete`), and the chunked download engines `models/pull/parallel.rs` (`pull_parallel`, 295 lines) and `models/pull/single.rs` (`pull_single`, 160 lines) are untested; `crates/tama-mock` (the purpose-built mock backend) is referenced by zero tests and has already drifted (it only serves `/health`). Decisions (already made): (a) `run_llama_bench` gets a testability extraction — its body moves to `pub(crate) async fn run_llama_bench_with_dir(config, db_dir, ...)` and the public fn only resolves `Config::config_dir()` and delegates; NO other behavior change; (b) `tama-mock` is extended to also serve `/v1/models`, `/v1/completions`, and `/v1/chat/completions` with canned JSON so it can stand in for llama-server in `spawn_server` tests and for the proxy-integration test (its `--crash-after` semantics are documented as-is: token output stops but the process and health server keep running — real crash detection is tested via SIGKILL); (c) all tests needing the `tama-mock` binary live in `crates/tama-mock/tests/` because `env!("CARGO_BIN_EXE_tama-mock")` is only defined for that package; (d) `exponential_backoff` is unit-tested in-module (it is private and duplicated in both files — the dedup is F37's job, not this plan's); (e) `LLAMA_BENCH_PATH` env + a stub shell script stand in for the real llama-bench binary (process-isolated under nextest).

**Files:**
- Modify: `crates/tama-core/src/bench/llama_bench/mod.rs`
- Modify: `crates/tama-core/src/bench/llama_cli_spec/server.rs` (tests only)
- Modify: `crates/tama-core/src/models/pull/parallel.rs` (tests only)
- Modify: `crates/tama-core/src/models/pull/single.rs` (tests only)
- Modify: `crates/tama-mock/src/main.rs`
- Modify: `crates/tama-mock/Cargo.toml`
- Create: `crates/tama-mock/tests/bench_server.rs`
- Create: `crates/tama-mock/tests/proxy_integration.rs`

**What to implement:**

1. **`bench/llama_bench/mod.rs` — extract `run_llama_bench_with_dir`.** Rename the existing body into:
   ```rust
   pub(crate) async fn run_llama_bench_with_dir(
       config: &Config,
       db_dir: &std::path::Path,
       model_id: &str,
       quant: Option<&str>,
       backend_name: Option<&str>,
       bench_config: &LlamaBenchConfig,
       progress: &dyn ProgressSink,
   ) -> Result<BenchReport>
   ```
   changing only the first two lines of the old body from `let db_dir = Config::config_dir()?; let OpenResult { conn, .. } = crate::db::open(&db_dir)?;` to `let OpenResult { conn, .. } = crate::db::open(db_dir)?;` (the parameter is now a reference — fix the two subsequent `&db_dir` uses to `db_dir`). The public `run_llama_bench` keeps its exact signature and becomes:
   ```rust
   let db_dir = Config::config_dir()?;
   run_llama_bench_with_dir(config, &db_dir, model_id, quant, backend_name, bench_config, progress).await
   ```
   Add `#[cfg(test)] mod tests` at the bottom of the file with one end-to-end test:
   - `test_run_llama_bench_with_stub_binary`: tempdir `tmp`. Write `tmp/llama-bench-stub` containing:
     ```sh
     #!/bin/sh
     echo '[{"n_prompt":128,"n_gen":0,"avg_ts":500.0,"stddev_ts":5.0},{"n_prompt":0,"n_gen":32,"avg_ts":45.0,"stddev_ts":1.0}]'
     ```
     `chmod 0o755`. `std::env::set_var("LLAMA_BENCH_PATH", stub path)`. Config: `Config::default()` with `config.general.models_dir = Some(tmp.path().join("models").to_string_lossy().into_owned())` and `config.backends.insert("llama_cpp", crate::config::BackendConfig { path: Some(tmp.path().join("llama-server")), version: None, gpu_variant: None })` (check `BackendConfig`'s exact field types at `config/types/backend.rs` — `path` is `Option<PathBuf>` or `Option<String>`; match it). Seed DB: `db::open(tmp)` + `queries::upsert_model_config` (repo_id `test/model`) + `queries::upsert_model_file` (filename `model-Q4_K_M.gguf`, quant `Some("Q4_K_M")`). Create the GGUF file `models/test/model/model-Q4_K_M.gguf` on disk (content irrelevant — `resolve_model_path` only checks existence). Progress: define `struct RecordingSink { logs: Mutex<Vec<String>>, results: Mutex<Vec<String>> }` implementing `crate::backends::ProgressSink`. Call `run_llama_bench_with_dir(&config, tmp.path(), "test--model", None, None, &LlamaBenchConfig::default()-ish, &sink)` (check whether `LlamaBenchConfig` derives `Default` — if not, construct all fields explicitly). Assert: `report.summaries` has 2 entries — `pp128` with `pp_mean == 500.0`, `tg32` with `tg_mean == 45.0`; `sink.results` has exactly 1 entry that parses as JSON containing `"gpu_type"` (field rename to `gpu_variant` is plan-173 — do not rename here); `report.model_info.backend == "llama_cpp"`.
   - `test_run_llama_bench_stub_failure_surfaces_stderr`: stub prints `boom` to stderr and `exit 3` → call returns `Err` whose message contains `"llama-bench exited with error"`.

2. **`tama-mock` — serve canned inference endpoints.** In `crates/tama-mock/src/main.rs`, extend the `response` match in the connection thread from the current `if request.contains("/health")` to:
   ```rust
   let response = if request.contains("/health") {
       "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nOK".to_string()
   } else if request.contains("/v1/chat/completions") {
       "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"timings\":{\"predicted_per_second\":42.5,\"draft_n\":10,\"draft_n_accepted\":7},\"usage\":{\"completion_tokens\":12}}".to_string()
   } else if request.contains("/v1/completions") {
       "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"timings\":{\"predicted_per_second\":42.5}}".to_string()
   } else if request.contains("/v1/models") {
       "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"object\":\"list\",\"data\":[{\"id\":\"mock-model\",\"object\":\"model\"}]}".to_string()
   } else {
       "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\r\nNot Found".to_string()
   };
   ```
   Order matters: `/v1/chat/completions` must be checked before `/v1/completions` (substring). Also add a doc comment at the top of `main.rs` stating the crash semantics: `--crash-after N` stops token output after ~N/10 seconds but the process and `/health` endpoint keep running; `--hang` suppresses all output while still serving HTTP. Do NOT change the arg parsing or the token loop.

3. **`crates/tama-mock/Cargo.toml`** — add:
   ```toml
   [dev-dependencies]
   tama-core = { path = "../tama-core" }
   axum.workspace = true
   tower = { version = "0.5", features = ["util"] }
   serde_json.workspace = true
   reqwest.workspace = true
   tempfile.workspace = true
   ```
   (Check `tower` is already a workspace dep — if yes use `tower.workspace = true` with the `util` feature enabled in the workspace table; otherwise declare it as above. Verify `axum`/`reqwest` feature needs are satisfied by the workspace definitions.)

4. **`crates/tama-mock/tests/bench_server.rs`** — tests for `tama_core::bench::llama_cli_spec::server` (module is `pub`): helper `fn mock_exe() -> &'static str { env!("CARGO_BIN_EXE_tama-mock") }` and `async fn free_port() -> u16` (bind `127.0.0.1:0`, read port, drop).
   - `test_spawn_server_wait_ready_and_complete`: `ServerArgs { binary: mock_exe().into(), model_path: "ignored.gguf".into(), port, ngl: None, flash_attn: true, spec_type: None, spec_ngram_n: None, spec_ngram_m: None, spec_ngram_min_hits: None, spec_ngram_min: None, spec_ngram_max: None, draft_max: None, draft_min: None, spec_draft_ngl: None, context_size: None }` → `spawn_server(&args, 10).await.unwrap()` → `handle.complete("hi", 8).await.unwrap() == 42.5`; `handle.chat_complete("m", &[("user", "hi")], 8).await.unwrap()` yields `predicted_per_second == 42.5`, `predicted_n == 12`, `draft_n == 10`, `draft_n_accepted == 7`; `handle.parse_acceptance_rate().await.is_none()`.
   - `test_spawn_server_ready_timeout`: `binary: "/bin/sleep".into()` (exists on Linux; the extra args are harmless to `sleep`? — NO, `sleep` errors on unknown args and exits, which is exactly what we want: nothing ever listens) → `spawn_server(&args, 1).await` → `Err` containing `"did not become ready"`.
   Note: `spawn_server` applies `crate::process::configure_backend_command(&mut child, &args.binary)` — verify it is a no-op for non-`.exe` paths on Linux before writing the test (read `crates/tama-core/src/process.rs`); if it interferes with `/bin/sleep`, use a tempdir shell script `#!/bin/sh\nexit 0` instead.

5. **`crates/tama-mock/tests/proxy_integration.rs`** — the proxy end-to-end test:
   ```rust
   use std::sync::Arc;
   use axum::{body::Body, http::Request, Router};
   use tower::ServiceExt;
   use tama_core::config::{Config, ModelConfig};
   use tama_core::proxy::{BackendState, ProxyState};
   use tama_core::proxy::handlers::forward::handle_forward_get;
   ```
   (Verify `handle_forward_get` is reachable at that path — it is `pub` in `crates/tama-core/src/proxy/handlers/forward.rs`; confirm `handlers` and `forward` modules are `pub` in `proxy/handlers/mod.rs`. If not, route through `tama_core::proxy::server::router::build_router` instead.)
   - `test_mock_backend_proxying_and_crash_detection`:
     1. Spawn `std::process::Command::new(env!("CARGO_BIN_EXE_tama-mock")).arg("--port").arg(port)` (std, not tokio — simpler `drop`/`kill`); `let pid = child.id();`
     2. Poll `reqwest::get(format!("http://127.0.0.1:{port}/health"))` until 200 (deadline 5s).
     3. `let state = Arc::new(ProxyState::new(Config::default(), None));` insert `ModelConfig { backend: "llama_cpp".into(), model: Some("test/model".into()), enabled: true, ..Default::default() }` under key `"model-a"` via `state.model_configs().write().await`; insert `BackendState::Ready { model_name: "model-a".into(), backend: "llama_cpp".into(), backend_pid: pid, backend_url: format!("http://127.0.0.1:{port}"), load_time: SystemTime::now(), last_accessed: Instant::now(), consecutive_failures: Arc::new(AtomicU32::new(0)), failure_timestamp: None, restart_count: 0 }` under `"model-a"` via `state.models().write().await` (both accessors are `pub`).
     4. `let app = Router::new().route("/*path", axum::routing::get(handle_forward_get)).with_state(state.clone());`
     5. GET `/v1/models` via `oneshot` → assert 200 and body contains `"mock-model"` — proves the request was proxied to the mock binary.
     6. `child.kill().unwrap(); child.wait().unwrap();` then GET `/v1/models` again → assert 502 and `body["error"]["type"] == "BackendCrashedError"`; assert `state.models().read().await` no longer contains `"model-a"` (dead-PID cleanup ran).
   - `test_mock_backend_health_and_hang_smoke`: spawn with `--hang --port <p>` → `/health` returns 200 and the process is still alive after 1s (documents that `--hang` suppresses output, not HTTP). Kill the child at test end.
   Both tests must kill their child in a guard/`Drop` or explicit final line so a failed assert doesn't leak a listener.

6. **`models/pull/parallel.rs` — add `#[cfg(test)] mod tests`** at the bottom:
   - `test_exponential_backoff_bounds`: for `attempt` in `[0, 1, 3, 10, 100]`: `let d = exponential_backoff(attempt);` assert `d >= Duration::from_millis(300 + (attempt as u64).pow(2))` and `d <= Duration::from_millis(10_000)`.
   - `test_pull_parallel_happy_path`: wiremock. File of 100 bytes: mount (in this order) `GET /file` with `header("Range", "bytes=50-99")` → 206 body `[b'b'; 50]`, then `GET /file` with `header("Range", "bytes=0-49")` → 206 body `[b'a'; 50]` (use `wiremock::matchers::header`). Call `pull_parallel(&client, &format!("{}/file", server.uri()), &dest, 100, 2, &ProgressBar::hidden(), None, None)` → `Ok`; assert `dest` contents are 50×`a` then 50×`b`; assert no `.file.part0`/`.file.part1` temp files remain in the tempdir.
   - `test_pull_parallel_chunk_failure_retries_then_completes`: mount the 206 mock for `bytes=0-49` FIRST, then mount a 500-response mock for the same range with `.up_to_n_times(1)` (wiremock matches most-recently-mounted first, so the 500 fires once and the 206 catches the retry); chunk 1 as before → `Ok`, correct reassembled content.
   - `test_pull_parallel_short_chunk_errors_after_retries`: serve `bytes=0-49` as 206 with only 10 bytes of body, `.up_to_n_times(3)` (MAX_RETRIES = 3), plus the always-206 mock for chunk 1 → `Err` whose chain contains `"incomplete"`; assert both `.partN` files were cleaned up (`cleanup_temp_files`).
   - `test_pull_parallel_rejects_bad_args`: `num_connections == 0` → err; `total_size < num_connections` → err.

7. **`models/pull/single.rs` — add `#[cfg(test)] mod tests`** at the bottom:
   - `test_exponential_backoff_bounds`: same table as parallel (duplicate — dedup is F37).
   - `test_pull_single_happy_path`: wiremock `GET /file` → 200 with 64-byte body → `pull_single(&client, &url, &dest, 64, &ProgressBar::hidden(), None, None)` → `Ok`; dest matches.
   - `test_pull_single_mid_stream_failure_resumes_and_completes`: hand-rolled scripted server with `tokio::net::TcpListener` on `127.0.0.1:0` that answers request #1 with `HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n` + 40 bytes then closes (reqwest surfaces a stream error), and answers request #2 (assert it carries `Range: bytes=40-`) with `206` + `Content-Range: bytes 40-99/100` + the remaining 60 bytes. Serve exactly 2 connections then exit. Assert dest == the full 100-byte payload.
   - `test_pull_single_permanent_failure_errors`: scripted server answering every request with `500` (4 connections = 1 initial + 3 retries) → `Err` containing `"status 500"`.

**Steps:**
- [ ] Write the failing `run_llama_bench_with_dir` tests in `crates/tama-core/src/bench/llama_bench/mod.rs` (compile fails until the extraction exists)
- [ ] Run `cargo nextest run --package tama-core -- bench::llama_bench` — verify red
- [ ] Implement the `run_llama_bench_with_dir` extraction
- [ ] Run `cargo nextest run --package tama-core -- bench::llama_bench` — green
- [ ] Extend `crates/tama-mock/src/main.rs` with the canned endpoints; add dev-deps to `crates/tama-mock/Cargo.toml`
- [ ] Write `crates/tama-mock/tests/bench_server.rs` and `crates/tama-mock/tests/proxy_integration.rs`
- [ ] Run `cargo nextest run --package tama-mock` — all pass
- [ ] Write the `parallel.rs` and `single.rs` test modules
- [ ] Run `cargo nextest run --package tama-core -- models::pull` — all pass
- [ ] Run `cargo nextest run --workspace` — full suite passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: cover bench execution, pull engines, and wire tama-mock into integration tests"

**Acceptance criteria:**
- [ ] `rg -l "CARGO_BIN_EXE_tama-mock" crates/tama-mock/tests/` — two files; tests spawn the real binary and assert proxying + crash detection
- [ ] `run_llama_bench`'s orchestration runs against a stub binary with no GPU/network — two tests green
- [ ] `pull_parallel` covers happy path, retry-after-500, short-chunk error, arg validation; `pull_single` covers happy path, mid-stream resume, permanent failure; `exponential_backoff` bounds pinned in both files
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 4: Fill route gaps, un-ignore backend API tests, cover tama system/model handlers (F36)

**Context:**
Seven `crates/tama/src/api` files have no route tests, all 5 tests in `crates/tama/tests/backends_api.rs` are `#[ignore = "Requires backend registry setup"]` even though `crates/tama/src/api/backends/manage/tests.rs` already demonstrates the full fixture (tempdir registry + `ProxyState::new(config, Some(tmp))` + `build_web_routes` + CSRF token pair), and tama-core's `tama_handlers/system.rs` (`handle_tama_system_health`, `handle_hf_list_quants`) plus `tama_handlers/models/handlers.rs` (list/get/load/cancel/unload) have no tests. Decisions (already made): the ignored tests are REWRITTEN (not deleted) on the established fixture — their bodies were empty TODO stubs, so "un-ignoring" means implementing them; route tests in `crates/tama/src/api/**` live in per-file `#[cfg(test)] mod tests` using a shared helper; `handle_tama_load_model` is only tested on its error paths because it hardcodes `&()` as the health checker (making it mockable is Task 1's pattern but explicitly out of scope here). All WebState construction uses the `test_web_state()` shape from `manage/tests.rs:8`; all POST/PATCH/DELETE routes go through `crate::router::build_web_routes` with the matching `tama_csrf_token` cookie + `X-CSRF-Token` header pair.

**Files:**
- Modify: `crates/tama/tests/backends_api.rs`
- Modify: `crates/tama/src/api/aliases/mod.rs`
- Modify: `crates/tama/src/api/hf.rs`
- Modify: `crates/tama/src/api/logs.rs`
- Modify: `crates/tama/src/api/backends/jobs.rs`
- Modify: `crates/tama/src/api/backends/list.rs`
- Modify: `crates/tama/src/api/models/files.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/system_tests.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/models/tests/model_handlers.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/mod.rs` (add `#[cfg(test)] mod system_tests;`)
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/mod.rs` (register the new tests file in the existing `mod tests`)
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/tests/helpers.rs`

**What to implement:**

1. **`crates/tama/tests/backends_api.rs` — rewrite the 5 ignored tests.** Delete everything except the `fixtures` module; add imports mirroring `manage/tests.rs` (`axum`, `tower::ServiceExt`, `tama_core::proxy::ProxyState`, `tama_core::config::Config`, `tama_web::web_types::{WebState, JobManager}`). Local helpers: `test_web_state()` copied verbatim from `manage/tests.rs:8-17`, and `fn build(state, web_state) -> Router` = `tama_web::router::build_web_routes(ws.clone()).with_state(state).layer(axum::extract::Extension(ws.as_ref().clone()))` (`router` is `pub` in `crates/tama/src/lib.rs:11`). Tests:
   - `test_get_backends_empty_registry_matches_snapshot`: `ProxyState::new(Config::default(), Some(tmp))` → GET `/tama/v1/backends` → 200; body has `backends == []`, `custom == []`, `available` is an array containing `"llama_cpp"`, and `compaction.enabled == false` (no `insta` — structural asserts).
   - `test_get_backends_includes_installed_entry`: seed `BackendManager::open(tmp).add_installation(BackendInfo { name: "llama_cpp", backend_type: BackendType::LlamaCpp, version: "b4800", path: tmp.path().join("llama_cpp"), installed_at: 0, gpu_variant: "cpu".into(), source: None })` → GET → `backends` contains one entry with `name == "llama_cpp"` and `installed == true` (check the exact DTO field names in `crates/tama/src/api/backends/types.rs` `BackendCardDto` before asserting).
   - `test_get_backends_custom_entry_appears_in_custom_array`: same with `BackendType::Custom` → entry lands in `custom`, not `backends`.
   - `test_get_capabilities_returns_supported_cuda_versions`: GET `/tama/v1/system/capabilities` → 200; body contains a `cuda_versions` array (read `crates/tama/src/api/backends/capabilities.rs` `system_capabilities` first and assert on the real field names).
   - `test_origin_enforcement_blocks_cross_origin_post`: POST `/tama/v1/backends/compaction` with JSON `{"enabled": false}` and NO CSRF pair → 403; repeat WITH matching pair → not 403 (any other status acceptable — the assertion is CSRF enforcement, documented in the test name/comment). Remove all `#[ignore]` attributes.
   Note the file is an integration test against the `tama_web` lib — everything used must be `pub`. If `build_web_routes` or `web_types` are not reachable, add the missing `pub` in `crates/tama/src/lib.rs` (they are: verify `pub mod web_types` and `pub mod router` first).

2. **`api/aliases/mod.rs` — CRUD round-trip test module.** Add `#[cfg(test)] mod tests`. The handlers take `State<Arc<ProxyState>>` and resolve the config dir from `state.db_dir()` (verify at `api/aliases/mod.rs:37-60`; if they use `Config::base_dir()` instead, STOP and route them through `state.db_dir()` first — that mismatch is F15, do not fix it here; in that case only write the tests that don't touch the DB). Use the fixture + `build_web_routes` + CSRF pair:
   - `test_alias_crud_round_trip`: POST `/tama/v1/aliases` `{"name": "fast", "model_id": 1}` → 200/201; GET `/tama/v1/aliases` → contains `fast`; GET `/tama/v1/aliases/fast` → 200 with `model_id == 1`; PATCH `/tama/v1/aliases/fast` `{"enabled": false}` → 200; DELETE `/tama/v1/aliases/fast` → 200; final GET list → empty. (Read the exact request/response DTOs in the file first — adjust field names; `UpdateAliasRequest` at line ~271 has `name/model_id/description/enabled`.)
   - `test_create_alias_rejects_invalid_name`: POST with `{"name": "bad name!", "model_id": 1}` → 400 (validation lives in the handler — verify and assert its status code).

3. **`api/hf.rs` — wiremock test.** `hf_metadata` (`api/hf.rs:15`) calls `tama_core::models::pull::fetch_hf_metadata`, which honors `HF_ENDPOINT`. Add `#[cfg(test)] mod tests`:
   - `test_hf_metadata_happy_path`: `MockServer`; mount `GET /api/models/unsloth/Foo-GGUF` → 200 `{"id": "unsloth/Foo-GGUF", "sha": "abc", "lastModified": "2026-01-01T00:00:00.000Z", "tags": ["gguf"], "siblings": []}` (read `fetch_hf_metadata` fully and match the fields it parses; it also fetches the README — mount `GET /unsloth/Foo-GGUF/raw/main/README.md` → 404 so the README arm degrades gracefully). `std::env::set_var("HF_ENDPOINT", server.uri())`; build the router with only this route (manual `Router::new().route("/tama/v1/hf/*repo", get(hf_metadata)).with_state(state)`) → GET `/tama/v1/hf/unsloth/Foo-GGUF` → 200, body has the parsed metadata fields.
   - `test_hf_metadata_rejects_traversal`: GET `/tama/v1/hf/..%2F..` or a raw `..` segment → 400 (read the handler's validation first and feed it exactly what it rejects).

4. **`api/logs.rs` — handler tests.** Add `#[cfg(test)] mod tests` with a manual router (`get(get_backend_logs)`):
   - `test_get_backend_logs_rejects_invalid_name`: backend `foo..bar` → 400 `ValidationError` (validation at `logs.rs:33 is_valid_backend_name` — also unit-test it directly: `"llama_cpp"` ok, `".."` / `"a/b"` / `"a\\b"` rejected).
   - `test_get_backend_logs_missing_file_404`: config with `general.logs_dir = Some(tmp)` → backend `llama_cpp` → 404 `NotFoundError`.
   - `test_get_backend_logs_returns_tail`: write `tmp/llama_cpp.log` with 5 lines → GET `/tama/v1/logs/llama_cpp?lines=3` → 200 `{"lines": [...]}` with 3 entries (uses `tama_core::logging::tail_lines`).
   Do NOT add tests for `get_all_logs` — it is unrouted dead code scheduled for deletion in plan-172.

5. **`api/backends/jobs.rs` — job snapshot tests.** Add `#[cfg(test)] mod tests` (manual router with `Extension(web_state)`):
   - `test_get_job_unknown_returns_404`: GET `/tama/v1/backends/jobs/nope` → 404.
   - `test_get_job_returns_snapshot`: submit a job via `web_state.jobs.as_ref().unwrap().submit(...)` (read `JobManager::submit`'s signature in `crates/tama/src/web_types.rs` first) → GET → 200 with matching `id`, `kind == "install"`, `status == "queued"`-ish (assert the real enum serialization).
   - `test_job_events_sse_unknown_job`: GET `/tama/v1/backends/jobs/nope/events` → assert the documented behavior of `job_events_sse` for unknown ids (read it first — 404 or immediate stream end; assert whichever it does, with a comment quoting the behavior).

6. **`api/backends/list.rs` and `api/models/files.rs` — validation-path tests.** Keep these minimal (heavy paths are job/network-bound):
   - `list.rs`: `test_list_backends_empty_registry` (same as the backends_api rewrite but as an in-crate unit test — fine to duplicate; assert 200 + empty arrays) and `test_list_backend_versions_unknown_404` (GET `/tama/v1/backends/nope/versions` → 404; read `list_backend_versions` at :542 for its real not-found behavior first).
   - `files.rs`: `test_refresh_model_metadata_unknown_model_404` and `test_verify_model_files_unknown_model_404` — both handlers start with the model-resolution chain (`Repository::open` + `resolve_model_id`); with an empty tempdir registry, any id must 404 without touching the network. Read both handlers' early returns to pin the exact status/body (`models/files.rs:40,193`).

7. **`tama_handlers/system_tests.rs`** (tama-core): register `#[cfg(test)] mod system_tests;` in `crates/tama-core/src/proxy/tama_handlers/mod.rs`. Tests (router per test, `Router::new().route(...).with_state(state)`):
   - `test_handle_tama_system_health`: `ProxyState::new(Config::default(), None)`; insert one Ready entry via `state.models().write().await` → GET `/tama/v1/system/health` (`handle_tama_system_health`) → 200; `status == "ok"`, `service == "tama"`, `models_loaded == 1`, numeric fields present.
   - `test_handle_hf_list_quants`: wiremock; `std::env::set_var("HF_ENDPOINT", server.uri())`; mount `GET /api/models/unsloth/Foo-GGUF` with `query_param("blobs", "true")` → `{"siblings": [{"rfilename": "B-Q8_0.gguf", "blobId": "b2", "size": 200, "lfs": {"sha256": "h2", "size": 200}}, {"rfilename": "A-Q4_K_M.gguf", "blobId": "b1", "size": 100, "lfs": {"sha256": "h1", "size": 100}}]}` → GET `/tama/v1/hf/unsloth/Foo-GGUF` (route `"/tama/v1/hf/*repo_id"`) → 200; array of 2 `QuantEntry` sorted by filename (`A-Q4_K_M.gguf` first); each has `quant`, `filename`, `size_bytes`, `kind` (check `QuantEntry` field names in `tama_handlers/types.rs`).
   - `test_handle_hf_list_quants_rejects_traversal`: repo_id `..` → 400 `{"error": "Invalid repo_id"}`.
   Do NOT test `handle_tama_system_restart` (calls `std::process::exit`).

8. **`tama_handlers/models/tests/model_handlers.rs`** (tama-core): register alongside the existing files in the `mod tests` block of `tama_handlers/models/mod.rs` (line ~21: add `mod model_handlers;`). Extend `tests/helpers.rs` with a generic caller:
   ```rust
   /// Helper: build a router for the given route+handler and oneshot a request.
   pub async fn call_route(
       state: Arc<ProxyState>,
       route: &str,
       method: axum::http::Method,
       handler: axum::routing::MethodRouter<Arc<ProxyState>>,
       uri: &str,
   ) -> (axum::http::StatusCode, serde_json::Value)
   ```
   Tests (state built via the existing `create_state_with_model` pattern; a Ready runtime entry inserted via `state.models().write().await`):
   - `test_handle_tama_list_models_states`: two configs; insert a `BackendState::Ready` for one → response `models` array: loaded entry has `state == "ready"`, `backend_pid == Some(pid)`; the other has `state == "idle"`, `backend_pid == null`. Note the enum serializes lowercase (`#[serde(rename_all = "lowercase")]` on `ModelState`).
   - `test_handle_tama_get_model_loaded`: Ready entry for `test-model` → GET → 200, `ready == true`, `owned_by == "llama_cpp"`, `object == "model"`.
   - `test_handle_tama_get_model_configured_not_loaded`: no runtime entry → 200 with `ready == false`.
   - `test_handle_tama_get_model_unknown_404`: empty state → 404, `error.type == "NotFoundError"`.
   - `test_handle_tama_load_model_failure_returns_500`: config with model whose backend has no binary → POST load → 500, `error.type == "LoadModelError"` (load path fails fast in `resolve_backend_path` — no process is spawned; document this in the test comment).
   - `test_handle_tama_cancel_load_starting`: insert `BackendState::Starting` with `backend_pid: 0` (pid 0 skips the kill path) → POST cancel → 200, `loaded == false`, entry removed from `models`.
   - `test_handle_tama_cancel_load_ready_conflict`: Ready entry → 409 `ModelAlreadyLoadedError`.
   - `test_handle_tama_cancel_load_unknown_404`: → 404 `ModelNotLoadingError`.
   - `test_handle_tama_unload_model_ready`: Ready entry with a bogus high PID (`backend_pid: 4_000_000` — kill failures are swallowed by `unload_model`'s `let _ =`) → POST unload → 200, `loaded == false`, entry gone.
   - `test_handle_tama_unload_model_unknown_404`: → 404 `NotFoundError`.

**Steps:**
- [ ] Rewrite `crates/tama/tests/backends_api.rs` (5 tests, no `#[ignore]`)
- [ ] Run `cargo nextest run --package tama --test backends_api` — iterate until green
- [ ] Add the test modules to `api/aliases/mod.rs`, `api/hf.rs`, `api/logs.rs`, `api/backends/jobs.rs`, `api/backends/list.rs`, `api/models/files.rs`
- [ ] Run `cargo nextest run --package tama -- api::` — all pass
- [ ] Add `system_tests.rs` and `models/tests/model_handlers.rs` (+ helpers extension) in tama-core
- [ ] Run `cargo nextest run --package tama-core -- proxy::tama_handlers` — all pass
- [ ] Run `cargo nextest run --workspace` — full suite passes
- [ ] Run `rg '#\[ignore' crates/tama/tests/` — zero hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: fill route gaps and un-ignore backend API tests"

**Acceptance criteria:**
- [ ] `crates/tama/tests/backends_api.rs` has 5 passing tests with no `#[ignore]` attribute
- [ ] Every file listed under Files has at least the specified tests; `rg -c "#\[tokio::test\]|#\[test\]"` on each is > 0
- [ ] No test performs network I/O outside wiremock/tempdir/loopback
- [ ] `cargo nextest run --workspace` passes; clippy clean
