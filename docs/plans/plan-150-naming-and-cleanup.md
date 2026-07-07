# Naming and Cleanup Plan

**Goal:** Rename `download` → `pull` in the model subsystem (domain term fix) and complete low-severity cleanup (handler return types, deprecated fields, rename_legacy deadline, ModelConfig composition note).

**Architecture:** Two independent tasks. Task 1 renames download → pull across the model subsystem (directory names, function names, types, DB table). Task 2 handles low-severity cleanup items.

**Tech Stack:** Rust, SQLite (rusqlite)

---

### Task 1: Rename `download` → `pull` in model subsystem

**Context:**
Finding #21 from the audit. CONTEXT.md defines **Pull** = "downloading a model" and forbids "download" and "fetch" for this concept. Yet `models/download/` directory, `download_single()`, `DownloadResult`, `DownloadQueueItem`, `log_download()`, `bytes_downloaded`, and DB table `download_queue` all use the forbidden term.

Note: `download` in TTS context (virtualenv, pip install) and network context (network stats) is **not** a violation — the domain term only applies to model downloading.

**Files:**
- Rename: `crates/tama-core/src/models/download/` → `crates/tama-core/src/models/pull_extra/` (or merge into existing `models/pull/`)
- Modify: `crates/tama-core/src/models/mod.rs` (update module reference)
- Modify: `crates/tama-core/src/models/manager.rs` (rename `log_download` → `log_pull`, `DownloadLogEntry` → `PullLogEntry`, `DownloadQueueItem` → `PullQueueItem`, `bytes_downloaded` → `bytes_pulled`)
- Modify: `crates/tama-core/src/models/pull/download.rs` (rename `download_gguf_with_progress` → `pull_gguf_with_progress`, `DownloadResult` → `PullResult`)
- Modify: `crates/tama-core/src/models/pull/mod.rs` (update comments: "file downloads" → "file pulls")
- Modify: `crates/tama-core/src/models/manager_tests.rs` (rename test functions and assertions)
- Create: `crates/tama-core/src/db/migrations/_00XX_rename_download_queue_to_pull_queue.rs` (DB migration)
- Modify: `crates/tama-core/src/db/migrations/mod.rs` (register new migration)
- Modify: `crates/tama-core/src/db/queries/download_queue_queries.rs` → rename to `pull_queue_queries.rs`
- Modify: `crates/tama-core/src/db/queries/mod.rs` (update module reference)
- Modify: `crates/tama-core/src/proxy/download_queue.rs` → rename to `pull_queue.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (update module reference)
- Modify: All files that import the renamed modules/types

**What to implement:**

1. **Type renames:**
   - `DownloadQueueItem` → `PullQueueItem`
   - `DownloadLogEntry` → `PullLogEntry`
   - `DownloadResult` → `PullResult`
   - `bytes_downloaded` → `bytes_pulled`

2. **Function renames:**
   - `download_single` → `pull_single`
   - `download_gguf_with_progress` → `pull_gguf_with_progress`
   - `log_download` → `log_pull`

3. **Module/directory renames:**
   - `models/download/` → merge into `models/pull/` or rename to `models/pull_extra/`
   - `db/queries/download_queue_queries.rs` → `pull_queue_queries.rs`
   - `proxy/download_queue.rs` → `pull_queue.rs`

4. **DB migration:** Rename `download_queue` table to `pull_queue`.

5. **Comment updates:** "file downloads" → "file pulls", "downloaded" → "pulled".

**Steps:**
- [ ] Audit all occurrences of `download` in the model subsystem (exclude TTS/network contexts)
- [ ] Rename types: `DownloadQueueItem` → `PullQueueItem`, `DownloadLogEntry` → `PullLogEntry`, `DownloadResult` → `PullResult`
- [ ] Rename functions: `download_single` → `pull_single`, `download_gguf_with_progress` → `pull_gguf_with_progress`, `log_download` → `log_pull`
- [ ] Rename fields: `bytes_downloaded` → `bytes_pulled`
- [ ] Rename modules: `download_queue_queries.rs` → `pull_queue_queries.rs`, `download_queue.rs` → `pull_queue.rs`
- [ ] Merge or rename `models/download/` directory
- [ ] Update all imports across the codebase
- [ ] Create DB migration to rename `download_queue` → `pull_queue` table
- [ ] Register new migration in `db/migrations/mod.rs`
- [ ] Update comments: "downloads" → "pulls", "downloaded" → "pulled"
- [ ] Update test function names and assertions
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: rename download to pull in model subsystem to match domain terminology"

**Acceptance criteria:**
- [ ] Zero occurrences of `DownloadQueueItem`, `DownloadLogEntry`, `DownloadResult` remain
- [ ] Zero occurrences of `download_single`, `download_gguf_with_progress`, `log_download` remain
- [ ] `bytes_downloaded` → `bytes_pulled` everywhere
- [ ] DB table renamed from `download_queue` to `pull_queue`
- [ ] `models/download/` directory merged or renamed
- [ ] TTS/network `download` usage preserved (not a violation)
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 2: Low-severity cleanup

**Context:**
Findings #29-32 from the audit. Four low-severity items: handler return types, deprecated fields, rename_legacy deadline, ModelConfig composition note.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs` (typed response for `handle_tama_system_health`)
- Modify: `crates/tama-core/src/proxy/tama_handlers/models.rs` (typed response for `handle_tama_list_models`)
- Modify: `crates/tama-core/src/proxy/handlers/status.rs` (typed response for `handle_status`)
- Modify: `crates/tama-core/src/gpu/system.rs` (remove deprecated field at line 233)
- Modify: `crates/tama/src/pages/dashboard/metrics.rs` (remove deprecated field at line 121)
- Modify: `crates/tama-core/src/config/rename_legacy.rs` (add deprecation deadline comment)
- Modify: `crates/tama-core/src/config/loader.rs` (add deprecation deadline comment)

**What to implement:**

1. **Handler return types (#29):** Define typed response structs for handlers that return `Json<serde_json::Value>`:
   ```rust
   #[derive(Serialize)]
   pub struct ListModelsResponse {
       pub models: Vec<ModelResponse>,
   }
   
   #[derive(Serialize)]
   pub struct ModelResponse {
       pub id: i64,
       pub name: String,
       // ... other fields
   }
   ```
   Replace `Json<serde_json::Value>` with `Json<ListModelsResponse>`.

2. **Deprecated fields (#30):** Remove `#[deprecated(since = "1.45.0")]` fields from `gpu/system.rs:233` and `dashboard/metrics.rs:121`. Verify no external consumers first.

3. **Rename legacy deadline (#31):** Add comment: `// TODO: Remove rename_legacy module in v1.50.0 (2026-Q4)`.

4. **ModelConfig composition (#32):** Add a design note comment in `config/types.rs` (or `config/types/model.rs` if already split): `// NOTE: Consider composing ModelConfig from BackendConfig, GpuConfig, SamplingConfig, SpecDecodingConfig sub-structs. Deferred to future refactor.`

**Steps:**
- [ ] Define typed response structs for `handle_tama_list_models` and `handle_status`
- [ ] Replace `Json<serde_json::Value>` with typed `Json<T>` in those handlers
- [ ] Verify deprecated fields have no consumers (`rg -n "loaded" crates/ | grep -v test`)
- [ ] Remove deprecated field from `gpu/system.rs:233`
- [ ] Remove deprecated field from `dashboard/metrics.rs:121`
- [ ] Add deprecation deadline comment to `rename_legacy.rs` and `loader.rs`
- [ ] Add design note comment to `config/types.rs` (or `model.rs`)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "chore: typed handler responses, remove deprecated fields, set rename_legacy deadline"

**Acceptance criteria:**
- [ ] `handle_tama_list_models` returns `Json<ListModelsResponse>` not `Json<serde_json::Value>`
- [ ] `handle_status` returns typed response
- [ ] Deprecated fields removed from `gpu/system.rs` and `dashboard/metrics.rs`
- [ ] Deprecation deadline comment added to `rename_legacy.rs`
- [ ] Design note added to ModelConfig
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass
