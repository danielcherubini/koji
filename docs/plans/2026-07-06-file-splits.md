# File Splits Plan — tama-core

**Goal:** Split 5 god files in tama-core (each over 1000 lines) into focused modules, improving navigability and reducing coupling.

**Architecture:** Each task splits one file into a module directory with focused submodules. All splits preserve public API — only internal organization changes. Each task is independently commitable.

**Tech Stack:** Rust

---

### Task 1: Split `config/types.rs` (1,407 lines) into 7 modules

**Context:**
The `config/types.rs` file is the #1 god file — it contains ALL config struct definitions, DB serialization, defaults, and tests. Five unrelated responsibilities in one file makes it hard to navigate and increases risk of accidental cross-domain coupling.

**Files:**
- Create: `crates/tama-core/src/config/types/general.rs`
- Create: `crates/tama-core/src/config/types/proxy.rs`
- Create: `crates/tama-core/src/config/types/model.rs`
- Create: `crates/tama-core/src/config/types/backend.rs`
- Create: `crates/tama-core/src/config/types/supervisor.rs`
- Create: `crates/tama-core/src/config/types/compaction.rs`
- Create: `crates/tama-core/src/config/types/mod.rs`
- Delete: `crates/tama-core/src/config/types.rs`
- Modify: `crates/tama-core/src/config/mod.rs` (update `types` reference from file to module dir)

**What to implement:**

1. **`general.rs`**: `General` struct, `impl Default for General`, any general-specific defaults.

2. **`proxy.rs`**: `ProxyConfig` struct, `impl Default for ProxyConfig`, proxy-specific defaults.

3. **`model.rs`**: `ModelConfig`, `QuantEntry`, `QuantKind`, `HealthCheck`, `SpecDecodingConfig`, `ModelModalities`, all impls (Default, to_db_record, from_db_record), model-specific defaults. This will be the largest submodule.

4. **`backend.rs`**: `BackendConfig` struct, `impl Default for BackendConfig`, backend-specific defaults.

5. **`supervisor.rs`**: `Supervisor` struct, `impl Default for Supervisor`, supervisor-specific defaults.

6. **`compaction.rs`**: `CompactionConfig` struct, `impl Default for CompactionConfig`, compaction-specific defaults.

7. **`mod.rs`**: `Config` struct (thin — just the top-level aggregate), `Config::from_db()`, `Config::to_db()`, re-exports of all submodules (`pub use general::*;` etc.), tests (move existing tests here or keep in a `tests.rs` submodule).

**Steps:**
- [ ] Read `config/types.rs` completely to map which lines belong to which submodule
- [ ] Create `config/types/` directory
- [ ] Create `mod.rs` with `Config` struct, `from_db()`, `to_db()`, and re-exports
- [ ] Create `general.rs` with `General` and defaults
- [ ] Create `proxy.rs` with `ProxyConfig` and defaults
- [ ] Create `model.rs` with `ModelConfig`, `QuantEntry`, `QuantKind`, `HealthCheck`, `SpecDecodingConfig`, `ModelModalities`, DB conversions, defaults
- [ ] Create `backend.rs` with `BackendConfig` and defaults
- [ ] Create `supervisor.rs` with `Supervisor` and defaults
- [ ] Create `compaction.rs` with `CompactionConfig` and defaults
- [ ] Move tests to `config/types/tests.rs` (or keep inline in mod.rs)
- [ ] Delete old `config/types.rs`
- [ ] Update `config/mod.rs` if needed (should be transparent since `types` was already a module)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: split config/types.rs into 7 focused modules"

**Acceptance criteria:**
- [ ] No single file in `config/types/` exceeds 400 lines
- [ ] All public types re-exported from `mod.rs` (no breaking changes to external consumers)
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 2: Split `proxy/tama_handlers/models.rs` (1,303 lines) into 4 modules

**Context:**
Mixes 5 CRUD handlers, 4 utility functions, 3 capability detection functions, and ~260 lines of tests. Three distinct concerns in one file.

**Files:**
- Create: `crates/tama-core/src/proxy/tama_handlers/models/handlers.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/models/opencode.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/models/tests.rs`
- Create: `crates/tama-core/src/proxy/tama_handlers/models/mod.rs`
- Delete: `crates/tama-core/src/proxy/tama_handlers/models.rs`

**What to implement:**

1. **`handlers.rs`**: `handle_tama_list_models`, `handle_tama_get_model`, `handle_tama_load_model`, `handle_tama_cancel_load`, `handle_tama_unload_model`.

2. **`opencode.rs`**: `handle_opencode_list_models`, `extract_capabilities`, `fetch_capabilities_from_backend`.

3. **`utils.rs`**: `resolve_model_id`, `build_model_entry`, `generate_display_name`, `capitalize_first`.

4. **`tests.rs`**: All existing tests (~260 lines).

5. **`mod.rs`**: Re-exports + any shared types used across submodules.

**Steps:**
- [ ] Read `models.rs` to map which lines belong to which submodule
- [ ] Create `models/` directory with `mod.rs`
- [ ] Create `utils.rs` with utility functions (no dependencies on other submodules)
- [ ] Create `opencode.rs` with opencode handler and capability functions
- [ ] Create `handlers.rs` with CRUD handlers (imports from utils.rs)
- [ ] Create `tests.rs` with all tests
- [ ] Create `mod.rs` with re-exports
- [ ] Delete old `models.rs`
- [ ] Update `tama_handlers/mod.rs` if needed
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: split proxy/tama_handlers/models.rs into handlers, opencode, utils, tests"

**Acceptance criteria:**
- [ ] No single file exceeds 400 lines
- [ ] All handlers, utils, and tests preserved
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 3: Split `updates/checker.rs` (1,079 lines) into 5 modules

**Context:**
Contains GgufListingCache, UpdateChecker with 5 methods, check_model (~200 lines with 3 nested phases), check_backend (~120 lines), and standalone helpers. Single file does caching, backend checking, model checking, and status determination.

**Files:**
- Create: `crates/tama-core/src/updates/checker/cache.rs`
- Create: `crates/tama-core/src/updates/checker/backend.rs`
- Create: `crates/tama-core/src/updates/checker/model.rs`
- Create: `crates/tama-core/src/updates/checker/helpers.rs`
- Create: `crates/tama-core/src/updates/checker/mod.rs`
- Delete: `crates/tama-core/src/updates/checker.rs`

**What to implement:**

1. **`cache.rs`**: `GgufListingCache` struct + impl.

2. **`backend.rs`**: `check_backend` logic (~120 lines).

3. **`model.rs`**: `check_model` logic (~200 lines with 3 nested phases).

4. **`helpers.rs`**: `determine_update_status`, `should_check_since`.

5. **`mod.rs`**: `UpdateChecker` struct, `run_check`, `save_check_result`, `get_results`, `should_check` methods + re-exports. Move tests here or to `tests.rs`.

**Steps:**
- [ ] Read `checker.rs` to map which lines belong to which submodule
- [ ] Create `checker/` directory with `mod.rs`
- [ ] Create `helpers.rs` with pure functions (no dependencies)
- [ ] Create `cache.rs` with GgufListingCache
- [ ] Create `backend.rs` with check_backend
- [ ] Create `model.rs` with check_model
- [ ] Create `mod.rs` with UpdateChecker orchestrator + re-exports
- [ ] Delete old `checker.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: split updates/checker.rs into cache, backend, model, helpers, mod"

**Acceptance criteria:**
- [ ] No single file exceeds 400 lines
- [ ] UpdateChecker orchestrator in mod.rs delegates to submodule functions
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 4: Split `proxy/forward.rs` (1,119 lines) into 5 modules

**Context:**
Header filtering, JSON rewriting, SSE processing, inference stats extraction, and the main forward_request (~350 lines with circuit breaker, dead process detection, streaming branches). Multiple responsibilities.

**Files:**
- Create: `crates/tama-core/src/proxy/forward/headers.rs`
- Create: `crates/tama-core/src/proxy/forward/json.rs`
- Create: `crates/tama-core/src/proxy/forward/sse.rs`
- Create: `crates/tama-core/src/proxy/forward/stats.rs`
- Create: `crates/tama-core/src/proxy/forward/request.rs`
- Create: `crates/tama-core/src/proxy/forward/mod.rs`
- Delete: `crates/tama-core/src/proxy/forward.rs`

**What to implement:**

1. **`headers.rs`**: Header filtering constants, `filter_request_headers`, `strip_response_headers`.

2. **`json.rs`**: `rewrite_json_model_name`, `build_forward_uri`.

3. **`sse.rs`**: `process_sse_line`.

4. **`stats.rs`**: `extract_inference_stats`.

5. **`request.rs`**: `forward_request` (~350 lines with circuit breaker, dead process detection, streaming/non-streaming branches).

6. **`mod.rs`**: Re-exports + tests (move existing ~300 lines of tests here or to `tests.rs`).

**Steps:**
- [ ] Read `forward.rs` to map which lines belong to which submodule
- [ ] Create `forward/` directory with `mod.rs`
- [ ] Create `headers.rs` with header filtering (no dependencies)
- [ ] Create `json.rs` with JSON rewriting (no dependencies)
- [ ] Create `sse.rs` with SSE processing (no dependencies)
- [ ] Create `stats.rs` with inference stats extraction (no dependencies)
- [ ] Create `request.rs` with forward_request (imports from other submodules)
- [ ] Create `mod.rs` with re-exports
- [ ] Move tests to `tests.rs` or keep in `mod.rs`
- [ ] Delete old `forward.rs`
- [ ] Update `proxy/mod.rs` if needed
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: split proxy/forward.rs into headers, json, sse, stats, request"

**Acceptance criteria:**
- [ ] No single file exceeds 400 lines
- [ ] `forward_request` isolated in `request.rs` with clear imports from other submodules
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 5: Split `gpu/system.rs` (1,121 lines) into 4 modules

**Context:**
Type definitions (~200 lines), NVIDIA detection (~80 lines), AMD detection (~350 lines of verbose sysfs reads), and metrics collection (~80 lines). Four unrelated responsibilities.

**Files:**
- Create: `crates/tama-core/src/gpu/types.rs`
- Create: `crates/tama-core/src/gpu/nvidia.rs`
- Create: `crates/tama-core/src/gpu/amd.rs`
- Create: `crates/tama-core/src/gpu/system.rs` (keep name for metrics + public API)
- Create: `crates/tama-core/src/gpu/mod.rs`
- Move: Current `system.rs` content into submodules

**What to implement:**

1. **`types.rs`**: All metric structs (`GpuDeviceStats`, `SystemMetrics`, `MetricSample`, `MetricBucket`, `MetricCurrent`, `ModelStatus`).

2. **`nvidia.rs`**: `query_nvidia_devices`, `parse_nvidia_smi_csv_line`.

3. **`amd.rs`**: `query_amd_device_names`, `query_amd_device_uuids`, `normalize_amd_uuid`, `query_amd_devices`, all sysfs reading functions.

4. **`system.rs`**: `collect_system_metrics`, `collect_system_metrics_with`, `assign_position_ids`, aggregates, public API.

5. **`mod.rs`**: Re-exports + tests.

**Steps:**
- [ ] Read `system.rs` to map which lines belong to which submodule
- [ ] Create `types.rs` with all metric structs (no dependencies)
- [ ] Create `nvidia.rs` with NVIDIA detection (imports from types.rs)
- [ ] Create `amd.rs` with AMD detection (imports from types.rs)
- [ ] Create new `system.rs` with metrics collection + public API (imports from all submodules)
- [ ] Create `mod.rs` with re-exports
- [ ] Move tests to `tests.rs` or keep in `mod.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "refactor: split gpu/system.rs into types, nvidia, amd, system"

**Acceptance criteria:**
- [ ] No single file exceeds 400 lines
- [ ] AMD detection isolated in `amd.rs` (~350 lines of sysfs reads)
- [ ] NVIDIA detection isolated in `nvidia.rs`
- [ ] Public API in `mod.rs` re-exports unchanged
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass
