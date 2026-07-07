# Type Safety Plan

**Goal:** Replace primitive strings with typed enums and DB tuples with typed records, improving compile-time safety across the codebase.

**Architecture:** Three independent tasks. Task 1 adds enums for GPU vendor and model state. Task 2 replaces DB tuples with typed records. Task 3 adds enums for config values (restart policy, log level, compaction device). Each task is independently commitable.

**Tech Stack:** Rust, serde, SQLite (rusqlite)

---

### Task 1: GPU Vendor & Model State enums

**Context:**
Finding #10 from the audit. `vendor: String` ("nvidia"/"amd") and `state: String` ("idle"/"loading"/"ready"/"unloading"/"failed") allow arbitrary values. Code compares `state == "failed"` as string literals. CONTEXT.md defines **ModelState** as the domain term.

**Files:**
- Modify: `crates/tama-core/src/gpu/types.rs` (add GpuVendor enum, update GpuDeviceStats)
- Modify: `crates/tama-core/src/gpu/nvidia.rs` (use GpuVendor::Nvidia)
- Modify: `crates/tama-core/src/gpu/amd.rs` (use GpuVendor::Amd)
- Modify: `crates/tama-core/src/proxy/types.rs` (add ModelState enum, update ModelStatus)
- Modify: `crates/tama/src/pages/dashboard/metrics.rs` (use ModelState enum)
- Modify: All files that compare state strings (e.g., `state == "failed"`)
- Modify: DB queries that read/write state strings (add serde conversion)

**What to implement:**

1. Add `GpuVendor` enum to `gpu/types.rs`:
   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "lowercase")]
   pub enum GpuVendor {
       Nvidia,
       Amd,
   }
   ```

2. Add `ModelState` enum to `proxy/types.rs`:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "lowercase")]
   pub enum ModelState {
       Idle,
       Loading,
       Ready,
       Unloading,
       Failed,
   }
   
   impl ModelState {
       pub fn as_str(&self) -> &'static str {
           match self {
               Self::Idle => "idle",
               Self::Loading => "loading",
               Self::Ready => "ready",
               Self::Unloading => "unloading",
               Self::Failed => "failed",
           }
       }
   }
   ```

3. Update `GpuDeviceStats.vendor` from `String` to `GpuVendor`.

4. Update `ModelStatus.state` from `String` to `ModelState`.

5. Replace all string comparisons (`state == "failed"`, `vendor == "nvidia"`) with enum pattern matching.

6. Add DB conversion helpers (read/write strings from SQLite, convert to/from enums).

**Steps:**
- [ ] Add `GpuVendor` enum to `gpu/types.rs` with serde support
- [ ] Add `ModelState` enum to `proxy/types.rs` with serde support
- [ ] Update `GpuDeviceStats.vendor` type and all usages
- [ ] Update `ModelStatus.state` type and all usages
- [ ] Replace string comparisons with enum pattern matching
- [ ] Add DB conversion helpers for enum ↔ String
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "feat: add GpuVendor and ModelState enums for type safety"

**Acceptance criteria:**
- [ ] `GpuVendor` enum with `Nvidia` and `Amd` variants
- [ ] `ModelState` enum with `Idle`, `Loading`, `Ready`, `Unloading`, `Failed` variants
- [ ] Zero string comparisons for vendor/state (all use enum matching)
- [ ] Serde serialization preserves existing string format ("nvidia", "idle", etc.)
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 2: DB tuples → typed records

**Context:**
Finding #11 from the audit. `get_supervisor()` returns `Option<(String, u32, u64, u64, u64, u32)>` (6-tuple), `get_proxy()` returns a 12-tuple. Config types destructure as `proxy_row.0`, `proxy_row.1`, etc. — opaque and error-prone.

**Files:**
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs` (add typed records, update queries)
- Modify: `crates/tama-core/src/config/types.rs` (update from_db to use typed records)
- Modify: Any other files that destructure DB tuples

**What to implement:**

1. Define typed record structs:
   ```rust
   #[derive(Debug)]
   pub struct SupervisorRecord {
       pub restart_policy: String,
       pub max_restarts: u32,
       pub restart_window_secs: u64,
       pub health_check_interval_secs: u64,
       pub idle_timeout_secs: u64,
       pub circuit_breaker_threshold: u32,
   }
   
   #[derive(Debug)]
   pub struct ProxyRecord {
       pub listen_addr: String,
       pub listen_port: u16,
       pub upstream_keepalive_secs: u64,
       pub request_timeout_secs: u64,
       pub idle_timeout_secs: u64,
       pub max_parallel_requests: u32,
       pub spec_decoding_enabled: bool,
       pub compaction_enabled: bool,
       pub auth_enabled: bool,
       pub auth_token: Option<String>,
       pub cors_allowed_origins: Option<String>,
       pub rate_limit_rpm: Option<u32>,
   }
   ```

2. Update query functions to return typed records instead of tuples:
   ```rust
   // Before:
   pub fn get_supervisor(conn: &Connection) -> rusqlite::Result<Option<(String, u32, u64, u64, u64, u32)>>
   
   // After:
   pub fn get_supervisor(conn: &Connection) -> rusqlite::Result<Option<SupervisorRecord>>
   ```

3. Update `Config::from_db()` to use named fields instead of tuple indices:
   ```rust
   // Before:
   let supervisor_row = get_supervisor(&conn)?;
   Supervisor {
       restart_policy: supervisor_row.and_then(|r| r.0),
       max_restarts: supervisor_row.map(|r| r.1).unwrap_or_default(),
       // ...
   }
   
   // After:
   let supervisor_row = get_supervisor(&conn)?;
   Supervisor {
       restart_policy: supervisor_row.as_ref().and_then(|r| Some(r.restart_policy.clone())),
       max_restarts: supervisor_row.as_ref().map(|r| r.max_restarts).unwrap_or_default(),
       // ...
   }
   ```

**Steps:**
- [ ] Read `app_config_queries.rs` to catalog all tuple-returning queries
- [ ] Define `SupervisorRecord`, `ProxyRecord`, `GeneralRecord`, `CompactionRecord` structs
- [ ] Update query functions to map rows to typed records
- [ ] Update `Config::from_db()` to use named fields
- [ ] Update any other consumers of tuple results
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: replace DB tuples with typed record structs"

**Acceptance criteria:**
- [ ] `get_supervisor()` returns `Option<SupervisorRecord>` not `Option<(String, u32, ...)>`
- [ ] `get_proxy()` returns `Option<ProxyRecord>` not `Option<(String, u16, ...)>`
- [ ] Zero tuple index access (`.0`, `.1`, etc.) in config DB code
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 3: Config enums (RestartPolicy, LogLevel, CompactionDevice)

**Context:**
Finding #22 from the audit. `Supervisor.restart_policy` (magic values "always"/"on-failure"), `General.log_level`, and `CompactionConfig.device` ("cpu"/"cuda"/"cuda:0"/"mps") stored as raw strings. Any caller can set arbitrary values.

**Files:**
- Modify: `crates/tama-core/src/config/types/supervisor.rs` (or `config/types.rs` if not yet split)
- Modify: `crates/tama-core/src/config/types/general.rs` (or `config/types.rs`)
- Modify: `crates/tama-core/src/config/types/compaction.rs` (or `config/types.rs`)
- Modify: DB queries that read/write these fields
- Modify: `crates/tama/src/types/config.rs` (WASM mirror types)
- Modify: `crates/tama/src/pages/config_editor.rs` (UI forms)

**What to implement:**

1. Define enums with serde support:
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "kebab-case")]
   pub enum RestartPolicy {
       Always,
       OnFailure,
   }
   
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "lowercase")]
   pub enum LogLevel {
       Debug,
       Info,
       Warn,
       Error,
   }
   
   impl From<LogLevel> for tracing::Level {
       fn from(level: LogLevel) -> Self {
           match level {
               LogLevel::Debug => tracing::Level::DEBUG,
               LogLevel::Info => tracing::Level::INFO,
               LogLevel::Warn => tracing::Level::WARN,
               LogLevel::Error => tracing::Level::ERROR,
           }
       }
       }
   
   #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
   pub enum CompactionDevice {
       #[serde(rename = "cpu")]
       Cpu,
       #[serde(rename = "cuda")]
       Cuda,
       #[serde(serialize_with = "serialize_cuda_device", deserialize_with = "deserialize_cuda_device")]
       CudaDevice(u32),
       #[serde(rename = "mps")]
       Mps,
   }
   ```

2. Update struct fields:
   - `Supervisor.restart_policy: String` → `RestartPolicy`
   - `General.log_level: String` → `LogLevel`
   - `CompactionConfig.device: String` → `CompactionDevice`

3. Update DB queries to convert between enums and strings.

4. Update WASM mirror types in `tama/src/types/config.rs`.

5. Update UI forms in `config_editor.rs` to use enum-select dropdowns.

**Steps:**
- [ ] Define `RestartPolicy`, `LogLevel`, `CompactionDevice` enums with serde support
- [ ] Update `Supervisor.restart_policy` type and defaults
- [ ] Update `General.log_level` type and defaults
- [ ] Update `CompactionConfig.device` type and defaults
- [ ] Update DB queries to convert between enums and strings
- [ ] Update WASM mirror types in `tama/src/types/config.rs`
- [ ] Update UI forms in `config_editor.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "feat: add RestartPolicy, LogLevel, CompactionDevice enums for config type safety"

**Acceptance criteria:**
- [ ] `RestartPolicy` enum with `Always` and `OnFailure` variants
- [ ] `LogLevel` enum with `Debug`, `Info`, `Warn`, `Error` variants + `From<LogLevel> for tracing::Level`
- [ ] `CompactionDevice` enum with `Cpu`, `Cuda`, `CudaDevice(u32)`, `Mps` variants
- [ ] Serde serialization preserves existing string format ("always", "on-failure", "cpu", "cuda:0", "mps")
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass
