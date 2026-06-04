# SSE-Powered Updates Page Plan

**Goal:** Replace fire-and-forget refresh buttons on the updates page with SSE-driven real-time updates, so clicking "Check" dynamically updates the UI without a page reload.

**Architecture:** Add a `broadcast::Sender<UpdateEvent>` to `UpdateChecker` in tama-core. Events carry `serde_json::Value` for the DTO payload (avoids cross-crate type dependency). Emit events from `check_backend()`/`check_model()` after each check completes. Add `GET /tama/v1/updates/events` SSE endpoint in tama-web. Frontend subscribes on mount using existing `SseConnection` utility and patches the `updates` RwSignal reactively.

**Tech Stack:** Rust, Axum SSE, tokio broadcast, Leptos CSR, `gloo-net` EventSource via `SseConnection`

---

### Data Format Reference (critical for correctness)

The REST API `GET /tama/v1/updates` transforms DB records before returning them to the frontend:

| Field | Backend DB format | Frontend DTO format | Model DB format | Frontend DTO format |
|-------|-------------------|---------------------|-----------------|---------------------|
| `item_id` | `"llama_cpp:cpu"` (composite) | `"llama_cpp"` (name only) | `"123"` (numeric) | `"model-123"` (config_key) |
| `variant` | — | `Some("cpu")` | — | `None` |

**The SSE events must emit DTOs in the FRONTEND DTO format**, not the DB format. This is the most common source of bugs — the `patch_list` matcher and `checking_key` lookups all depend on this.

---

## Task 1: Add UpdateEvent enum and broadcast sender to UpdateChecker

**Context:**
The `UpdateChecker` in tama-core has no awareness of any event system. It needs a broadcast sender so that `check_backend()` and `check_model()` can emit events after each check completes. We use `serde_json::Value` for the DTO payload in `CheckCompleted` to avoid defining `UpdateCheckDto` in tama-core (it currently only exists in tama-web). The frontend deserializes the Value into its own `UpdateCheckDto`.

**CRITICAL:** The DTO in `CheckCompleted` events must use frontend DTO format:
- Backend: `item_id` = `backend_name` (e.g., `"llama_cpp"`), `variant` = `Some(gpu_variant)` (e.g., `"cpu"`)
- Model: `item_id` = `format!("model-{}", model_id)` (e.g., `"model-123"`), `variant` = `None`

**Files:**
- Modify: `crates/tama-core/src/updates/checker.rs`
- Modify: `crates/tama-core/src/updates/mod.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

1. In `crates/tama-core/src/updates/checker.rs`:

   a. Add `UpdateEvent` enum after imports, guarded by `#[cfg(feature = "web-ui")]`:
   ```rust
   #[cfg(feature = "web-ui")]
   #[derive(Debug, Clone, serde::Serialize)]
   pub enum UpdateEvent {
       CheckStarted {
           item_type: String,
           item_id: String,
           variant: Option<String>,
       },
       CheckCompleted {
           item_type: String,
           item_id: String,
           variant: Option<String>,
           dto: serde_json::Value,
       },
       CheckError {
           item_type: String,
           item_id: String,
           variant: Option<String>,
           error: String,
       },
       CheckSkipped {
           item_type: String,
           reason: String,
       },
   }
   ```

   b. Add `update_events_tx` field to `UpdateChecker` struct (after existing fields):
   ```rust
   #[cfg(feature = "web-ui")]
   pub update_events_tx: Option<tokio::sync::broadcast::Sender<UpdateEvent>>,
   ```

   c. Update `UpdateChecker::new()` — add `update_events_tx: None` to the struct initialization.

   d. Add `set_update_events_tx` method:
   ```rust
   #[cfg(feature = "web-ui")]
   pub fn set_update_events_tx(&mut self, tx: tokio::sync::broadcast::Sender<UpdateEvent>) {
       self.update_events_tx = Some(tx);
   }
   ```

   e. Add private `emit` helper (non-blocking, fire-and-forget):
   ```rust
   #[cfg(feature = "web-ui")]
   fn emit(&self, event: UpdateEvent) {
       if let Some(ref tx) = self.update_events_tx {
           if let Err(e) = tx.try_send(event) {
               tracing::trace!("Dropped update event: {}", e);
           }
       }
   }
   ```

   f. In `check_backend()` — add event emission:

      - At the very top of the function (after computing `item_id` on line 204):
        ```rust
        // item_id is "name:variant" internally, but frontend DTO uses name-only
        #[cfg(feature = "web-ui")]
        self.emit(UpdateEvent::CheckStarted {
            item_type: "backend".to_string(),
            item_id: backend_name.to_string(),
            variant: Some(gpu_variant.to_string()),
        });
        ```

      - Replace the final `save_check_result` call (lines 257-268) with:
        ```rust
        let save_result = self.save_check_result(
            config_dir, "backend", &item_id,
            current_version.as_deref(), latest_version.as_deref(),
            update_available, status, None, None,
        ).await;

        #[cfg(feature = "web-ui")]
        if save_result.is_ok() {
            let dto = serde_json::json!({
                "item_type": "backend",
                "item_id": backend_name,
                "variant": gpu_variant,
                "current_version": current_version,
                "latest_version": latest_version,
                "update_available": update_available,
                "status": status,
                "error_message": null,
                "checked_at": chrono::Utc::now().timestamp(),
                "details_json": null,
            });
            self.emit(UpdateEvent::CheckCompleted {
                item_type: "backend".to_string(),
                item_id: backend_name.to_string(),
                variant: Some(gpu_variant.to_string()),
                dto,
            });
        }
        save_result
        ```

      - In the early-return error path (lines 225-237, where `check_latest_version` fails):
        After the existing `save_check_result` call, add:
        ```rust
        #[cfg(feature = "web-ui")]
        self.emit(UpdateEvent::CheckError {
            item_type: "backend".to_string(),
            item_id: backend_name.to_string(),
            variant: Some(gpu_variant.to_string()),
            error: e.to_string(),
        });
        ```

   g. In `check_model()` — same pattern:

      - At the very top of the function (after the repo_id validation block, before Phase 1):
        ```rust
        // Frontend DTO uses config_key format "model-{id}"
        #[cfg(feature = "web-ui")]
        let model_config_key = format!("model-{}", model_id);
        #[cfg(feature = "web-ui")]
        self.emit(UpdateEvent::CheckStarted {
            item_type: "model".to_string(),
            item_id: model_config_key.clone(),
            variant: None,
        });
        ```

      - After each successful `save_check_result` in the function (there may be multiple paths), emit `CheckCompleted` with:
        ```rust
        #[cfg(feature = "web-ui")]
        if save_result.is_ok() {
            let dto = serde_json::json!({
                "item_type": "model",
                "item_id": model_config_key,
                "variant": null,
                "current_version": current_version,
                "latest_version": latest_version,
                "update_available": update_available,
                "status": status,
                "error_message": null,
                "checked_at": chrono::Utc::now().timestamp(),
                "details_json": details_json,
            });
            self.emit(UpdateEvent::CheckCompleted {
                item_type: "model".to_string(),
                item_id: model_config_key,
                variant: None,
                dto,
            });
        }
        ```

      - In the early-return error path (lines 283-296, "no source repo"):
        After the existing `save_check_result`, add:
        ```rust
        #[cfg(feature = "web-ui")]
        self.emit(UpdateEvent::CheckError {
            item_type: "model".to_string(),
            item_id: model_config_key,
            variant: None,
            error: "Model has no source repo configured".to_string(),
        });
        ```

      - In the final error path (if `check_model` returns an error after the main logic), emit `CheckError` with the error message.

   h. In `run_check()` — emit `CheckSkipped` when lock fails (replace lines 110-115):
   ```rust
   let _guard = match self.lock.try_lock() {
       Ok(guard) => guard,
       Err(_) => {
           #[cfg(feature = "web-ui")]
           self.emit(UpdateEvent::CheckSkipped {
               item_type: "all".to_string(),
               reason: "Update check already in progress".to_string(),
           });
           tracing::info!("Update check already in progress, skipping");
           return Ok(());
       }
   };
   ```

2. In `crates/tama-core/src/updates/mod.rs`:
   - Add re-export (with `#[cfg(feature = "web-ui")]` guard):
   ```rust
   #[cfg(feature = "web-ui")]
   pub use checker::UpdateEvent;
   ```

3. In `crates/tama-core/src/proxy/state.rs`:
   - **Replace** the existing line `web_update_checker: Arc::new(crate::updates::UpdateChecker::new()),` with:
   ```rust
   #[cfg(feature = "web-ui")]
   web_update_checker: {
       let (tx, _) = tokio::sync::broadcast::channel::<crate::updates::UpdateEvent>(256);
       let mut checker = crate::updates::UpdateChecker::new();
       checker.set_update_events_tx(tx);
       Arc::new(checker)
   },
   ```

**Steps:**
- [ ] Add `UpdateEvent` enum to `checker.rs` with `serde_json::Value` for DTO payload
- [ ] Add `update_events_tx` field to `UpdateChecker` struct
- [ ] Update `new()` and add `set_update_events_tx()` and `emit()`
- [ ] Wire events into `check_backend()` — CheckStarted at entry, CheckCompleted after save, CheckError on failure. **Use `backend_name` for `item_id`, NOT the composite `item_id`.**
- [ ] Wire events into `check_model()` — same pattern. **Use `format!("model-{}", model_id)` for `item_id`.**
- [ ] Wire `CheckSkipped` into `run_check()` when lock fails
- [ ] Add `pub use checker::UpdateEvent;` to `mod.rs` with `#[cfg(feature = "web-ui")]`
- [ ] Replace `web_update_checker` initialization in `ProxyState::new()` to create channel and wire it
- [ ] Run `cargo build --package tama-core --features web-ui`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add UpdateEvent broadcast to UpdateChecker"

**Acceptance criteria:**
- [ ] `UpdateChecker` has `update_events_tx` field initialized from `ProxyState::new()`
- [ ] `check_backend()` emits CheckStarted with `item_id = backend_name` (not composite)
- [ ] `check_backend()` emits CheckCompleted with `item_id = backend_name`, `variant = gpu_variant`
- [ ] `check_model()` emits CheckStarted with `item_id = "model-{id}"` (config_key format)
- [ ] `check_model()` emits CheckCompleted with same config_key format
- [ ] `run_check()` emits CheckSkipped when lock is contested
- [ ] All events use `try_send` (non-blocking)
- [ ] Channel capacity is 256
- [ ] `UpdateEvent` is re-exported from `tama_core::updates`
- [ ] Code compiles with `--features web-ui`

---

## Task 2: Add SSE endpoint and wire route

**Context:**
The backend needs an SSE endpoint that the frontend can subscribe to. It follows the exact pattern from `download_events_sse` in `api/downloads.rs`. The endpoint maps `UpdateEvent` variants to named SSE events.

**Files:**
- Modify: `crates/tama-web/src/api/updates.rs`
- Modify: `crates/tama-web/src/router.rs`

**What to implement:**

1. In `crates/tama-web/src/api/updates.rs`:

   a. Add imports at the top of the file:
   ```rust
   use axum::response::{sse::Event, Sse, KeepAlive};
   use futures_util::Stream;
   use async_stream::stream;
   use tama_core::updates::UpdateEvent;
   ```

   b. Add `update_events_sse` handler (place it after `check_single`):
   ```rust
   /// GET /tama/v1/updates/events — SSE stream of update check lifecycle events.
   pub async fn update_events_sse(
       State(state): State<Arc<ProxyState>>,
   ) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
       let tx = state.web_update_checker.update_events_tx
           .as_ref()
           .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
       let mut rx = tx.subscribe();

       let event_stream = stream! {
           loop {
               match rx.recv().await {
                   Ok(event) => {
                       let (event_name, data) = match &event {
                           UpdateEvent::CheckStarted { item_type, item_id, variant } => (
                               "CheckStarted",
                               serde_json::json!({
                                   "item_type": item_type,
                                   "item_id": item_id,
                                   "variant": variant,
                               }),
                           ),
                           UpdateEvent::CheckCompleted { item_type, item_id, variant, dto } => (
                               "CheckCompleted",
                               serde_json::json!({
                                   "item_type": item_type,
                                   "item_id": item_id,
                                   "variant": variant,
                                   "dto": dto,
                               }),
                           ),
                           UpdateEvent::CheckError { item_type, item_id, variant, error } => (
                               "CheckError",
                               serde_json::json!({
                                   "item_type": item_type,
                                   "item_id": item_id,
                                   "variant": variant,
                                   "error": error,
                               }),
                           ),
                           UpdateEvent::CheckSkipped { item_type, reason } => (
                               "CheckSkipped",
                               serde_json::json!({
                                   "item_type": item_type,
                                   "reason": reason,
                               }),
                           ),
                       };
                       yield Event::default()
                           .event(event_name)
                           .json_data(data)
                           .map_err(axum::Error::new)?;
                   }
                   Err(broadcast::error::RecvError::Lagged(n)) => {
                       yield Event::default()
                           .event("Lagged")
                           .json_data(serde_json::json!({ "lagged": n }))?;
                   }
                   Err(broadcast::error::RecvError::Closed) => break,
               }
           }
       };

       Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
   }
   ```

2. In `crates/tama-web/src/router.rs`:
   - Add route after existing updates routes (around line 155, after the `check_single` route):
   ```rust
   .route("/tama/v1/updates/events", get(api::updates::update_events_sse))
   ```

**Steps:**
- [ ] Add SSE imports to `api/updates.rs`
- [ ] Implement `update_events_sse` handler with proper error handling (`ok_or` for Option)
- [ ] Add route to `router.rs`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add /tama/v1/updates/events SSE endpoint"

**Acceptance criteria:**
- [ ] `GET /tama/v1/updates/events` returns an SSE stream
- [ ] Returns `SERVICE_UNAVAILABLE` if sender is not initialized
- [ ] Events are properly formatted with named event types
- [ ] Lagged events are handled gracefully
- [ ] Route is registered in router
- [ ] Code compiles cleanly

---

## Task 3: Frontend SSE subscription and reactive updates

**Context:**
The updates page needs to subscribe to the SSE endpoint on mount and reactively patch its `updates` signal when events arrive. This replaces the fire-and-forget pattern with real-time UI updates. Rename "Refresh" → "Check" and add loading states.

**Key design decisions:**
- Server emits **named** events (CheckStarted, CheckCompleted, etc.). The `SseConnection::subscribe()` wraps `EventSource.subscribe(channel)` which uses `addEventListener()`. Named events dispatch to their own channel — so we subscribe to each named event type individually on a **single shared connection**.
- We use an `outstanding_checks` counter to track when all checks from "Check Now" are done.
- The `item_checking` key format is `"backend:{name}:{variant}"` for backends and `"model:{item_id}"` for models. The `item_id` for models is the config_key (e.g., `"model-123"` or `"unsloth--Qwen3.6-35B-A3B-GGUF"`).

**Files:**
- Modify: `crates/tama-web/src/pages/updates.rs`

**What to implement:**

1. Add new signals (after existing signal declarations, around line 160):
   ```rust
   // Cancelled flag for SSE cleanup on unmount
   let cancelled = RwSignal::new(false);
   OnCleanup::new(move || cancelled.set(true));

   // Per-item checking state: "backend:name:variant" → bool, "model:id" → bool
   let item_checking: RwSignal<std::collections::HashMap<String, bool>> =
       RwSignal::new(std::collections::HashMap::new());

   // Outstanding checks counter for "Check Now"
   let outstanding_checks = RwSignal::new(0u32);
   ```

2. Add `patch_list` helper function at module level (before the `Updates` component, after the DTO structs):
   ```rust
   /// Merge a DTO into the updates list, matching on (item_id, variant) for backends,
   /// item_id for models. Replaces existing entry or appends if new.
   fn patch_list(list: &mut Vec<UpdateCheckDto>, dto: &UpdateCheckDto) {
       if let Some(existing) = list.iter_mut().find(|i| {
           i.item_id == dto.item_id && i.variant == dto.variant
       }) {
           *existing = dto.clone();
       } else {
           list.push(dto.clone());
       }
   }
   ```

3. Add `handle_update_event` function at module level (before `Updates` component):
   ```rust
   fn handle_update_event(
       event_type: &str,
       data: &serde_json::Value,
       updates: RwSignal<UpdatesListResponse>,
       last_checked: RwSignal<Option<i64>>,
       item_checking: RwSignal<std::collections::HashMap<String, bool>>,
       checking: RwSignal<bool>,
       error: RwSignal<Option<String>>,
       outstanding: RwSignal<u32>,
   ) {
       let item_type: Option<String> = data.get("item_type").and_then(|v| v.as_str().map(String::from));
       let item_id: Option<String> = data.get("item_id").and_then(|v| v.as_str().map(String::from));
       let variant: Option<String> = data.get("variant").and_then(|v| v.as_str().map(String::from));

       // Build checking key: "backend:name:variant" or "model:id"
       let checking_key = match (&item_type, &item_id, &variant) {
           (Some("backend"), Some(id), Some(v)) => format!("backend:{}:{}", id, v),
           (Some("backend"), Some(id), None) => format!("backend:{}", id),
           (Some("model"), Some(id), _) => format!("model:{}", id),
           _ => format!("{}:{}", item_type.as_deref().unwrap_or(""), item_id.as_deref().unwrap_or("")),
       };

       match event_type {
           "CheckStarted" => {
               item_checking.update(|m| m.insert(checking_key.clone(), true));
               outstanding.update(|n| *n += 1);
           }
           "CheckCompleted" => {
               item_checking.update(|m| m.remove(&checking_key));
               outstanding.update(|n| if *n > 0 { *n -= 1 } else { *n });
               if outstanding.get() == 0 {
                   checking.set(false);
               }
               // Patch the updates list
               if let Some(dto_value) = data.get("dto") {
                   if let Ok(dto) = serde_json::from_value::<UpdateCheckDto>(dto_value.clone()) {
                       updates.update(|u| {
                           match item_type.as_deref() {
                               Some("backend") => patch_list(&mut u.backends, &dto),
                               Some("model") => patch_list(&mut u.models, &dto),
                               _ => {}
                           }
                       });
                       last_checked.set(Some(dto.checked_at));
                   }
               }
           }
           "CheckError" => {
               item_checking.update(|m| m.remove(&checking_key));
               outstanding.update(|n| if *n > 0 { *n -= 1 } else { *n });
               if outstanding.get() == 0 {
                   checking.set(false);
               }
               if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
                   error.set(Some(format!("{}: {}", item_id.as_deref().unwrap_or("item"), err)));
               }
           }
           "CheckSkipped" => {
               checking.set(false);
               outstanding.set(0);
               item_checking.set(std::collections::HashMap::new());
               if let Some(reason) = data.get("reason").and_then(|v| v.as_str()) {
                   error.set(Some(reason.to_string()));
               }
           }
           "Lagged" => {
               item_checking.set(std::collections::HashMap::new());
           }
           _ => {}
       }
   }
   ```

4. Add SSE subscription Effect (after the existing mount fetch Effect, around line 180):
   ```rust
   Effect::new(move |_| {
       let updates = updates;
       let last_checked = last_checked;
       let item_checking = item_checking;
       let checking = checking;
       let error = error;
       let outstanding = outstanding_checks;
       wasm_bindgen_futures::spawn_local(async move {
           // Create ONE connection, subscribe to multiple named event types
           let conn = crate::utils::sse_stream::create(
               "/tama/v1/updates/events".to_string(),
               cancelled,
               None,
           );
           if conn.connect_once().await.is_err() {
               return;
           }

           let event_types = ["CheckStarted", "CheckCompleted", "CheckError", "CheckSkipped", "Lagged"];
           for event_type in &event_types {
               // Clone signals for each spawned task
               let u = updates;
               let lc = last_checked;
               let ic = item_checking;
               let ch = checking;
               let er = error;
               let out = outstanding;
               let et = event_type.to_string();

               // Subscribe on the SAME connection (not a new one)
               match conn.subscribe(event_type) {
                   Ok(mut stream) => {
                       wasm_bindgen_futures::spawn_local(async move {
                           use futures_util::StreamExt;
                           while let Some(result) = stream.next().await {
                               if let Ok(event) = result {
                                   let data: serde_json::Value =
                                       serde_json::from_str(&event.data).unwrap_or_default();
                                   handle_update_event(&et, &data, u, lc, ic, ch, er, out);
                               }
                           }
                       });
                   }
                   Err(e) => {
                       tracing::debug!("Failed to subscribe to {}: {}", event_type, e);
                   }
               }
           }
       });
   });
   ```

5. Replace per-backend "Refresh" button with "Check" and loading state:

   a. Add `on_check_backend` handler (add it near the other handlers, before `on_update_backend`):
   ```rust
   let on_check_backend = move |(name, variant): (String, Option<String>)| {
       let key = match &variant {
           Some(v) => format!("backend:{}:{}", name, v),
           None => format!("backend:{}", name),
       };
       item_checking.update(|m| m.insert(key.clone(), true));
       let error_key = key.clone();
       wasm_bindgen_futures::spawn_local(async move {
           let url = format!("/tama/v1/updates/check/backend/{}", urlencoding::encode(&name));
           match post_request(&url).send().await {
               Ok(resp) if !resp.ok() => {
                   let text = resp.text().await.unwrap_or_default();
                   error.update(|e| *e = Some(format!("Check failed: {}", text)));
                   item_checking.update(|m| m.remove(&error_key));
               }
               Err(e) => {
                   error.update(|e| *e = Some(format!("Check failed: {}", e)));
                   item_checking.update(|m| m.remove(&error_key));
               }
               _ => { /* success — SSE clears checking state */ }
           }
       });
   };
   ```

   b. Update the button rendering in the backend list (replace lines 375-384, the inline Refresh button):
   ```rust
   {/* Check button with loading state */}
   {let btn_key = match &variant_for_update {
       Some(v) => format!("backend:{}:{}", item_id, v),
       None => format!("backend:{}", item_id),
   };
   let is_checking = move || item_checking.with(|m| m.get(&btn_key).copied().unwrap_or(false));
   view! {
       <button
           class="btn btn-ghost"
           disabled=is_checking
           on:click=move |_| on_check_backend((item_id.clone(), variant_for_update.clone()))
       >
           {move || if is_checking() { "Checking..." } else { "Check" }}
       </button>
   }}
   ```

   c. Remove the old inline `wasm_bindgen_futures::spawn_local` closure that was directly inside the button's `on:click` attribute (lines 376-382).

6. Update "Check Now" handler — remove 2s blind poll, keep 30s fallback:
   ```rust
   let on_check_now = move |_| {
       checking.set(true);
       error.set(None);
       outstanding_checks.set(0);
       wasm_bindgen_futures::spawn_local(async move {
           match post_request("/tama/v1/updates/check").send().await {
               Ok(resp) if resp.ok() => {
                   // SSE events update cards progressively.
                   // Fallback: if no events arrive within 30s, poll once.
                   gloo_timers::future::TimeoutFuture::new(30000).await;
                   if checking.get() && outstanding_checks.get() == 0 {
                       if let Ok(resp2) = get_request("/tama/v1/updates").send().await {
                           if let Ok(data) = resp2.json::<UpdatesListResponse>().await {
                               updates.set(data);
                           }
                       }
                       checking.set(false);
                   }
               }
               _ => {
                   error.set(Some("Failed to trigger check".to_string()));
                   checking.set(false);
               }
           }
       });
   };
   ```

7. Add model "Check" button:

   a. Add `on_check_model` handler (replace the existing `_on_refresh_model`):
   ```rust
   let on_check_model = move |id: String| {
       let key = format!("model:{}", id);
       item_checking.update(|m| m.insert(key.clone(), true));
       let error_key = key.clone();
       wasm_bindgen_futures::spawn_local(async move {
           // The item_id in the DTO is the config_key (e.g., "model-123" or "owner--repo-name")
           let url = format!("/tama/v1/updates/check/model/{}", urlencoding::encode(&id));
           match post_request(&url).send().await {
               Ok(resp) if !resp.ok() => {
                   let text = resp.text().await.unwrap_or_default();
                   error.update(|e| *e = Some(format!("Check failed: {}", text)));
                   item_checking.update(|m| m.remove(&error_key));
               }
               Err(e) => {
                   error.update(|e| *e = Some(format!("Check failed: {}", e)));
                   item_checking.update(|m| m.remove(&error_key));
               }
               _ => { /* success — SSE clears checking state */ }
           }
       });
   };
   ```

   b. In the model card actions area (replace the existing `actions=Some(Box::new(...))` around line 491-495):
   ```rust
   actions=Some(Box::new(move || {
       let m_id = m_item_id_for_actions.clone();
       let model_key = format!("model:{}", m_id);
       let is_checking = move || item_checking.with(|m| m.get(&model_key).copied().unwrap_or(false));
       view! {
           <a href=format!("/tama/model/{}/edit", m_item_id_for_actions.clone()) class="btn btn-ghost btn-sm">
               "Edit"
           </a>
           <button
               class="btn btn-ghost btn-sm"
               disabled=is_checking
               on:click=move |_| on_check_model(m_id.clone())
           >
               {move || if is_checking() { "Checking..." } else { "Check" }}
           </button>
       }.into_any()
   }))
   ```

   c. Remove the old `_on_refresh_model` handler (line 248-253).

**Steps:**
- [ ] Add `cancelled`, `item_checking`, `outstanding_checks` signals and `OnCleanup`
- [ ] Add `patch_list` helper function at module level
- [ ] Add `handle_update_event` function at module level
- [ ] Add SSE subscription Effect: ONE connection, subscribe to each named event type, spawn task per channel
- [ ] Add `on_check_backend` handler with proper error handling (clears checking on failure)
- [ ] Replace per-backend "Refresh" button with "Check" + loading state
- [ ] Update "Check Now" to remove 2s blind poll, add 30s fallback, reset `outstanding_checks`
- [ ] Add `on_check_model` handler (replaces `_on_refresh_model`)
- [ ] Add model "Check" button in card actions alongside "Edit"
- [ ] Remove old `_on_refresh_model` handler
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: SSE-driven updates page with real-time Check buttons"

**Acceptance criteria:**
- [ ] SSE subscription opens on mount, closes on unmount via `cancelled` signal
- [ ] Single connection subscribes to all named event types (not 5 separate connections)
- [ ] Clicking "Check" on a backend shows "Checking..." then updates card from SSE event
- [ ] Clicking "Check" on a model shows "Checking..." then updates card from SSE event
- [ ] Failed POST requests clear checking state and show error
- [ ] Clicking "Check Now" updates cards progressively via SSE events
- [ ] 30-second fallback poll if SSE events don't arrive
- [ ] `CheckSkipped` shows error message and clears all checking states
- [ ] `Lagged` resets all item checking states
- [ ] `patch_list` matches on `(item_id, variant)` for backends
- [ ] `outstanding_checks` counter tracks in-flight checks, clears `checking` when zero
- [ ] Button text is "Check" (not "Refresh")
- [ ] `_on_refresh_model` is removed
- [ ] Code compiles cleanly

---

## Verification

After all tasks complete:
1. `cargo build --release --workspace` — full build
2. `cargo clippy --workspace -- -D warnings` — linting
3. `cargo fmt --all` — formatting
4. `cargo test --workspace` — all tests pass
5. Manual test: open updates page, click "Check" on a backend, verify card updates without page reload
6. Manual test: click "Check Now", verify all cards update progressively
7. Manual test: click "Check" twice rapidly, verify second click shows "already in progress" error
8. Manual test: click "Check" on a model, verify card updates
