# Split Large Files Plan

**Goal:** Split 4 files exceeding 1,000 LOC into focused sub-modules by logical concern, reducing the largest file from 1,410 to ~450 LOC.

**Architecture:** Pure extraction — no logic changes, no behavior changes. Each large `impl ProxyState` method or group of related methods is moved to its own file. The parent `mod.rs` re-exports via `mod` declarations so all existing call sites remain unchanged.

**Tech Stack:** Rust, cargo

---

## Pre-Flight Checklist

Before starting any task, verify the baseline:
- [ ] Run `cargo build --workspace` — must succeed
- [ ] Run `cargo test --workspace 2>&1 | tee /tmp/baseline-tests.log` — save the output. Diff against this log after all 4 tasks to confirm no new failures.
- [ ] Run `cargo clippy --workspace -- -D warnings` — must pass
- [ ] Run `cargo fmt --all` — must succeed

---

### Task 1: Split `proxy/lifecycle/mod.rs` (1,410 → ~450 LOC)

**Context:**
The `lifecycle/mod.rs` file is the largest single file in the project at 1,410 LOC. It contains all `ProxyState` methods for model lifecycle (load/unload/evict), idle timeout checking, TTS backend management, and compaction backend management in one monolithic `impl` block. This makes it hard to navigate and understand any single concern without scrolling through unrelated code. The split extracts each concern into its own file while keeping the `impl ProxyState` block pattern.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (reduce from 1,410 to ~450 LOC)
- Create: `crates/tama-core/src/proxy/lifecycle/idle_timeout.rs` (~340 LOC)
- Create: `crates/tama-core/src/proxy/lifecycle/tts.rs` (~280 LOC)
- Create: `crates/tama-core/src/proxy/lifecycle/compaction.rs` (~200 LOC)
- Keep unchanged: `crates/tama-core/src/proxy/lifecycle/tests.rs`

**What to implement:**

The file currently has the following structure (all inside `impl ProxyState`):
- Lines 18-347: `load_model` — keep in mod.rs
- Lines 348-449: `evict_lru_if_needed` — keep in mod.rs
- Lines 450-534: `unload_model` — keep in mod.rs
- Lines 535-874: `check_idle_timeouts` — extract to `idle_timeout.rs`
- Lines 875-1080: `load_tts_backend` — extract to `tts.rs`
- Lines 1081-1139: `unload_tts_backend` — extract to `tts.rs`
- Lines 1140-1153: `get_tts_server` — extract to `tts.rs`
- Lines 1154-1351: `load_compaction_backend` — extract to `compaction.rs`
- Lines 1352-1372: `_resolve_gpu_device` — keep in mod.rs (helper for load_model)
- Lines 1374-1390: `resolve_gpu_device_to_backend_name` — keep in mod.rs (helper for load_model)

Each extracted file must contain its own `impl ProxyState` block with the moved methods. The imports needed by each method must be copied into the new file. The parent `mod.rs` must declare each new module with `mod idle_timeout;`, `mod tts;`, `mod compaction;`.

**Important details:**
- All new files are private modules (no `pub` on the `mod` declaration) since `lifecycle` itself is private in `proxy/mod.rs`
- Each new file needs its own imports — copy only what each method actually uses
- The `impl ProxyState` pattern is used (not a separate struct), so each file will have `impl crate::proxy::types::ProxyState { ... }` or use a `use` import for `ProxyState`
- The existing `tests.rs` file must remain unchanged — it tests through the public API which doesn't change
- Do NOT modify any method signatures or logic — pure cut-and-paste extraction

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/lifecycle/idle_timeout.rs`:
  - Add exact imports:
    ```rust
    use std::time::{Duration, Instant};
    use tracing::{debug, info, warn};
    use crate::proxy::types::{ModelState, ProxyState};
    use super::process::{
        check_health, force_kill_process_group,
        is_process_alive, is_process_group_alive, kill_process_group,
    };
    ```
  - Add `impl ProxyState { pub async fn check_idle_timeouts(&self) -> Vec<String> { ... } }` with the exact method body from lines 535-874
  - Note: `check_idle_timeouts` is `pub async fn` and remains callable as `state.check_idle_timeouts()` from `server/mod.rs` via method resolution, regardless of the private submodule — no visibility changes needed
- [ ] Create `crates/tama-core/src/proxy/lifecycle/tts.rs`:
  - Add imports needed by `load_tts_backend`, `unload_tts_backend`, `get_tts_server`
  - Add `impl ProxyState { ... }` with all three methods (lines 875-1153)
- [ ] Create `crates/tama-core/src/proxy/lifecycle/compaction.rs`:
  - Add imports needed by `load_compaction_backend`
  - Add `impl ProxyState { pub async fn load_compaction_backend(&self) -> Result<()> { ... } }` with the exact method body from lines 1154-1351
- [ ] Modify `crates/tama-core/src/proxy/lifecycle/mod.rs`:
  - Add `mod idle_timeout;`, `mod tts;`, `mod compaction;` at the top (after existing imports, before the `impl` block)
  - Remove the three extracted methods (`check_idle_timeouts`, `load_tts_backend`, `unload_tts_backend`, `get_tts_server`, `load_compaction_backend`) from the `impl ProxyState` block
  - Keep: `load_model`, `evict_lru_if_needed`, `unload_model`, `_resolve_gpu_device`, `resolve_gpu_device_to_backend_name`
  - Remove any imports no longer needed in mod.rs (only keep what remaining methods use)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix import paths and re-run before continuing.
- [ ] Run `cargo test --package tama-core -- lifecycle`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix warnings (likely unused imports in mod.rs).
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: split proxy/lifecycle/mod.rs into idle_timeout, tts, compaction modules"

**Acceptance criteria:**
- [ ] `mod.rs` is under 600 LOC
- [ ] All 4 files compile without warnings
- [ ] All existing lifecycle tests pass
- [ ] No method signatures changed
- [ ] No behavior changed (pure extraction)

---

### Task 2: Split `api/models/crud/mod.rs` (1,222 → ~300 LOC)

**Context:**
The `crud/mod.rs` file in tama-web contains the `ModelBody` struct, the `apply_model_body` function, validation helpers, and 22 inline tests. The tests alone are ~900 LOC — nearly 75% of the file. The pattern already established in `lifecycle/` (separate `tests.rs`) should be applied here. The production code (~300 LOC) is already reasonably focused, so only the tests need extraction.

**Files:**
- Modify: `crates/tama-web/src/api/models/crud/mod.rs` (reduce from 1,222 to ~300 LOC)
- Create: `crates/tama-web/src/api/models/crud/tests.rs` (~900 LOC)

**What to implement:**

The file currently has:
- Lines 1-28: Imports
- Lines 29-72: `ModelBody` struct definition
- Lines 74-197: `apply_model_body` function
- Lines 198-210: `is_valid_repo_id` function
- Lines 212-291: `validate_model_body` function
- Lines 294-1222: `#[cfg(test)] mod tests { ... }` with 21 tests

Extract the entire `#[cfg(test)]` module into a separate `tests.rs` file. The test module uses `use super::*` to access production code, plus additional test-specific imports.

**Important details:**
- The existing `mod.rs` already has sibling files: `create.rs`, `delete.rs`, `rename.rs`, `update.rs`
- The test module starts at line 294 with `#[cfg(test)]` and ends at line 1222
- Tests use `use super::*` which will work correctly when moved to `tests.rs` (it will import from `mod.rs`)
- Copy all test-specific imports (like `use std::collections::BTreeMap;`, `use tama_core::config::{ModelConfig, QuantEntry};`) into the new file
- Remove the `#[cfg(test)]` attribute when moving to a separate file (the file itself is only compiled for tests)

**Steps:**
- [ ] Create `crates/tama-web/src/api/models/crud/tests.rs`:
  - Copy everything from line 293 to end of file (inside the `#[cfg(test)]` block)
  - Replace `use super::*` with the exact imports needed: `use super::*;` (this works from tests.rs too), plus any additional imports the tests need
  - Do NOT include `#[cfg(test)]` wrapper — the file itself is gated by the `mod tests;` declaration
- [ ] Modify `crates/tama-web/src/api/models/crud/mod.rs`:
  - Add `#[cfg(test)] mod tests;` at the bottom of the file (after all production code)
  - Remove the entire `#[cfg(test)] mod tests { ... }` block (lines 292-1222)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix import paths and re-run.
- [ ] Run `cargo test --package tama-web -- crud`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: extract api/models/crud tests into separate tests.rs"

**Acceptance criteria:**
- [ ] `mod.rs` is under 350 LOC
- [ ] All 22 CRUD tests still pass
- [ ] No test logic changed
- [ ] No production code changed

---

### Task 3: Split `proxy/server/mod.rs` (1,172 → ~500 LOC)

**Context:**
The `server/mod.rs` file contains `ProxyServer` construction, stale process cleanup, idle timeout checker task, system metrics collection task, router setup, and 11 inline integration tests. The metrics collection (~175 LOC) is a self-contained concern that can be extracted. The tests (~650 LOC) should be moved to a separate file following the established pattern.

**Files:**
- Modify: `crates/tama-core/src/proxy/server/mod.rs` (reduce from 1,172 to ~500 LOC)
- Create: `crates/tama-core/src/proxy/server/metrics.rs` (~175 LOC)
- Create: `crates/tama-core/src/proxy/server/tests.rs` (~650 LOC)

**What to implement:**

The file currently has:
- Lines 1-8: Module declarations (`pub mod listener;`, `pub mod router;`) and imports
- Lines 9-18: `ProxyServer` struct definition
- Lines 19-262: `impl ProxyServer` — `new` method (includes metrics task spawn inside it)
- Lines 264-337: `cleanup_stale_processes` (private method)
- Lines 339-359: `start_idle_timeout_checker` (private method)
- Lines 361-392: `row_into_sample` (private function)
- Lines 394-420: `into_router`, `into_unified_router`, `run` methods
- Lines 449-1172: `#[cfg(test)] mod tests { ... }` with 11 tests

Extract metrics collection (`row_into_sample` + the metrics collection logic currently inline in `new`) into `metrics.rs`. Extract all tests into `tests.rs`.

**Important details:**
- The metrics task is spawned inside `ProxyServer::new` — the spawn call stays in mod.rs, but the task closure body and `row_into_sample` helper move to metrics.rs
- **CRITICAL: `history_buf` must move with the metrics task.** The buffer is created as a local variable in `new` at lines 102–110, seeded from the DB, then captured by the spawned closure. To extract into a free function, the agent must move the buffer creation AND the DB seeding logic into `start_metrics_collector`. The spawned closure must capture this local `history_buf` by move.
- `start_idle_timeout_checker` stays in mod.rs (it's small and tied to server lifecycle)
- `cleanup_stale_processes` stays in mod.rs
- The metrics.rs file should export a free function `pub fn start_metrics_collector(state: Arc<ProxyState>) -> tokio::task::JoinHandle<()>`. Unlike Tasks 1's `impl ProxyState` pattern, this uses a free function because the metrics task captures local state (history buffer, network handles) that doesn't fit the `&self` method signature.
- The existing `mod.rs` already has sibling files: `listener.rs`, `router.rs`

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/server/metrics.rs`:
  - Add `pub fn start_metrics_collector(state: Arc<crate::proxy::ProxyState>) -> tokio::task::JoinHandle<()>` — extract the metrics task spawn logic from `ProxyServer::new`
  - **Inside `start_metrics_collector`:** Create `history_buf` locally and seed it from DB using the exact logic from `ProxyServer::new` lines 102–110 (move verbatim):
    ```rust
    let mut history_buf: VecDeque<crate::gpu::MetricSample> = VecDeque::with_capacity(450);
    if let Some(seed_conn) = state.open_db() {
        if let Ok(rows) = crate::db::queries::get_recent_system_metrics(&seed_conn, 450) {
            for row in rows {
                history_buf.push_back(row_into_sample(&row));
            }
        }
    }
    ```
  - The spawned closure must capture this local `history_buf` by move (same as original)
  - Add `fn row_into_sample(row: &crate::db::queries::SystemMetricsRow) -> crate::gpu::MetricSample { ... }` — copy exact body from lines 361-392
  - The function signature and body must match exactly what's called from `new`
- [ ] Create `crates/tama-core/src/proxy/server/tests.rs`:
  - Copy everything from line 449 to end of file (inside the `#[cfg(test)]` block)
  - Use `use super::*;` for accessing production code
  - Do NOT include `#[cfg(test)]` wrapper
- [ ] Modify `crates/tama-core/src/proxy/server/mod.rs`:
  - Add `mod metrics;` at the top with other module declarations
  - Add `#[cfg(test)] mod tests;` at the bottom
  - Replace inline metrics task spawn in `new` with call to `metrics::start_metrics_collector(self.state.clone())`
  - Remove `row_into_sample` function (moved to metrics.rs)
  - Remove entire `#[cfg(test)]` block
  - Keep: `ProxyServer` struct, `new`, `cleanup_stale_processes`, `start_idle_timeout_checker`, `into_router`, `into_unified_router`, `run`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix import paths and re-run.
- [ ] Run `cargo test --package tama-core -- server`
  - Did all 11 tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: split proxy/server/mod.rs into metrics module and tests.rs"

**Acceptance criteria:**
- [ ] `mod.rs` is under 550 LOC
- [ ] All 11 server tests still pass
- [ ] Metrics collection behavior unchanged
- [ ] No method signatures changed on `ProxyServer`

---

### Task 4: Split `proxy/tama_handlers/pull/download.rs` (1,096 → ~550 LOC)

**Context:**
The `download.rs` file contains two large functions: `start_download_from_queue` (~574 LOC) which is the queue processor entry point, and `run_verification` (~500 LOC) which performs post-download integrity checks. These are distinct concerns — downloading vs verifying — and each is large enough to warrant its own file.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs` (reduce from 1,096 to ~574 LOC)
- Create: `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs` (~500 LOC)
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/mod.rs` (add module declaration)

**What to implement:**

The file currently has:
- Lines 1-15: Imports
- Lines 16-589: `pub async fn start_download_from_queue(...)` — the queue processor entry point
- Lines 590-1096: `async fn run_verification(...)` — post-download integrity verification

Extract `run_verification` into `verify.rs`. The function is `pub(crate)` or private (not `pub`), so it's only called from within the `pull` module.

**Important details:**
- `run_verification` is called from `start_download_from_queue` — the call site must be updated to use `super::verify::run_verification(...)` or `crate::proxy::tama_handlers::pull::verify::run_verification(...)`
- The `pull/mod.rs` already declares `pub mod download;` and `pub mod handlers;` — add `mod verify;` (private since verify is internal)
- Copy only the imports that `run_verification` actually uses into verify.rs
- Remove unused imports from download.rs after extraction

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs`:
  - Add imports needed by `run_verification` (copy from the top of download.rs, keeping only what verify uses)
  - Add `pub(super) async fn run_verification(...)` with the exact function body from lines 590-1096
  - Use `pub(super)` visibility so it's accessible from sibling `download.rs` via `super::verify::run_verification`
- [ ] Modify `crates/tama-core/src/proxy/tama_handlers/pull/download.rs`:
  - Remove `run_verification` function (lines 590-1096)
  - Update the call site to `super::verify::run_verification(...)`
  - Remove imports no longer needed (only keep what `start_download_from_queue` uses)
- [ ] Modify `crates/tama-core/src/proxy/tama_handlers/pull/mod.rs`:
  - Add `mod verify;` (private module)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix import paths and re-run.
- [ ] Run `cargo test --package tama-core -- pull`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: extract post-download verification into proxy/tama_handlers/pull/verify.rs"

**Acceptance criteria:**
- [ ] `download.rs` is under 600 LOC
- [ ] `verify.rs` is created with `run_verification`
- [ ] All pull-related tests pass
- [ ] No behavior changed (pure extraction)

---

## Post-Implementation Verification

After all 4 tasks are complete:

1. Run `cargo build --workspace` — must succeed
2. Run `cargo test --workspace` — all tests must pass
3. Run `cargo clippy --workspace -- -D warnings` — must pass
4. Run `cargo fmt --all` — must succeed
5. Verify no file exceeds 700 LOC:
   ```bash
   find crates -name '*.rs' -not -path '*/target/*' | xargs wc -l | sort -rn | head -10
   ```
6. Verify total LOC is roughly unchanged (pure extraction):
   ```bash
   find crates -name '*.rs' -not -path '*/target/*' | xargs wc -l | tail -1
   ```
