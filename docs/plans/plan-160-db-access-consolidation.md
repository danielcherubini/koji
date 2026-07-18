# DB Access Consolidation Plan

**Goal:** Make `Repository` the single data-access entry point for the `tama` API layer (reads AND writes), demote `ModelManager`/`BackendManager` to tama-core-internal proxy lifecycle use, collapse the DTO/Record duplication to one struct per table, seal the raw-`rusqlite` funnel leaks, and construct the `Repository` once at startup instead of 28 times per request.

**Architecture:** Audit findings F1+F2. ADR-0017 chose centralized managers over a repository, but plan-146 later added `crates/tama-core/src/db/repository.rs` (908 lines), so two competing layers now coexist with method-level overlap (`Repository::get_model_config` ↔ `ModelManager::get_config`, etc.) and handlers open BOTH in one request (two SQLite connections). Decision (already made — Task 6 records it in ADR-0017): the `Repository` wins for the `tama` API layer; it absorbs the handful of write methods the API handlers need from `ModelManager`; the managers stay for tama-core-internal proxy lifecycle (`PullQueueService`, `ProxyState`, lifecycle code). The Record types win over the DTOs (they are `ModelManager`'s whole public API and are field-for-field identical to the DTOs). One shared `Repository` lives in `WebState` behind `Arc<std::sync::Mutex<…>>`, so migrations run once at startup instead of inside every one of the 28 per-request `Repository::open` calls.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio

---

### Task 1: Absorb model write methods into `Repository` and migrate model handlers off `ModelManager`

**Context:**
`Repository` (reads) and `ModelManager` (writes) are both opened by the same handlers today: `crates/tama/src/api/models/crud/update.rs:40,78` and `:123,161`, `crud/delete.rs:31,39`, `crud/rename.rs:61`, `crud/create.rs:61`, `api/models/files.rs:130,257`. The write surface the `tama` crate actually uses from `ModelManager` is small (verified by rg): `save_model_config` (create.rs:122, update.rs ×2, rename.rs:108, delete.rs:90), `get_config_by_repo_id` (create.rs:68, rename.rs:87), `get_files` (files.rs:134,154,260), `upsert_file` (files.rs:145), `get_pull` (files.rs:155), `upsert_pull` (files.rs:131), `delete_file` (delete.rs:116). Everything else `ModelManager` exposes (`queue_*`, `insert_active`, `enable_model`, …) is tama-core-internal and stays. Decision: `Repository` gains thin delegating methods with the SAME names the handlers already call on `ModelManager` where possible, so handler diffs are mechanical. `Repository` must NOT gain queue/active-model/lifecycle methods — those stay `ModelManager`-only. The `delete_model` handler's raw-SQL transaction (`crud/delete.rs:194-200`) is Task 3, not this task — this task only migrates `delete_quant` in delete.rs.

**Files:**
- Modify: `crates/tama-core/src/db/repository.rs`
- Modify: `crates/tama/src/api/models/crud/create.rs`
- Modify: `crates/tama/src/api/models/crud/update.rs`
- Modify: `crates/tama/src/api/models/crud/rename.rs`
- Modify: `crates/tama/src/api/models/crud/delete.rs`
- Modify: `crates/tama/src/api/models/files.rs`

**What to implement:**

1. **New `Repository` methods** in `crates/tama-core/src/db/repository.rs` (append to the `impl Repository` block, in a `// ── Model writes ──` section). Each body is the SAME delegation the corresponding `ModelManager` method performs in `crates/tama-core/src/models/manager.rs` — copy the delegation, not the semantics:
   ```rust
   /// Convenience method to save a ModelConfig as a DB record.
   ///
   /// Converts config_key to repo_id, converts ModelConfig → ModelConfigRecord,
   /// sets api_name default, and upserts. Returns the model id.
   pub fn save_model_config(
       &self,
       config_key: &str,
       mc: &crate::config::ModelConfig,
   ) -> anyhow::Result<i64> {
       let repo_id = crate::models::config_key_to_repo_id(config_key);
       let mut record = mc.to_db_record(&repo_id);
       if record.api_name.as_deref().is_none_or(str::is_empty) {
           record.api_name = Some(repo_id.clone());
       }
       queries::upsert_model_config(&self.conn, &record)
   }

   /// Get all stored file records for a model.
   pub fn get_files(&self, model_id: i64) -> anyhow::Result<Vec<queries::ModelFileRecord>> {
       queries::get_model_files(&self.conn, model_id)
   }

   /// Insert or update a model file record.
   pub fn upsert_file(
       &self,
       model_id: i64,
       repo_id: &str,
       filename: &str,
       quant: Option<&str>,
       lfs_oid: Option<&str>,
       size_bytes: Option<i64>,
   ) -> anyhow::Result<()> {
       queries::upsert_model_file(&self.conn, model_id, repo_id, filename, quant, lfs_oid, size_bytes)
   }

   /// Delete a single model file record by (model_id, filename).
   pub fn delete_file(&self, model_id: i64, filename: &str) -> anyhow::Result<()> {
       queries::delete_model_file(&self.conn, model_id, filename)
   }

   /// Insert or update the pull record for a model.
   pub fn upsert_pull(&self, model_id: i64, repo_id: &str, commit_sha: &str) -> anyhow::Result<()> {
       queries::upsert_model_pull(&self.conn, model_id, repo_id, commit_sha)
   }

   /// Get the stored pull record for a model. Returns None if never pulled.
   pub fn get_pull(&self, model_id: i64) -> anyhow::Result<Option<queries::ModelPullRecord>> {
       queries::get_model_pull(&self.conn, model_id)
   }

   /// Delete the model configuration by id. CASCADE deletes model_pulls and model_files.
   pub fn delete_config(&self, id: i64) -> anyhow::Result<()> {
       queries::delete_model_config(&self.conn, id)
   }
   ```
   (Verify each `queries::*` function name against `crates/tama-core/src/models/manager.rs` — it already calls exactly these.) Note: `repository.rs` importing `crate::models::config_key_to_repo_id` is acceptable — intra-crate module edges compile fine; `crate::models` must NOT import `crate::db::repository` (check with rg after the edit).
   Also: `Repository` struct field `conn` is currently private — the existing test helper `test_repo()` (repository.rs:570-573) constructs `Repository { conn }` from inside the module, which keeps working.

2. **Migrate the handlers** — every `tama_core::models::ModelManager::open(&config_dir)` in the six files above becomes usage of the already-open `Repository` (or opens a `Repository` where only a manager was opened):
   - `crud/create.rs:61` — replace `let mgr = ModelManager::open(...)` with `let repo = Repository::open(...)`; `mgr.get_config_by_repo_id(&repo_id)` (line 68) → `repo.get_model_config_by_repo_id(&repo_id)`; `mgr.save_model_config(&repo_id, &model_config)` (line 122) → `repo.save_model_config(&repo_id, &model_config)`. NOTE: `save_model_config` is documented as taking a config_key, but create.rs passes `repo_id` — this works today only because the input is already lowercase-with-`--` or because `config_key_to_repo_id` is its own inverse for single-segment ids; preserve the exact argument, do NOT "fix" it here (plan-162 owns the config_key semantics).
   - `crud/update.rs:78` and `:161` — delete the `let mgr = ModelManager::open(...)` blocks entirely; the two `mgr.save_model_config(&config_key, &updated_config)` calls become `repo.save_model_config(...)`. The `repo` from lines 40/123 stays.
   - `crud/rename.rs:61` — replace with the `Repository` opened for the read path; `mgr.get_config_by_repo_id` → `repo.get_model_config_by_repo_id` (line 87), `mgr.save_model_config` → `repo.save_model_config` (line 108), `mgr.delete_update_check` (line 114) → `repo.delete_update_check` (already exists on `Repository`).
   - `crud/delete.rs` — ONLY the `delete_quant` handler (the `let mgr` at line 39 and its uses at lines 90 `save_model_config` and 116 `delete_file`): migrate to `repo`. Do NOT touch the `delete_model` handler's `let mut mgr` at line 152 / `mgr.transaction` at line 194 — that is Task 3.
   - `api/models/files.rs:130` and `:257` — replace `let mgr = ModelManager::open(...)` with the `Repository` already opened in the same handler (line 52) or a fresh `Repository::open`; `mgr.upsert_pull` → `repo.upsert_pull` (:131), `mgr.get_files` → `repo.get_files` (:134, :154, :260), `mgr.upsert_file` → `repo.upsert_file` (:145), `mgr.get_pull` → `repo.get_pull` (:155). NOTE the return-type mismatch: `mgr.get_files` returned `Vec<ModelFileRecord>` and `mgr.get_pull` returned `Option<ModelPullRecord>`; the repo methods above return the same record types, so the only adjustments are receiver names — until Task 2 lands, `Repository::get_model_files` (DTO) still exists alongside; use the NEW record-returning `get_files` in files.rs.
   - After the edits: `rg "ModelManager" crates/tama/src/api/models/` must return zero hits.

3. **Tests** in the existing `#[cfg(test)] mod tests` in `crates/tama-core/src/db/repository.rs` (uses `test_repo()` + `insert_model_config` helpers, repository.rs:566-620): add
   - `test_save_model_config_round_trip` — build a `crate::config::ModelConfig::default()` (check fields), call `repo.save_model_config("owner--repo", &mc)`, assert the returned id > 0 and `repo.get_model_config(id)?.unwrap().repo_id == "owner/repo"` and `api_name == Some("owner/repo")` (the default-fill branch).
   - `test_upsert_and_get_pull` — insert a model config (helper), `repo.upsert_pull(id, "owner/repo", "abc123")`, assert `repo.get_pull(id)?.unwrap().commit_sha == "abc123"`.
   - `test_upsert_file_and_delete_file` — insert config, `repo.upsert_file(id, "owner/repo", "m-q4.gguf", Some("Q4_K_M"), None, Some(123))`, assert `repo.get_files(id)?` has 1 row, then `repo.delete_file(id, "m-q4.gguf")` and assert empty.
   - `test_delete_config_cascades` — insert config + one file, `repo.delete_config(id)`, assert `get_model_config(id)?` is `None` and `get_files(id)?` is empty (CASCADE requires `PRAGMA foreign_keys=ON`, which `open_in_memory` sets — verify; if the pragma is not set for in-memory DBs, set it in the test before asserting).

**Steps:**
- [ ] Write the four failing tests in `crates/tama-core/src/db/repository.rs`
- [ ] Run `cargo nextest run --package tama-core -- db::repository` — verify they fail (methods don't exist yet)
- [ ] Implement the seven `Repository` methods
- [ ] Run `cargo nextest run --package tama-core -- db::repository` — tests pass
- [ ] Migrate the six handler files per above
- [ ] Run `cargo nextest run --package tama -- api::models` — all pass
- [ ] Run `rg "ModelManager" crates/tama/src/api/models/` — zero hits; `rg "repository::Repository" crates/tama-core/src/models/` — zero hits (no back-edge)
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: absorb model write methods into Repository, migrate model API handlers off ModelManager"

**Acceptance criteria:**
- [ ] No file under `crates/tama/src/api/models/` references `ModelManager`
- [ ] `Repository` exposes `save_model_config`, `get_files`, `upsert_file`, `delete_file`, `upsert_pull`, `get_pull`, `delete_config` — all thin delegations to `crate::db::queries`
- [ ] Four new repository tests pass; whole-workspace `cargo nextest run --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 2: Collapse DTO/Record duplication — records win, DTOs deleted

**Context:**
`ModelConfigDto` (repository.rs:20) duplicates `ModelConfigRecord` (`db/queries/types.rs:17`) field-for-field (43 fields); the same holds for `ModelFileDto`/`ModelFileRecord`, `AliasDto`/`AliasResponse` (`db/queries/alias_queries.rs:9`), `BenchmarkDto`/`BenchmarkRow` (`db/queries/benchmark_queries.rs:8`, including `benchmark_type`), `PullQueueDto`/`PullQueueItem` (`db/queries/pull_queue_queries.rs:11`, including `kind`/`quant`/`context_length`), and `UpdateCheckDto`/`UpdateCheckRecord` (`db/queries/types.rs:129`). `ModelPullDto` is a strict 2-field subset of `ModelPullRecord`. `ModelConfigRecord` is `#[deprecated]` yet is `ModelManager`'s entire public API return type, and repository.rs's module doc (lines 7-9) falsely claims record types are `pub(crate)` — they are all `pub`. Decision: keep the record types (they are the DB layer's honest shape), delete all seven DTOs and the six converter functions, make `Repository` return records, un-deprecate the two deprecated records, and delete `ModelConfig::from_db_record_for_repo` (`config/types/model.rs:306`) — `from_db_record` (model.rs:241) becomes the single constructor. `BenchmarkParams` (repository.rs:92) is NOT a record duplicate — it is an owned insert-params struct used by handlers; keep it.

**Files:**
- Modify: `crates/tama-core/src/db/repository.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/config/types/model.rs`
- Modify: `crates/tama/src/api/models/info.rs`
- Modify: `crates/tama/src/api/models/files.rs`
- Modify: `crates/tama/src/api/models/crud/update.rs`
- Modify: `crates/tama/src/api/models/crud/rename.rs`
- Modify: `crates/tama/src/api/models/crud/delete.rs`
- Modify: `crates/tama/src/api/aliases/mod.rs`
- Modify: `crates/tama/src/api/benchmarks/run.rs`, `crates/tama/src/api/benchmarks/spec.rs`, `crates/tama/src/api/benchmarks/mtp.rs`, `crates/tama/src/api/benchmarks/history.rs`
- Modify: `crates/tama/src/api/updates.rs`
- Modify: `crates/tama/src/api/backends/install.rs`, `crates/tama/src/api/backends/list.rs`, `crates/tama/src/api/backends/manage/remove.rs`
- Modify: `crates/tama/src/api/downloads.rs`
- Modify: `crates/tama-core/src/proxy/pull_queue.rs`

**What to implement:**

1. **Un-deprecate** in `crates/tama-core/src/db/queries/types.rs`: delete the `#[deprecated(...)]` attributes at lines 13–16 (`ModelConfigRecord`) and 69–72 (`ModelFileRecord`). Rewrite the module doc (lines 1–5) to: `//! Record types for database query results. These types are the canonical row representations; the API layer uses them directly via db::repository::Repository.` Update the two structs' doc comments (currently "API handlers should prefer Repository methods that return DTOs instead") to state they are the canonical row types returned by `ModelManager` and `Repository`.

2. **Delete from `crates/tama-core/src/db/repository.rs`:** the struct definitions `ModelConfigDto` (:20), `ModelFileDto` (:61), `AliasDto` (:78), `BenchmarkDto` (:116), `PullQueueDto` (:142), `UpdateCheckDto` (:163), `ModelPullDto` (:178); the converter functions `record_to_dto` (:432), `file_record_to_dto` (:472, note it is `pub` — check for external callers first with `rg file_record_to_dto crates/`), `alias_response_to_dto` (:488), `benchmark_row_to_dto` (:501), `queue_item_to_dto` (:526), `update_check_record_to_dto` (:546); the dead methods `Repository::conn` (:206, has `#[allow(dead_code)]`) and `Repository::get_pull_queue_item` (:397, zero callers — verified with rg). Fix the module doc (lines 1–9): replace the "DTO types instead of DB record types" and false `pub(crate)` claims with a truthful description ("exposes typed record types from `db::queries`").

3. **Change `Repository` return types** (mechanical — delete the `.map(record_to_dto)` / `.map(queue_item_to_dto)` / `.into_iter().map(...).collect()` conversions and return the query results directly):
   - `get_model_config` → `anyhow::Result<Option<queries::ModelConfigRecord>>`
   - `get_model_config_by_repo_id` → `anyhow::Result<Option<queries::ModelConfigRecord>>`
   - `get_model_files` → `anyhow::Result<Vec<queries::ModelFileRecord>>`
   - `load_model_configs` → `anyhow::Result<std::collections::HashMap<String, queries::ModelConfigRecord>>`
   - `get_model_pull` → reimplement as `queries::get_model_pull(&self.conn, model_id)` returning `anyhow::Result<Option<queries::ModelPullRecord>>` (delete the hand-rolled SQL at :294-311 — it duplicates the query fn; Task 1's `get_pull` and this method are then duplicates — keep ONLY `get_pull` and rename callers of `get_model_pull` to `get_pull`)
   - `get_all_aliases` → `anyhow::Result<Vec<queries::AliasResponse>>`; `get_alias_by_id` → `anyhow::Result<Option<queries::AliasResponse>>`
   - `list_benchmarks` → `anyhow::Result<Vec<queries::BenchmarkRow>>`
   - `get_active_pull_by_filename` → `anyhow::Result<Option<queries::PullQueueItem>>`
   - `get_all_update_checks` → `anyhow::Result<Vec<queries::UpdateCheckRecord>>`
   Also update the doc comments on each ("Returns a ModelConfigRecord", etc.) and the `serde::{Deserialize, Serialize}` import — after the DTO deletions, check whether repository.rs still derives serde on anything (`BenchmarkParams` derives only Debug+Clone — it does not need serde; remove the serde import if unused, clippy will tell you).

4. **Delete `ModelConfig::from_db_record_for_repo`** in `crates/tama-core/src/config/types/model.rs` (line 306) and switch its seven call sites to `from_db_record`: `crates/tama/src/api/models/info.rs:155,246`, `crud/update.rs:75,158`, `crud/rename.rs:58`, `crud/delete.rs:63,187`. The records now returned by `Repository` are exactly what `from_db_record` takes.

5. **Fix `crates/tama` imports/field accesses.** Because every DTO is field-for-field identical to its record, almost all changes are import swaps:
   - `info.rs:13` — `use tama_core::db::repository::{ModelConfigDto, ModelFileDto, Repository};` → `use tama_core::db::repository::Repository;` plus `use tama_core::db::queries::{ModelConfigRecord, ModelFileRecord};` where the types are named (also the test import at :282).
   - `files.rs:13` — `use tama_core::db::repository::ModelFileDto;` → `use tama_core::db::queries::ModelFileRecord;`
   - `aliases/mod.rs`, `benchmarks/*.rs`, `updates.rs`, `backends/install.rs`, `backends/list.rs`, `backends/manage/remove.rs` — swap `repository::{AliasDto, BenchmarkDto, PullQueueDto, UpdateCheckDto, …}` imports for the corresponding `tama_core::db::queries::*` record types; rename local variables only if clippy/readability demands it (do NOT mass-rename — fields are identical).
   - `crates/tama-core/src/proxy/pull_queue.rs` and `crates/tama/src/api/downloads.rs` — BOTH consume `PullQueueDto` (verified: pull_queue.rs imports it at :11, `get_active_items_dto` :250 / `get_history_items_dto` :267 return `Vec<PullQueueDto>` via the private `item_to_dto` converter at :326; downloads.rs:58 has its own `item_to_dto(&PullQueueDto) -> PullQueueItemDto`). Deleting `PullQueueDto` breaks both. Decision: `pull_queue.rs` switches its two DTO methods and its `item_to_dto` to `queries::PullQueueItem` (KEEP the method names `get_active_items_dto`/`get_history_items_dto` — renaming is out of scope); `downloads.rs:58` changes its local `item_to_dto` parameter to `&tama_core::db::queries::PullQueueItem` — fields are identical, so the body is unchanged. Add `Serialize, Deserialize` derives to `PullQueueItem`, `BenchmarkRow`, `ModelConfigRecord`, `ModelFileRecord`, `ModelPullRecord`, `UpdateCheckRecord` if missing, since these types now cross the wire (`AliasResponse` already derives both; `PullQueueItem` — check `db/queries/pull_queue_queries.rs:10-11` for its current derive list).
   - Verify the wire format does NOT change: field names are identical, so serialized JSON is unchanged. The plan-161 error tests and any serialization tests must still pass.

6. Update `crates/tama-core/src/db/repository.rs` tests: `insert_model_config` helper constructs `queries::ModelConfigRecord` — unchanged; any test asserting on DTO types now asserts on records (same fields).

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- db::repository` — green baseline
- [ ] Make the edits in order: types.rs (un-deprecate) → repository.rs (delete DTOs/converters/dead methods, change return types, fix doc) → model.rs (delete `from_db_record_for_repo`) → tama-core callers (pull_queue.rs) → crates/tama callers
- [ ] Run `cargo check --workspace` — fix only type/import fallout (no logic changes)
- [ ] Run `rg "Dto" crates/tama-core/src/db/ crates/tama/src/api/` — no hits for the seven deleted DTOs (other DTOs like `BenchmarkDto`-unrelated types may exist — check by name)
- [ ] Run `rg "deprecated" crates/tama-core/src/db/queries/types.rs` — zero hits
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean (remove the `#![allow(deprecated)]`-dependent workarounds only if they become unused — do NOT touch `crates/tama/src/lib.rs:1-2` blanket allows, that is a different plan's job)
- [ ] Commit with message: "refactor: collapse DTO/Record duplication — Repository returns record types"

**Acceptance criteria:**
- [ ] `ModelConfigDto`, `ModelFileDto`, `AliasDto`, `BenchmarkDto`, `PullQueueDto`, `UpdateCheckDto`, `ModelPullDto` and the six `*_to_dto` converters no longer exist
- [ ] `ModelConfigRecord`/`ModelFileRecord` carry no `#[deprecated]`; `from_db_record` is the only record→config constructor
- [ ] `rg "Repository::get_pull_queue_item|\.conn\(\)" crates/tama-core/src/db/repository.rs` — zero hits
- [ ] Serialized API responses are field-identical to before (spot-check `GET /tama/v1/aliases` and `/tama/v1/benchmarks/history` tests still pass unchanged)
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 3: Replace `delete.rs` raw SQL with `Repository::delete_config` and drop `rusqlite` from `crates/tama`

**Context:**
The `delete_model` handler in `crates/tama/src/api/models/crud/delete.rs` opens a `ModelManager` (`let mut mgr`, line 152) purely to run raw SQL through its escape hatch: `mgr.transaction(|tx| tx.execute("DELETE FROM model_configs WHERE id = ?1", rusqlite::params![model_id]))` (lines 194–200) — while `ModelManager::delete_config(id)` (and after Task 1, `Repository::delete_config(id)`) exists and does the same thing (CASCADE handles `model_files`/`model_pulls`, exactly as the existing comment at lines 189–192 says). `rusqlite` is a dependency of the `tama` crate (`crates/tama/Cargo.toml:51`, optional, in the `ssr` feature list at :63) SOLELY for this `rusqlite::params!` call — the only other `rusqlite` hit in `crates/tama/src` is a comment in `api/models/files.rs:35`. Decision: use the repository method, delete the manager open, drop the dependency. Keep the surrounding two-phase structure (DB delete first, best-effort file cleanup after) and the existing `repo.delete_update_check` best-effort step — only the SQL block changes.

**Files:**
- Modify: `crates/tama/src/api/models/crud/delete.rs`
- Modify: `crates/tama/Cargo.toml`

**What to implement:**

1. In `crates/tama/src/api/models/crud/delete.rs`, `delete_model` handler: replace the block from `let mut mgr = tama_core::models::ModelManager::open(&config_dir)...` (line 152) through the closing of the `mgr.transaction(...)` `if let Err(e) = result { ... }` error mapping (lines ~189–210) with:
   ```rust
   // Step 1: Delete model config — all-or-nothing. CASCADE handles
   // model_files and model_pulls. If this fails, no files are touched yet
   // and the DB remains consistent.
   {
       tracing::debug!("Deleting model config for id={}", model_id);
       if let Err(e) = repo.delete_config(model_id) {
           tracing::error!("Failed to delete model records from database: {e}");
           return Err((
               StatusCode::INTERNAL_SERVER_ERROR,
               error_body("Failed to delete model records from database", None),
           ));
       }
   }
   ```
   Keep everything else: the `repo.delete_update_check("model", &model_id.to_string())` best-effort step, the file/dir cleanup, and the model-card deletion. After the edit, verify `delete.rs` has no `ModelManager` and no `rusqlite` references. The handler keeps opening ONE `Repository` (line 31) — note `repo` must be in scope where the deleted `mgr` block was; both opens took `&config_dir`, so reuse the existing `repo` binding (move it earlier in the closure if scoping requires).

2. In `crates/tama/Cargo.toml`: delete line 51 (`rusqlite = { workspace = true, optional = true }`) and remove `"dep:rusqlite", ` from the `ssr` feature list (line 63).

3. Update the stale comment in `crates/tama/src/api/models/files.rs:35` only if it still mentions `rusqlite::Connection` in a way that no longer applies (it reads "Structured to keep `rusqlite::Connection` off `.await` points" — still true in spirit since the Repository wraps a Connection; keep it).

4. Test: `crates/tama/src/api/models/crud/` has a `tests.rs` (or inline tests — check `crates/tama/src/api/models/crud/mod.rs`); add or extend a route-level test `test_delete_model_removes_db_row`: seed a tempdir DB (`tempfile::tempdir()`, `tama_core::db::open(dir.path())`, insert a `ModelConfigRecord` via `queries::upsert_model_config` — reuse the record literal pattern from repository.rs's `insert_model_config` test helper), build the router with `ProxyState::new(config, Some(db_dir))`, DELETE `/tama/v1/models/:id` with a valid CSRF cookie+header pair (pattern from `crates/tama/src/api/backends/manage/tests.rs`), assert 200 and that `queries::get_model_config(&conn, id)` returns `None`. If a delete route test already exists, extend it with the DB-row assertion instead of adding a new one.

**Steps:**
- [ ] Write/adjust the failing test (`test_delete_model_removes_db_row`) in `crates/tama/src/api/models/crud/`
- [ ] Run `cargo nextest run --package tama -- api::models::crud` — baseline (new test may already pass if a delete test existed — then the acceptance is the DB assertion, not redness)
- [ ] Replace the raw-SQL transaction with `repo.delete_config(model_id)`; delete the `ModelManager::open` at line 152
- [ ] Remove `rusqlite` from `crates/tama/Cargo.toml` (dependency line + ssr feature entry)
- [ ] Run `rg "rusqlite" crates/tama/src crates/tama/Cargo.toml` — zero hits
- [ ] Run `cargo nextest run --package tama` — all pass (checks feature unification still works: `cargo check --package tama --no-default-features --features csr` must also compile)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: use Repository::delete_config in delete_model, drop rusqlite from tama crate"

**Acceptance criteria:**
- [ ] `rg "rusqlite" crates/tama/` hits only `target/` (i.e., zero source/Cargo.toml hits)
- [ ] `delete_model` opens exactly one `Repository`, no `ModelManager`, no raw SQL
- [ ] Deleting a model still removes the `model_configs` row and cascades — proven by test
- [ ] `cargo check --package tama --no-default-features --features csr` compiles; `cargo nextest run --workspace` passes

---

### Task 4: Seal the funnel — `pub(crate)` escape hatches and `ApiKeyStore`

**Context:**
`crates/tama-core/src/db/mod.rs:15` has `pub use rusqlite::Connection;` (verified: zero users — `rg "db::Connection" crates/` is empty), `ModelManager::conn()` (manager.rs:40) and `ModelManager::transaction()` (manager.rs:45) are public escape hatches, and `crates/tama-core/src/proxy/api_keys.rs` exposes 7 public free functions taking `&Connection`: `validate_key` (:98), `create_key` (:148), `list_keys` (:194), `revoke_key` (:225), `update_key_scopes` (:259), `get_key` (:279), `get_key_name` (:304) — every caller (`tama_handlers/api_keys.rs` ×5 handlers, `auth.rs:104`, `forward/request.rs:9`) first grabs a raw connection via `ProxyState::open_db()` and passes it in. Decision: make the re-export and the two manager escape hatches `pub(crate)` (tama-core-internal lifecycle code keeps working; the `tama` crate no longer needs them after Tasks 1–3), and bundle the api_keys DB functions into a small `ApiKeyStore<'a>` that borrows a connection — this keeps the per-request `open_db()` flow intact (no lifecycle change) while ending the "public API takes raw connections" pattern. `generate_key`/`hash_key`/`extract_prefix` stay free functions (no DB access). Do NOT touch `ProxyState::open_db` itself (F32's accessor cleanup is a different plan).

**Files:**
- Modify: `crates/tama-core/src/db/mod.rs`
- Modify: `crates/tama-core/src/models/manager.rs`
- Modify: `crates/tama-core/src/proxy/api_keys.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/api_keys.rs`
- Modify: `crates/tama-core/src/proxy/auth.rs`
- Modify: `crates/tama-core/src/proxy/forward/request.rs`
- Modify: `crates/tama-core/src/proxy/scope_middleware.rs` (tests only)
- Modify: `crates/tama-core/src/config/types/config_tests.rs` (tests only)

**What to implement:**

1. `crates/tama-core/src/db/mod.rs:15` — change `pub use rusqlite::Connection;` to `pub(crate) use rusqlite::Connection;`. If clippy then reports it unused (zero users today), DELETE the line instead — prefer deletion over `pub(crate)` if it has no users. Either way the acceptance is: no `pub` re-export of `rusqlite::Connection` from `tama_core::db`.

2. `crates/tama-core/src/models/manager.rs` — change `pub fn conn()` (line 40) to `pub(crate) fn conn()` and `pub fn transaction` (line 45) to `pub(crate) fn transaction`. Fix any visibility fallout inside tama-core (all callers are intra-crate, so none expected; verify with `cargo check -p tama-core`). Also update the doc comment on `conn()` — remove "This is a permanent escape hatch" wording; state it is crate-internal.

3. **`ApiKeyStore`** in `crates/tama-core/src/proxy/api_keys.rs`:
   ```rust
   /// Database access for API keys.
   ///
   /// Borrows a `Connection` for the duration of one request/operation;
   /// obtain the connection from `ProxyState::open_db()`.
   pub struct ApiKeyStore<'a> {
       conn: &'a Connection,
   }

   impl<'a> ApiKeyStore<'a> {
       pub fn new(conn: &'a Connection) -> Self {
           Self { conn }
       }
       // methods below
   }
   ```
   Convert the seven free functions into methods by dropping the leading `conn: &Connection` parameter and using `self.conn`: `pub fn validate_key(&self, raw_key: &str) -> Result<Option<(i64, Vec<Scope>)>>`, `pub fn create_key(&self, name: &str, raw_key: &str, scopes: &[Scope], created_by: &str, expires_at: Option<&str>) -> Result<i64>` (copy the exact current signature of `create_key` at :148 — check the real parameter list before writing), `pub fn list_keys(&self) -> Result<Vec<ApiKeyRecord>>`, `pub fn revoke_key(&self, key_id: i64) -> Result<bool>`, `pub fn update_key_scopes(&self, key_id: i64, scopes: &[Scope]) -> Result<ApiKeyRecord>`, `pub fn get_key(&self, key_id: i64) -> Result<Option<ApiKeyRecord>>`, `pub fn get_key_name(&self, key_id: i64) -> Result<Option<String>>`. Bodies unchanged except `conn` → `self.conn`. Keep the free functions' doc comments on the methods.

4. **Update callers:**
   - `crates/tama-core/src/proxy/auth.rs:104` — inside the `spawn_blocking` closure: `db.map(|conn| ApiKeyStore::new(&conn).validate_key(&raw_token_for_db))`; update the `use crate::proxy::api_keys::{self, validate_key, AuthSubject};` import (line 23) to `{ApiKeyStore, AuthSubject}` (+ `self` if still needed). Update the three test call sites (`api_keys::create_key(&conn, ...)` at :1173, :1285, :1488) to `ApiKeyStore::new(&conn).create_key(...)`.
   - `crates/tama-core/src/proxy/tama_handlers/api_keys.rs` — five handlers: after each `let conn = state…open_db().unwrap();` (lines 177, 217, 245, 336, 372, 434, 470) construct `let store = api_keys::ApiKeyStore::new(&conn);` and call `store.create_key(...)`, `store.get_key(...)`, `store.list_keys()`, `store.update_key_scopes(...)`, `store.revoke_key(...)`. Test at :528 likewise.
   - `crates/tama-core/src/proxy/forward/request.rs:9` — `use crate::proxy::api_keys::get_key_name;` → `use crate::proxy::api_keys::ApiKeyStore;`; at the call site (find with `rg get_key_name crates/tama-core/src/proxy/forward/`), construct the store from the connection in scope.
   - `crates/tama-core/src/proxy/scope_middleware.rs:531` (test) and `crates/tama-core/src/config/types/config_tests.rs:343,371,401` (tests) — same mechanical conversion.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- proxy::api_keys` and `cargo nextest run --package tama-core -- proxy::auth` — green baseline
- [ ] Convert the free functions to `ApiKeyStore` methods in `crates/tama-core/src/proxy/api_keys.rs` (keep the free-function wrappers TEMPORARILY if you want a smaller diff — NO, do the full cutover in one commit: delete the free fns, update all callers in the same commit)
- [ ] Update all callers listed above
- [ ] Make the `db/mod.rs:15` re-export `pub(crate)` or delete it (delete if unused); make `ModelManager::conn()`/`transaction()` `pub(crate)`
- [ ] Run `cargo nextest run --package tama-core` — all pass (api_keys, auth, scope_middleware, config tests all exercise the store)
- [ ] Run `cargo nextest run --package tama` — confirms the `tama` crate never used the now-sealed items
- [ ] Run `rg "pub fn (conn|transaction)" crates/tama-core/src/models/manager.rs` and `rg "pub use rusqlite" crates/tama-core/src/db/mod.rs` — no `pub` hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: seal DB escape hatches (pub(crate)) and introduce ApiKeyStore"

**Acceptance criteria:**
- [ ] `tama_core::db` no longer publicly re-exports `rusqlite::Connection`; `ModelManager::conn`/`transaction` are `pub(crate)`
- [ ] `crates/tama-core/src/proxy/api_keys.rs` exposes zero public free functions taking `&Connection` (DB access goes through `ApiKeyStore`)
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 5: Construct `Repository` once at startup, share via `WebState`, delete the 28 per-request opens

**Context:**
Every API handler that needs the DB calls `Repository::open(&config_dir)` — 28 call sites across 14 files (verified by rg: `api/updates.rs` ×4, `api/aliases/mod.rs` ×5 sites' worth of db_dir plumbing, `api/benchmarks/{run,spec,mtp,history}.rs`, `api/models/{info,files}.rs`, `api/models/crud/{update,rename,delete}.rs`, `api/backends/{install,list}.rs`, `api/backends/manage/remove.rs`) — and every `open()` re-runs the migration suite (`db::open` → `migrations::run`, `crates/tama-core/src/db/mod.rs:108`). Many handlers also carry the same `state.db_dir().clone().unwrap_or_else(|| Config::config_dir()…)` fallback block (F15 owns the full config-dir mess — this task removes only the plumbing that existed to feed `Repository::open`). Decision: add one shared repository to `WebState` (`crates/tama/src/web_types.rs:407`), constructed once in `crates/tama/src/main.rs` after the existing DB setup block (lines 66–96), and give handlers a helper. `Repository` wraps `rusqlite::Connection` which is `Send` but NOT `Sync`, so the shared handle is `Arc<std::sync::Mutex<Repository>>` — the same pattern `PullQueueService` uses for `ModelManager` (`crates/tama-core/src/proxy/pull_queue.rs:59`, `.lock().unwrap()`). All DB work still happens on `spawn_blocking` where it does today; do NOT hold the lock guard across `.await` points. Migrations still run at startup (the existing `tama_core::db::open` in main.rs:67 and this one `Repository::open` — both idempotent); the win is removing 28 per-request migration runs. `ProxyState::db_dir()` stays (BackendManager helpers in `api/helpers.rs` still use it — out of scope).

**Files:**
- Modify: `crates/tama/src/web_types.rs`
- Modify: `crates/tama/src/main.rs`
- Modify: `crates/tama/src/api/helpers.rs`
- Modify: `crates/tama/src/api/backends/manage/tests.rs` (and any other `test_web_state` constructors — find with `rg "WebState {" crates/tama/src`)
- Modify: the 14 files with `Repository::open` sites: `crates/tama/src/api/updates.rs`, `crates/tama/src/api/aliases/mod.rs`, `crates/tama/src/api/backup.rs` (check — it uses `db_dir` but may not open a Repository; only touch if it does), `crates/tama/src/api/benchmarks/{run,spec,mtp,history}.rs`, `crates/tama/src/api/models/{info,files}.rs`, `crates/tama/src/api/models/crud/{update,rename,delete}.rs`, `crates/tama/src/api/backends/{install,list}.rs`, `crates/tama/src/api/backends/manage/remove.rs`

**What to implement:**

1. **`WebState` field** in `crates/tama/src/web_types.rs` (struct at line 407):
   ```rust
   /// Shared repository for the management API. Constructed once at startup
   /// (migrations run once). `None` when the DB directory is not configured
   /// (tests). Locked with `std::sync::Mutex` — all use is synchronous and
   /// must happen inside `spawn_blocking` or other non-async-holding scopes.
   pub repository: Option<std::sync::Arc<std::sync::Mutex<tama_core::db::repository::Repository>>>,
   ```
   Use the fully qualified `std::sync::Mutex`/`std::sync::Arc` in the field type — `web_types.rs` already imports `tokio::sync::Mutex` (line 14) and the names must not clash.

2. **Construction** in `crates/tama/src/main.rs`: after the DB setup block (after line 96, before `let proxy_state = …` at line 98 or right before the `WebState` literal at line 107), add:
   ```rust
   // Shared repository for the management API — opened once; migrations
   // already ran above (idempotent), so this open is cheap.
   let repository = db_dir.as_ref().and_then(|dir| {
       match tama_core::db::repository::Repository::open(dir) {
           Ok(r) => Some(Arc::new(std::sync::Mutex::new(r))),
           Err(e) => {
               tracing::error!("Failed to open shared repository: {}", e);
               None
           }
       }
   });
   ```
   and add `repository,` to the `WebState { … }` literal (line 107–115). NOTE: `repository` is computed outside the `#[cfg(feature = "ssr")]` block but only consumed inside it — if the non-ssr build warns about an unused variable, move the construction inside the `#[cfg(feature = "ssr")]` block (check `cargo check --package tama --no-default-features`).

3. **Helper** in `crates/tama/src/api/helpers.rs`:
   ```rust
   /// Clone the shared Repository handle from WebState, or an error response
   /// when the database is not configured.
   pub fn shared_repository(
       web_state: &crate::web_types::WebState,
   ) -> Result<
       std::sync::Arc<std::sync::Mutex<tama_core::db::repository::Repository>>,
       axum::response::Response,
   > {
       web_state.repository.clone().ok_or_else(|| {
           error_response_simple(
               StatusCode::SERVICE_UNAVAILABLE,
               "Database not configured",
           )
       })
   }
   ```

4. **Test constructors**: add `repository: None,` to `test_web_state()` in `crates/tama/src/api/backends/manage/tests.rs:8` and EVERY other `WebState { … }` literal (`rg "WebState {" crates/tama/src` — check `crates/tama/tests/` too). Tests that need a real DB construct their own tempdir repository: `Some(Arc::new(std::sync::Mutex::new(tama_core::db::repository::Repository::open(tmp.path()).unwrap())))`.

5. **Migrate all 28 `Repository::open` sites.** Per file, the mechanical edit is:
   - Add `axum::extract::Extension` import if missing; add `Extension(web_state): Extension<crate::web_types::WebState>` to the handler signature (Extension is layered over ALL routes in `crates/tama/src/router.rs` — the `.layer(axum::extract::Extension(web_state.as_ref().clone()))` near the end of `build_web_routes` — so every handler may extract it).
   - Replace `let repo = Repository::open(&db_dir).map_err(...)?` (and the `let db_dir = state.db_dir().clone().unwrap_or_else(...)` block that fed it, when it has no other consumer in the handler) with `let repo = shared_repository(&web_state)?;` before the `spawn_blocking` closure and `let repo_handle = repo.clone();` moved into the closure; inside, `let repo = repo_handle.lock().unwrap();`.
   - Handlers that today do DB work directly on the async executor (e.g. `aliases/mod.rs::list_aliases` at :37) — keep the same execution placement (do NOT fix F20's spawn_blocking inconsistency here); locking a std Mutex for a short synchronous critical section in async context is acceptable, just never hold the guard across `.await`.
   - `api/updates.rs` — 4 sites (:128 inside a closure, :298, :670, :752). Each closure already runs in `spawn_blocking`; clone the Arc in. The `check_single` site at :298 also uses `config_dir_clone` for other things — remove only the `Repository::open` line.
   - IMPORTANT: after each file's edit, if `config_dir`/`db_dir` becomes unused in that handler, delete the now-dead plumbing — but many handlers use `config_dir` for `configs_dir`/`models_dir` too (e.g. `info.rs::list_models` uses `config_dir.join("configs")`) — in that case keep it.

6. Add one regression test proving a single shared connection: in `crates/tama/src/api/aliases/mod.rs` tests (or a new test in `api/backends/manage/tests.rs` style), construct a WebState whose repository is `Some(...)` over a tempdir DB seeded with one alias (insert via `tama_core::db::queries::insert_alias` on a connection from `tama_core::db::open`), build the router, GET `/tama/v1/aliases`, assert the seeded alias is returned — proving handlers read through the shared handle, not a per-request open.

**Steps:**
- [ ] Add the WebState field + fix all construction sites; `cargo check --package tama` compiles (and `--no-default-features`)
- [ ] Add the `shared_repository` helper in `crates/tama/src/api/helpers.rs`
- [ ] Construct the repository in `crates/tama/src/main.rs` and wire it into the `WebState` literal
- [ ] Write the failing aliases regression test
- [ ] Run `cargo nextest run --package tama -- api::aliases` — verify it fails before migration (handler opens its own repo over `db_dir`… actually it will PASS if `db_dir` points at the same tempdir — set up the test so `ProxyState::new(config, None)` has NO db_dir; then the current handler's `Config::config_dir()` fallback must fail/differ from the tempdir, making the test red until the handler uses the shared handle. Design the test accordingly and note the trick in a comment.)
- [ ] Migrate the 28 sites file by file: aliases → benchmarks → models/crud → models/info+files → updates → backends/install,list,remove
- [ ] Run `rg "Repository::open" crates/tama/src` — zero hits
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo nextest run --workspace` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "perf: share one Repository via WebState instead of 28 per-request opens"

**Acceptance criteria:**
- [ ] `rg "Repository::open" crates/tama/src` returns zero hits; the only `Repository::open` outside tests is `crates/tama/src/main.rs`
- [ ] `WebState` carries `repository: Option<Arc<std::sync::Mutex<Repository>>>`; all constructors updated
- [ ] No handler resolves `db_dir` solely to open a Repository (remaining `db_dir()` uses are for BackendManager/config — untouched)
- [ ] The aliases regression test passes through the shared handle
- [ ] `cargo nextest run --workspace` passes; clippy clean; csr build (`--no-default-features --features csr`) compiles

---

### Task 6: Amend ADR-0017 to record the new layering

**Context:**
`docs/decisions/0017-centralized-managers.md` says centralized managers were "chosen over a generic repository pattern" — that decision has been superseded for the API layer by Tasks 1–5. ADRs are append-only records: do NOT rewrite the original text; mark it amended and add a dated amendment section. This is the documentation half of the architectural decision and must land with the code.

**Files:**
- Modify: `docs/decisions/0017-centralized-managers.md`

**What to implement:**

1. Change the status line `**Status:** accepted` to `**Status:** amended (2026-07-18, plan-160)`.
2. Append at the end of the file:
   ```markdown
   ## Amendment (2026-07-18, plan-160): Repository is the API-layer entry point

   The "managers over repository" decision is amended for the `tama` API layer:

   - `db::repository::Repository` is the single data-access entry point for ALL
     handlers in `crates/tama/src/api/**` — reads AND writes. The model-domain
     write methods (`save_model_config`, `upsert_file`, `delete_file`,
     `upsert_pull`, `get_pull`, `get_files`, `delete_config`) were absorbed from
     `ModelManager`.
   - `BackendManager` and `ModelManager` remain for tama-core-internal proxy
     lifecycle use (`ProxyState`, `PullQueueService`, lifecycle/update code).
     Their raw-connection escape hatches (`conn()`, `transaction()`) are
     `pub(crate)`; `tama_core::db` no longer publicly re-exports
     `rusqlite::Connection`.
   - One struct per table: the `db::queries` record types
     (`ModelConfigRecord`, `ModelFileRecord`, `AliasResponse`, `BenchmarkRow`,
     `PullQueueItem`, `UpdateCheckRecord`, `ModelPullRecord`) are the canonical
     row representations returned by both `Repository` and the managers. The
     parallel DTO hierarchy in `db::repository` was deleted.
   - One shared `Repository` is constructed at startup and stored in
     `WebState` (`Option<Arc<std::sync::Mutex<Repository>>>`), so migrations
     run once — handlers no longer call `Repository::open` per request.
   - API keys use `proxy::api_keys::ApiKeyStore`, a small struct borrowing a
     `&Connection`, instead of public free functions taking raw connections.

   Rationale: two competing access layers (managers + Repository) with
   method-level overlap forced handlers to open both (two SQLite connections
   per request) and produced a field-for-field duplicated DTO hierarchy.
   Centralizing on `Repository` for the API layer keeps ADR-0017's funnel
   benefit (one auditable file per access pattern) where the API actually
   lives, without disturbing the proxy's lifecycle internals.
   ```

**Steps:**
- [ ] Edit `docs/decisions/0017-centralized-managers.md` per above
- [ ] Run `rg -l "centralized-managers" docs/ README.md AGENTS.md` — check for other references to ADR-0017 that need a pointer to the amendment (add " (amended)" to any status tables listing it)
- [ ] Commit with message: "docs: amend ADR-0017 — Repository is the API-layer data-access entry point"

**Acceptance criteria:**
- [ ] ADR-0017 status reads "amended (2026-07-18, plan-160)" and the amendment section matches the implemented layering (Tasks 1–5)
- [ ] Original ADR text is otherwise unchanged
- [ ] No code changes in this commit
