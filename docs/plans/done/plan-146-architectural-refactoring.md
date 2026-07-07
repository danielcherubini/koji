# Architectural Refactoring Plan

**Goal:** Decouple the API layer from the DB layer, encapsulate ProxyState internals, and move web_types out of tama-core.

**Architecture:** Three independent tasks. Task 1 creates a repository layer to abstract DB access. Task 2 encapsulates ProxyState fields. Task 3 moves web_types to the tama crate. Each task is independently commitable.

**Tech Stack:** Rust, Axum, SQLite (rusqlite)

---

### Task 1: DB Repository Layer — abstract DB access from API handlers

**Context:**
Finding #8 from the audit. API handlers in `crates/tama/src/api/` import `tama_core::db::queries::*` directly (35 calls), open DB connections themselves (8 calls to `db::open()`), and use DB record types (`ModelFileRecord`, `ModelConfigRecord`, `DownloadQueueItem`) in function signatures. Zero abstraction between HTTP and SQLite.

This task creates a `Repository` struct in `tama-core` that wraps DB operations with domain-level methods. API handlers call repository methods instead of raw queries. DB record types become private to `tama_core::db`.

**Files:**
- Create: `crates/tama-core/src/db/repository.rs`
- Modify: `crates/tama-core/src/db/mod.rs` (export repository module)
- Modify: `crates/tama/src/api/updates.rs` (use Repository instead of direct DB calls)
- Modify: `crates/tama/src/api/models/info.rs` (use Repository)
- Modify: `crates/tama/src/api/models/files.rs` (use Repository)
- Modify: `crates/tama/src/api/models/crud/delete.rs` (use Repository)
- Modify: `crates/tama/src/api/benchmarks/spec.rs` (use Repository)
- Modify: `crates/tama/src/api/benchmarks/history.rs` (use Repository)
- Modify: `crates/tama/src/api/benchmarks/run.rs` (use Repository)
- Modify: `crates/tama/src/api/benchmarks/mtp.rs` (use Repository)
- Modify: `crates/tama/src/api/backends/install.rs` (use Repository)
- Modify: `crates/tama/src/api/backends/manage.rs` (use Repository)
- Modify: `crates/tama/src/api/backends/list.rs` (use Repository)
- Modify: `crates/tama/src/api/aliases/mod.rs` (use Repository)
- Modify: `crates/tama/src/api/downloads.rs` (use Repository)

**What to implement:**

1. Create `crates/tama-core/src/db/repository.rs` with a `Repository` struct:
   ```rust
   use std::path::Path;
   
   pub struct Repository {
       conn: rusqlite::Connection,
   }
   
   impl Repository {
       pub fn open(db_dir: &Path) -> anyhow::Result<Self> {
           // Open DB connection
       }
       
       // --- Model Config ---
       pub fn get_model_config(&self, id: i64) -> anyhow::Result<Option<ModelConfigDto>> { ... }
       pub fn get_model_config_by_repo_id(&self, repo_id: &str) -> anyhow::Result<Option<ModelConfigDto>> { ... }
       pub fn get_model_files(&self, config_id: i64) -> anyhow::Result<Vec<ModelFileDto>> { ... }
       pub fn load_model_configs(&self) -> anyhow::Result<Vec<ModelConfigDto>> { ... }
       
       // --- Aliases ---
       pub fn get_all_aliases(&self) -> anyhow::Result<Vec<AliasDto>> { ... }
       pub fn get_alias_by_id(&self, id: i64) -> anyhow::Result<Option<AliasDto>> { ... }
       pub fn insert_alias(&self, name: &str, model_id: i64) -> anyhow::Result<i64> { ... }
       pub fn update_alias(&self, id: i64, name: &str, model_id: i64) -> anyhow::Result<()> { ... }
       pub fn delete_alias(&self, id: i64) -> anyhow::Result<()> { ... }
       
       // --- Benchmarks ---
       pub fn insert_benchmark(&self, params: &BenchmarkParams) -> anyhow::Result<i64> { ... }
       pub fn list_benchmarks(&self) -> anyhow::Result<Vec<BenchmarkDto>> { ... }
       pub fn delete_benchmark(&self, id: i64) -> anyhow::Result<()> { ... }
       
       // --- Download Queue ---
       pub fn get_download_queue_item(&self, id: i64) -> anyhow::Result<Option<DownloadQueueDto>> { ... }
       
       // --- Update Checks ---
       pub fn get_all_update_checks(&self) -> anyhow::Result<Vec<UpdateCheckDto>> { ... }
       pub fn delete_update_check(&self, key: &str) -> anyhow::Result<()> { ... }
       pub fn delete_update_checks_by_pattern(&self, pattern: &str) -> anyhow::Result<()> { ... }
   }
   ```

2. Create **DTO types** (not DB record types) that the Repository returns:
   ```rust
   // These replace ModelConfigRecord, ModelFileRecord, etc. in the API layer
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct ModelConfigDto { /* fields needed by API */ }
   
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct ModelFileDto { /* fields needed by API */ }
   
   // etc.
   ```

3. In each API handler file, replace:
   - `db::open(&config_dir)` → `Repository::open(&config_dir)`
   - `db::queries::get_model_config(&conn, id)` → `repo.get_model_config(id)`
   - `ModelFileRecord` → `ModelFileDto`
   - `ModelConfigRecord` → `ModelConfigDto`

4. Make DB record types (`ModelConfigRecord`, `ModelFileRecord`, `DownloadQueueItem`) `pub(crate)` instead of `pub` (they're implementation details of the DB layer).

5. Move `config_key_to_repo_id` from `tama_core::db` to `tama_core::models` (semantically belongs there).

**Steps:**
- [ ] Audit all 35 direct DB calls from `tama/src/api/` and catalog the operations needed
- [ ] Create `db/repository.rs` with `Repository` struct and all needed methods
- [ ] Create DTO types in `db/repository.rs` (or a separate `db/dto.rs`)
- [ ] Add `pub mod repository;` to `db/mod.rs`
- [ ] Make DB record types `pub(crate)` in `db/queries/types.rs`
- [ ] Refactor `api/updates.rs` to use Repository
- [ ] Refactor `api/models/info.rs` to use Repository + DTOs
- [ ] Refactor `api/models/files.rs` to use Repository + DTOs
- [ ] Refactor `api/models/crud/delete.rs` to use Repository
- [ ] Refactor `api/benchmarks/spec.rs` to use Repository
- [ ] Refactor `api/benchmarks/history.rs` to use Repository
- [ ] Refactor `api/benchmarks/run.rs` to use Repository
- [ ] Refactor `api/benchmarks/mtp.rs` to use Repository
- [ ] Refactor `api/backends/install.rs` to use Repository
- [ ] Refactor `api/backends/manage.rs` to use Repository
- [ ] Refactor `api/backends/list.rs` to use Repository
- [ ] Refactor `api/aliases/mod.rs` to use Repository
- [ ] Refactor `api/downloads.rs` to use Repository + DTOs
- [ ] Move `config_key_to_repo_id` to `tama_core::models`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: create Repository layer to abstract DB access from API handlers"

**Acceptance criteria:**
- [ ] Zero direct imports of `tama_core::db::queries::*` in `tama/src/api/`
- [ ] Zero direct calls to `tama_core::db::open()` in `tama/src/api/`
- [ ] DB record types (`ModelConfigRecord`, `ModelFileRecord`) are `pub(crate)` not `pub`
- [ ] API handlers use DTO types, not DB record types
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 2: ProxyState encapsulation — pub fields → pub(crate) + WebState extraction

**Context:**
Finding #9 from the audit. `ProxyState` has 25+ public fields exposing internal synchronization primitives (RwLock, Semaphore, watch::Sender, HashMaps). External code can directly mutate internal state. Web UI fields (`web_jobs`, `web_update_checker`, etc.) are public on the core type.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs` (make fields pub(crate), extract web fields)
- Modify: All files in `crates/tama/src/` that access ProxyState fields directly

**What to implement:**

1. In `proxy/types.rs`, change all `pub` fields to `pub(crate)`.

2. Extract web UI fields into a `WebState` struct (feature-gated):
   ```rust
   #[cfg(feature = "web-ui")]
   pub struct WebState {
       pub jobs: Option<Arc<JobManager>>,
       pub capabilities: Option<Arc<CapabilitiesCache>>,
       pub update_checker: Arc<UpdateChecker>,
       pub binary_version: String,
       pub update_tx: Arc<Mutex<Option<broadcast::Sender<String>>>>,
       pub upload_lock: Arc<RwLock<HashMap<String, UploadEntry>>>,
       // etc.
   }
   ```

3. Add `web_state: Option<Arc<WebState>>` to `ProxyState` (feature-gated).

4. Add accessor methods to `ProxyState`:
   ```rust
   impl ProxyState {
       pub fn model_configs(&self) -> &Arc<RwLock<HashMap<...>>> { &self.model_configs }
       pub fn models(&self) -> &Arc<RwLock<HashMap<...>>> { &self.models }
       pub fn config(&self) -> &Arc<RwLock<Config>> { &self.config }
       // etc. — read-only accessors
   }
   ```

5. Update all callers in `tama/src/` to use accessors instead of direct field access.

**Steps:**
- [ ] Read `proxy/types.rs` to catalog all 25+ fields
- [ ] Change all `pub` fields to `pub(crate)`
- [ ] Extract web UI fields into `WebState` struct (feature-gated)
- [ ] Add read-only accessor methods for commonly-accessed fields
- [ ] Update callers in `tama/src/` to use accessors
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: encapsulate ProxyState fields and extract WebState"

**Acceptance criteria:**
- [ ] All ProxyState fields are `pub(crate)` (not `pub`)
- [ ] Web UI fields extracted into `WebState` struct
- [ ] Read-only accessors for commonly-accessed fields
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 3: Move `web_types` from `tama-core` to `tama` crate

**Context:**
Finding #20 from the audit. `tama-core` has a `web_types` module (JobManager, CapabilitiesCache, UploadEntry, JobKind, JobStatus) gated behind `#[cfg(feature = "web-ui")]`. The core library shouldn't know about web concepts.

**Files:**
- Move: `crates/tama-core/src/web_types.rs` → `crates/tama/src/web_types.rs` (or integrate into existing modules)
- Modify: `crates/tama-core/src/lib.rs` (remove `web_types` module)
- Modify: `crates/tama-core/src/proxy/types.rs` (remove web_types imports)
- Modify: `crates/tama/Cargo.toml` (add any needed dependencies)
- Modify: All files in `tama/src/` that import `tama_core::web_types`

**What to implement:**

1. Move `web_types.rs` from `tama-core` to `tama` crate.

2. Update all imports: `tama_core::web_types::*` → `crate::web_types::*` (within `tama` crate).

3. Remove `#[cfg(feature = "web-ui")] pub mod web_types;` from `tama-core/src/lib.rs`.

4. Update `ProxyState` to use associated types or generics for web-specific fields (or reference them through `WebState` from Task 2).

5. Add any needed dependencies to `tama/Cargo.toml` that were previously in `tama-core/Cargo.toml`.

**Steps:**
- [ ] Read `tama-core/src/web_types.rs` to understand all types and dependencies
- [ ] Copy `web_types.rs` to `crates/tama/src/web_types.rs`
- [ ] Update imports in the copied file (remove `tama_core::` prefixes where needed)
- [ ] Add `pub mod web_types;` to `tama/src/lib.rs` or appropriate module
- [ ] Update all imports in `tama/src/` from `tama_core::web_types` to `crate::web_types`
- [ ] Remove `web_types` module from `tama-core/src/lib.rs`
- [ ] Remove web_types references from `tama-core/src/proxy/types.rs` (use WebState from Task 2)
- [ ] Update `tama/Cargo.toml` with any needed dependencies
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: move web_types from tama-core to tama crate"

**Acceptance criteria:**
- [ ] `web_types` module removed from `tama-core`
- [ ] `web_types` present in `tama` crate with all types preserved
- [ ] `tama-core` has no web-specific types or imports
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass
