# Langfuse Integration — Core Plan

**Goal:** Add Langfuse observability to the proxy — config, telemetry structs, client wrapper, and request interception (streaming + non-streaming). Zero latency impact on the response path. Opt-in via config, fully inert when disabled.

**Architecture:** Transparent proxy pattern — `forward_request()` extracts request fields before sending, tees streaming responses for background accumulation, and spawns async tasks to POST trace + generation events to Langfuse via `langfuse-ergonomic` SDK. Energy cost computed from `GpuDeviceStats.power_w` × llama.cpp `timings` duration × configured electricity price.

**Tech Stack:** `langfuse-ergonomic` crate (genai-rs, v0.6.3), existing `tokio`/`reqwest`/`serde_json`, SQLite config persistence.

**Depends on:** Nothing (self-contained).
**Required by:** [plan-157-langfuse-web-ui.md](plan-157-langfuse-web-ui.md) (WASM mirrors need `LangfuseConfig` in core).

---

### Task 1: LangfuseConfig + Database Persistence

**Context:**
Langfuse integration needs configuration (credentials, host, feature flags). The config is stored in SQLite (config-to-db plan). This task adds the `LangfuseConfig` struct, DB table migration, and wires it into the existing `Config` struct's `from_db()`/`to_db()` methods. The migration number is **0037** (0035 = oauth2, 0036 = api_keys).

**Files:**
- Modify: `crates/tama-core/src/config/types/mod.rs` — add `mod langfuse`, `pub use langfuse::*`, add `langfuse` field to `Config`
- Create: `crates/tama-core/src/config/types/langfuse.rs` — `LangfuseConfig` struct
- Modify: `crates/tama-core/src/config/loader.rs` — add `langfuse: LangfuseConfig::default()` to `Config::default()` struct literal
- Modify: `crates/tama-core/src/config/mod.rs` — add `LangfuseConfig` to `pub use types::{ ... }` re-export
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs` — add `LangfuseRecord` row type, `get_langfuse()`, `upsert_langfuse()`, seed in `seed_defaults()`
- Create: `crates/tama-core/src/db/migrations/_0037_add_langfuse.rs` — migration for `app_langfuse` table
- Modify: `crates/tama-core/src/db/migrations.rs` — add `mod _0037_add_langfuse;`, append to `MIGRATIONS`, bump `LATEST_VERSION` from 36 to 37

**What to implement:**

1. **`LangfuseConfig` struct** in `config/types/langfuse.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangfuseConfig {
    pub enabled: bool,
    pub public_key: String,
    pub secret_key: String,
    pub host: String,
    pub environment: String,
    pub capture_input: bool,
    pub capture_output: bool,
    pub capture_streaming: bool,
    pub telemetry_max_bytes: usize,
    pub electricity_price_per_kwh: f64,
}

impl Default for LangfuseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            public_key: String::new(),
            secret_key: String::new(),
            host: "https://cloud.langfuse.com".to_string(),
            environment: "default".to_string(),
            capture_input: true,
            capture_output: true,
            capture_streaming: true,
            telemetry_max_bytes: 1048576, // 1 MB
            electricity_price_per_kwh: 0.0,
        }
    }
}
```

2. **Add to `Config` struct** in `config/types/mod.rs`:
   - Add `mod langfuse;` and `pub use langfuse::*;`
   - Add `#[serde(default)] pub langfuse: LangfuseConfig,` field to `Config`

3. **Re-export `LangfuseConfig`** from `config/mod.rs`:
   - Add `LangfuseConfig` to the `pub use types::{ ... }` re-export list
   - This is required so `tama_core::config::LangfuseConfig` resolves (used by WASM mirror `From` impls in plan-157)

4. **Add to `Config::default()`** in `config/loader.rs`:
   - Add `langfuse: LangfuseConfig::default(),` to the `Config { ... }` struct literal

5. **DB migration** `migrations/_0037_add_langfuse.rs` — follow existing migration pattern (e.g., `migrations/_0035_add_oauth2_config.rs`). The migration type is `(i32, bool, &str)` with raw SQL:

```rust
pub const MIGRATION: (i32, bool, &str) = (
    37,
    false, // not reversible
    r#"CREATE TABLE IF NOT EXISTS app_langfuse (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        enabled INTEGER NOT NULL DEFAULT 0,
        public_key TEXT NOT NULL DEFAULT '',
        secret_key TEXT NOT NULL DEFAULT '',
        host TEXT NOT NULL DEFAULT 'https://cloud.langfuse.com',
        environment TEXT NOT NULL DEFAULT 'default',
        capture_input INTEGER NOT NULL DEFAULT 1,
        capture_output INTEGER NOT NULL DEFAULT 1,
        capture_streaming INTEGER NOT NULL DEFAULT 1,
        telemetry_max_bytes INTEGER NOT NULL DEFAULT 1048576,
        electricity_price_per_kwh REAL NOT NULL DEFAULT 0.0
    )"#,
);
```

6. **Register migration** in `db/migrations.rs`:
   - Add `mod _0037_add_langfuse;` to the mod declarations
   - Append `_0037_add_langfuse::MIGRATION,` to the `MIGRATIONS` array
   - Bump `pub const LATEST_VERSION: i32 = 37;` (from 36)

7. **DB queries** in `db/queries/app_config_queries.rs`:

Add a `LangfuseRecord` struct following the `GeneralRecord`/`ProxyRecord` pattern:

```rust
#[derive(Debug)]
pub struct LangfuseRecord {
    pub enabled: bool,
    pub public_key: String,
    pub secret_key: String,
    pub host: String,
    pub environment: String,
    pub capture_input: bool,
    pub capture_output: bool,
    pub capture_streaming: bool,
    pub telemetry_max_bytes: usize,
    pub electricity_price_per_kwh: f64,
}
```

Add `get_langfuse(conn) -> Option<LangfuseRecord>` and `upsert_langfuse(conn, ...)` functions.

8. **Wire into `Config::from_db()`** in `config/types/mod.rs`:
   - After reading other config sections, read langfuse row
   - Construct `LangfuseConfig` from `LangfuseRecord`
   - **Critical:** Add `langfuse` to the final `Ok(Config { general, backends, supervisor, proxy, compaction, sampling_templates, langfuse })` struct literal

9. **Wire into `Config::to_db()`** in `config/types/mod.rs`:
   - Call `upsert_langfuse()` with values from `self.langfuse`

10. **Seed defaults** in `db/queries/app_config_queries.rs`:
    - Add `INSERT OR IGNORE INTO app_langfuse (id) VALUES (1);` to the existing `seed_defaults()` function

11. **Migration test** in `db/migrations/migrations_tests.rs`:
    - Add `test_migration_v37_creates_app_langfuse_table` following existing pattern (e.g., `test_migration_v35_adds_oauth2_columns`)

**Steps:**
- [ ] Write unit test for `LangfuseConfig::default()` values in `config/types/langfuse.rs`
- [ ] Run `cargo nextest run --package tama-core -- config::types`
  - Did it pass? If not, fix and re-run.
- [ ] Create `LangfuseConfig` struct in `config/types/langfuse.rs`
- [ ] Add `mod langfuse` and `pub use langfuse::*` to `config/types/mod.rs`
- [ ] Add `langfuse: LangfuseConfig` field to `Config` struct in `config/types/mod.rs`
- [ ] Add `LangfuseConfig` to `pub use types::{ ... }` re-export in `config/mod.rs`
- [ ] Add `langfuse: LangfuseConfig::default()` to `Config::default()` in `config/loader.rs`
- [ ] Create migration `_0037_add_langfuse.rs` following `_0035_add_oauth2_config.rs` pattern
- [ ] Register migration in `db/migrations.rs`: add mod, append to MIGRATIONS array, bump LATEST_VERSION to 37
- [ ] Add `LangfuseRecord`, `get_langfuse()`, `upsert_langfuse()` to `db/queries/app_config_queries.rs`
- [ ] Wire `LangfuseConfig` into `Config::from_db()` — read from DB, construct, **add to final Config { ... } literal**
- [ ] Wire `LangfuseConfig` into `Config::to_db()` — call `upsert_langfuse()`
- [ ] Add `INSERT OR IGNORE INTO app_langfuse (id) VALUES (1)` to `seed_defaults()` in `app_config_queries.rs`
- [ ] Add migration test following existing pattern
- [ ] Update `config/types/config_tests.rs::test_config_db_roundtrip` — add `langfuse: LangfuseConfig::default()` to the `Config { ... }` literal (line ~133)
- [ ] Run `cargo nextest run --package tama-core -- config`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo nextest run --package tama-core -- migrations`
  - Did all migration tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add LangfuseConfig with SQLite persistence (migration 0037)"

**Acceptance criteria:**
- [ ] `LangfuseConfig` struct compiles with all fields matching the design spec
- [ ] `Config` struct has `langfuse: LangfuseConfig` field with `#[serde(default)]`
- [ ] `Config::default()` in `loader.rs` includes `langfuse: LangfuseConfig::default()`
- [ ] `LangfuseConfig` re-exported from `config/mod.rs`
- [ ] Migration 0037 creates `app_langfuse` table with correct schema
- [ ] `LATEST_VERSION` bumped to 37, migration registered in `MIGRATIONS` array
- [ ] `Config::from_db()` reads and constructs `LangfuseConfig` from DB, includes in final struct literal
- [ ] `Config::to_db()` persists `LangfuseConfig` to DB
- [ ] `seed_defaults()` seeds `app_langfuse` row
- [ ] Default values match design spec (enabled=false, empty keys, 1MB telemetry max, 0 electricity price)
- [ ] All existing config and migration tests pass (including `test_config_db_roundtrip`)

---

### Task 2: LangfuseTelemetry Struct + Energy Cost + Header Extraction

**Context:**
This task creates the core telemetry data structure and helper functions in a new `proxy/forward/langfuse.rs` module. It defines `LangfuseTelemetry` (per-request data), `LangfuseUsage`, `LangfuseTimings`, and provides helpers for extracting `langfuse_*` headers, parsing usage/timings from JSON, and computing energy cost. No Langfuse SDK dependency yet — just pure data types and helpers.

**Files:**
- Create: `crates/tama-core/src/proxy/forward/langfuse.rs` — `LangfuseTelemetry`, `LangfuseUsage`, `LangfuseTimings`, all helper functions
- Modify: `crates/tama-core/src/proxy/forward/mod.rs` — add `pub(super) mod langfuse;` and `pub use langfuse::*;`

**What to implement:**

1. **Data types** in `proxy/forward/langfuse.rs`:

```rust
use std::time::Instant;

/// Per-request token usage extracted from backend response.
#[derive(Debug, Clone)]
pub struct LangfuseUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Per-request timings extracted from backend response.
#[derive(Debug, Clone)]
pub struct LangfuseTimings {
    pub prompt_ms: f64,
    pub predicted_ms: f64,
}

/// Accumulated telemetry data for a single inference request.
#[derive(Debug)]
pub struct LangfuseTelemetry {
    // From request
    pub model: String,
    pub input: Option<serde_json::Value>,       // messages array (if capture_input)
    pub model_params: Option<serde_json::Value>, // max_tokens, temperature, etc.

    // From response
    pub output: Option<String>,                  // accumulated completion text (if capture_output)
    pub usage: Option<LangfuseUsage>,
    pub timings: Option<LangfuseTimings>,

    // Timing
    pub start_time: Instant,
    pub end_time: Option<Instant>,

    // From langfuse_* headers
    pub trace_id: Option<String>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tags: Option<Vec<String>>,

    // Computed
    pub energy_cost: Option<f64>,                // in user's currency
    pub energy_wh: Option<f64>,                  // watt-hours consumed
    pub gpu_watts: Option<f64>,                  // from GpuDeviceStats (best-effort)
}
```

2. **Header extraction function:**

```rust
use axum::http::HeaderMap;

/// Extract Langfuse trace context from request headers.
/// Compatible with LiteLLM Proxy convention (langfuse_* prefixed headers).
/// Returns (trace_id, user_id, session_id, metadata, tags).
pub fn extract_langfuse_headers(headers: &HeaderMap) -> (Option<String>, Option<String>, Option<String>, Option<serde_json::Value>, Option<Vec<String>>) {
    let trace_id = headers.get("langfuse_trace_id").and_then(|v| v.to_str().ok().map(|s| s.to_string()));
    let user_id = headers.get("langfuse_trace_user_id").and_then(|v| v.to_str().ok().map(|s| s.to_string()));
    let session_id = headers.get("langfuse_session_id").and_then(|v| v.to_str().ok().map(|s| s.to_string()));
    let metadata = headers.get("langfuse_trace_metadata")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| serde_json::from_str(s).ok());
    let tags = headers.get("langfuse_trace_tags")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());

    (trace_id, user_id, session_id, metadata, tags)
}
```

3. **Energy cost computation:**

```rust
/// Compute energy cost per inference.
///
/// Returns Some((energy_wh, cost_in_currency)) when price_per_kwh > 0.
/// `power_w` is from GpuDeviceStats.power_w (in watts).
/// `prompt_ms` + `predicted_ms` are from llama.cpp timings.
/// `price_per_kwh` is from LangfuseConfig.electricity_price_per_kwh.
///
/// Formula: energy_wh = power_w × duration_s / 3600.0
///          cost = (energy_wh / 1000.0) × price_per_kwh
pub fn compute_energy_cost(
    power_w: f64,
    prompt_ms: f64,
    predicted_ms: f64,
    price_per_kwh: f64,
) -> Option<(f64, f64)> {
    if price_per_kwh <= 0.0 {
        return None;
    }
    let duration_s = (prompt_ms + predicted_ms) / 1000.0;
    let energy_wh = power_w * duration_s / 3600.0;
    let cost = (energy_wh / 1000.0) * price_per_kwh;
    Some((energy_wh, cost))
}
```

**Example:** 300W GPU, 5000ms total (3000ms prompt + 2000ms predicted), 1.0 krone/kWh:
- `duration_s = 5.0`
- `energy_wh = 300 * 5.0 / 3600.0 = 0.4167 Wh`
- `cost = 0.4167 / 1000.0 * 1.0 = 0.000417 krone`

4. **Parse `usage` from JSON response:**

```rust
/// Extract LangfuseUsage from an OpenAI-compatible response JSON.
/// Works for both non-streaming responses and the final streaming chunk.
pub fn extract_usage(json: &serde_json::Value) -> Option<LangfuseUsage> {
    let usage = json.get("usage")?;
    let prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_u64())?;
    let completion_tokens = usage.get("completion_tokens").and_then(|v| v.as_u64())?;
    let total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64())?;
    Some(LangfuseUsage { prompt_tokens, completion_tokens, total_tokens })
}
```

5. **Parse `timings` from JSON response:**

```rust
/// Extract LangfuseTimings from an OpenAI-compatible response JSON.
/// Works for both non-streaming responses and the final streaming chunk.
pub fn extract_timings(json: &serde_json::Value) -> Option<LangfuseTimings> {
    let timings = json.get("timings")?;
    let prompt_ms = timings.get("prompt_ms").and_then(|v| v.as_f64())?;
    let predicted_ms = timings.get("predicted_ms").and_then(|v| v.as_f64())?;
    Some(LangfuseTimings { prompt_ms, predicted_ms })
}
```

6. **Extract request body fields:**

```rust
/// Extract telemetry-relevant fields from an OpenAI-compatible request body.
/// Returns (model, input_messages_or_prompt, model_params).
pub fn extract_request_fields(body_bytes: &[u8]) -> Option<(String, Option<serde_json::Value>, Option<serde_json::Value>)> {
    let body: serde_json::Value = serde_json::from_slice(body_bytes).ok()?;
    let model = body.get("model").and_then(|v| v.as_str())?.to_string();
    let input = if body.get("messages").is_some() {
        Some(body["messages"].clone())
    } else if body.get("prompt").is_some() {
        Some(body["prompt"].clone())
    } else {
        None
    };
    // Model params: everything except model, messages, prompt, stream, stream_options
    let mut params = serde_json::Map::new();
    if let Some(obj) = body.as_object() {
        for (k, v) in obj {
            if !["model", "messages", "prompt", "stream", "stream_options"].contains(&k.as_str()) {
                params.insert(k.clone(), v.clone());
            }
        }
    }
    let model_params = if params.is_empty() { None } else { Some(serde_json::Value::Object(params)) };
    Some((model, input, model_params))
}
```

7. **Get GPU power (best-effort)** — helper that reads first available GPU's `power_w` from `SystemMetrics`:

```rust
/// Get GPU power in watts from system metrics (best-effort).
/// Returns the first GPU's power_w if available. This is a simplification —
/// per-backend GPU mapping would require resolving backend->GPU device assignment.
pub fn get_gpu_power_watts(system_metrics: &crate::gpu::SystemMetrics) -> Option<f64> {
    system_metrics
        .gpus
        .first()
        .and_then(|g| g.power_w)
        .map(|w| w as f64)
}
```

**Steps:**
- [ ] Write unit test for `compute_energy_cost()` — `compute_energy_cost(300.0, 3000.0, 2000.0, 1.0)` returns `Some((0.4167, 0.000417))` (approximately)
- [ ] Write unit test for `compute_energy_cost()` with `price_per_kwh = 0.0` returns `None`
- [ ] Write unit test for `extract_langfuse_headers()` with sample headers
- [ ] Write unit test for `extract_usage()` with sample JSON
- [ ] Write unit test for `extract_timings()` with sample JSON
- [ ] Write unit test for `extract_request_fields()` with sample chat completions JSON
- [ ] Run `cargo nextest run --package tama-core -- langfuse`
  - Did tests fail (not compiled yet)? Good — expected.
- [ ] Create `LangfuseTelemetry`, `LangfuseUsage`, `LangfuseTimings` structs in `proxy/forward/langfuse.rs`
- [ ] Implement all helper functions: `extract_langfuse_headers()`, `compute_energy_cost()`, `extract_usage()`, `extract_timings()`, `extract_request_fields()`, `get_gpu_power_watts()`
- [ ] Add `pub(super) mod langfuse;` and `pub use langfuse::*;` to `proxy/forward/mod.rs`
- [ ] Run `cargo nextest run --package tama-core -- langfuse`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add LangfuseTelemetry struct, energy cost, header extraction"

**Acceptance criteria:**
- [ ] `LangfuseTelemetry`, `LangfuseUsage`, `LangfuseTimings` structs compile
- [ ] `extract_langfuse_headers()` correctly parses all 5 `langfuse_*` headers
- [ ] `compute_energy_cost(300.0, 3000.0, 2000.0, 1.0)` returns `Some((~0.4167, ~0.000417))`
- [ ] `compute_energy_cost()` returns `None` when `price_per_kwh <= 0`
- [ ] `extract_usage()` parses `usage` from response JSON
- [ ] `extract_timings()` extracts `prompt_ms` and `predicted_ms` from `timings` JSON
- [ ] `extract_request_fields()` parses model, messages/prompt, and model params from request JSON
- [ ] `get_gpu_power_watts()` returns first GPU's `power_w` as `f64`
- [ ] Module exported from `proxy/forward/mod.rs` with `pub(super)` visibility

---

### Task 3: Langfuse Client Wrapper + Reporting Logic

**Context:**
This task adds the `langfuse-ergonomic` dependency and creates a `LangfuseClient` wrapper that builds and sends trace + generation events. The client is lazy-initialized from config inside `ProxyState::new()` (which is synchronous and uses a single `Self { ... }` literal in `proxy/state.rs`).

**Files:**
- Modify: `crates/tama-core/Cargo.toml` — add `langfuse-ergonomic` dependency
- Modify: `crates/tama-core/src/proxy/forward/langfuse.rs` — add `LangfuseClient` struct and `report_generation()` function
- Modify: `crates/tama-core/src/proxy/types.rs` — add `langfuse_client: Option<Arc<LangfuseClient>>` to `ProxyState`
- Modify: `crates/tama-core/src/proxy/state.rs` — initialize `LangfuseClient` inside `ProxyState::new()` `Self { ... }` literal

**What to implement:**

1. **Add dependency** to `crates/tama-core/Cargo.toml`:

```toml
langfuse-ergonomic = "0.6.3"
```

2. **`LangfuseClient` wrapper** in `proxy/forward/langfuse.rs`:

The `langfuse-ergonomic` crate uses `ClientBuilder` to construct a `LangfuseClient`. Read the crate's docs.rs or README for the exact API before implementing. The pattern is:

```rust
use std::sync::Arc;
use langfuse_ergonomic::{ClientBuilder, LangfuseClient as InnerClient};

#[derive(Clone)]
pub struct LangfuseClient {
    inner: Arc<InnerClient>,
    config: Arc<crate::config::LangfuseConfig>,
}

impl LangfuseClient {
    /// Create a new LangfuseClient from config.
    /// Returns None if langfuse is not enabled or credentials are missing.
    pub fn from_config(config: &crate::config::LangfuseConfig) -> Option<Self> {
        if !config.enabled || config.public_key.is_empty() || config.secret_key.is_empty() {
            return None;
        }

        let inner = ClientBuilder::new()
            .public_key(&config.public_key)
            .secret_key(&config.secret_key)
            .base_url(config.host.clone())
            .build()
            .ok()?;

        Some(Self {
            inner: Arc::new(inner),
            config: Arc::new(config.clone()),
        })
    }

    /// Report a generation to Langfuse.
    ///
    /// Creates a trace + generation with token usage, input/output, energy cost,
    /// and trace context from headers. Runs asynchronously — failures are logged
    /// but don't affect the response to the client.
    ///
    /// Read the SDK docs.rs to determine the correct API:
    /// - If the SDK has `create_trace()` / `create_generation()` methods, use those
    ///   with the appropriate builder types (e.g., `CreateTraceBody`, `CreateGenerationBody`)
    /// - If the SDK has a batcher pattern (`create_batcher()`), use that for buffered ingestion
    /// - The SDK handles batching, retries, compression automatically
    ///
    /// Key field mappings (adapt to actual SDK types):
    /// - trace.name = telemetry.model
    /// - trace.userId = telemetry.user_id
    /// - trace.sessionId = telemetry.session_id
    /// - trace.input = telemetry.input (if capture_input)
    /// - trace.output = telemetry.output (if capture_output)
    /// - trace.metadata = { energy_wh, gpu_watts, duration_sec }
    /// - trace.tags = telemetry.tags
    /// - generation.model = telemetry.model
    /// - generation.modelParameters = telemetry.model_params
    /// - generation.usage = { input, output, total } from telemetry.usage
    /// - generation.costDetails = { "energy": energy_cost }
    /// - generation.startTime/endTime = telemetry.start_time/end_time
    pub async fn report_generation(&self, telemetry: LangfuseTelemetry) {
        // Read langfuse-ergonomic docs.rs for the exact builder API.
        // The SDK provides `client.trace()` and `client.generation()` builders.
        // Map LangfuseTelemetry fields to SDK types:
        //
        //   trace.name = telemetry.model
        //   trace.userId = telemetry.user_id
        //   trace.sessionId = telemetry.session_id
        //   trace.input = telemetry.input (if capture_input)
        //   trace.output = telemetry.output (if capture_output)
        //   trace.metadata = { energy_wh, gpu_watts, duration_sec }
        //   trace.tags = telemetry.tags
        //   generation.model = telemetry.model
        //   generation.modelParameters = telemetry.model_params
        //   generation.usage = { input, output, total } from telemetry.usage
        //   generation.costDetails = { "energy": energy_cost }
        //   generation.startTime/endTime = telemetry.start_time/end_time
        //
        // Failures should be logged (tracing::error!) but not propagated.
        //
        // NOTE: The `create_batcher()` method takes `self: Arc<Self>`, so if using
        // the batcher pattern, call `self.inner.clone().create_batcher(None).await`.
        // The direct builder approach (`self.inner.trace()`) may be simpler for
        // per-request fire-and-forget reporting.
    }
}
```

**Important:** Read the `langfuse-ergonomic` crate documentation (docs.rs or GitHub README) to understand its exact API. The crate uses builder patterns and the actual method names may differ from the pseudocode above. Adapt to the real API.

**⚠️ Note:** After this task, `cargo build --workspace` will temporarily fail because the `tama` crate's `From<tama_core::config::Config>` impls in `types/config/mod.rs` need `langfuse` handling. This is fixed by plan-157. Only run `cargo check --package tama-core` for verification.

3. **Add `langfuse_client` to `ProxyState`** in `types.rs`:

Add the field to the `ProxyState` struct:

```rust
pub(crate) langfuse_client: Option<Arc<crate::proxy::forward::langfuse::LangfuseClient>>,
```

**Critical:** Also update the manual `Clone` impl for `ProxyState` (same file) to include:

```rust
langfuse_client: self.langfuse_client.clone(),
```

in the `Self { ... }` literal. Without this, the Clone impl will fail to compile.

4. **Initialize at startup** in `proxy/state.rs`, `ProxyState::new()`:

The `new()` function owns `config: crate::config::Config` by value and builds `Self { ... }` as a single literal. Initialize the langfuse client **before** wrapping config in `Arc<RwLock>`:

```rust
// In ProxyState::new(), before the Self { ... } literal:
let langfuse_client = crate::proxy::forward::langfuse::LangfuseClient::from_config(&config.langfuse)
    .map(Arc::new);

// Inside Self { ... } literal, add:
langfuse_client,
```

**Steps:**
- [ ] Add `langfuse-ergonomic = "0.6.3"` to `crates/tama-core/Cargo.toml`
- [ ] Run `cargo check --package tama-core` to verify dependency resolves
  - Did it succeed? If not, check available versions on crates.io and adjust.
- [ ] Read `langfuse-ergonomic` crate docs (docs.rs/crates.io/GitHub README) to understand its exact API
- [ ] Implement `LangfuseClient::from_config()` using `ClientBuilder` pattern
- [ ] Implement `LangfuseClient::report_generation()` — construct trace + generation events using the SDK's actual API
- [ ] Add `langfuse_client` field to `ProxyState` in `types.rs`
- [ ] Update manual `Clone` impl for `ProxyState` to include `langfuse_client: self.langfuse_client.clone()`
- [ ] Initialize `langfuse_client` in `ProxyState::new()` in `state.rs` — create from config before `Self { ... }` literal, include in literal
- [ ] Write unit test for `LangfuseClient::from_config()` — disabled returns None, empty keys returns None, valid config returns Some
- [ ] Run `cargo nextest run --package tama-core -- langfuse`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add LangfuseClient wrapper with report_generation"

**Acceptance criteria:**
- [ ] `langfuse-ergonomic` dependency resolves and compiles
- [ ] `LangfuseClient::from_config()` returns None when disabled or credentials missing
- [ ] `LangfuseClient::from_config()` returns Some with valid credentials
- [ ] `report_generation()` constructs trace + generation with all telemetry fields using the SDK's actual API
- [ ] `langfuse_client` field added to `ProxyState`
- [ ] `ProxyState` Clone impl updated with `langfuse_client`
- [ ] Client initialized at startup in `ProxyState::new()` from config

---

### Task 4: Non-Streaming Telemetry Hook in forward_request()

**Context:**
This task adds telemetry collection to the non-streaming response path in `forward_request()`. It captures request data before sending, parses the response JSON for usage + timings, computes energy cost, and spawns a background reporting task.

**Files:**
- Modify: `crates/tama-core/src/proxy/forward/request.rs` — add telemetry collection in non-streaming path
- Modify: `crates/tama-core/src/proxy/forward/langfuse.rs` — (helpers already implemented in Task 2)

**What to implement:**

In `forward_request()` in `request.rs`:

1. **Capture request data before sending** — `body_bytes` is `&[u8]` (a reference) and remains valid after `.body(body_bytes.to_vec()).send()`. **Critical:** The `body_bytes` parameter is **shadowed** by `let body_bytes = match response.bytes().await` inside the non-streaming `else` branch, so request field extraction must happen **before** the streaming/non-streaming branch (where `body_bytes` is still the request parameter).

```rust
// Before the state.client.request(...).send() chain:
let langfuse_headers = extract_langfuse_headers(&parts.headers);
let telemetry_start = std::time::Instant::now();
// Extract request fields NOW — before body_bytes is shadowed by response in non-streaming branch
let langfuse_req_fields = extract_request_fields(body_bytes).unwrap_or_default();
```

2. **In the non-streaming response branch** (the `else` branch of `let body = if is_streaming { ... } else { ... }`), add telemetry extraction **inside the `if let Ok(parsed) = ...` block, BEFORE `rewrite_json_model_name(parsed, ...)` consumes `parsed`**:

The existing code is:
```rust
let new_body = if let Ok(parsed) = serde_json::from_slice::<JsonValue>(&body_bytes) {
    let _stats = extract_inference_stats(backend_name, &parsed, &state.inference_stats);
    let rewritten = rewrite_json_model_name(parsed, model_name);  // parsed MOVED here
    serde_json::to_vec(&rewritten).unwrap_or(body_bytes.to_vec())
} else { ... };
```

Insert telemetry extraction between `extract_inference_stats` and `rewrite_json_model_name`:

```rust
    // Collect Langfuse telemetry (non-streaming path) — fire-and-forget
    // MUST be before rewrite_json_model_name(parsed, ...) which consumes parsed
    {
        let langfuse_cfg = state.config.read().await.langfuse.clone();
        if langfuse_cfg.enabled {
            let langfuse_client = state.langfuse_client.clone();

            // Use langfuse_req_fields captured before the send (body_bytes is shadowed here by response body)
            let (req_model, input, model_params) = langfuse_req_fields.clone();

            // Extract response fields from &parsed (borrow — parsed still owned)
            let usage = extract_usage(&parsed);
            let timings = extract_timings(&parsed);

            // Extract output (completion text)
            let output = if langfuse_cfg.capture_output {
                // Chat completions: choices[0].message.content
                parsed.get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|c| c.first())
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string())
                    // Completions (non-chat): choices[0].text
                    .or_else(|| {
                        parsed.get("choices")
                            .and_then(|c| c.as_array())
                            .and_then(|c| c.first())
                            .and_then(|c| c.get("text"))
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string())
                    })
            } else {
                None
            };

            // Compute energy cost (best-effort — uses first GPU's power_w)
            let (energy_cost, energy_wh, gpu_watts) = {
                let metrics = state.system_metrics.read().await;
                let power_w = get_gpu_power_watts(&*metrics);
                if let (Some(pw), Some(t)) = (power_w, &timings) {
                    match compute_energy_cost(pw, t.prompt_ms, t.predicted_ms, langfuse_cfg.electricity_price_per_kwh) {
                        Some((wh, cost)) => (Some(cost), Some(wh), Some(pw)),
                        None => (None, None, None),
                    }
                } else {
                    (None, None, None)
                }
            };

            // Build LangfuseTelemetry
            let (trace_id, user_id, session_id, metadata, tags) = langfuse_headers.clone();
            let telemetry = LangfuseTelemetry {
                model: req_model,
                input: if langfuse_cfg.capture_input { input } else { None },
                model_params,
                output,
                usage,
                timings,
                start_time: telemetry_start,
                end_time: Some(std::time::Instant::now()),
                trace_id, user_id, session_id, metadata, tags,
                energy_cost, energy_wh, gpu_watts,
            };

            // Spawn background reporting task
            if let Some(client) = langfuse_client {
                tokio::spawn(async move {
                    client.report_generation(telemetry).await;
                });
            }
        }
    }
```

**Key integration points:**
- Capture `telemetry_start: Instant::now()` before the `.send()` call
- **`extract_request_fields(body_bytes)` must be called BEFORE the streaming/non-streaming branch** — the `body_bytes` parameter is shadowed by the response body inside the non-streaming `else` branch. Store results in `langfuse_req_fields` and reuse in both branches.
- Use `state.system_metrics.read().await` for GPU power (async, non-blocking)
- **Telemetry extraction from `&parsed` must occur BEFORE `rewrite_json_model_name(parsed, ...)`** which consumes `parsed`
- Clone `langfuse_headers` before moving into the telemetry block (needed for both streaming and non-streaming paths)
- Only spawn background task when `config.langfuse.enabled` is true
- The telemetry block must not block or affect the response to the client

**Steps:**
- [ ] Extract `langfuse_*` headers from `parts.headers` before `.send()` call
- [ ] Capture `telemetry_start: Instant::now()` before `.send()` call
- [ ] Call `extract_request_fields(body_bytes)` before streaming/non-streaming branch and store in `langfuse_req_fields`
- [ ] Add telemetry collection block in the non-streaming response branch (after JSON parse/rewrite)
- [ ] Extract usage, timings, output from `&parsed` (borrow — before `rewrite_json_model_name` consumes it)
- [ ] Compute energy cost using `state.system_metrics.read().await` and `compute_energy_cost()`
- [ ] Build `LangfuseTelemetry` struct
- [ ] Spawn background `report_generation()` task with `tokio::spawn`
- [ ] Run `cargo nextest run --package tama-core -- forward`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add non-streaming Langfuse telemetry in forward_request"

**Acceptance criteria:**
- [ ] `extract_request_fields(body_bytes)` called BEFORE streaming/non-streaming branch (before `body_bytes` is shadowed by response)
- [ ] Stored `langfuse_req_fields` reused in non-streaming telemetry block
- [ ] Telemetry extraction from `&parsed` occurs BEFORE `rewrite_json_model_name(parsed, ...)` consumes parsed
- [ ] `langfuse_*` headers extracted from request
- [ ] Non-streaming path extracts `usage` and `timings` from response JSON
- [ ] Non-streaming path extracts output (completion text) from response JSON
- [ ] Energy cost computed when `electricity_price_per_kwh > 0` and GPU power available
- [ ] `LangfuseTelemetry` built with all fields
- [ ] Background `report_generation()` task spawned (fire-and-forget, doesn't block response)
- [ ] Telemetry collection is skipped when `langfuse.enabled = false`
- [ ] Existing tests pass — no regression in forwarding behavior

---

### Task 5: Streaming Telemetry (Tee + Accumulation)

**Context:**
This task adds telemetry collection to the streaming (SSE) response path. It injects `stream_options.include_usage: true` into the upstream request, preserves the existing `unfold` SSE processing for the client, and adds an `mpsc` channel inside the unfold to tee raw bytes for background accumulation.

**Files:**
- Modify: `crates/tama-core/src/proxy/forward/request.rs` — inject stream_options, add tee inside unfold
- Modify: `crates/tama-core/src/proxy/forward/langfuse.rs` — add `parse_sse_accumulated()` function

**What to implement:**

1. **Inject `stream_options.include_usage: true`** into the request body. In `request.rs`, before the `.body(...)` call, check if this is a streaming chat completions request and langfuse is enabled:

```rust
// Before the state.client.request(...).body(...) chain:
let is_chat_streaming = parts.uri.path().ends_with("/chat/completions")
    && serde_json::from_slice::<serde_json::Value>(body_bytes)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);

let langfuse_enabled = state.config.read().await.langfuse.enabled;
let inject_usage = is_chat_streaming && langfuse_enabled;

let body_to_send = if inject_usage {
    let mut body: serde_json::Value = serde_json::from_slice(body_bytes)
        .unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = body.as_object_mut() {
        let stream_opts = obj.entry("stream_options")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(opts) = stream_opts.as_object_mut() {
            opts.insert("include_usage".to_string(), serde_json::json!(true));
        }
    }
    serde_json::to_vec(&body).unwrap_or_else(|_| body_bytes.to_vec())
} else {
    body_bytes.to_vec()
};
```

Use `body_to_send` in the `.body(body_to_send)` call instead of `body_bytes.to_vec()`.

2. **SSE accumulation helper** in `langfuse.rs`:

```rust
/// Parse accumulated SSE text for Langfuse telemetry.
/// Extracts: accumulated content (delta.content concatenated), usage (from final chunk), timings.
pub fn parse_sse_accumulated(raw: &str) -> (Option<String>, Option<LangfuseUsage>, Option<LangfuseTimings>) {
    let mut content_parts = Vec::new();
    let mut usage = None;
    let mut timings = None;

    for line in raw.lines() {
        if let Some(data_content) = line.strip_prefix("data: ") {
            let trimmed = data_content.trim_end();
            if trimmed == "[DONE]" {
                continue;
            }
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                // Accumulate content from delta
                if let Some(content) = json.get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|c| c.first())
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|c| c.as_str())
                {
                    if !content.is_empty() {
                        content_parts.push(content.to_string());
                    }
                }
                // Extract usage from final chunk (empty choices)
                if json.get("usage").is_some() {
                    usage = extract_usage(&json);
                }
                // Extract timings
                if json.get("timings").is_some() {
                    timings = extract_timings(&json);
                }
            }
        }
    }

    let content = if content_parts.is_empty() { None } else { Some(content_parts.join("")) };
    (content, usage, timings)
}
```

3. **Tee the streaming response** — modify the existing `unfold` in `request.rs`. To avoid duplicating the unfold closure, use `Option<UnboundedSender<Bytes>>` so the channel is created only when langfuse streaming capture is enabled:

Inside the streaming branch of `let body = if is_streaming { ... }`:

```rust
// Read langfuse config once
let langfuse_cfg = state.config.read().await.langfuse.clone();
let capture_streaming = langfuse_cfg.enabled && langfuse_cfg.capture_streaming;

let langfuse_client = state.langfuse_client.clone();

// Channel for tee'd bytes — None when capture disabled
let (tx, rx) = if capture_streaming {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<bytes::Bytes>();
    (Some(tx), Some(rx))
} else {
    (None, None)
};

// Spawn background accumulation + reporting (only if capture enabled)
if let Some(mut rx) = rx {
    let max_bytes = langfuse_cfg.telemetry_max_bytes;
    let (trace_id, user_id, session_id, metadata, tags) = langfuse_headers.clone();
    let (req_model, input, model_params) = langfuse_req_fields.clone();
    let start_time = telemetry_start;

    tokio::spawn(async move {
        let mut buf = bytes::BytesMut::new();
        let mut total_bytes = 0usize;
        while let Some(chunk) = rx.recv().await {
            if total_bytes + chunk.len() <= max_bytes {
                buf.extend_from_slice(&chunk);
                total_bytes += chunk.len();
            }
            // Keep draining the channel even if over limit (must consume all)
        }
        let raw = String::from_utf8_lossy(&buf).to_string();
        let (content, usage, timings) = parse_sse_accumulated(&raw);

        if let Some(client) = langfuse_client {
            let telemetry = LangfuseTelemetry {
                model: req_model,
                input: if langfuse_cfg.capture_input { input } else { None },
                model_params,
                output: if langfuse_cfg.capture_output { content } else { None },
                usage,
                timings,
                start_time,
                end_time: Some(std::time::Instant::now()),
                trace_id, user_id, session_id, metadata, tags,
                energy_cost: None, energy_wh: None, gpu_watts: None,
            };
            client.report_generation(telemetry).await;
        }
    });
}

// The existing unfold — add tee inside the Ok(chunk) arm:
// Inside the unfold closure, change:
//   Ok(chunk) => { ... processing ... Ok(Bytes::from(out.into_bytes())) }
// to:
//   Ok(chunk) => {
//       // Tee: send clone to background accumulator (if channel active)
//       if let Some(ref sender) = tx { let _ = sender.send(chunk.clone()); }
//       ... existing processing ... Ok(Bytes::from(out.into_bytes()))
//   }
```

**Key detail:** `tx` is `Option<UnboundedSender<Bytes>>` and is moved into the unfold closure. The `if let Some(ref sender) = tx` check inside the closure avoids the send when langfuse is disabled, with zero overhead (the Option check is a single pointer comparison).

**Concrete unfold modification:** The existing `unfold` closure receives `(byte_stream, line_buf)` and processes chunks. Inside the `Ok(chunk)` arm, add the tee before the existing processing:

```rust
Ok(chunk) => {
    // Tee: send clone to background accumulator (if channel active)
    if let Some(ref sender) = tx { let _ = sender.send(chunk.clone()); }

    // ... existing SSE line processing (model rewrite, inference stats) ...
    Ok(Bytes::from(out.into_bytes()))
}
```

The `tx` (`Option<UnboundedSender<Bytes>>`) is moved into the unfold closure.

**Steps:**
- [ ] Write unit test for `parse_sse_accumulated()` with sample SSE text (multiple content chunks, usage chunk, timings chunk, [DONE])
- [ ] Write unit test for `parse_sse_accumulated()` with edge cases: empty content, malformed JSON, no usage
- [ ] Run `cargo nextest run --package tama-core -- langfuse`
  - Did tests pass? If not, fix and re-run.
- [ ] Implement `parse_sse_accumulated()` in `langfuse.rs`
- [ ] Add `stream_options.include_usage` injection before `.body()` call in `request.rs` (with safe guard — use `as_object_mut()` + check, not `unwrap()`)
- [ ] Create `Option<UnboundedSender<Bytes>>` channel (Some when capture enabled, None otherwise)
- [ ] Spawn background accumulation + reporting task (only when rx is Some)
- [ ] Wire `if let Some(ref sender) = tx { let _ = sender.send(chunk.clone()); }` inside the unfold's `Ok(chunk)` arm
- [ ] Read langfuse config once at the start of the streaming branch (avoid multiple `config.read().await` calls)
- [ ] Run `cargo nextest run --package tama-core -- forward`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add streaming Langfuse telemetry with response tee"

**Acceptance criteria:**
- [ ] `parse_sse_accumulated()` extracts content, usage, and timings from accumulated SSE text
- [ ] `stream_options.include_usage: true` injected into streaming chat completions requests when langfuse enabled
- [ ] Injection uses safe guards (`as_object_mut()` + check), no panics
- [ ] Streaming response teed — client receives stream immediately via existing unfold (zero latency)
- [ ] `mpsc` channel inside unfold sends cloned chunks to background accumulator
- [ ] Background task accumulates raw bytes (bounded by `telemetry_max_bytes`)
- [ ] Background task parses accumulated SSE and reports to Langfuse
- [ ] Existing SSE processing (model rewrite, inference stats) preserved unchanged
- [ ] Telemetry skipped when `capture_streaming = false`
- [ ] Existing tests pass — no regression in streaming behavior

---

## Verification

After all tasks are complete:

```bash
cargo check --package tama-core
cargo fmt --all
cargo clippy --package tama-core -- -D warnings
cargo nextest run --package tama-core
```

## Manual Testing Checklist

- [ ] Enable Langfuse in DB config with valid credentials
- [ ] Send a non-streaming `/v1/chat/completions` request — verify trace appears in Langfuse dashboard
- [ ] Send a streaming `/v1/chat/completions` request — verify trace appears with content + usage
- [ ] Verify `langfuse_trace_id` header is respected (trace appears under provided ID)
- [ ] Set `electricity_price_per_kwh` > 0 — verify energy cost appears in Langfuse `costDetails`
- [ ] Disable Langfuse (`enabled = false`) — verify no Langfuse HTTP traffic
- [ ] Verify response latency is unchanged with Langfuse enabled (background reporting)
