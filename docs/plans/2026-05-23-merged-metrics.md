# Merged `/metrics` Endpoint Plan

**Goal:** Merge Tama's proxy metrics with backend (llama.cpp) metrics into a single Prometheus-format `/metrics` endpoint for Grafana ingestion.

**Architecture:** The `handle_metrics` handler fetches `/metrics` from all Ready backends concurrently, injects a `{server="<name>"}` label into each backend metric line, and appends Tama's own proxy metrics prefixed with `tama:`. Returns `text/plain; version=0.0.4; charset=utf-8`.

**Tech Stack:** Rust, axum, reqwest, Prometheus text format

---

### Task 1: Create Prometheus metrics helper module

**Context:**
The current `handle_metrics` returns JSON. We need to convert it to Prometheus text format and also parse/inject labels into backend metrics. A dedicated helper module keeps the handler clean and makes the formatting logic testable.

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/metrics.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**

Create a new module `crates/tama-core/src/proxy/handlers/metrics.rs` with two public functions:

1. **`fn format_tama_metrics(metrics: &ProxyMetrics, active_models: usize) -> String`**
   - Takes Tama's `ProxyMetrics` (from `crate::proxy::types::ProxyMetrics`) and active model count
   - Returns a String in Prometheus format with `# HELP` and `# TYPE` lines for each metric
   - Metrics to include (all type `gauge`, using `Relaxed` ordering for atomics):
     - `tama:total_requests` — total number of requests proxied
     - `tama:successful_requests` — requests that returned 2xx
     - `tama:failed_requests` — requests that returned non-2xx
     - `tama:models_loaded` — cumulative model load events
     - `tama:models_unloaded` — cumulative model unload events
     - `tama:active_models` — current number of loaded models (the `active_models` parameter)
   - Format example:
     ```
     # HELP tama:total_requests Total number of requests proxied.
     # TYPE tama:total_requests gauge
     tama:total_requests 98
     ```

2. **`fn inject_server_label(line: &str, server_name: &str) -> String`**
   - Takes a single line from a backend's `/metrics` response and the server name
   - For metric data lines (not starting with `#`): inject `{server="<name>"}` label
     - If line already has labels like `name{existing="label"} value` → `name{existing="label",server="my-model"} value`
     - If line has no labels like `name value` → `name{server="my-model"} value`
   - For comment lines (`# HELP`, `# TYPE`, or blank lines): pass through unchanged
   - Use simple string parsing (no external crate needed):
     1. Find the first `{` and first ` ` (space) in the line
     2. If `{` comes before ` ` (line has labels): find the matching `}`, inject `,server="<name>"` before `}`
     3. If ` ` comes before `{` or no `{` exists (line has no labels): inject `{server="<name>"}` before the space
     4. The metric name portion (left of `{` or ` `) is passed through unchanged
   - Only the injected server value needs escaping: `\` → `\\`, `"` → `\"`, `\n` → `\\n`
   - Pre-existing labels are left unchanged

3. **`fn format_backend_metrics(lines: &[String], server_name: &str) -> String`**
   - Takes all lines from a backend's `/metrics` response and the server name
   - Applies `inject_server_label` to each line
   - Returns the joined string

Update `crates/tama-core/src/proxy/handlers/mod.rs`:
- Add `pub mod metrics;` to the module declarations
- Add `pub use metrics::{format_backend_metrics, format_tama_metrics, inject_server_label};` to the re-exports

**Steps:**
- [ ] Write test for `inject_server_label` with labeled input in `crates/tama-core/src/proxy/handlers/metrics.rs`
  - [ ] Test: line with existing labels → labels preserved + server added
  - [ ] Test: line without labels → server label injected
  - [ ] Test: `# HELP` line → passed through unchanged
  - [ ] Test: `# TYPE` line → passed through unchanged
  - [ ] Test: empty line → passed through unchanged
  - [ ] Test: server name with special chars → properly escaped
- [ ] Write test for `format_tama_metrics` with known values
  - [ ] Test: all 6 metrics present with correct HELP/TYPE lines
  - [ ] Test: atomic values read correctly
- [ ] Implement `inject_server_label` function
- [ ] Implement `format_tama_metrics` function
- [ ] Implement `format_backend_metrics` function
- [ ] Update `mod.rs` to export the new module
- [ ] Run `cargo test --package tama-core -- proxy::handlers::metrics`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix errors and re-run.
- [ ] Commit with message: "feat(core): add Prometheus metrics formatting helpers"

**Acceptance criteria:**
- [ ] `inject_server_label` correctly handles labeled, unlabeled, and comment lines
- [ ] `format_tama_metrics` produces valid Prometheus format for all 6 metrics
- [ ] All tests pass, clippy clean, fmt clean

---

### Task 2: Rewrite handle_metrics to fetch and merge backend metrics

**Context:**
The current `handle_metrics` in `proxy/handlers/status.rs` returns JSON. We need to rewrite it to fetch `/metrics` from all Ready backends, merge with Tama's metrics, and return Prometheus text format.

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/status.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs` (update re-export)

**What to implement:**

Replace the existing `handle_metrics` function in `crates/tama-core/src/proxy/handlers/status.rs` with a new implementation:

1. **New signature:** `pub async fn handle_metrics(state: State<Arc<ProxyState>>) -> Response`
   - Return type changes from `Json<serde_json::Value>` to `axum::response::Response`
   - This allows setting the content type header

2. **Logic:**
   a. Read `state.models` (read lock) and collect all `Ready` backends that are NOT TTS backends (skip via `is_tts_backend()`) into a `Vec<(String, String)>` of `(server_name, backend_url)` pairs
   b. **Immediately drop the read lock** before spawning any HTTP tasks (never hold a lock across network I/O)
   c. For each backend, spawn a concurrent task using `tokio::task::JoinSet`:
      - Clone `state.client` for each task (reqwest::Client is Clone)
      - Construct `{backend_url}/metrics`
      - Use `client.get(url).timeout(Duration::from_secs(5)).send().await` to fetch
      - On success: read body as text via `.text().await`, split into lines with `.lines().map(|s| s.to_string()).collect::<Vec<_>>()`, call `format_backend_metrics(&lines, server_name)`
      - On failure: log a warning with `tracing::warn!(backend = %server_name, error = %e, "Failed to fetch backend metrics")` and return `None`
      - Handle task panics via `JoinSet::join_next()` — log as warning on `Err`
   d. Collect all successful backend results from the JoinSet
   e. `active_models` counts only `Ready` non-TTS backends (same filter as step a)
   e. Build the final output:
      - Join all backend metrics blocks with `\n`
      - Append `\n` + Tama's own metrics from `format_tama_metrics(&state.metrics, active_count)`
   f. Return as `Response` with:
      - Status: 200 OK
      - Header: `content-type: text/plain; version=0.0.4; charset=utf-8`
      - Body: the merged metrics string

3. **Edge cases:**
   - No backends running: just return Tama's metrics
   - All backends fail: just return Tama's metrics (with warnings logged)
   - Mix of success/failure: include what succeeded, skip failures

4. **Update imports in status.rs:**
   - Add `use axum::response::Response;`
   - Add `use crate::proxy::handlers::metrics::{format_backend_metrics, format_tama_metrics};`
   - Add `use std::time::Duration;`
   - Add `use tracing;`
   - **Do NOT remove** `use serde_json::json;` — it's still used by `handle_status`, `handle_reload_configs`, and `handle_health`

5. **Update `mod.rs` re-export:**
   - The `handle_metrics` is already re-exported from `status` module in `mod.rs` — no change needed there since the function stays in the same module

**Steps:**
- [ ] Write integration tests in `crates/tama-core/src/proxy/server/mod.rs` (add to existing test module):
  - [ ] Test: `/metrics` returns Prometheus content type header (`text/plain; version=0.0.4; charset=utf-8`)
  - [ ] Test: `/metrics` includes `tama:` prefixed metrics
  - [ ] Test: `/metrics` gracefully handles no backends (returns just Tama metrics)
  - [ ] Test: `/metrics` merges backend metrics correctly — use `wiremock::MockServer` to serve fake Prometheus metrics (`llamacpp:some_counter 42`), mount it as a Ready backend, call `GET /metrics`, verify response contains `llamacpp:some_counter{server="<name>"} 42`
- [ ] Run `cargo test --package tama-core -- proxy::server` to verify new tests fail (RED)
  - Tests should fail because the handler still returns JSON
- [ ] Implement the new `handle_metrics` in `status.rs`
- [ ] Update imports in `status.rs`
- [ ] Run `cargo test --package tama-core -- proxy::server`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix errors and re-run.
- [ ] Commit with message: "feat(core): merge backend metrics into /metrics endpoint"

**Acceptance criteria:**
- [ ] `GET /metrics` returns `text/plain; version=0.0.4; charset=utf-8` content type
- [ ] Response includes `tama:*` metrics for proxy counters
- [ ] Response includes backend metrics with `{server="<name>"}` labels when backends are running
- [ ] Gracefully degrades when backends are unavailable (returns Tama metrics only)
- [ ] All existing tests still pass
- [ ] Clippy clean, fmt clean

---

### Task 3: Update OpenAPI spec and verify end-to-end

**Context:**
The OpenAPI spec documents the `/metrics` endpoint. It currently describes a JSON response. We need to update it to reflect the new Prometheus text format.

**Files:**
- Modify: `docs/openapi/openai-compat.yaml`

**What to implement:**

1. In `docs/openapi/openai-compat.yaml`, find the `/metrics` endpoint definition
2. Update the response to:
   ```yaml
   responses:
     "200":
       description: Prometheus text format metrics (tama proxy + backend merged)
       content:
         text/plain:
           schema:
             type: string
             description: Prometheus exposition format. Tama metrics use `tama:` prefix. Backend metrics include `{server="<name>"}` label.
           example: |
             # HELP tama:total_requests Total number of requests proxied.
             # TYPE tama:total_requests gauge
             tama:total_requests 98
             # HELP llamacpp:prompt_tokens_total Number of prompt tokens processed.
             # TYPE llamacpp:prompt_tokens_total counter
             llamacpp:prompt_tokens_total{server="my-model"} 32479
   ```
3. If the spec has other examples for `/metrics`, update them with Prometheus format

**Steps:**
- [ ] Read `docs/openapi/openai-compat.yaml` and locate the `/metrics` endpoint
- [ ] Update the response to reflect Prometheus text format
- [ ] Run `cargo build --workspace` to ensure everything still builds
- [ ] Run `cargo test --workspace` to ensure all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "docs: update /metrics OpenAPI spec to Prometheus format"

**Acceptance criteria:**
- [ ] OpenAPI spec accurately describes the new Prometheus format response
- [ ] All tests pass
- [ ] Clippy clean, fmt clean

---

## Verification

After all tasks are complete:

1. Start tama server with a model loaded
2. `curl http://localhost:11434/metrics`
3. Verify output contains:
   - `tama:total_requests` line
   - `llamacpp:*` lines with `{server="..."}` labels
4. Verify content-type header: `curl -I http://localhost:11434/metrics | grep content-type`
5. Verify the output is parseable by Prometheus (no malformed lines)
