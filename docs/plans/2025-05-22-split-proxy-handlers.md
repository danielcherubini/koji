# Split proxy/handlers/mod.rs Plan

**Goal:** Split the 2109 LOC `proxy/handlers/mod.rs` into 6 focused files by responsibility.

**Architecture:** Each handler group (chat, models, forwarding, status) gets its own module. Tests move to a dedicated `tests.rs`. Shared helpers stay in `mod.rs` as `pub(super)`. Re-exports maintain backward compatibility.

**Tech Stack:** Rust, Axum

**Dependencies:** `1 → 2 → 3 → 4 → 5 → 6` (all serial; each modifies the same `mod.rs`. Tasks 1-4 are commutative but cannot be parallelized since they edit the same file.)

---

### Task 1: Extract chat.rs

**Context:**
The chat completions handlers (`handle_chat_completions`, `handle_stream_chat_completions`) are ~200 LOC dedicated to OpenAI-compatible chat routing. They should be in their own file.

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/chat.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
1. Create `chat.rs` with:
   - `pub async fn handle_chat_completions` — exact copy from mod.rs
   - `pub async fn handle_stream_chat_completions` — exact copy from mod.rs
   - Add `use super::{json_error_response, update_last_used_best_effort};` plus all existing imports from mod.rs that these functions need
   - NOTE: Do NOT include `find_model_in_entries` — it belongs in `models.rs` (used by `handle_get_model`, not chat)
2. In `mod.rs`, add `pub mod chat;` at the top
3. In `mod.rs`, remove the two chat functions

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/handlers/chat.rs` with the two functions copied from mod.rs
- [ ] Add `use super::{json_error_response, update_last_used_best_effort};` import
- [ ] Add all other imports needed by the two functions (read mod.rs imports and copy what's needed)
- [ ] In mod.rs, add `pub mod chat;` after `pub mod tts;`
- [ ] In mod.rs, remove the two chat functions
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix missing imports and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor(core): extract chat handlers to handlers/chat.rs"

**Acceptance criteria:**
- [ ] `chat.rs` exists with 2 functions
- [ ] `mod.rs` no longer contains the chat functions
- [ ] `cargo check --package tama-core` passes

---

### Task 2: Extract models.rs

**Context:**
The model management handlers (`handle_get_model`, `handle_list_models`) and model fetching helpers (`parse_models_response`, `fetch_models_from_backend`) are ~251 LOC dedicated to model operations.

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/models.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
1. Create `models.rs` with:
   - `pub async fn handle_get_model` — exact copy from mod.rs
   - `pub async fn handle_list_models` — exact copy from mod.rs
   - `fn find_model_in_entries` — exact copy from mod.rs (used by `handle_get_model`, NOT by chat)
   - `pub fn parse_models_response` — exact copy from mod.rs
   - `pub async fn fetch_models_from_backend` — exact copy from mod.rs
   - Add imports needed by these functions (read mod.rs imports and copy what's needed). These functions do NOT use `json_error_response` or `update_last_used_best_effort`.
2. In `mod.rs`, add `pub mod models;`
3. In `mod.rs`, remove the five functions

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/handlers/models.rs` with the five functions copied from mod.rs
- [ ] Add all imports needed by the five functions (read mod.rs imports and copy what's needed)
- [ ] Do NOT add `use super::{json_error_response, update_last_used_best_effort};` — not needed
- [ ] In mod.rs, add `pub mod models;`
- [ ] In mod.rs, remove the four functions
- [ ] Run `cargo check --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor(core): extract model handlers to handlers/models.rs"

**Acceptance criteria:**
- [ ] `models.rs` exists with 4 functions
- [ ] `mod.rs` no longer contains the model functions
- [ ] `cargo check --package tama-core` passes

---

### Task 3: Extract forward.rs

**Context:**
The HTTP forwarding handlers (`handle_forward_post`, `handle_forward_get`, `handle_fallback`) are ~155 LOC dedicated to proxying requests to backends.

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/forward.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
1. Create `forward.rs` with:
   - `pub async fn handle_fallback` — exact copy from mod.rs
   - `pub async fn handle_forward_post` — exact copy from mod.rs
   - `pub async fn handle_forward_get` — exact copy from mod.rs
   - Add `use super::update_last_used_best_effort;` import (used by `handle_forward_post`)
   - Add `use super::forward::forward_request;` import (used by forwarding handlers)
2. In `mod.rs`, add `pub mod forward;`
3. In `mod.rs`, remove the three functions

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/handlers/forward.rs` with the three functions copied from mod.rs
- [ ] Add `use super::update_last_used_best_effort;` import
- [ ] Add all other imports needed by the three functions
- [ ] In mod.rs, add `pub mod forward;`
- [ ] In mod.rs, remove the three functions
- [ ] Run `cargo check --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor(core): extract forwarding handlers to handlers/forward.rs"

**Acceptance criteria:**
- [ ] `forward.rs` exists with 3 functions
- [ ] `mod.rs` no longer contains the forwarding functions
- [ ] `cargo check --package tama-core` passes

---

### Task 4: Extract status.rs

**Context:**
The status/health/metrics handlers (`handle_status`, `handle_reload_configs`, `handle_health`, `handle_metrics`) are ~37 LOC — thin wrappers around ProxyState methods.

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/status.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
1. Create `status.rs` with:
   - `pub async fn handle_status` — exact copy from mod.rs
   - `pub async fn handle_reload_configs` — exact copy from mod.rs
   - `pub async fn handle_health` — exact copy from mod.rs
   - `pub async fn handle_metrics` — exact copy from mod.rs
2. In `mod.rs`, add `pub mod status;`
3. In `mod.rs`, remove the four functions

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/handlers/status.rs` with the four functions copied from mod.rs
- [ ] Add imports: `use axum::{extract::State, response::IntoResponse, Json};`, `use crate::proxy::ProxyState;`, `use std::sync::Arc;`, and any others needed
- [ ] In mod.rs, add `pub mod status;`
- [ ] In mod.rs, remove the four functions
- [ ] Run `cargo check --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor(core): extract status handlers to handlers/status.rs"

**Acceptance criteria:**
- [ ] `status.rs` exists with 4 functions
- [ ] `mod.rs` no longer contains the status functions
- [ ] `cargo check --package tama-core` passes

---

### Task 5: Extract tests.rs

**Context:**
The `#[cfg(test)]` module is ~1417 LOC (67% of the file). Moving tests to a dedicated file is the biggest reduction.

**Requires:** Tasks 1-4 to be completed first (tests import from `super::chat`, `super::models`, etc.)

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/tests.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
1. Create `tests.rs` with the entire `#[cfg(test)]` module content (without the `#[cfg(test)]` wrapper and `mod tests` — the file itself is the test module)
2. Update test imports to use `use super::chat::...`, `use super::models::...`, etc. instead of `use super::...`
3. In `mod.rs`, add `#[cfg(test)] mod tests;`
4. In `mod.rs`, remove the entire `#[cfg(test)]` module

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/handlers/tests.rs` with the test content copied from mod.rs
- [ ] Update all test imports: `super::handle_chat_completions` → `super::chat::handle_chat_completions`, etc.
- [ ] In mod.rs, add `#[cfg(test)] mod tests;`
- [ ] In mod.rs, remove the entire `#[cfg(test)]` module
- [ ] Run `cargo test --package tama-core -- proxy::handlers`
  - Did all tests pass? If not, fix import paths and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor(core): extract handler tests to handlers/tests.rs"

**Acceptance criteria:**
- [ ] `tests.rs` exists with all tests
- [ ] `mod.rs` no longer contains the test module
- [ ] `cargo test --package tama-core -- proxy::handlers` passes

---

### Task 6: Clean up mod.rs with re-exports

**Context:**
After extracting all code, `mod.rs` should contain only: module declarations, shared helpers (`json_error_response`, `update_last_used_best_effort`), and re-exports for backward compatibility.

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`

**What to implement:**
1. Add re-exports at the bottom of `mod.rs`:
```rust
pub use chat::{handle_chat_completions, handle_stream_chat_completions};
pub use models::{handle_get_model, handle_list_models};
pub use forward::{handle_forward_post, handle_forward_get, handle_fallback};
pub use status::{handle_status, handle_reload_configs, handle_health, handle_metrics};
```
2. Verify `mod.rs` is ~60 LOC (module declarations + 2 helpers + re-exports)

**Steps:**
- [ ] Add re-exports to mod.rs
- [ ] Verify mod.rs is ~60 LOC
- [ ] Run `cargo check --workspace`
  - Did it succeed? If not, fix any broken imports and re-run.
- [ ] Run `cargo test --workspace -- proxy::handlers`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor(core): add re-exports to handlers/mod.rs for backward compatibility"

**Acceptance criteria:**
- [ ] `mod.rs` is ~60 LOC
- [ ] All re-exports present
- [ ] `cargo check --workspace` passes
- [ ] `cargo test --workspace -- proxy::handlers` passes
