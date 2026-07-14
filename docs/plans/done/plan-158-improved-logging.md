# Improved Logging Plan

**Goal:** Fix stale logs bug and upgrade to non-blocking JSON file logging with pretty console output, respecting config `log_level` and `logs_dir`, plus GPU structured fields on inference events.

**Architecture:** Two-layer `tracing-subscriber` via `registry()`: (1) pretty console layer → stdout, (2) JSON file layer → `tracing-appender` NonBlockingFileWriter → `tama.log`. Size-based rotation checked on startup (10MB × 5 files). GPU info added as structured fields on proxy/inference events.

**Tech Stack:** `tracing-subscriber` (existing), `tracing-appender` (new dep), `tracing` (existing)

---

## Task 1: Add `tracing-appender` Dependency + Enable JSON Feature

**Context:** The current logging setup writes only to stdout, causing `tama.log` to be stale. We need `tracing-appender` for non-blocking file writes and the `json` feature on `tracing-subscriber` for structured JSON output. This task adds both to workspace and binary crate configs.

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/tama/Cargo.toml`

**What to implement:**
1. In workspace `Cargo.toml`, add `tracing-appender = "0.2"` to `[workspace.dependencies]` section (after `tracing-subscriber`).
2. In workspace `Cargo.toml`, add `"json"` to the existing `tracing-subscriber` features array, changing it from `features = ["env-filter"]` to `features = ["env-filter", "json"]`. The `.json()` method on `fmt::layer()` is gated behind this feature.
3. In `crates/tama/Cargo.toml`, add `tracing-appender = { workspace = true, optional = true }` under the existing `tracing-subscriber` line in `[dependencies]`, gated behind `ssr` feature (same as tracing).
4. In `crates/tama/Cargo.toml`, add `"dep:tracing-appender"` to the `ssr` feature list.

**Steps:**
- [ ] Add `tracing-appender = "0.2"` to workspace `[workspace.dependencies]` in `Cargo.toml`
- [ ] Change workspace `tracing-subscriber` line from `features = ["env-filter"]` to `features = ["env-filter", "json"]`
- [ ] Add `tracing-appender = { workspace = true, optional = true }` to `crates/tama/Cargo.toml` `[dependencies]` (after `tracing-subscriber` line)
- [ ] Add `"dep:tracing-appender"` to the `ssr` feature array in `crates/tama/Cargo.toml`
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "chore: add tracing-appender dep and enable tracing-subscriber json feature"

**Acceptance criteria:**
- [ ] `cargo check --package tama` succeeds with no errors
- [ ] `tracing-appender` is available as a workspace dep and optional ssr dep in tama crate
- [ ] `tracing-subscriber` has the `json` feature enabled in workspace deps

---

## Task 2: Replace Tracing Init with Two-Layer Subscriber

**Context:** The current `main.rs` initializes tracing with stdout-only output and ignores the config `log_level`. This task replaces it with a two-layer subscriber that writes pretty output to console AND JSON to file, respects the configured log level, and uses `tracing-appender` for non-blocking writes. The `WorkerGuard` must be kept alive for the program's lifetime — if dropped, the background writer thread exits and all subsequent file logs are silently lost.

**Files:**
- Modify: `crates/tama/src/main.rs`

**What to implement:**

Replace the tracing initialization block (lines ~24-27) AND move config loading before it:

```rust
// Before:
tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .init();

// Load configuration
let config = Config::load()?;
```

With:
```rust
// Load configuration FIRST (needed for log_level and logs_dir)
let config = Config::load()?;

// Initialize tracing with two layers: pretty console + JSON file
// The guard must stay in scope for the program's lifetime to keep the
// background writer thread alive — if dropped, file logging silently stops.
let _log_guard = init_tracing(&config)?;
```

Add a new function at the bottom of `main.rs`:

```rust
/// Initialize tracing with two layers:
/// - Console: pretty-formatted output to stdout
/// - File: JSON lines written non-blockingly to tama.log with size-based rotation
///
/// Returns the WorkerGuard that must be kept alive for the program's lifetime.
fn init_tracing(config: &Config) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

    // Determine log level from config
    let log_level: tracing::Level = config.general.log_level.into();
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(log_level.into())
        .from_env_lossy();

    // Ensure logs directory exists
    let logs_dir = config.logs_dir().with_context(|| {
        "Failed to resolve logs directory from config"
    })?;
    std::fs::create_dir_all(&logs_dir).with_context(|| {
        format!("Failed to create logs directory: {}", logs_dir.display())
    })?;

    // Size-based rotation check on startup (reuses constants from logging module)
    let log_path = logs_dir.join("tama.log");
    if log_path.exists() {
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > tama_core::logging::MAX_LOG_SIZE {
                tama_core::logging::rotate_logs(&logs_dir, "tama", tama_core::logging::MAX_LOG_FILES)?;
            }
        }
    }

    // Open non-blocking file writer for JSON output
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    // Build two-layer subscriber
    tracing_subscriber::registry()
        .with(fmt::layer()
            .with_target(false)
            .with_file(false)
            .with_line_number(false)
            .with_filter(env_filter.clone()))
        .with(fmt::layer()
            .json()
            .with_writer(non_blocking)
            .with_filter(env_filter))
        .init();

    Ok(guard)
}
```

### Making logging module items public

Before Task 2 can compile, `logging.rs` needs to expose its rotation constants and function. Add this step to Task 2 (or do it inline):

In `crates/tama-core/src/logging.rs`, change:
- `const MAX_LOG_SIZE: u64` → `pub const MAX_LOG_SIZE: u64`
- `const MAX_LOG_FILES: usize` → `pub const MAX_LOG_FILES: usize`
- `fn rotate_logs(logs_dir: &Path, profile: &str) -> Result<()>` → `pub fn rotate_logs(logs_dir: &Path, profile: &str) -> Result<()>`

These are already used internally by `open_log()` — making them public lets `main.rs` reuse them for tama.log rotation without duplicating constants or logic.

The order in `main()` should be:
1. Load config (`Config::load()?)`)
2. Init tracing with config (`init_tracing(&config)`) — returns guard kept in `_log_guard`
3. Rest of startup (HF token, DB, proxy state, server)

Also remove the redundant `std::fs::create_dir_all(&logs_dir)` block from the `#[cfg(feature = "ssr")]` section (lines ~115-118) since `init_tracing` already creates it.

**Import changes:** Change existing `use anyhow::Result;` to `use anyhow::{Context, Result};` (add `Context` for `.with_context()` calls).

**Steps:**
- [ ] In `crates/tama-core/src/logging.rs`, make rotation items public:
  - `const MAX_LOG_SIZE` → `pub const MAX_LOG_SIZE`
  - `const MAX_LOG_FILES` → `pub const MAX_LOG_FILES`
  - `fn rotate_logs(...)` → `pub fn rotate_logs(...)`
- [ ] Move `let config = Config::load()?;` before the tracing initialization in `main.rs`
- [ ] Replace the `tracing_subscriber::fmt()...init()` block with:
  ```rust
  let _log_guard = init_tracing(&config)?;
  ```
- [ ] Change `use anyhow::Result;` to `use anyhow::{Context, Result};`
- [ ] Implement `init_tracing()` function that returns `Result<WorkerGuard>` as described above (uses `tama_core::logging::rotate_logs`, `MAX_LOG_SIZE`, `MAX_LOG_FILES` — no duplicate rotation code)
- [ ] Remove the redundant `std::fs::create_dir_all(&logs_dir)` block from `#[cfg(feature = "ssr")]` section (lines ~115-118)
- [ ] Run `cargo build --package tama`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "fix: wire tracing to write JSON logs to file with config-respecting log level"

**Acceptance criteria:**
- [ ] `cargo build --package tama` succeeds
- [ ] After starting tama, `tama.log` is created in the configured logs directory
- [ ] Log lines appear in `tama.log` as valid JSON (one object per line)
- [ ] Console output remains pretty-formatted (not JSON)
- [ ] The configured `log_level` from config controls both layers' filtering
- [ ] `_log_guard` binding stays in scope for the entire `main()` function (file logging works throughout program lifetime)

---

## Task 3: Add GPU Structured Fields to Inference Events

**Context:** GPU device info is useful for debugging inference issues but should only appear on relevant events. This task adds structured `gpu` fields to tracing calls in proxy forwarding and model lifecycle code, so JSON log lines include GPU context where it matters.

**Important:** All lookups that return references from `RwLockReadGuard` must `.clone()` the value to own it — otherwise the borrow checker rejects the code (temporary guard dropped while reference is still used). The chat handler's routing log is NOT modified because `model_name` at that point may be an alias or raw repo_id, and resolving the correct `ModelConfig` requires `resolve_backends_for_model()` which needs both config and model_configs read locks — too complex for a simple log line. GPU info is instead captured in lifecycle (load/unload) and forwarding (where we have the resolved backend).

**Files:**
- Modify: `crates/tama-core/src/proxy/forward/request.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`

**What to implement:**

### 3a. Forward request — add GPU field to forwarding log

In `crates/tama-core/src/proxy/forward/request.rs`, find the line:
```rust
info!("Forwarding request to: {}", target_uri);
```
(around line 155, after `target_uri` is computed).

Replace with GPU-aware logging. **Use `backend_name` as the lookup key** (NOT `model_name`) — `backend_name` is the config_key format used in `model_configs` HashMap (e.g., `"owner--repo"`), while `model_name` is a resolved repo_id or alias (e.g., `"owner/repo"`). The `backend_name` parameter is already available as a function argument.

```rust
// Resolve GPU device from model config using backend_name (the correct HashMap key).
// Clone to own the value — can't borrow from temporary RwLockReadGuard.
let gpu_info: String = state
    .model_configs
    .read()
    .await
    .get(backend_name)
    .and_then(|mc| mc.gpu_device.clone())
    .unwrap_or_else(|| "default".to_string());

info!(gpu = %gpu_info, "Forwarding request to: {}", target_uri);
```

Note: The `model_configs.read().await` creates a temporary guard. `.get(backend_name)` returns `Option<&ModelConfig>` borrowing from it. `.and_then(|mc| mc.gpu_device.clone())` produces `Option<String>` (owned). This compiles because the owned `String` doesn't borrow from the dropped guard.

### 3b. Lifecycle — add GPU field to model load events

In `crates/tama-core/src/proxy/lifecycle/mod.rs`, find the "Loading model" debug event at line ~66:
```rust
debug!("Loading model: {}", model_name);
```

**Do NOT modify this line** — `server_config` is not in scope yet (it's resolved later at line ~144). Instead, add a new log line AFTER `server_config` is resolved. Find the block around line 158-160 where `gpu_variant` and `default_args` are resolved:

```rust
let gpu_variant = server_config.gpu_variant.as_deref().unwrap_or("cpu");
let default_args = manager.get_default_args(&server_config.backend, gpu_variant);
```

Add after this block (before the model loading continues):
```rust
tracing::debug!(
    gpu = %server_config.gpu_device.as_deref().unwrap_or("default"),
    "Loading model '{}' with backend '{}'",
    model_name,
    server_config.backend
);
```

Note: The GPU device is captured as a structured field only — the message body doesn't repeat it (the structured `gpu` field in JSON output provides the context).

### 3c. Lifecycle — add GPU field to model unload events

In `crates/tama-core/src/proxy/lifecycle/mod.rs`, find the `unload_model` function (line ~590). The existing log lines are:
- Line 591: `debug!("Unloading backend: {}", backend_name);`
- Line 629: `info!("Stopping backend '{}'", backend_name);`
- Line 676: `info!("Backend '{}' unloaded", backend_name);`

Add GPU info to the "Stopping backend" log. Must look up from `model_configs` using `backend_name` as key (which IS a valid config key — see `evict_lru_if_needed`). Clone to own:

```rust
// Before: info!("Stopping backend '{}'", backend_name);
// After:
let gpu_info: String = self
    .model_configs
    .read()
    .await
    .get(backend_name)
    .and_then(|mc| mc.gpu_device.clone())
    .unwrap_or_else(|| "default".to_string());
info!(gpu = %gpu_info, "Stopping backend '{}'", backend_name);
```

Similarly for the "Backend unloaded" log (line 676):
```rust
// Before: info!("Backend '{}' unloaded", backend_name);
// After (reuse gpu_info from above — move the lookup before it's needed):
info!(gpu = %gpu_info, "Backend '{}' unloaded", backend_name);
```

**Steps:**
- [ ] In `forward/request.rs`, replace the `info!("Forwarding request to: {}", target_uri)` line with GPU-aware version that clones gpu_device from model_configs
- [ ] In `lifecycle/mod.rs`, add a new `tracing::debug!` after `server_config` is resolved (line ~158) with gpu device info — do NOT modify the existing "Loading model" debug at line 66
- [ ] In `lifecycle/mod.rs`, add GPU info lookup before "Stopping backend" log and include in both "Stopping" and "unloaded" logs
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add gpu structured field to forwarding and lifecycle log events"

**Acceptance criteria:**
- [ ] `cargo check --workspace` succeeds
- [ ] JSON log lines for forwarding contain a `"gpu"` field (e.g., `{"gpu": "cuda:0", "message": "Forwarding request to: ..."}`)
- [ ] JSON log lines for model load/unload contain GPU device info
- [ ] GPU field defaults to `"default"` when no gpu_device is configured
- [ ] Console output still shows the message text (structured fields are JSON-only)

---

## Task 4: Handle JSON Lines in Logs API

**Context:** After Task 2, `tama.log` contains JSON objects (one per line) instead of human-readable text. The logs API (`/tama/v1/logs`) reads raw lines via `tail_lines()` and returns them to the web UI, which renders each line verbatim in a `<div>`. Without this fix, users will see raw JSON like `{"timestamp":"...","level":"INFO","fields":{"message":"Starting tama..."}}` instead of readable log text. This task updates the logs API to parse JSON lines and return human-readable formatted strings, keeping the existing API contract and UI working unchanged.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs`
- Modify: `crates/tama-core/src/logging.rs` (add helper function)

**What to implement:**

### 4a. Add a JSON-to-human-readable conversion helper in `logging.rs`

Add a new public function to `crates/tama-core/src/logging.rs`:

```rust
/// Parse a JSON log line (from tracing-subscriber's json format) and return
/// a human-readable string: "2024-01-01T12:00:00.000000Z INFO target: message"
///
/// Returns the original line unchanged if it's not valid JSON or doesn't contain
/// the expected fields (e.g., backend logs that are plain text).
pub fn format_log_line(line: &str) -> String {
    // Fast path: if it doesn't start with '{', it's a plain text line
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return line.to_string();
    }

    // Try to parse as JSON and extract fields
    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let timestamp = v.get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        let level = v.get("level")
            .and_then(|l| l.as_str())
            .unwrap_or("");
        let target = v.get("target")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        // Message is nested under fields.message in tracing-subscriber json format
        let message = v.pointer("/fields/message")
            .and_then(|m| m.as_str())
            .unwrap_or(v.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or(""));

        if message.is_empty() {
            // Fallback: return original line if we couldn't extract a message
            return line.to_string();
        }

        format!("{} {} {}: {}", timestamp, level, target, message)
    } else {
        // Not valid JSON — return as-is (e.g., backend logs)
        line.to_string()
    }
}
```

Note: This requires adding `serde_json` to the function scope. Check if it's already imported in `logging.rs`. If not, add `use serde_json;` (it's already a workspace dependency).

### 4b. Apply formatting in the logs API handler

In `crates/tama-core/src/proxy/tama_handlers/backend_logs.rs`, the `handle_all_logs` function reads lines:

```rust
let lines =
    match tokio::task::spawn_blocking(move || logging::tail_lines(&tama_path, n)).await
    {
        Ok(Ok(l)) => l,
        _ => Vec::new(),
    };
```

After getting the lines, format each one:
```rust
let lines: Vec<String> = lines.into_iter()
    .map(|line| logging::format_log_line(&line))
    .collect();
```

Apply the same transformation to backend log lines (the second `tail_lines` call in the same function, around line 106).

Also apply to the endpoint in `crates/tama/src/api.rs` (line ~45-50) if it exists — this is a separate logs endpoint in the tama crate that also reads from log files.

**Steps:**
- [ ] Add `pub fn format_log_line(line: &str) -> String` to `crates/tama-core/src/logging.rs` as described above
- [ ] In `handle_all_logs` in `backend_logs.rs`, map each line through `logging::format_log_line()` before returning
- [ ] Apply the same formatting to the tama crate's logs endpoint in `crates/tama/src/api.rs` (if it reads from log files directly)
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "fix: parse JSON log lines for human-readable display in logs API"

**Acceptance criteria:**
- [ ] `cargo check --workspace` succeeds
- [ ] The `/tama/v1/logs` API returns human-readable strings (not raw JSON) for tama.log entries
- [ ] Backend log lines (plain text, not JSON) pass through unchanged
- [ ] The web UI displays readable log text (no user-facing changes needed)

---

## Task 5: Clean Up Deprecated Logging Code

**Context:** `logging.rs` contains deprecated functions (`init()`, `init_with_file()`, `MultiWriter`) that are no longer used since tracing init moved to `main.rs`. This task removes dead code while keeping functions still in use.

**CRITICAL:** Do NOT remove `rotate_logs()` — it is called by `open_log()` which is still used by backend lifecycle and process.rs. Only remove the truly unused top-level init functions and MultiWriter.

**Files:**
- Modify: `crates/tama-core/src/logging.rs`

**What to implement:**

Remove the following from `crates/tama-core/src/logging.rs`:
1. `pub fn init()` — deprecated, not used anywhere
2. `pub fn init_with_file(logs_dir: &Path) -> Result<()>` — deprecated, replaced by main.rs tracing init
3. `struct MultiWriter { stdout, file }` and its `impl std::io::Write for MultiWriter` — dead code (was only used by `init_with_file`)

Keep the following (still actively used):
- `pub fn log_path(logs_dir: &Path, profile: &str) -> PathBuf` — used by backend logs
- `pub fn open_log(logs_dir: &Path, profile: &str) -> Result<File>` — used by backend lifecycle and process.rs. **Calls `rotate_logs()` internally.**
- `pub fn rotate_logs(logs_dir: &Path, profile: &str) -> Result<()>` — called by `open_log()` AND by `main.rs` init_tracing (made public in Task 2). **Must stay.**
- `pub const MAX_LOG_SIZE` and `pub const MAX_LOG_FILES` — used by `open_log()` and `init_tracing` (made public in Task 2). **Must stay.**
- `pub fn tail_lines(path: &Path, n: usize) -> Result<Vec<String>>` — used by logs API endpoint
- `pub fn format_log_line(line: &str) -> String` — added in Task 4, used by logs API
- All existing tests for kept functions

Also remove unused imports that become unused after cleanup:
- `tracing_subscriber::{fmt, EnvFilter}` — only used by removed init functions
- `tracing::Level` — only used by removed init functions  
- `std::sync::{Arc, Mutex}` — only used by removed MultiWriter and init_with_file
- Keep `use tracing::debug!` if present (used in tests)

**Steps:**
- [ ] Remove `pub fn init()` and its doc comment from `logging.rs`
- [ ] Remove `pub fn init_with_file()` and its doc comment
- [ ] Remove `struct MultiWriter` and `impl std::io::Write for MultiWriter`
- [ ] Do NOT remove `rotate_logs()`, `open_log()`, `log_path()`, `tail_lines()`, or any constants/tests
- [ ] Remove unused imports: `tracing_subscriber::{fmt, EnvFilter}`, `std::sync::{Arc, Mutex}`
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run existing tests: `cargo nextest run --package tama-core -- logging`
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: remove deprecated logging init functions and MultiWriter"

**Acceptance criteria:**
- [ ] `cargo check --workspace` succeeds with no warnings
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] All existing logging tests pass (`cargo nextest run --package tama-core -- logging`)
- [ ] `tail_lines()`, `open_log()`, and `log_path()` remain available and functional
- [ ] `rotate_logs()` still exists (called by `open_log()`)
- [ ] No references to removed functions remain in the codebase

---

## Task 6: Verify End-to-End

**Context:** Final verification that the full logging pipeline works: tama starts, writes JSON to file, console shows pretty output, logs API returns fresh human-readable data, GPU fields appear on inference events, and rotation works.

**Files:**
- Test: Integration via manual testing or existing test suite

**What to implement:**

Run the full workspace check and targeted tests:

1. **Build and format:**
   - `cargo build --workspace`
   - `cargo fmt --all`

2. **Linting:**
   - `cargo clippy --workspace -- -D warnings`

3. **Unit tests (logging module):**
   - `cargo nextest run --package tama-core -- logging`

4. **Server tests (proxy + handlers):**
   - `cargo nextest run --package tama-core -- proxy`

5. **Full workspace tests:**
   - `cargo nextest run --workspace`

**Steps:**
- [ ] Run `cargo build --workspace` — did it succeed?
- [ ] Run `cargo fmt --all` — did it succeed?
- [ ] Run `cargo clippy --workspace -- -D warnings` — did it pass?
- [ ] Run `cargo nextest run --package tama-core -- logging` — did all pass?
- [ ] Run `cargo nextest run --workspace` — did all pass?
- [ ] If any failures, fix and re-run until green
- [ ] Commit with message: "test: verify improved logging end-to-end"

**Acceptance criteria:**
- [ ] Full workspace build succeeds
- [ ] Clippy passes with no warnings
- [ ] All workspace tests pass
- [ ] No regressions in existing functionality

---

## Rollback Plan

If issues arise, each task is independently revertable:
1. Task 6 (tests) — safe to always run
2. Task 5 (cleanup) — just restore `logging.rs` from git
3. Task 4 (JSON lines formatting) — revert `logging.rs` and `backend_logs.rs`
4. Task 3 (GPU fields) — revert `forward/request.rs` and `lifecycle/mod.rs`
5. Task 2 (tracing init) — revert `main.rs` to single-layer init
6. Task 1 (dependency) — remove `tracing-appender` from Cargo.toml files
