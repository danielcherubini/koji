# Langfuse Integration — Web UI Plan

**Goal:** Wire Langfuse config into the web UI — WASM mirror types, structured config API, config editor form. Enables users to configure Langfuse from the browser.

**Architecture:** Mirror `LangfuseConfig` from tama-core into WASM-compatible types, add PATCH support, and create a config editor form section.

**Tech Stack:** Leptos (SvelteKit), serde, existing config editor patterns.

**Depends on:** [plan-156-langfuse-core.md](plan-156-langfuse-core.md) — needs `LangfuseConfig` in tama-core with `pub use` re-export.

---

### Task 1: WASM Mirror Config Types + Structured Config API

**Context:**
Adding `langfuse` to the core `Config` struct requires updating all WASM mirror types that must stay structurally identical. The comment at the top of `types/config/mod.rs` explicitly warns: "If you add/remove fields here, mirror the change." This task updates all mirror types, `From` impls, patch types, merge functions, and test fixtures.

**Files:**
- Modify: `crates/tama/src/types/config/mod.rs` — add `langfuse` field to `Config`, `StructuredConfigBody`, and all 3 `From` impls
- Create: `crates/tama/src/types/config/langfuse.rs` — WASM mirror `LangfuseConfig` + `From` impls
- Modify: `crates/tama/src/types/config/patch.rs` — add `LangfuseConfigPatch`, add `langfuse` to `ConfigPatchBody`
- Modify: `crates/tama/src/api.rs` — add `merge_langfuse()` function, add `langfuse` to `merge_config_patch()`, update test fixtures

**What to implement:**

1. **WASM mirror `LangfuseConfig`** in `types/config/langfuse.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

impl From<tama_core::config::LangfuseConfig> for LangfuseConfig {
    fn from(c: tama_core::config::LangfuseConfig) -> Self {
        Self {
            enabled: c.enabled,
            public_key: c.public_key,
            secret_key: c.secret_key,
            host: c.host,
            environment: c.environment,
            capture_input: c.capture_input,
            capture_output: c.capture_output,
            capture_streaming: c.capture_streaming,
            telemetry_max_bytes: c.telemetry_max_bytes,
            electricity_price_per_kwh: c.electricity_price_per_kwh,
        }
    }
}

impl From<LangfuseConfig> for tama_core::config::LangfuseConfig {
    fn from(c: LangfuseConfig) -> Self {
        Self {
            enabled: c.enabled,
            public_key: c.public_key,
            secret_key: c.secret_key,
            host: c.host,
            environment: c.environment,
            capture_input: c.capture_input,
            capture_output: c.capture_output,
            capture_streaming: c.capture_streaming,
            telemetry_max_bytes: c.telemetry_max_bytes,
            electricity_price_per_kwh: c.electricity_price_per_kwh,
        }
    }
}
```

2. **Register in `types/config/mod.rs`:**
   - Add `mod langfuse;` and `pub use langfuse::*;`
   - Add `#[serde(default)] pub langfuse: LangfuseConfig,` to `Config` struct
   - Add `#[serde(default)] pub langfuse: LangfuseConfig,` to `StructuredConfigBody` struct
   - Add `langfuse` to all 3 `From` impls (note: the `From<StructuredConfigBody>` impl uses parameter `b`, the other two use `c`):
     - `From<tama_core::config::Config> for Config` → `langfuse: c.langfuse.into()`
     - `From<StructuredConfigBody> for tama_core::config::Config` → `langfuse: b.langfuse.into()`
     - `From<Config> for tama_core::config::Config` → `langfuse: c.langfuse.into()`
   - Add `pub use patch::LangfuseConfigPatch;` to the re-export list in `mod.rs` (alongside `CompactionConfigPatch` etc.)

3. **Patch type** in `types/config/patch.rs`:

```rust
/// PATCH body for Langfuse section.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LangfuseConfigPatch {
    pub enabled: Option<bool>,
    pub public_key: Option<String>,
    pub secret_key: Option<String>,
    pub host: Option<String>,
    pub environment: Option<String>,
    pub capture_input: Option<bool>,
    pub capture_output: Option<bool>,
    pub capture_streaming: Option<bool>,
    pub telemetry_max_bytes: Option<usize>,
    pub electricity_price_per_kwh: Option<f64>,
}
```

Add `#[serde(default)] pub langfuse: Option<LangfuseConfigPatch>,` to `ConfigPatchBody`.

4. **Merge function** in `api.rs`:

Add `merge_langfuse()` following the `merge_compaction()` pattern:

```rust
fn merge_langfuse(
    existing: crate::types::config::LangfuseConfig,
    patch: crate::types::config::LangfuseConfigPatch,
) -> crate::types::config::LangfuseConfig {
    crate::types::config::LangfuseConfig {
        enabled: patch.enabled.unwrap_or(existing.enabled),
        public_key: patch.public_key.or(existing.public_key),
        secret_key: patch.secret_key.or(existing.secret_key),
        host: patch.host.or(existing.host),
        environment: patch.environment.or(existing.environment),
        capture_input: patch.capture_input.unwrap_or(existing.capture_input),
        capture_output: patch.capture_output.unwrap_or(existing.capture_output),
        capture_streaming: patch.capture_streaming.unwrap_or(existing.capture_streaming),
        telemetry_max_bytes: patch.telemetry_max_bytes.unwrap_or(existing.telemetry_max_bytes),
        electricity_price_per_kwh: patch.electricity_price_per_kwh.unwrap_or(existing.electricity_price_per_kwh),
    }
}
```

Add `langfuse` handling to `merge_config_patch()`:

```rust
let langfuse = match patch.langfuse {
    Some(p) => merge_langfuse(existing.langfuse, p),
    None => existing.langfuse,
};
```

And add `langfuse` to the final `Config { ... }` literal in `merge_config_patch()`.

5. **Update test fixtures** in `api.rs` — the `test_merge_config_patch_*` tests construct `Config` and `ConfigPatchBody` structs. Add `langfuse: Default::default()` to all test fixture struct literals.

**Steps:**
- [ ] Create `types/config/langfuse.rs` with mirror `LangfuseConfig` and both `From` impls
- [ ] Register `mod langfuse` and `pub use langfuse::*` in `types/config/mod.rs`
- [ ] Add `langfuse` field to `Config` and `StructuredConfigBody` in `types/config/mod.rs`
- [ ] Add `langfuse` to all 3 `From` impls in `types/config/mod.rs` (use `b.langfuse.into()` for `From<StructuredConfigBody>`, `c.langfuse.into()` for the other two)
- [ ] Add `pub use patch::LangfuseConfigPatch;` to re-exports in `types/config/mod.rs`
- [ ] Add `LangfuseConfigPatch` to `types/config/patch.rs`
- [ ] Add `langfuse` field to `ConfigPatchBody` in `types/config/patch.rs`
- [ ] Add `merge_langfuse()` and `langfuse` handling to `merge_config_patch()` in `api.rs`
- [ ] Add `langfuse` to final `Config { ... }` literal in `merge_config_patch()`
- [ ] Update all test fixtures in `api.rs` to include `langfuse: Default::default()`
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo nextest run --package tama -- merge_config_patch`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add WASM mirror types for LangfuseConfig"

**Acceptance criteria:**
- [ ] `types/config/langfuse.rs` compiles with mirror `LangfuseConfig` and both `From` impls
- [ ] `Config` and `StructuredConfigBody` in `types/config/mod.rs` have `langfuse` field
- [ ] All 3 `From` impls include `langfuse` field (correct parameter: `b` for StructuredConfigBody, `c` for others)
- [ ] `LangfuseConfigPatch` re-exported from `types/config/mod.rs`
- [ ] `LangfuseConfigPatch` added to `patch.rs`
- [ ] `ConfigPatchBody` has `langfuse: Option<LangfuseConfigPatch>` field
- [ ] `merge_langfuse()` follows `merge_compaction()` pattern
- [ ] `merge_config_patch()` handles `langfuse` section
- [ ] All test fixtures updated with `langfuse: Default::default()`
- [ ] `cargo check --package tama` succeeds
- [ ] All `merge_config_patch` tests pass

---

### Task 2: Config Editor Local Types

**Context:**
The config editor in `pages/config_editor/types.rs` defines its own local type definitions (not imported from `tama_core`). Adding `langfuse` to the editor's `Config` mirror requires defining a local `LangfuseConfig` struct in this file. This must come before Task 3 (the form) which references this type.

**Files:**
- Modify: `crates/tama/src/pages/config_editor/types.rs` — define local `LangfuseConfig` struct, add `langfuse` field to editor's `Config` mirror

**What to implement:**

1. **Define local `LangfuseConfig`** in `pages/config_editor/types.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
```

2. **Add `langfuse: LangfuseConfig`** to the editor's `Config` struct in the same file.

**Steps:**
- [ ] Define `LangfuseConfig` struct in `pages/config_editor/types.rs` with `Default` derive
- [ ] Add `langfuse: LangfuseConfig` field to the editor's `Config` struct
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add LangfuseConfig to config editor local types"

**Acceptance criteria:**
- [ ] Local `LangfuseConfig` struct compiles with all fields (no `PartialEq` — matches sibling structs)
- [ ] Editor's `Config` struct has `#[serde(default)] pub langfuse: LangfuseConfig` field
- [ ] `cargo check --package tama` succeeds

---

### Task 3: Web UI Config Editor Integration

**Context:**
This task adds the Langfuse config section to the web UI config editor, allowing users to enable/disable Langfuse and configure credentials from the browser. The config editor uses tab-based sections with form components in `pages/config_editor/forms/`.

**Files:**
- Modify: `crates/tama/src/pages/config_editor/mod.rs` — add `Section::Langfuse` variant, tab, icon, routing
- Create: `crates/tama/src/pages/config_editor/forms/langfuse.rs` — `LangfuseForm` component
- Modify: `crates/tama/src/pages/config_editor/forms/mod.rs` — register `LangfuseForm`

**What to implement:**

1. **Add `Section::Langfuse`** to the `Section` enum in `pages/config_editor/mod.rs`:

```rust
enum Section {
    General,
    Proxy,
    Supervisor,
    Sampling,
    Compaction,
    Langfuse,
}
```

Add `Section::Langfuse => "Langfuse"` to the `name()` match.
Add `Section::Langfuse => "📊"` to the icon match.
Add `Section::Langfuse` to the tab array in the UI.
Add `Section::Langfuse => "cfg-langfuse"` to the `scroll_id` match inside the nav button `.map()` closure.

2. **Create `LangfuseForm`** component in `pages/config_editor/forms/langfuse.rs`:

Follow the pattern from `forms/compaction.rs` or `forms/general.rs`. The form should have:
- `enabled` — checkbox
- `public_key` — text input (placeholder `pk-lf-...`)
- `secret_key` — password input (placeholder `sk-lf-...`)
- `host` — text input (default `https://cloud.langfuse.com`)
- `environment` — text input (default `default`)
- `capture_input` — checkbox (default checked)
- `capture_output` — checkbox (default checked)
- `capture_streaming` — checkbox (default checked)
- `telemetry_max_bytes` — number input (default 1048576)
- `electricity_price_per_kwh` — number input with step 0.01 (default 0)

When `enabled` is unchecked, credential fields should be disabled (but still editable when checked).

3. **Register in `forms/mod.rs`** — export `LangfuseForm`.

4. **Wire the form** in `mod.rs` — the config editor renders **all sections stacked** in a single scrollable column (not conditionally). Add `<div id="cfg-langfuse"><LangfuseForm config=config /></div>` after the `cfg-compaction` div. The `Section` enum / nav buttons drive `scroll_into_view` navigation, not conditional rendering.

**Steps:**
- [ ] Add `Section::Langfuse` to enum, `name()`, icon, tab array, `scroll_id` match in `pages/config_editor/mod.rs`
- [ ] Add `LangfuseForm` to the `use crate::pages::config_editor::forms::{...}` import in `mod.rs`
- [ ] Create `forms/langfuse.rs` with `LangfuseForm` component following `compaction.rs` pattern
- [ ] Export `LangfuseForm` from `forms/mod.rs`
- [ ] Add `<div id="cfg-langfuse"><LangfuseForm config=config /></div>` to the stacked form container (after `cfg-compaction` div)
- [ ] Add validation in save handler: check `enabled && (public_key.empty || secret_key.empty)` → show inline error + early-return
- [ ] Bind `disabled=move || !enabled.get()` on credential input fields
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run.
- [ ] Test in browser — verify form renders, saves, and persists
- [ ] Commit with message: "feat: add Langfuse config section to web UI"

**Acceptance criteria:**
- [ ] `Section::Langfuse` added to enum with name, icon, tab, `scroll_id` match
- [ ] `LangfuseForm` imported in `mod.rs`
- [ ] `LangfuseForm` renders all config fields
- [ ] Form fields save correctly to config (via existing POST/PATCH endpoints)
- [ ] Validation prevents saving with enabled=true but empty credentials
- [ ] Config persists through page reload
- [ ] Follows existing config editor styling and patterns

---

### Task 4: Workspace Verification

**Context:**
This task runs the full workspace gate to verify both plans compile and test together. The core telemetry tests (non-streaming, streaming, energy cost, headers, disabled, config round-trip) are part of plan-156 and should already be in place.

**⚠️ Prerequisite:** plan-156 must be fully complete (all 5 tasks). This task requires `LangfuseTelemetry`, `LangfuseClient`, and the telemetry hooks from plan-156.

**Files:**
- (No new files — verification only)

**Steps:**
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix any remaining compilation errors.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo nextest run --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: "test: verify Langfuse integration workspace gate"

**Acceptance criteria:**
- [ ] `cargo check --workspace` succeeds (no compilation errors)
- [ ] Clippy passes with -D warnings across workspace
- [ ] All workspace tests pass — no regressions

---

## Verification

After all tasks are complete:

```bash
cargo check --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace
```

## Manual Testing Checklist

- [ ] Enable Langfuse in web UI config with valid credentials
- [ ] Send a non-streaming `/v1/chat/completions` request — verify trace appears in Langfuse dashboard
- [ ] Send a streaming `/v1/chat/completions` request — verify trace appears with content + usage
- [ ] Verify `langfuse_trace_id` header is respected (trace appears under provided ID)
- [ ] Set `electricity_price_per_kwh` > 0 — verify energy cost appears in Langfuse `costDetails`
- [ ] Disable Langfuse (`enabled = false`) — verify no Langfuse HTTP traffic
- [ ] Verify response latency is unchanged with Langfuse enabled (background reporting)
