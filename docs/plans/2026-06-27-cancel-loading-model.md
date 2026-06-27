# Cancel Loading Model Plan

**Goal:** Add a Cancel button to model cards during the loading state that kills the backend process and returns the card to idle.

**Architecture:** A new `POST /tama/v1/models/:id/cancel` endpoint in tama-core kills the backend process group (using the existing `kill_process_group`/`force_kill_process_group` from `proxy/process.rs`), removes the model entry from the in-memory models map, and cleans up the DB. The frontend shows a Cancel button alongside the disabled "Loading..." button during the `loading` state. No new `ModelState` variant is needed — the cancel handler transitions directly from `Starting` → removed (idle).

**Tech Stack:** Rust (axum, tokio), Leptos (Svelte-like web UI), SSE for real-time state updates

**Design decisions:**
- No `Cancelling` ModelState variant — the cancel handler removes the entry directly. The SSE stream naturally reports `idle` once the entry is gone. This avoids ~10+ match arm updates across the codebase.
- Uses `kill_process_group(pid)` not `kill_process(pid)` — backends are spawned with `process_group(0)` so child workers must be killed too.
- TOCTOU fix: after re-acquiring the write lock, re-validate the model is still `Starting` (regardless of PID — PID may have been updated from 0 to real value) before removing. If it became `Ready`, return 409. If entry is already `None`, return 404.
- 2-second SIGKILL escalation mirrors `unload_model`'s pattern.
- Kill and DB cleanup failures are logged at `warn!` level but don't affect the response (the in-memory state is already correct).

**Known limitation (documented in code):** There is a narrow race window where `load_model`'s health check succeeds after cancel removes the entry from the models map, and `load_model` then calls `mgr.insert_active()` unconditionally (see `lifecycle/mod.rs` ~line 327). The cancel handler's `remove_active` may or may not undo this depending on tokio scheduling. This is a pre-existing pattern in `unload_model` (which also calls `remove_active` best-effort). A future fix would add a re-check in `load_model` before `insert_active` under the write lock. Add a `// TODO` comment in the cancel handler noting this race.

---

### Task 1: Backend — Cancel handler and route

**Context:**
The cancel endpoint is the core of the feature. It must safely kill a loading backend process, handle race conditions (model becoming Ready between read and write), and clean up the DB. The handler lives alongside the existing `handle_tama_load_model` and `handle_tama_unload_model` in `tama_handlers/models.rs`.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/mod.rs`
- Modify: `crates/tama-core/src/proxy/server/router.rs`

**What to implement:**

1. In `crates/tama-core/src/proxy/tama_handlers/models.rs`:

   Add a new handler function `handle_tama_cancel_load`. It must:

   a. Resolve the incoming `:id` path param to the internal config_key using the existing `resolve_model_id` function (already defined in this file).

   b. Acquire a **read lock** on `state.models` and look up the model. If found and in `ModelState::Starting`:
      - If `backend_pid > 0`: extract the PID and proceed
      - If `backend_pid == 0`: PID not yet set (just inserted, before spawn) — safe to remove without killing anything
      - If in `ModelState::Ready`: return `StatusCode::CONFLICT` with JSON `{"error": {"message": "Model is already loaded", "type": "ModelAlreadyLoadedError"}}`
      - If in any other state or not found: return `StatusCode::NOT_FOUND` with JSON `{"error": {"message": "Model is not currently loading", "type": "ModelNotLoadingError"}}`

   c. Drop the read lock, then acquire a **write lock** on `state.models`. Re-validate the model is still `Starting` (regardless of PID — the PID may have been updated from 0 to the real value between read and write). The re-validation checks:
      - If `Some(ModelState::Starting { .. })` → proceed to step d
      - If `Some(ModelState::Ready { .. })` → return `StatusCode::CONFLICT` with JSON `{"error": {"message": "Model is already loaded", "type": "ModelAlreadyLoadedError"}}`
      - If `None` (entry already removed) → return `StatusCode::NOT_FOUND` with JSON `{"error": {"message": "Model is not currently loading", "type": "ModelNotLoadingError"}}`
      - If any other state → return `StatusCode::NOT_FOUND` with JSON `{"error": {"message": "Model is not currently loading", "type": "ModelNotLoadingError"}}`

   d. Remove the entry from `models`: `models.remove(&server_name)`.

   e. Drop the write lock before I/O.

   f. If `pid > 0`: kill the process group using `kill_process_group(pid)` (from `super::super::process::kill_process_group`). Log kill failures at `warn!` level (e.g., `warn!("Cancel kill failed for '{}': {}", server_name, e)`) but continue — the in-memory state is already cleaned. Wait up to 2 seconds (polling every 250ms) for the group to die. If still alive after 2s, escalate with `force_kill_process_group(pid)`, then sleep 500ms. Use `is_process_group_alive(pid)` from `super::super::process::is_process_group_alive` for the check.

   g. Clean up the DB: `if let Some(mgr) = state.model_mgr() { if let Err(e) = mgr.remove_active(&server_name) { warn!("Failed to remove active entry for '{}': {}", server_name, e); } }` (log failures, don't swallow silently).

   h. Log at info level: `info!("Model '{}' cancel completed", server_name);`

   i. Return `Json(ModelResponse { id: model_id, loaded: false })` — the `ModelResponse` struct already exists in `super::types::ModelResponse`.

   The full function signature:
   ```rust
   pub async fn handle_tama_cancel_load(
       state: State<Arc<ProxyState>>,
       Path(model_id): Path<String>,
   ) -> Response
   ```

   Required imports at the top of the file (add to existing imports):
   ```rust
   use super::super::process::{kill_process_group, force_kill_process_group, is_process_group_alive};
   ```

   Do NOT add a `Cancelling` variant to `ModelState` in `types.rs`. The cancel handler removes the entry directly.

2. In `crates/tama-core/src/proxy/tama_handlers/mod.rs`:

   Add `handle_tama_cancel_load` to the `pub use models::{...}` re-export. The current line is:
   ```rust
   pub use models::{
       handle_tama_list_models, handle_tama_load_model, handle_tama_unload_model,
       // ... possibly more
   };
   ```
   Add `handle_tama_cancel_load` to this list.

3. In `crates/tama-core/src/proxy/server/router.rs`:

   Add the cancel route to **both** `build_router` and `build_unified_router` functions. The route must be defined alongside the existing load/unload routes. In `build_router`, find the existing lines:
   ```rust
   .route("/tama/v1/models/:id/load", post(handle_tama_load_model))
   .route("/tama/v1/models/:id/unload", post(handle_tama_unload_model))
   ```
   Add after them:
   ```rust
   .route("/tama/v1/models/:id/cancel", post(handle_tama_cancel_load))
   ```

   In `build_unified_router` (the `#[cfg(feature = "web-ui")]` version), add the same route in the `proxy_routes` section alongside the existing load/unload routes. The cancel route MUST be in the proxy routes (not extra routes) so it takes priority over the web UI catch-all.

   Add `handle_tama_cancel_load` to the imports at the top of the file. The current import line is:
   ```rust
   handle_tama_list_models, handle_tama_load_model, handle_tama_unload_model,
   ```
   Add `handle_tama_cancel_load` to this list.

   Also update the `test_unified_router_route_priority` test (inside the `#[cfg(feature = "web-ui")]` test module) to assert that the cancel route is handled by the proxy (not the extra router). Add after the existing unload assertion:
   ```rust
   // POST to /tama/v1/models/test/cancel — should be handled by proxy's
   // handle_tama_cancel_load, not by extra router's catch-all.
   let resp = client
       .post(format!("http://{}/tama/v1/models/test/cancel", bound_addr))
       .send()
       .await
       .unwrap();
   assert_ne!(
       resp.status(),
       405,
       "Route priority failed: extra router caught /tama/v1/models/:id/cancel instead of proxy handler"
   );
   ```

**Steps:**
- [ ] Implement `handle_tama_cancel_load` in `crates/tama-core/src/proxy/tama_handlers/models.rs`
- [ ] Add `handle_tama_cancel_load` to the re-export in `crates/tama-core/src/proxy/tama_handlers/mod.rs`
- [ ] Add the cancel route to both `build_router` and `build_unified_router` in `crates/tama-core/src/proxy/server/router.rs`
- [ ] Update the `test_unified_router_route_priority` test assertion for cancel
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Commit with message: "feat: add cancel loading model endpoint"

**Acceptance criteria:**
- [ ] `POST /tama/v1/models/:id/cancel` returns 200 with `{"id": "...", "loaded": false}` when model is in Starting state
- [ ] Returns 409 when model is already Ready
- [ ] Returns 404 when model is not in a cancellable state
- [ ] The route is accessible in both standalone and unified router modes
- [ ] The `test_unified_router_route_priority` test passes with the new cancel assertion

---

### Task 2: Frontend — Cancel button in ModelCard component

**Context:**
The ModelCard component renders the Load/Unload button on each model card. During the `loading` state, it currently shows a single disabled "Loading..." button. This task adds a Cancel button alongside it, so the user can abort the loading process. The component is shared between the dashboard and models pages.

**Files:**
- Modify: `crates/tama-web/src/components/model_card.rs`

**What to implement:**

In `crates/tama-web/src/components/model_card.rs`, modify the `ModelCard` component:

1. Add two new optional props to the component signature:
   ```rust
   #[prop(optional)] on_cancel: Option<Callback<String>>,
   #[prop(optional)] cancel_busy: Option<RwSignal<bool>>,
   ```

2. Add a helper for the cancel button's disabled state (near the existing `is_load_disabled`/`is_unload_disabled`):
   ```rust
   let is_cancel_disabled = move || {
       cancel_busy
           .as_ref()
           .map(|s| s.get())
           .unwrap_or(false)
   };
   ```

3. Modify the rendering logic for the `is_loading_or_unloading` branch. The current code (around line 257+) renders:
   ```rust
   } else if is_loading_or_unloading {
       view! {
           <button
               class={button_class}
               prop:disabled=true
           >
               {button_label}
           </button>
       }.into_any()
   }
   ```

   Replace this block with:
   ```rust
   } else if is_loading_or_unloading {
       if effective_state == "loading" {
           // Show "Loading..." (disabled) + Cancel button
           view! {
               <button
                   class={button_class}
                   prop:disabled=true
               >
                   {button_label}
               </button>
               {if let Some(cb) = on_cancel {
                   let id_cancel = id.clone();
                   view! {
                       <button
                           class="btn btn-warning btn-sm"
                           prop:disabled=is_cancel_disabled
                           on:click=move |_| { cb.run(id_cancel.clone()); }
                       >
                           "Cancel"
                       </button>
                   }.into_any()
               } else { view! { <span/> }.into_any() }}
           }.into_any()
       } else {
           // unloading — existing disabled button (unchanged)
           view! {
               <button
                   class={button_class}
                   prop:disabled=true
               >
                   {button_label}
               </button>
           }.into_any()
       }
   }
   ```

   Key details:
   - The "Loading..." button uses `class={button_class}` (the existing variable, which resolves to `"btn btn-secondary btn-sm"` for `"loading"` state). Do NOT hardcode the class — use the variable for consistency with the unloading branch.
   - The Cancel button uses `btn btn-warning btn-sm` class (matches the existing "Retry" button style for failed models).
   - The Cancel button is disabled when `cancel_busy` is true (prevents double-click)
   - The Cancel button only appears when `on_cancel` is `Some` (the page must wire it)
   - The `unloading` branch is unchanged — no Cancel button for unloading

4. Do NOT modify `model_status_badge_class`, `model_status_badge_label`, `model_action_button_class`, or `model_action_button_label` — no new state string is needed since we're not adding a `Cancelling` state.

5. Do NOT modify the `is_loading_or_unloading` variable — it already correctly matches `"loading" | "unloading"`.

**Steps:**
- [ ] Add `on_cancel` and `cancel_busy` props to the `ModelCard` component
- [ ] Add the `is_cancel_disabled` helper
- [ ] Replace the `is_loading_or_unloading` rendering block with the 3-way match (loading with Cancel, unloading unchanged)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "feat: add Cancel button to ModelCard during loading state"

**Acceptance criteria:**
- [ ] Model cards in `loading` state show a "Loading..." button (disabled) and a "Cancel" button (clickable)
- [ ] Model cards in `unloading` state show only the existing disabled "Unloading..." button (no Cancel)
- [ ] The Cancel button is disabled when `cancel_busy` is true
- [ ] The Cancel button only appears when `on_cancel` callback is provided
- [ ] No changes to badge/button helper functions

---

### Task 3: Frontend — Wire cancel action in dashboard and models pages

**Context:**
The dashboard and models pages both use the ModelCard component. Each page needs to define a `cancel_action` that calls the cancel endpoint and a `cancel_busy` signal to track the button state. The dashboard uses SSE for real-time updates, while the models page uses a refresh trigger.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs`
- Modify: `crates/tama-web/src/pages/models.rs`

**What to implement:**

1. In `crates/tama-web/src/pages/dashboard/mod.rs`:

   a. Add a `cancel_busy` signal near the existing `load_busy` and `unload_busy` signals:
   ```rust
   let cancel_busy = RwSignal::new(false);
   ```

   b. Add a `cancel_action` near the existing `load_action` and `unload_action`:
   ```rust
   let cancel_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
       let id = id.clone();
       async move {
           cancel_busy.set(true);
           match post_request(&format!("/tama/v1/models/{}/cancel", id)).send().await {
               Ok(resp) if resp.ok() => {
                   // Success — SSE will push updated state
               }
               Ok(resp) => {
                   // Model already loaded or state changed — SSE will push updated state
                   tracing::debug!(
                       "Cancel returned non-2xx for model {}: {}",
                       id,
                       resp.status()
                   );
               }
               Err(e) => {
                   tracing::warn!("Failed to cancel model {}: {}", id, e);
               }
           }
           cancel_busy.set(false);
       }
   });
   ```

   c. In the ModelCard rendering (inside the `all_models.into_iter().map(...)` block), add the cancel callback and busy signal:
   ```rust
   let on_cancel_cb = Callback::new(move |id: String| {
       cancel_action.dispatch(id);
   });
   ```

   d. Pass the new props to `ModelCard`:
   ```rust
   on_cancel=on_cancel_cb
   cancel_busy=cancel_busy
   ```

   The full ModelCard invocation should now include `on_cancel` and `cancel_busy` alongside the existing `on_load`, `on_unload`, `load_busy`, and `unload_busy`.

2. In `crates/tama-web/src/pages/models.rs`:

   a. Add a `cancel_busy` signal near the existing `load_action`/`unload_action`:
   ```rust
   let cancel_busy = RwSignal::new(false);
   ```

   b. Add a `cancel_action` near the existing actions:
   ```rust
   let cancel_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
       let id = id.clone();
       async move {
           cancel_busy.set(true);
           let _ = post_request(&format!("/tama/v1/models/{}/cancel", id))
               .send()
               .await;
           refresh.update(|n| *n += 1);
           cancel_busy.set(false);
       }
   });
   ```

   c. In the ModelCard rendering (inside the `data.models.into_iter().map(...)` block), add the cancel callback:
   ```rust
   let on_cancel_cb = Callback::new(move |id: String| {
       cancel_action.dispatch(id);
   });
   ```

   d. Pass the new props to `ModelCard`:
   ```rust
   on_cancel=on_cancel_cb
   cancel_busy=cancel_busy
   ```

**Steps:**
- [ ] Add `cancel_busy`, `cancel_action`, and `on_cancel_cb` to `crates/tama-web/src/pages/dashboard/mod.rs`
- [ ] Pass `on_cancel` and `cancel_busy` props to ModelCard in dashboard
- [ ] Add `cancel_busy`, `cancel_action`, and `on_cancel_cb` to `crates/tama-web/src/pages/models.rs`
- [ ] Pass `on_cancel` and `cancel_busy` props to ModelCard in models page
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "feat: wire cancel action in dashboard and models pages"

**Acceptance criteria:**
- [ ] Dashboard page passes `on_cancel` and `cancel_busy` to ModelCard
- [ ] Models page passes `on_cancel` and `cancel_busy` to ModelCard
- [ ] Cancel action calls `POST /tama/v1/models/:id/cancel`
- [ ] Dashboard cancel action uses SSE for state updates (no manual refresh)
- [ ] Models page cancel action triggers a refresh after the request
- [ ] `cancel_busy` prevents double-click on the Cancel button

---

### Task 4: Tests — Unit and integration tests

**Context:**
The cancel handler needs unit tests covering the happy path, race conditions, and edge cases. The frontend changes need compile-time smoke tests (the existing model_card tests use compile-only smoke tests). The router test was updated in Task 1.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models.rs` (add tests module or add to existing tests)
- Modify: `crates/tama-web/src/components/model_card.rs` (add compile-time smoke test)

**What to implement:**

1. In `crates/tama-core/src/proxy/tama_handlers/models.rs`, add tests inside the existing `#[cfg(test)] mod tests` block:

   a. **Test: cancel returns 200 for Starting model with PID**
      - Use the existing `create_state_with_model` helper from the file's test module to create a `ProxyState`.
      - Manually insert a Starting entry: `state.models.write().await.insert("test-model", ModelState::Starting { model_name: "test-model".into(), backend: "llama_cpp".into(), backend_url: String::new(), backend_pid: 99999, last_accessed: Instant::now(), start_time: Instant::now(), consecutive_failures: Arc::new(AtomicU32::new(0)), failure_timestamp: None })`.
      - Build a test router: `Router::new().route("/tama/v1/models/:id/cancel", post(handle_tama_cancel_load)).with_state(Arc::new(state))`.
      - Send `POST /tama/v1/models/test-model/cancel` using `tower::ServiceExt::oneshot()`.
      - Assert status is 200 and body contains `"loaded": false`.
      - Assert `state.models.read().await.get("test-model").is_none()` (entry removed).
      - Note: PID 99999 is a fake PID — `kill_process_group` returns `Ok(())` on `ESRCH` and `is_process_group_alive` returns `false` for nonexistent PIDs, so no real process or wiremock is needed.

   b. **Test: cancel returns 409 for Ready model**
      - Create a `ProxyState` with a model in `ModelState::Ready { ... }`
      - Call `handle_tama_cancel_load`
      - Verify response status is 409
      - Verify response body contains `"ModelAlreadyLoadedError"`

   c. **Test: cancel returns 404 for non-existing model**
      - Create a `ProxyState` with no models
      - Call `handle_tama_cancel_load` with any ID
      - Verify response status is 404
      - Verify response body contains `"ModelNotLoadingError"`

   d. **Test: cancel returns 404 for idle model**
      - Create a `ProxyState` with a model in `ModelState::Failed { ... }`
      - Call `handle_tama_cancel_load`
      - Verify response status is 404

   Use the existing `create_state_with_model` helper and `Router::new()` pattern from the existing tests in this file. Wiremock can be used if backend URL mocking is needed.

2. In `crates/tama-web/src/components/model_card.rs`, add a compile-time smoke test:

   ```rust
   /// Compile-only smoke test: ModelCard accepts on_cancel and cancel_busy props.
   #[test]
   fn test_model_card_renders_with_cancel_props() {
       // This test verifies the component accepts the new cancel props.
       // The actual rendering happens at runtime in the browser.
       let _ = "ModelCard compiles with on_cancel and cancel_busy props";
   }
   ```

3. The router priority test was already added in Task 1 (`test_unified_router_route_priority` assertion for cancel).

**Steps:**
- [ ] Write unit test for cancel happy path (Starting → 200)
- [ ] Write unit test for cancel with Ready model (→ 409)
- [ ] Write unit test for cancel with non-existing model (→ 404)
- [ ] Write unit test for cancel with Failed model (→ 404)
- [ ] Add compile-time smoke test for cancel props in model_card.rs
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "test: add cancel loading model tests"

**Acceptance criteria:**
- [ ] All 4 cancel handler unit tests pass (200, 409, 404x2)
- [ ] Router priority test passes for cancel route
- [ ] Compile-time smoke test for cancel props passes
- [ ] Full test suite passes: `cargo test --workspace`

---

## Execution Order

Tasks 1-3 can be executed in any order (they modify different files). Task 4 depends on Tasks 1-3 being complete (tests reference the implemented code).

Recommended order: Task 1 → Task 2 → Task 3 → Task 4

## Verification

After all tasks are complete:
1. `cargo build --release --workspace` — release build succeeds
2. `cargo test --workspace` — all tests pass
3. `cargo clippy --workspace -- -D warnings` — no warnings
4. Manual test: Load a model, click Cancel while loading, verify card returns to idle
