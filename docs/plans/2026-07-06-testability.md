# Testability Plan

**Goal:** Add trait abstractions for backend lifecycle testing and unit tests for update checker and download queue processor.

**Architecture:** Two independent tasks. Task 1 adds trait abstractions (HealthChecker, ProcessSpawner, PortAllocator) to make lifecycle code testable. Task 2 adds tests for update checker and download queue processor.

**Tech Stack:** Rust, tokio, wiremock, tempfile, rusqlite (in-memory)

---

### Task 1: Backend lifecycle trait abstractions + tests

**Context:**
Finding #14 from the audit. ~900 lines of critical process management code (spawn, health poll, idle timeout, dead PID detection, auto-restart, graceful shutdown) has zero test coverage. No trait abstractions for subprocess execution, port allocation, or health checking — making it impossible to test without real processes.

This task adds trait abstractions with default impls calling real functions and test impls returning mock data. Then adds tests for the 3-phase idle timeout logic and load_model pipeline.

**Files:**
- Create: `crates/tama-core/src/proxy/lifecycle/traits.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (use traits, add tests)
- Modify: `crates/tama-core/src/proxy/lifecycle/idle_timeout.rs` (use traits)
- Modify: `crates/tama-core/src/proxy/lifecycle/compaction.rs` (use traits)
- Modify: `crates/tama-core/src/proxy/lifecycle/tts.rs` (use traits)

**What to implement:**

1. Create `traits.rs` with trait abstractions:
   ```rust
   #[async_trait::async_trait]
   pub trait HealthChecker: Send + Sync {
       async fn check_health(&self, url: &str, timeout: Option<u64>) -> bool;
   }
   
   #[async_trait::async_trait]
   pub trait ProcessSpawner: Send + Sync {
       async fn spawn(
           &self,
           cmd: &str,
           args: &[String],
           env: &[(&str, String)],
           cwd: Option<&std::path::Path>,
       ) -> anyhow::Result<SpawnedProcess>;
       
       async fn kill_process_group(&self, pid: u32) -> anyhow::Result<()>;
       async fn force_kill_process_group(&self, pid: u32) -> anyhow::Result<()>;
   }
   
   pub trait PortAllocator: Send + Sync {
       fn allocate_port(&self) -> anyhow::Result<u16>;
   }
   
   pub trait ProcessChecker: Send + Sync {
       fn is_process_alive(&self, pid: u32) -> bool;
       fn is_process_group_alive(&self, pid: u32) -> bool;
   }
   
   #[derive(Debug, Clone)]
   pub struct SpawnedProcess {
       pub pid: u32,
   }
   ```

2. Provide default impls that call the real functions:
   ```rust
   #[async_trait::async_trait]
   impl HealthChecker for () {
       async fn check_health(&self, url: &str, timeout: Option<u64>) -> bool {
           crate::proxy::lifecycle::check_health(url, timeout).await
       }
   }
   // ... similar for other traits
   ```

3. Provide mock impls for testing:
   ```rust
   #[cfg(test)]
   pub struct MockHealthChecker {
       pub responses: std::sync::Mutex<Vec<bool>>,
   }
   
   #[async_trait::async_trait]
   impl HealthChecker for MockHealthChecker {
       async fn check_health(&self, _url: &str, _timeout: Option<u64>) -> bool {
           *self.responses.lock().unwrap().drain(..1).next().unwrap_or(false)
       }
   }
   ```

4. Update `load_model()`, `unload_model()`, `check_idle_timeouts()` to accept trait objects (or use generic parameters with default `()` impl).

5. Add tests:
   - Test 3-phase idle timeout logic (collect → confirm → mutate)
   - Test load_model pipeline with mock health checker
   - Test unload_model graceful shutdown
   - Test dead PID detection

**Steps:**
- [ ] Create `traits.rs` with `HealthChecker`, `ProcessSpawner`, `PortAllocator`, `ProcessChecker` traits
- [ ] Implement default impls calling real functions
- [ ] Implement mock impls for testing
- [ ] Update `load_model()` to accept generic `H: HealthChecker` parameter (default `()`)
- [ ] Update `check_idle_timeouts()` to accept trait parameters
- [ ] Write test: 3-phase idle timeout logic with mock health checker
- [ ] Write test: load_model pipeline with mock health checker
- [ ] Write test: unload_model graceful shutdown
- [ ] Write test: dead PID detection
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo test --workspace -- proxy::lifecycle`
  - Did all lifecycle tests pass? If not, fix failures and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "test: add trait abstractions and tests for backend lifecycle"

**Acceptance criteria:**
- [ ] `HealthChecker`, `ProcessSpawner`, `PortAllocator`, `ProcessChecker` traits defined
- [ ] Default impls call real functions (no behavior change in production)
- [ ] Mock impls available for tests
- [ ] At least 4 tests covering: idle timeout, load_model, unload_model, dead PID detection
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 2: Update checker + download queue processor tests

**Context:**
Finding #25 from the audit. `check_backend()`, `check_model()`, `run_check()`, `GgufListingCache` are untested (async, network-dependent). `queue_processor_loop()` (~150 lines of CAS-based claiming, dead task detection, stale recovery) is completely untested.

**Files:**
- Modify: `crates/tama-core/src/updates/checker.rs` (add tests module)
- Modify: `crates/tama-core/src/proxy/download_queue.rs` (add tests for queue_processor_loop)

**What to implement:**

1. **Update checker tests** (in `updates/checker.rs` `#[cfg(test)]` module):
   - Test `GgufListingCache` TTL behavior (cache hit, cache miss, cache expiry)
   - Test `check_backend()` with `wiremock` server mocking HF API
   - Test `check_model()` with mocked GGUF listing
   - Test `determine_update_status` edge cases (already have some, add more)

2. **Download queue processor tests** (in `proxy/download_queue.rs` `#[cfg(test)]` module):
   - Test `queue_processor_loop` with in-memory DB:
     - Dequeue and mark running (CAS)
     - Dead task detection and recovery
     - Stale item recovery
     - Multiple items in queue
   - Test `DownloadQueueService` edge cases:
     - Concurrent enqueue/dequeue
     - Status transitions (queued → running → completed/failed)

**Steps:**
- [ ] Add `wiremock` to `tama-core/Cargo.toml` dev-dependencies (if not already present)
- [ ] Write test: `GgufListingCache` TTL (cache hit, miss, expiry)
- [ ] Write test: `check_backend()` with wiremock HF API
- [ ] Write test: `check_model()` with mocked GGUF listing
- [ ] Write test: `queue_processor_loop` dequeue and CAS with in-memory DB
- [ ] Write test: `queue_processor_loop` dead task detection and recovery
- [ ] Write test: `queue_processor_loop` stale item recovery
- [ ] Write test: `DownloadQueueService` concurrent enqueue/dequeue
- [ ] Write test: `DownloadQueueService` status transitions
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo test --workspace -- updates::checker`
  - Did all checker tests pass? If not, fix failures and re-run
- [ ] Run `cargo test --workspace -- proxy::download_queue`
  - Did all queue tests pass? If not, fix failures and re-run
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Commit with message: "test: add tests for update checker and download queue processor"

**Acceptance criteria:**
- [ ] At least 3 tests for update checker (cache TTL, check_backend, check_model)
- [ ] At least 4 tests for download queue processor (CAS, dead task, stale recovery, concurrent)
- [ ] Tests use wiremock for network calls and in-memory DB for queue
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass
