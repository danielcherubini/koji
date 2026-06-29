# Config TOML to SQLite Plan

**Goal:** Eliminate `config.toml` as the source of truth — move all global settings (general, proxy, supervisor, compaction, sampling_templates) into typed SQLite tables.

**Architecture:** Five new SQLite tables mirror the five TOML sections. A one-time merged backfill migrates both `[backends]` and global config from `config.toml` to the DB in a single pass, then renames the file to `config.toml.migrated`. The `Config` Rust struct remains the API surface — callers use `config.proxy.host`, `config.general.log_level`, etc. exactly as before. The web UI's JSON contract is unchanged.

**Tech Stack:** SQLite (rusqlite), serde, existing migration system. No new dependencies.

**Key decisions:**
- Backup contains only `tama.db` (no `config.toml`). Restore replaces the DB directly.
- `tama config edit` is removed entirely (config is DB-only).
- Raw TOML API endpoints (`GET/POST /tama/v1/config`) return 410 Gone.
- The existing `[backends]` TOML migration and the new global config migration are merged into one pass.

---

### Task 1: Create SQLite schema and migration

**Context:**
The foundation — five typed tables that replace the TOML config file. Each singleton table (`app_general`, `app_proxy`, `app_supervisor`, `app_compaction`) has a single row with `id = 1`. The `sampling_templates` table has one row per template (coding, chat, analysis, creative). This follows the existing pattern used by `model_configs` and `backend_configs`.

**Files:**
- Create: `crates/tama-core/src/db/migrations/_0031_create_app_config.rs`
- Modify: `crates/tama-core/src/db/migrations.rs` (register the new migration, bump `LATEST_VERSION` to 31)

**What to implement:**

Create `_0031_create_app_config.rs` following the existing pattern (e.g., `_0023_create_backend_configs.rs`):

```sql
-- app_general: Global settings (single row, id=1)
CREATE TABLE IF NOT EXISTS app_general (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    log_level TEXT NOT NULL DEFAULT 'info',
    models_dir TEXT,
    logs_dir TEXT,
    hf_token TEXT,
    update_check_interval INTEGER NOT NULL DEFAULT 12
);

-- app_proxy: Proxy server settings (single row, id=1)
CREATE TABLE IF NOT EXISTS app_proxy (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    host TEXT NOT NULL DEFAULT '0.0.0.0',
    port INTEGER NOT NULL DEFAULT 11434,
    auto_unload INTEGER NOT NULL DEFAULT 0,
    idle_timeout_secs INTEGER NOT NULL DEFAULT 300,
    startup_timeout_secs INTEGER NOT NULL DEFAULT 120,
    circuit_breaker_threshold INTEGER NOT NULL DEFAULT 3,
    circuit_breaker_cooldown_seconds INTEGER NOT NULL DEFAULT 60,
    metrics_retention_secs INTEGER NOT NULL DEFAULT 86400,
    download_queue_poll_interval_secs INTEGER NOT NULL DEFAULT 2,
    max_loaded_models INTEGER NOT NULL DEFAULT 1,
    authenticator_url TEXT,
    authenticator_skip_paths TEXT NOT NULL DEFAULT '["/health","/metrics"]'
);

-- app_supervisor: Process restart and health-check settings (single row, id=1)
CREATE TABLE IF NOT EXISTS app_supervisor (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    restart_policy TEXT NOT NULL DEFAULT 'always',
    max_restarts INTEGER NOT NULL DEFAULT 10,
    restart_delay_ms INTEGER NOT NULL DEFAULT 3000,
    health_check_interval_ms INTEGER NOT NULL DEFAULT 5000,
    health_check_timeout_ms INTEGER NOT NULL DEFAULT 30000,
    health_check_retries INTEGER NOT NULL DEFAULT 3
);

-- app_compaction: LLMLingua-2 compaction settings (single row, id=1)
CREATE TABLE IF NOT EXISTS app_compaction (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    server_path TEXT,
    device TEXT NOT NULL DEFAULT 'cpu',
    port INTEGER,
    request_timeout_ms INTEGER NOT NULL DEFAULT 30000
);

-- sampling_templates: Named sampling parameter presets (many rows)
CREATE TABLE IF NOT EXISTS sampling_templates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    temperature REAL,
    top_k INTEGER,
    top_p REAL,
    min_p REAL,
    presence_penalty REAL,
    frequency_penalty REAL,
    repeat_penalty REAL
);
```

In `migrations.rs`, register this migration with version 31. Bump `LATEST_VERSION` from 30 to 31. The file shape matches `_0023_create_backend_configs.rs`:

```rust
/// v31 — Create app config tables (app_general, app_proxy, app_supervisor, app_compaction, sampling_templates)
pub const MIGRATION: (i32, bool, &str) = (
    31,
    false,
    r#"
        CREATE TABLE IF NOT EXISTS app_general (
        ...
        );
        -- ... remaining tables
    "#,
);
```

**Steps:**
- [ ] Create `_0031_create_app_config.rs` with the `MIGRATION` constant following the pattern above
- [ ] Register the migration in `migrations.rs` as version 31
- [ ] Bump `LATEST_VERSION` to 31
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --package tama-core -- db::migrations`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add SQLite schema for global config (app_general, app_proxy, app_supervisor, app_compaction, sampling_templates)"

**Acceptance criteria:**
- [ ] Migration creates all 5 tables without errors on a fresh SQLite database
- [ ] All columns have correct types and defaults matching the Rust struct defaults
- [ ] `sampling_templates` has `id INTEGER PRIMARY KEY AUTOINCREMENT` and `name TEXT UNIQUE NOT NULL`
- [ ] Singleton tables have `CHECK (id = 1)` constraint
- [ ] `LATEST_VERSION` is 31
- [ ] Existing migration tests still pass
- [ ] Test: inserting a duplicate `name` into `sampling_templates` fails (UNIQUE constraint)
- [ ] Test: inserting `id != 1` into any singleton table fails (CHECK constraint)

---

### Task 2: DB query functions for app config

**Context:**
CRUD functions to read/write each config table. Follows the existing pattern in `db/queries/model_config_queries.rs`. Each function takes a `&Connection` and returns `anyhow::Result`. Singleton tables use `INSERT OR REPLACE ... WHERE id = 1`. The `sampling_templates` table uses standard CRUD.

**Files:**
- Create: `crates/tama-core/src/db/queries/app_config_queries.rs`
- Modify: `crates/tama-core/src/db/queries/mod.rs` (add module and re-exports)

**What to implement:**

Create `app_config_queries.rs` with the following public functions. See `model_config_queries.rs::tests` for the in-memory DB setup pattern.

**Singleton tables:**
- `fn upsert_general(conn: &Connection, log_level: &str, models_dir: Option<&str>, logs_dir: Option<&str>, hf_token: Option<&str>, update_check_interval: u32) -> Result<()>`
- `fn get_general(conn: &Connection) -> Result<Option<(String, Option<String>, Option<String>, Option<String>, u32)>>`
- `fn upsert_proxy(conn: &Connection, host: &str, port: u16, auto_unload: bool, idle_timeout_secs: u64, startup_timeout_secs: u64, circuit_breaker_threshold: u32, circuit_breaker_cooldown_seconds: u64, metrics_retention_secs: u64, download_queue_poll_interval_secs: u64, max_loaded_models: u32, authenticator_url: Option<&str>, authenticator_skip_paths: &[String]) -> Result<()>`
- `fn get_proxy(conn: &Connection) -> Result<Option<(String, u16, bool, u64, u64, u32, u64, u64, u64, u32, Option<String>, Vec<String>)>>`
- `fn upsert_supervisor(conn: &Connection, restart_policy: &str, max_restarts: u32, restart_delay_ms: u64, health_check_interval_ms: u64, health_check_timeout_ms: u64, health_check_retries: u32) -> Result<()>`
- `fn get_supervisor(conn: &Connection) -> Result<Option<(String, u32, u64, u64, u64, u32)>>`
- `fn upsert_compaction(conn: &Connection, enabled: bool, server_path: Option<&str>, device: &str, port: Option<u16>, request_timeout_ms: u64) -> Result<()>`
- `fn get_compaction(conn: &Connection) -> Result<Option<(bool, Option<String>, String, Option<u16>, u64)>>`

**Sampling templates:**
- `fn upsert_sampling_template(conn: &Connection, name: &str, temperature: Option<f64>, top_k: Option<u32>, top_p: Option<f64>, min_p: Option<f64>, presence_penalty: Option<f64>, frequency_penalty: Option<f64>, repeat_penalty: Option<f64>) -> Result<()>`
- `fn get_all_sampling_templates(conn: &Connection) -> Result<Vec<(String, Option<f64>, Option<u32>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>)>>`
- `fn delete_all_sampling_templates(conn: &Connection) -> Result<()>`

**Seed defaults:**
- `fn seed_defaults(conn: &Connection) -> Result<()>` — Inserts default rows into all singleton tables (`INSERT OR IGNORE INTO ... WHERE id = 1`) and seeds the 4 built-in sampling templates with values matching `Config::default()` in `types.rs`:
  - **coding**: temperature=0.3, top_k=50, top_p=0.9, min_p=0.05, presence_penalty=0.1
  - **chat**: temperature=0.7, top_k=40, top_p=0.95, min_p=0.05, presence_penalty=0.0
  - **analysis**: temperature=0.3, top_k=20, top_p=0.9, min_p=0.05, presence_penalty=0.0
  - **creative**: temperature=0.9, top_k=50, top_p=0.95, min_p=0.02, presence_penalty=0.0

Each `upsert_*` function for singleton tables uses:
```sql
INSERT OR REPLACE INTO app_general (id, log_level, models_dir, ...) VALUES (1, ?, ?, ...)
```

Each `get_*` function uses:
```sql
SELECT log_level, models_dir, ... FROM app_general WHERE id = 1
```

The `authenticator_skip_paths` field is stored as a JSON string (use `serde_json::to_string` / `serde_json::from_str`) and deserialized on read.

**Steps:**
- [ ] Create `app_config_queries.rs` with all functions listed above
- [ ] Add `mod app_config_queries;` and `pub use app_config_queries::*;` to `queries/mod.rs`
- [ ] Write unit tests for each function using an in-memory SQLite database (follow pattern in `model_config_queries.rs::tests`)
- [ ] Test: `test_seed_defaults_creates_all_rows` — creates all singleton rows + 4 sampling templates
- [ ] Test: `test_general_roundtrip` — upsert + get for general
- [ ] Test: `test_proxy_roundtrip` — upsert + get for proxy (including authenticator_skip_paths JSON)
- [ ] Test: `test_supervisor_roundtrip`
- [ ] Test: `test_compaction_roundtrip`
- [ ] Test: `test_sampling_template_crud` — upsert, get_all, delete_all
- [ ] Run `cargo test --package tama-core -- app_config_queries`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add DB query functions for global config tables"

**Acceptance criteria:**
- [ ] All upsert functions use `INSERT OR REPLACE` with `id = 1` for singleton tables
- [ ] All get functions return `Option<...>` (None if no row exists)
- [ ] `seed_defaults` creates all singleton rows and 4 sampling templates with correct values
- [ ] `authenticator_skip_paths` is stored as JSON string and round-trips correctly
- [ ] Each function has at least one unit test with in-memory DB
- [ ] No clippy warnings

---

### Task 3: Config struct DB integration + convert test fixtures + remove `loaded_from`

**Context:**
Bridge the `Config` Rust struct to the new DB tables. Adds `Config::from_db()` and `Config::to_db()` methods. Removes the `loaded_from` field entirely — path info is derived from `Config::config_dir()` (a static method) instead. This is the largest single change: `loaded_from` is referenced in ~20 production files and ~30 test files across 4 crates.

**CRITICAL ORDERING:** Test fixtures that set `config.loaded_from = Some(...)` must be converted BEFORE the field is removed, otherwise `cargo build --workspace` will fail. The steps below are ordered: (1) add new methods, (2) convert ALL test fixtures, (3) remove the field from production code.

**Production files:**
- Modify: `crates/tama-core/src/config/types.rs` (add `from_db`/`to_db`, remove `loaded_from`, update `configs_dir`/`models_dir`)
- Modify: `crates/tama-core/src/config/loader.rs` (remove `loaded_from` assignments, update `logs_dir`)
- Modify: `crates/tama-core/src/config/migrate/model_to_db.rs` (absorbed into unified migration — see Task 4; for now just remove `loaded_from` usage)
- Delete: `crates/tama-core/src/config/migrate/model_to_db.rs` (absorbed into `migrate_toml_to_db` in Task 4)
- Modify: `crates/tama-core/src/config/migrate/mod.rs` (remove `model_to_db` module)
- Modify: `crates/tama-core/src/proxy/server/mod.rs` (remove `migrate_models_to_db` call — absorbed into unified migration)
- Modify: `crates/tama-cli/src/commands/model/migrate.rs` (remove — absorbed into unified migration)
- Modify: `crates/tama-cli/src/commands/model/mod.rs` (remove `migrate` module)
- Modify: `crates/tama-web/src/types/config.rs` (remove `loaded_from` from mirror type and conversions)
- Modify: `crates/tama-web/src/api.rs` (remove all `loaded_from` references — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/backends/install.rs` (3 references — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/backends/manage.rs` (5 references — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/backends/list.rs` (3 references — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/backends/compaction.rs` (1 reference — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/benchmarks/mtp.rs` (1 reference — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/benchmarks/run.rs` (1 reference — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/benchmarks/spec.rs` (1 reference — replace with `Config::config_dir()`)
- Modify: `crates/tama-web/src/api/backup.rs` (4 references — replace with `Config::config_dir()`)
- Modify: `crates/tama-cli/src/commands/model/pull.rs` (1 production reference — replace with `Config::config_dir()`)
- Modify: `crates/tama-cli/src/commands/backup.rs` (4 references — replace with `Config::config_dir()`)
- Modify: `crates/tama-cli/src/handlers/server/rm.rs` (1 reference — replace with `Config::config_dir()`)
- Modify: `crates/tama-cli/src/handlers/server/add.rs` (1 reference — replace with `Config::config_dir()`)
- Modify: `crates/tama-cli/src/handlers/server/edit.rs` (1 reference — replace with `Config::config_dir()`)

**Test files (convert BEFORE removing `loaded_from`):**
- Modify: `crates/tama-core/src/config/resolve/tests/basic.rs` (5 fixtures)
- Modify: `crates/tama-core/src/config/resolve/tests/aliases.rs` (4 fixtures)
- Modify: `crates/tama-core/src/config/resolve/tests/kv_cache_types.rs` (5 fixtures)
- Modify: `crates/tama-core/src/config/resolve/tests/context_np.rs` (6 fixtures)
- Modify: `crates/tama-core/src/config/resolve/tests/unified_slots.rs` (5 fixtures)
- Modify: `crates/tama-core/src/config/resolve/tests/gpu_device.rs` (5 fixtures)
- Modify: `crates/tama-core/src/config/resolve/tests/spec_decoding/general.rs` (3 fixtures)
- Modify: `crates/tama-core/src/config/resolve/tests/spec_decoding/mtp.rs` (10 fixtures)
- Modify: `crates/tama-core/src/proxy/tama_handlers/tests.rs` (3 fixtures)
- Modify: `crates/tama-core/src/proxy/mod.rs` (1 fixture)
- Modify: `crates/tama-core/src/db/backfill/initial_backfill.rs` (1 fixture)
- Modify: `crates/tama-cli/tests/tests.rs` (2 fixtures)
- Modify: `crates/tama-web/tests/config_structured_test.rs` (1 fixture)
- Modify: `crates/tama-web/tests/server_test.rs` (4 fixtures)

**What to implement:**

**Phase 1 — Add `from_db`/`to_db` to `Config` (types.rs):**

```rust
impl Config {
    /// Load Config from a SQLite database at the given path.
    pub fn from_db(db_path: &std::path::Path) -> anyhow::Result<Self> {
        let conn = Connection::open(db_path)?;
        // Seed defaults if tables are empty
        seed_defaults(&conn)?;
        // Read each table and map to Config struct
        // ...
    }

    /// Persist Config to a SQLite database at the given path.
    pub fn to_db(&self, db_path: &std::path::Path) -> anyhow::Result<()> {
        let conn = Connection::open(db_path)?;
        // Upsert each section
        // ...
        Ok(())
    }
}
```

**Phase 2 — Convert ALL test fixtures (before removing the field):**

For every test file in the "Test files" list above, replace:
```rust
config.loaded_from = Some(temp_dir.path().to_path_buf());
```
with:
```rust
// No-op: loaded_from removed. Config::configs_dir() and Config::models_dir()
// now use Config::config_dir() (static). If the test needs a custom models_dir,
// set config.general.models_dir = Some(...) directly.
```

Most resolve tests only need `loaded_from` for `config.models_dir()` or `config.configs_dir()`. After the change, these methods use `Config::config_dir()` (static). If a test needs a custom directory, set `config.general.models_dir = Some(temp_dir.path().to_string())` directly on the Config struct.

For `config_structured_test.rs`: Remove `loaded_from` from the ProxyState setup. The structured endpoint uses `Config::load()` which doesn't need `loaded_from`.

For `server_test.rs`: Replace TOML fixture writing with DB seeding. Remove `loaded_from` from ProxyState.

**Phase 3 — Remove `loaded_from` from production code:**

**In `types.rs`:**
- Remove `loaded_from` field from `Config` struct
- Update `configs_dir()` and `models_dir()` to use `Self::config_dir()` instead of `self.loaded_from`

**In `loader.rs`:**
- Remove `loaded_from = Some(...)` assignment in `load_from()`
- Update `logs_dir()` to use `Self::base_dir()` instead of `self.loaded_from`
- Remove `loaded_from: None` from `Config::default()`

**In `types/config.rs` (web mirror):**
- Remove `loaded_from` from the mirror `Config` struct
- Remove `loaded_from` from all `From` conversions
- Remove `#[serde(skip)]` attribute

**For ALL other production files in the "Files" list above:**
Replace `state.config.read().await.loaded_from.clone()` or `config.loaded_from` with `Config::config_dir()?`. The pattern is:
- If the code needs the **config directory** (parent of tama.db): use `Config::config_dir()?`
- If the code needs the **DB path**: use `Config::config_dir()?.join("tama.db")`

**Delete `migrate_models_to_db`** — The `[models]` section migration is folded into the unified `migrate_toml_to_db` in Task 4. Remove:
- `crates/tama-core/src/config/migrate/model_to_db.rs` (entire file)
- `pub mod model_to_db;` from `migrate/mod.rs`
- `crates/tama-cli/src/commands/model/migrate.rs` (entire file)
- `mod migrate;` from `commands/model/mod.rs`
- The call in `crates/tama-core/src/proxy/server/mod.rs:31`

**Keep the following as-is (no changes):**
- All struct field definitions (General, ProxyConfig, Supervisor, CompactionConfig, etc.)
- All default functions
- All existing `#[derive]` macros
- TOML serde attributes (they stay for the migration pass)

**Steps:**
- [ ] **Phase 1:** Add `from_db(db_path)` and `to_db(&self, db_path)` to `Config` in `types.rs`
- [ ] **Phase 1:** Write unit test: `test_config_db_roundtrip` — create temp DB, write Config, read back, assert all fields match
- [ ] **Phase 1:** Write unit test: `test_config_from_empty_db_seeds_defaults`
- [ ] **Phase 1:** Run `cargo build --package tama-core` — verify new methods compile
- [ ] **Phase 2:** Convert ALL test fixtures listed above (remove `loaded_from` assignments)
- [ ] **Phase 2:** Run `cargo test --workspace` — verify tests still pass with converted fixtures
- [ ] **Phase 3:** Remove `loaded_from` field from `Config` struct in `types.rs`
- [ ] **Phase 3:** Update `configs_dir()` and `models_dir()` to use `Self::config_dir()`
- [ ] **Phase 3:** Update `loader.rs` — remove `loaded_from` assignments, update `logs_dir()`
- [ ] **Phase 3:** Update `Config::default()` — remove `loaded_from: None`
- [ ] **Phase 3:** Update `tama-web/src/types/config.rs` — remove `loaded_from` from mirror type and all conversions
- [ ] **Phase 3:** Update every production file in the "Files" list: replace `loaded_from` with `Config::config_dir()?`
- [ ] **Phase 3:** Delete `model_to_db.rs`, `migrate.rs` (CLI command), and their module declarations
- [ ] **Phase 3:** Remove `migrate_models_to_db` call from `proxy/server/mod.rs`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix remaining `loaded_from` references and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add Config::from_db/to_db, convert test fixtures, remove loaded_from field"

**Acceptance criteria:**
- [ ] `Config::from_db()` reads all 5 tables and constructs a complete Config
- [ ] `Config::to_db()` writes all fields to the correct tables
- [ ] Empty DB triggers `seed_defaults` before read
- [ ] `loaded_from` field is fully removed — `grep -r "loaded_from" --include="*.rs" crates/` returns nothing
- [ ] `migrate_models_to_db` function and its CLI command are fully removed
- [ ] `configs_dir()` and `models_dir()` work using `Config::config_dir()`
- [ ] Full round-trip test passes
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes

---

### Task 4: Replace Config loader with DB + merged TOML migration backfill

**Context:**
The core change — `Config::load()` now reads from SQLite instead of TOML. A unified backfill migrates ALL TOML sections (`[backends]`, `[models]`, and global config) to the DB in a single pass, then renames the file to `config.toml.migrated`. This absorbs the existing `migrate_backend_config_from_toml` from `initial_backfill.rs` AND the `[models]` migration from `model_to_db.rs` (deleted in Task 3).

**Files:**
- Modify: `crates/tama-core/src/config/loader.rs` (replace `load()`/`save()` with DB, remove `config_path()`/`save_to()`)
- Create: `crates/tama-core/src/db/backfill/migrate_toml_to_db.rs` (unified TOML → DB migration)
- Modify: `crates/tama-core/src/db/backfill/mod.rs` (add new module)
- Modify: `crates/tama-core/src/db/backfill/initial_backfill.rs` (remove `migrate_backend_config_from_toml` — absorbed)

**What to implement:**

**In `loader.rs`, replace `Config::load()`:**

```rust
impl Config {
    pub fn load() -> Result<Self> {
        let config_dir = Self::config_dir()?;
        let db_path = config_dir.join("tama.db");

        // Run one-time TOML → DB migration if config.toml exists
        if config_dir.join("config.toml").exists() {
            migrate_toml_to_db(&config_dir, &db_path)?;
        }

        // Load from DB
        Self::from_db(&db_path)
    }

    pub fn save(&self) -> Result<()> {
        let config_dir = Self::config_dir()?;
        let db_path = config_dir.join("tama.db");
        self.to_db(&db_path)
    }
}
```

**Remove `config_path()`** — it returns `config_dir/join("config.toml")` which is no longer the source of truth. No callers remain.

**Remove `save_to()`** — no longer needed.

**Keep `load_from()` with updated signature** — it takes a `db_path: &Path` instead of `config_dir: &Path`. Used by `tama web` CLI handler. Renamed internally but the public name stays for compatibility.

**In `migrate_toml_to_db.rs` (new unified migration):**

```rust
pub fn migrate_toml_to_db(config_dir: &Path, db_path: &Path) -> Result<()> {
    // 1. Idempotency check: if app_general has a row (id=1), skip (already migrated)
    // 2. Read and parse config.toml using existing TOML deserialization
    // 3. Open/create the SQLite DB at db_path
    // 4. Run migrations to ensure tables exist
    // 5. Migrate [models] section → model_configs table (absorbed from model_to_db.rs)
    // 6. Migrate [backends] section → backend_configs table (absorbed from initial_backfill.rs)
    // 7. Migrate global config (general, proxy, supervisor, compaction, sampling_templates) → app_* tables
    // 8. Rename config.toml → config.toml.migrated (backup)
    // 9. Log info message with count of migrated items per section
}
```

The migration must be **idempotent** — if called when tables already have data, it skips entirely (check if `app_general` has a row with `id = 1`).

**In `initial_backfill.rs`:**
- Remove `migrate_backend_config_from_toml` function (absorbed into the new unified migration)
- Keep any other functions in the file that are still used
- Update `mod.rs` to remove the `migrate_backend_config_from_toml` re-export if present

**Steps:**
- [ ] Create `migrate_toml_to_db.rs` with the unified migration function
- [ ] Absorb the `[backends]` migration logic from `initial_backfill.rs::migrate_backend_config_from_toml`
- [ ] Absorb the `[models]` migration logic from the deleted `model_to_db.rs` (code is in git — read from `git show HEAD:crates/tama-core/src/config/migrate/model_to_db.rs`)
- [ ] Add `mod migrate_toml_to_db;` to `db/backfill/mod.rs`
- [ ] Replace `Config::load()` in `loader.rs` to use DB + migration
- [ ] Replace `Config::save()` in `loader.rs` to use DB
- [ ] Remove `config_path()` method from `Config`
- [ ] Remove `save_to()` method from `Config`
- [ ] Update `load_from()` signature to take `db_path: &Path` (used by `tama web` CLI)
- [ ] Remove `migrate_backend_config_from_toml` from `initial_backfill.rs`
- [ ] Write test: `test_migrate_toml_to_db` — create temp dir with config.toml (with [backends], [models], and global sections), run migration, verify DB rows, verify TOML renamed
- [ ] Write test: `test_migrate_toml_to_db_idempotent` — run migration twice, verify no error
- [ ] Write test: `test_config_load_from_db` — create DB with seed data, load, verify Config matches
- [ ] Run `cargo test --package tama-core -- config`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo build --workspace`
  - Fix any remaining `config_path()`, `save_to()` references
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: replace Config::load/save with SQLite, add unified TOML→DB migration"

**Acceptance criteria:**
- [ ] `Config::load()` reads from SQLite DB
- [ ] `Config::save()` writes to SQLite DB
- [ ] Existing `config.toml` is migrated to DB on first load ([backends], [models], and global config)
- [ ] After migration, `config.toml` is renamed to `config.toml.migrated`
- [ ] Migration is idempotent (safe to call multiple times)
- [ ] Fresh install (no TOML, no DB rows) seeds defaults correctly
- [ ] `config_path()` and `save_to()` are removed
- [ ] `load_from()` takes a `db_path` parameter
- [ ] `migrate_backend_config_from_toml` is removed from `initial_backfill.rs`
- [ ] All workspace tests pass

---

### Task 5: Update web API, CLI, backup, and remove stale endpoints

**Context:**
The web API handlers, CLI commands, and backup system all reference `config.toml` or `loaded_from`. This task updates them for the DB-only world, removes `tama config edit`, returns 410 Gone for raw TOML endpoints, and simplifies backup to DB-only.

**Files:**
- Modify: `crates/tama-web/src/api.rs` (get_structured_config, save_structured_config, get_config, save_config)
- Modify: `crates/tama-web/src/components/backup_section.rs` (update HTML — remove config.toml reference)
- Modify: `crates/tama-web/src/api/openapi.rs` (update 5 doc strings — remove config.toml references)
- Modify: `crates/tama-web/src/api/backends/compaction.rs` (remove toml_path usage)
- Modify: `crates/tama-web/src/pages/config_editor.rs` (update comment on line 60)
- Modify: `crates/tama-core/src/proxy/handlers/compaction.rs` (update error message)
- Modify: `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs` (update comment on line 51)
- Modify: `crates/tama-core/src/config/resolve/mod.rs` (update comment on line 625)
- Modify: `crates/tama-core/src/config/types.rs` (update doc comments on lines 260, 444)
- Modify: `crates/tama-cli/src/commands/backup.rs` (update references, simplify restore to DB-only)
- Modify: `crates/tama-cli/src/commands/backend/list.rs` (update "config.toml" references — backends are DB-only)
- Modify: `crates/tama-cli/src/commands/backend/install.rs` (update "config.toml" references)
- Modify: `crates/tama-cli/src/handlers/config.rs` (keep only `Show`, remove `Edit` and `Path`)
- Modify: `crates/tama-cli/src/handlers/web.rs` (use `Config::load()`, remove `config_path` parameter)
- Modify: `crates/tama-cli/src/cli.rs` (remove `Edit`/`Path` from ConfigCommands, remove `config_path` from web)
- Modify: `crates/tama-cli/src/lib.rs` (remove `config_path` from `cmd_web` call)
- Modify: `crates/tama-core/src/backup/archive.rs` (remove config.toml from archive — DB only)

**What to implement:**

**Web API — `api.rs`:**
- `get_structured_config`: Replace `Config::load_from(&config_dir)` with `Config::load()`. Remove `config_path`/`config_dir` extraction. Remove `loaded_from` restoration.
- `save_structured_config`: Replace `new_config.save_to(&config_dir)` with `new_config.save()`. Remove `config_path`/`config_dir` extraction. Remove `loaded_from` restoration.
- `get_config` (raw TOML GET): Replace entire function body with `410 Gone` response: `{"error": "TOML config is no longer used. Use GET /tama/v1/config/structured instead."}`
- `save_config` (raw TOML POST): Replace entire function body with `410 Gone` response: `{"error": "TOML config is no longer used. Use POST /tama/v1/config/structured instead."}`
- `load_config_from_state`: Update to use `Config::config_dir()` instead of `loaded_from` fallback.

**Web API — other files:**
- `backup_section.rs:200`: Change `<li>"config.toml"</li>` to `<li>"tama.db (all settings)"</li>`
- `openapi.rs`: Update 5 doc strings (lines 328, 339, 362, 374, 385) — replace "config.toml" with "database"
- `backends/compaction.rs:47`: Remove `toml_path` usage — compaction config is DB-only
- `config_editor.rs:60`: Update comment — remove "(config.toml)" reference

**CLI — update `tama config` subcommand:**
- `Edit` → remove (no TOML file to edit)
- `Path` → remove (no config file path; the DB is an implementation detail)
- `Show` → keep, but load from DB instead of serializing a TOML-loaded Config. It still prints the Config as TOML for debugging/export.

Update `crates/tama-cli/src/handlers/config.rs`:
- Remove `ConfigCommands::Edit` and `ConfigCommands::Path` arms from the match
- For `ConfigCommands::Show`: load Config via `Config::load()` (DB), serialize to TOML, print
- Remove `ConfigCommands::Edit` and `ConfigCommands::Path` variants from `cli.rs`
- Update `cli.rs` to only have `Show` variant (or make `config` command default to show if no subcommand)

**CLI — `tama web` handler:**
- Modify `crates/tama-cli/src/handlers/web.rs`: Replace `Config::load_from(&cd)` with `Config::load()`. The `config_path` parameter is deprecated — the handler always uses `Config::load()` which reads from the default config dir. Remove the `config_path` parameter from `cmd_web()` and its callers.
- Modify `crates/tama-cli/src/cli.rs`: Remove `config_path: Option<std::path::PathBuf>` from the `web` subcommand args.
- Modify `crates/tama-cli/src/lib.rs`: Remove `config_path` from the `cmd_web` call.

**CLI — update messages:**
- `backup.rs`: Update dry-run output from "config.toml" to "tama.db". Update restore to copy DB from backup (no TOML merge).
- `backend/list.rs:78`: Change "To pin a version in config.toml, add:" to "To pin a version, use:"
- `backend/install.rs:239,242`: Change "config.toml" references to "database" or remove entirely (backends are DB-only)

**Backup — DB-only:**
- `archive.rs`: Remove `config.toml` from the archive. The archive contains only `tama.db` + model configs (`.toml` model cards in `configs/`).
- Update `ExtractedArchive.config_path` field — remove it (no longer needed).
- Update create/extract logic to not handle `config.toml`.
- Update `backup.rs` restore: copy `tama.db` from backup to config dir (no TOML merge).

**Proxy — update messages:**
- `compaction.rs:139`: Change `"Add [compaction] section to config.toml."` to `"Enable compaction in the Configuration page or CLI."`
- `backend_logs.rs:51`: Update comment — remove config.toml reference
- `config/resolve/mod.rs:625`: Update comment — remove config.toml reference
- `config/types.rs:260,444`: Update doc comments — remove config.toml references

**Steps:**
- [ ] Update `get_structured_config` to use `Config::load()` (no config_dir extraction)
- [ ] Update `save_structured_config` to use `Config::save()` (no config_dir extraction)
- [ ] Replace `get_config` with 410 Gone response
- [ ] Replace `save_config` with 410 Gone response
- [ ] Update `load_config_from_state` to use `Config::config_dir()`
- [ ] Update `backup_section.rs` HTML
- [ ] Update `openapi.rs` doc strings
- [ ] Update `backends/compaction.rs` — remove toml_path
- [ ] Update `config_editor.rs` comment
- [ ] Update `tama-cli/src/handlers/config.rs` — keep only `Show`, remove `Edit` and `Path`
- [ ] Update `cli.rs` — remove `Edit` and `Path` variants from `ConfigCommands`
- [ ] Update `tama web` handler: `handlers/web.rs` — use `Config::load()`, remove `config_path` parameter
- [ ] Update `cli.rs` — remove `config_path` from `web` subcommand
- [ ] Update `lib.rs` — remove `config_path` from `cmd_web` call
- [ ] Update `backup.rs` — DB-only restore (copy tama.db, no TOML merge)
- [ ] Update `archive.rs` — remove config.toml from archive
- [ ] Update CLI messages (backend/list.rs, backend/install.rs)
- [ ] Update proxy messages (compaction.rs, backend_logs.rs)
- [ ] Update doc comments (types.rs, resolve/mod.rs)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: update web API and CLI for DB-backed config, remove TOML endpoints and config edit"

**Acceptance criteria:**
- [ ] `GET /tama/v1/config/structured` reads from SQLite DB
- [ ] `POST /tama/v1/config/structured` writes to SQLite DB
- [ ] `GET /tama/v1/config` returns 410 Gone
- [ ] `POST /tama/v1/config` returns 410 Gone
- [ ] `tama config show` works (loads from DB, prints TOML)
- [ ] `tama config edit` and `tama config path` no longer exist
- [ ] `tama web` works without `--config-path` flag
- [ ] Backup archive contains only `tama.db` + model configs (no `config.toml`)
- [ ] Restore copies `tama.db` from backup (no TOML merge)
- [ ] No references to "config.toml" in user-facing strings (error messages, CLI output, HTML)
- [ ] All workspace tests pass
- [ ] No clippy warnings

---

### Task 6: Final test cleanup — remaining fixtures, backup tests, template removal

**Context:**
Most `loaded_from` test fixtures were converted in Task 3. This task handles the remaining test files that use `save_to()`, `config.toml` writing, or backup archive assertions. Also removes the `config/tama.toml` template file.

**Note:** The `config/resolve/tests/*.rs` files and most `loaded_from` fixtures were already converted in Task 3 (Phase 2). This task only handles files NOT covered by Task 3.

**Files:**
- Modify: `crates/tama-core/src/config/types.rs` (convert remaining TOML round-trip tests to DB round-trip)
- Modify: `crates/tama-core/src/proxy/tests/restart_test.rs` (convert from TOML to DB fixture)
- Modify: `crates/tama-core/src/updates/tests.rs` (2 uses of `save_to()` — replace with `to_db()`)
- Modify: `crates/tama-core/src/backup/archive.rs` (update tests — no config.toml in archive)
- Modify: `crates/tama-web/tests/server_test.rs` (update tests for 410 Gone endpoints)
- Remove: `config/tama.toml` (template file — no longer generated on first run)
- Optionally keep: `crates/tama-core/test_config/config.toml` (only if migration backfill tests need it)

**What to implement:**

**For `types.rs` tests:**
- Convert `test_sampling_templates_toml_roundtrip` → `test_sampling_templates_db_roundtrip` (use temp DB)
- Convert `test_sampling_templates_serde_custom` → DB round-trip
- Keep TOML deserialization tests ONLY if they test the migration parser

**For `restart_test.rs`:**
- Read the existing test to identify which Config fields are overridden beyond defaults (e.g., `general.log_level`, `proxy.port`, `[[models]]` entries)
- Instead of writing a hand-crafted `config.toml`, seed a temp DB with `app_config_queries::seed_defaults` + `upsert_*` for overridden values + `model_config_queries` for model entries
- The test should still verify the same behavior (model restart after failure), just with DB-backed config

**For `updates/tests.rs`:**
- Replace `config.save_to(&config_dir).unwrap()` with `config.to_db(&config_dir.join("tama.db")).unwrap()`
- Seed DB via `app_config_queries::seed_defaults` if needed

**For `backup/archive.rs` tests:**
- Update to not expect `config.toml` in the archive
- Update `ExtractedArchive` to not have `config_path` field
- Update test assertions that check for `config.toml`

**For `server_test.rs`:**
- Tests that expect 404 from `GET/POST /tama/v1/config` now expect 410 Gone
- Update `test_404_when_config_path_not_configured` → `test_410_gone_for_raw_toml_config`

**Do NOT modify:**
- `config/rename_legacy.rs` — this is the kronk→tama directory migration, not TOML config. Leave it alone.

**Steps:**
- [ ] Convert remaining TOML round-trip tests in `types.rs` to DB round-trip tests
- [ ] Rewrite `restart_test.rs` to use DB seeding instead of TOML fixture
- [ ] Update `updates/tests.rs` — replace `save_to()` with `to_db()`
- [ ] Update `backup/archive.rs` tests — no config.toml in archive
- [ ] Update `server_test.rs` — 410 Gone for raw TOML endpoints
- [ ] Remove `config/tama.toml` template
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: final test cleanup — convert remaining TOML fixtures, remove template"

**Acceptance criteria:**
- [ ] All workspace tests pass
- [ ] No test calls `save_to()` (grep returns nothing)
- [ ] No test writes `config.toml` (except migration backfill tests)
- [ ] No test sets `loaded_from` (grep returns nothing)
- [ ] `config/tama.toml` template removed
- [ ] No clippy warnings
- [ ] `cargo test --workspace` passes cleanly

---

## Verification Checklist

After all tasks are complete:

- [ ] `cargo build --release --workspace` succeeds
- [ ] `cargo test --workspace` passes (all tests)
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all` shows no changes
- [ ] `grep -r "loaded_from" --include="*.rs" crates/` returns nothing
- [ ] `grep -r "config_path()" --include="*.rs" crates/` returns nothing (except ldconfig)
- [ ] `grep -r "save_to" --include="*.rs" crates/` returns nothing
- [ ] Fresh install: no TOML, no DB → seeds defaults, Config::load() returns valid config
- [ ] Migration: existing `config.toml` → migrated to DB ([backends], [models], global config), renamed to `.migrated`, Config::load() returns same values
- [ ] Web UI: Config Editor page loads and saves correctly (DB-backed)
- [ ] Web UI: `GET/POST /tama/v1/config` returns 410 Gone
- [ ] CLI: `tama serve` starts with DB-backed config
- [ ] CLI: `tama config show` works (prints TOML from DB)
- [ ] CLI: `tama config edit` and `tama config path` no longer exist
- [ ] CLI: `tama web` works without `--config-path` flag
- [ ] Backup: archive contains only tama.db + model configs, restore copies DB
- [ ] `config/tama.toml` template removed
