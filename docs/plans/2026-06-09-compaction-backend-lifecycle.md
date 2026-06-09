# Compaction Backend Lifecycle Plan

**Goal:** Route the compaction server through the existing backend lifecycle (Kokoro TTS pattern) instead of custom subprocess management.

**Architecture:** Add `BackendType::Compaction`, register "compaction" in the model registry, and use the shared ModelState lifecycle (Starting → Ready/Failed) for spawn, health poll, reaping, and auto-restart. The compaction handler forwards to the backend URL like TTS does.

**Tech Stack:** Rust, `uvx` for Python dependency management, `include_dir!` for embedded server files.

---

## Key Design Decisions (from brainstorming)

- `load_compaction_backend()` uses **embedded extraction + `uvx`** (NOT BackendManager like Kokoro). It mirrors Kokoro only for registry registration and state transitions.
- Registration key in `state.models`: `"compaction"`
- Spawn command: `uvx --project <dir> uvicorn main:app --host 127.0.0.1 --port <port>`
- Compaction backend is skipped in `check_idle_timeouts()` and `evict_lru_if_needed()` via new `is_non_inference_backend()` helper
- Port: honor `compaction.port` if set, auto-assign otherwise (fix listener leak with `drop(listener)`)
- `startup_timeout_secs` removed from `CompactionConfig` — uses proxy's `startup_timeout_secs` (120s default)
- `timeout_ms` renamed to `request_timeout_ms` for clarity

---

### Task 1: Add `BackendType::Compaction` and helper

**Context:** The compaction server needs to be recognized as a backend type so the lifecycle can manage it. This is a pure addition — no existing behavior changes.

**Files:**
- Modify: `crates/tama-core/src/backends/types.rs`

**What to implement:**
- Add `Compaction` variant to `BackendType` enum (after `TtsKokoro`, before `Custom`)
- Add `Display` arm: `BackendType::Compaction => write!(f, "compaction")`
- Add `is_tts()` arm: `BackendType::Compaction => false`
- Add `default_git_url()` arm: return fallback string (never reached in practice, same as TtsKokoro)
- Add `from_str()` arm: `"compaction" => Ok(BackendType::Compaction)`
- Add new helper method `is_non_inference_backend(&self) -> bool` that returns `true` for `TtsKokoro` and `Compaction`

**Steps:**
- [ ] Add `Compaction` variant to `BackendType` enum
- [ ] Add all match arms (Display, is_tts, default_git_url, from_str)
- [ ] Add `is_non_inference_backend()` method
- [ ] Add test `test_is_non_inference_backend()` in `backends/types.rs` `#[cfg(test)]` module:
  ```rust
  #[test]
  fn test_is_non_inference_backend() {
      assert!(BackendType::TtsKokoro.is_non_inference_backend());
      assert!(BackendType::Compaction.is_non_inference_backend());
      assert!(!BackendType::LlamaCpp.is_non_inference_backend());
      assert!(!BackendType::IkLlama.is_non_inference_backend());
      assert!(!BackendType::Custom.is_non_inference_backend());
  }
  ```
- [ ] Run `cargo test --package tama-core backends::types::tests`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add BackendType::Compaction and is_non_inference_backend helper"

**Acceptance criteria:**
- [ ] `BackendType::Compaction` exists with all match arms
- [ ] `is_non_inference_backend()` returns true for TtsKokoro and Compaction, false for others
- [ ] Clippy clean

---

### Task 2: Clean up `CompactionConfig` — rename `timeout_ms`, remove `startup_timeout_secs`

**Context:** The compaction-specific `startup_timeout_secs` is no longer needed (uses proxy's default). Rename `timeout_ms` to `request_timeout_ms` for clarity. This affects core config, web mirror types, and From impls.

**Files:**
- Modify: `crates/tama-core/src/config/types.rs`
- Modify: `crates/tama-web/src/types/config.rs`
- Modify: `crates/tama-web/src/pages/config_editor.rs`

**What to implement:**

In `tama-core/src/config/types.rs`:
- Rename `timeout_ms` → `request_timeout_ms` in `CompactionConfig` struct
- Update serde default function name: `default_compaction_request_timeout_ms`
- Update `Default` impl to use new field name
- Remove `startup_timeout_secs` field and `default_compaction_startup_timeout_secs` function
- Update TOML round-trip test to use new field name

In `tama-web/src/types/config.rs`:
- Rename `timeout_ms` → `request_timeout_ms` in `CompactionConfig` mirror type
- Update default function name: `default_compaction_request_timeout_ms`
- Remove `startup_timeout_secs` field and `default_compaction_startup_timeout_secs` function
- Update both `From<CompactionConfig> for CoreCompactionConfig` and `From<CoreCompactionConfig> for CompactionConfig` to use new field name and remove old field

In `tama-web/src/pages/config_editor.rs` (standalone struct, NOT the mirror type):
- **Struct declaration (line ~124):** Rename `timeout_ms` → `request_timeout_ms`, remove `startup_timeout_secs` field
- **Default functions (line ~140):** Rename `default_compaction_timeout_ms` → `default_compaction_request_timeout_ms`, remove `default_compaction_startup_timeout_secs`
- **CompactionForm component (line ~763):** Update `get_compaction().timeout_ms` → `get_compaction().request_timeout_ms`, update label from `"Timeout (ms)"` to `"Request Timeout (ms)"`, remove the startup timeout input block entirely

**Steps:**
- [ ] Rename `timeout_ms` → `request_timeout_ms` in core `CompactionConfig`
- [ ] Remove `startup_timeout_secs` from core `CompactionConfig`
- [ ] Update core Default impl and test
- [ ] Update web mirror types (both `types/config.rs` and `config_editor.rs`)
- [ ] Update all `From` impls for CompactionConfig
- [ ] Update `CompactionForm` component
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --package tama-core config::types::tests`
- [ ] Commit with message: "refactor: rename timeout_ms to request_timeout_ms, remove startup_timeout_secs from CompactionConfig"

**Acceptance criteria:**
- [ ] `CompactionConfig` has `request_timeout_ms` (renamed from `timeout_ms`)
- [ ] `startup_timeout_secs` removed from all CompactionConfig types (core + web)
- [ ] All `From` impls updated
- [ ] Web UI form updated (no startup timeout field)
- [ ] Tests pass, clippy clean

---

### Task 3: Add `load_compaction_backend()` to lifecycle

**Context:** This is the core of the refactor — replace custom compaction subprocess management with the shared model registry lifecycle. The new method follows the Kokoro TTS pattern for registry registration and state transitions, but uses embedded file extraction + `uvx` for spawning (not BackendManager).

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

Add `load_compaction_backend(&self) -> anyhow::Result<()>` method to `ProxyState` impl in lifecycle/mod.rs:

1. **Read config** (scoped read lock): get `compaction` config from `self.config`
2. **Check enabled:** return error if not enabled
3. **Fast path — already starting or ready:** check `state.models` for "compaction" entry. If `Starting` or `Ready`, return Ok.
4. **Extract embedded files:** call `crate::compaction_server::get_server_dir(&base_dir)` to get server directory
5. **Resolve entrypoint:** call `crate::compaction_server::get_server_entrypoint(&compaction, &base_dir)`
6. **Determine port:** honor `compaction.port` if set, else auto-assign via `TcpListener::bind("127.0.0.1:0")`. **Important:** `drop(listener)` after getting port.
7. **Register in model registry:** insert `ModelState::Starting` with key `"compaction"`, `model_name: "compaction"`, `backend: "compaction"`
8. **Derive uvicorn target:** from entrypoint filename (e.g., "main.py" → "main:app")
9. **Spawn via uvx:**
   ```rust
   let mut child = tokio::process::Command::new("uvx");
   configure_process_group(&mut child);
   child
       .arg("--project")
       .arg(server_dir)
       .arg("uvicorn")
       .arg(&uvicorn_target)
       .arg("--host").arg("127.0.0.1")
       .arg("--port").arg(port.to_string())
       .env("COMPACTION_PORT", port.to_string())
       .env("COMPACTION_DEVICE", &compaction.device)
       .current_dir(server_dir);
   ```
10. **Update PID in Starting state** (write lock on models)
11. **Spawn reaper task** (same pattern as Kokoro — `child.wait()` in spawned task)
12. **Health poll loop:** every 500ms, poll `http://127.0.0.1:<port>/health`. Require 2 consecutive successes. Timeout = `proxy.startup_timeout_secs`. On timeout: kill process group, set `ModelState::Failed`.
13. **Transition to Ready:** update `ModelState` with `backend_url` and `backend_pid`

Also update `check_idle_timeouts()` to skip compaction:
- Replace `state.is_tts_backend()` with `state.is_non_inference_backend()`

Also update `evict_lru_if_needed()` to skip compaction:
- Extend the filter to also skip backends named `"compaction"`. The filter checks `mc.backend` (ModelConfig field), so use:
  ```rust
  .filter(|server_name| {
      !model_configs
          .get(server_name.as_str())
          .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
  })
  ```

**Steps:**
- [ ] Implement `load_compaction_backend()` method
- [ ] Update `check_idle_timeouts()` to use `is_non_inference_backend()`
- [ ] Update `evict_lru_if_needed()` to skip compaction
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add load_compaction_backend using model registry lifecycle"

**Acceptance criteria:**
- [ ] `load_compaction_backend()` exists and follows Kokoro pattern for registry + state transitions
- [ ] Spawn uses `uvx --project <dir> uvicorn main:app`
- [ ] Port assignment honors config port, auto-assigns with `drop(listener)` if not set
- [ ] Health poll uses proxy's `startup_timeout_secs`
- [ ] `check_idle_timeouts()` skips compaction backend
- [ ] `evict_lru_if_needed()` skips compaction backend
- [ ] Clippy clean

---

### Task 4: Rewrite compaction handler to use model registry

**Context:** The compaction handler currently calls `ensure_compaction_server()` which uses the custom `CompactionServerState`. Rewrite to use `ensure_compaction_backend()` which checks the model registry (like TTS handler does).

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/compaction.rs`
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs` (if needed for shared helper)

**What to implement:**

In `proxy/handlers/compaction.rs`:

1. **Add `ensure_compaction_backend()` function** (mirrors `ensure_tts_server()`):
   ```rust
   const COMPACTION_BACKEND_NAME: &str = "compaction";

   async fn ensure_compaction_backend(state: &ProxyState) -> anyhow::Result<String> {
       // Check if already loaded and get URL from ModelState
       if let Some(url) = get_backend_url(state, COMPACTION_BACKEND_NAME).await? {
           return Ok(url);
       }
       // Not loaded — try to load it
       state.load_compaction_backend().await?;
       // After loading, get the server URL from models map
       get_backend_url(state, COMPACTION_BACKEND_NAME)
           .await?
           .ok_or_else(|| anyhow::anyhow!("Compaction backend loaded but URL not set"))
   }
   ```

2. **Add `get_backend_url()` helper** (extract from TTS handler or duplicate):
   ```rust
   async fn get_backend_url(state: &ProxyState, backend_name: &str) -> anyhow::Result<Option<String>> {
       let models = state.models.read().await;
       Ok(models.get(backend_name).and_then(|ms| ms.backend_url()).map(|u| u.to_string()))
   }
   ```

3. **Rewrite handler** to call `ensure_compaction_backend()` instead of `state.ensure_compaction_server()`:
   - Check `compaction.enabled` from config
   - Call `ensure_compaction_backend(&state)`
   - Forward request to returned URL

4. **Extract `get_backend_url()` to shared location:**
   - Create `crates/tama-core/src/proxy/handlers/helpers.rs` with:
     ```rust
     pub(crate) async fn get_backend_url(state: &ProxyState, backend_name: &str) -> anyhow::Result<Option<String>> {
         let models = state.models.read().await;
         Ok(models.get(backend_name).and_then(|ms| ms.backend_url()).map(|u| u.to_string()))
     }
     ```
   - Add `pub(crate) mod helpers;` to `handlers/mod.rs`
   - In `tts.rs`, replace local `get_backend_url()` with `use super::helpers::get_backend_url;`
   - In `compaction.rs`, use `use super::helpers::get_backend_url;`

**Note:** After Task 2, the field `config.compaction.timeout_ms` is renamed to `request_timeout_ms`. Use the new name.

**Steps:**
- [ ] Create `crates/tama-core/src/proxy/handlers/helpers.rs` with `get_backend_url()`
- [ ] Add `pub(crate) mod helpers;` to `handlers/mod.rs`
- [ ] Update `tts.rs` to import `get_backend_url` from helpers instead of local definition
- [ ] Add `ensure_compaction_backend()` function in `compaction.rs`
- [ ] In `compaction.rs`, import `get_backend_url` from helpers
- [ ] Rewrite handler to use `ensure_compaction_backend()` and `request_timeout_ms` (renamed in Task 2)
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: rewrite compaction handler to use model registry"

**Acceptance criteria:**
- [ ] Handler calls `ensure_compaction_backend()` which checks model registry
- [ ] `get_backend_url()` helper shared between TTS and compaction handlers
- [ ] Handler still checks `compaction.enabled` from config
- [ ] Clippy clean

---

### Task 5: Delete custom compaction lifecycle and clean up

**Context:** Now that the compaction server is managed through the model registry, delete the old custom lifecycle code and clean up types.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (delete methods)
- Modify: `crates/tama-core/src/proxy/types.rs` (delete `CompactionServerState`)
- Modify: `crates/tama-core/src/proxy/state.rs` (delete `compaction_server` field and shutdown cleanup)

**What to implement:**

In `proxy/lifecycle/mod.rs`:
- Delete `ensure_compaction_server()` method
- Delete `spawn_compaction_server()` method
- Delete `wait_for_compaction_ready()` method

In `proxy/types.rs`:
- Delete `CompactionServerState` enum entirely
- Remove any imports/re-exports of `CompactionServerState`

In `proxy/state.rs`:
- Delete `compaction_server` field from `ProxyState` struct
- Delete compaction server shutdown cleanup in `shutdown()` method
- Update `ProxyState::new()` to not initialize `compaction_server`

**Steps:**
- [ ] Delete `ensure_compaction_server()`, `spawn_compaction_server()`, `wait_for_compaction_ready()` from lifecycle
- [ ] Delete `CompactionServerState` from proxy/types.rs
- [ ] Delete `compaction_server` field from ProxyState
- [ ] Delete compaction shutdown cleanup from state.rs
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --package tama-core`
- [ ] Commit with message: "refactor: remove custom compaction lifecycle, use model registry"

**Acceptance criteria:**
- [ ] `CompactionServerState` enum deleted
- [ ] `compaction_server` field removed from `ProxyState`
- [ ] Custom lifecycle methods deleted
- [ ] No references to deleted code remain
- [ ] Tests pass, clippy clean

---

### Task 6: Update web API — remove compaction from StructuredConfigBody if needed, verify round-trip

**Context:** After removing `startup_timeout_secs` from `CompactionConfig`, verify the web API round-trip still works. The `StructuredConfigBody` already has `compaction` field (added in PR #113).

**Files:**
- Modify: `crates/tama-web/src/api.rs` (if needed)
- Modify: `crates/tama-web/src/types/config.rs` (From impls already updated in Task 2)

**What to implement:**
- Verify `StructuredConfigBody` still has `compaction` field
- Verify `From<StructuredConfigBody> for CoreConfig` uses `b.compaction.into()` (fixed in PR #114)
- Verify `From<Config> for CoreConfig` uses `c.compaction.into()`
- Run full workspace check to catch any remaining issues

**Steps:**
- [ ] Verify StructuredConfigBody has compaction field
- [ ] Verify all From impls compile with new CompactionConfig
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "fix: verify web API round-trip for compaction config"

**Acceptance criteria:**
- [ ] Full workspace builds and clippy clean
- [ ] All tests pass
- [ ] Compaction config round-trips correctly through web API

---

## Rollout

1. Tasks 1-2 can be done in parallel (independent)
2. Task 3 depends on Task 1 (needs `BackendType::Compaction`)
3. Task 4 depends on Task 3 (needs `load_compaction_backend()`)
4. Task 5 depends on Task 4 (safe to delete old code after new code works)
5. Task 6 is verification, can run after Task 5
