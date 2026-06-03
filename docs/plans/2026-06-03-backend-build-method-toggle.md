# Backend Build Method Toggle Plan

**Goal:** Add a "Build from source" toggle on each backend card that persists to the DB, letting users switch between prebuilt download and source build for updates.

**Architecture:** The toggle updates the `source` JSON column in the `backend_installations` SQLite table via a new POST endpoint. The existing update flow already reads `source` from DB, so no changes to the update handler are needed. The frontend uses a callback pattern (existing `BackendCard` convention) to keep API calls in the parent page.

**Tech Stack:** Rust (tama-core, tama-web), Leptos 0.7 (WASM frontend), SQLite (rusqlite)

---

### Task 1: Core — DB query + manager method for updating build method

**Context:**
The `backend_installations` table has a `source TEXT` column (JSON-serialized `BackendSource`). We need a DB query to update just this column on the active row, and a manager method that constructs the correct `BackendSource` enum variant and persists it. This is the foundation — all other tasks depend on it.

**Files:**
- Modify: `crates/tama-core/src/backends/types.rs`
- Modify: `crates/tama-core/src/db/queries/backend_queries.rs`
- Modify: `crates/tama-core/src/backends/manager.rs`

**What to implement:**

0. **In `types.rs`, add `default_git_url` helper on `BackendType`:**
   ```rust
   impl BackendType {
       /// Return the canonical git URL for cloning this backend's source code.
       pub fn default_git_url(&self) -> &'static str {
           match self {
               BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
               BackendType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
               BackendType::TtsKokoro | BackendType::Custom => {
                   "https://github.com/ggml-org/llama.cpp.git" // fallback, never reached in practice
               }
           }
       }
   }
   ```
   This centralizes the git URL strings that are currently duplicated across `install.rs` and `manage.rs`.

1. **In `backend_queries.rs`, add `update_backend_source` function:**
   ```rust
   pub fn update_backend_source(
       conn: &Connection,
       name: &str,
       gpu_variant: &str,
       source_json: &str,
   ) -> Result<()>
   ```
   - SQL: `UPDATE backend_installations SET source = ?1 WHERE name = ?2 AND gpu_variant = ?3 AND is_active = 1`
   - **Explicitly check zero rows:** `let rows = conn.execute(...)?; if rows == 0 { anyhow::bail!("No active backend '{}' variant '{}' found", name, gpu_variant); }`
   - Follow the existing pattern: use `conn.execute()`, wrap in `Result`

2. **In `backend_queries.rs`, add tests** (in the existing `mod tests` block):
   - `test_update_backend_source_success`: insert a record, update source, verify it changed
   - `test_update_backend_source_not_found`: update non-existent backend, expect error

3. **In `manager.rs`, add `update_build_method` method on `BackendManager`:**
   ```rust
   pub fn update_build_method(
       &self,
       name: &str,
       gpu_variant: &str,
       build_from_source: bool,
   ) -> Result<()>
   ```
   - First, call `self.get_active(name, gpu_variant)` to get the current `BackendInfo`
   - If not found, return `anyhow::anyhow!("Backend '{}' variant '{}' not found", name, gpu_variant)`
   - Construct the new `BackendSource`:
      - If `build_from_source == true`:
        - If existing `source` is `SourceCode { git_url, commit, .. }`, reuse `git_url` and `commit`
        - If existing `source` is `None` or `Prebuilt`, use `backend_info.backend_type.default_git_url()` for `git_url`, `commit: None`
        - `version` = existing `BackendInfo.version`
      - If `build_from_source == false`:
        - `BackendSource::Prebuilt { version: existing_info.version.clone() }`
    - Serialize to JSON with `serde_json::to_string(&new_source)`
    - Call `crate::db::queries::update_backend_source(&self.conn, name, gpu_variant, &source_json)`

4. **In `manager.rs`, add tests** (in the existing `#[cfg(test)]` module):
   - `test_update_build_method_prebuilt_to_source`: start with Prebuilt, switch to SourceCode, verify git_url
   - `test_update_build_method_source_to_prebuilt`: start with SourceCode, switch to Prebuilt
   - `test_update_build_method_not_found`: non-existent backend returns error
   - `test_update_build_method_preserves_git_url`: when existing source is SourceCode, switching back to source preserves the same git_url

**Steps:**
- [ ] Add `default_git_url()` method to `BackendType` in `types.rs`
- [ ] Write a quick test: `assert_eq!(BackendType::LlamaCpp.default_git_url(), "https://github.com/ggml-org/llama.cpp.git")`
- [ ] Write failing test `test_update_backend_source_success` in `backend_queries.rs`
- [ ] Run `cargo test --package tama-core db::queries::backend_queries::tests::test_update_backend_source_success`
  - Did it fail with compilation error (function doesn't exist)? If not, investigate.
- [ ] Implement `update_backend_source` in `backend_queries.rs`
- [ ] Run `cargo test --package tama-core db::queries::backend_queries::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Write failing test `test_update_build_method_prebuilt_to_source` in `manager.rs`
- [ ] Run `cargo test --package tama-core backends::manager::tests::test_update_build_method_prebuilt_to_source`
  - Did it fail with compilation error (method doesn't exist)? If not, investigate.
- [ ] Implement `update_build_method` in `manager.rs`
- [ ] Run `cargo test --package tama-core backends::manager::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add update_build_method to BackendManager for toggling source type"

**Acceptance criteria:**
- [ ] `update_backend_source` query updates the `source` column on the active row
- [ ] `update_build_method` constructs correct `BackendSource` variant and persists it
- [ ] All new tests pass
- [ ] Clippy clean, formatted

---

### Task 2: Web API — POST endpoint for changing build method

**Context:**
The frontend needs an API endpoint to call when the user toggles the checkbox. Following the existing pattern (all mutations use POST), we add `POST /tama/v1/backends/:name/source` with a JSON body `{ "build_from_source": bool }`. This follows the reviewer feedback to use POST (not PATCH) for CORS compatibility and consistency.

**Files:**
- Modify: `crates/tama-web/src/api/backends/manage.rs`
- Modify: `crates/tama-web/src/api/backends/types.rs`
- Modify: `crates/tama-web/src/api/backends/mod.rs`
- Modify: `crates/tama-web/src/router.rs`

**What to implement:**

1. **In `types.rs`, add request/response DTOs:**
   ```rust
   #[derive(Debug, Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub struct UpdateSourceRequest {
       pub build_from_source: bool,
   }

   #[derive(Debug, Serialize)]
   #[serde(rename_all = "snake_case")]
   pub struct UpdateSourceResponse {
       pub build_from_source: bool,
   }
   ```

2. **In `manage.rs`, add the handler function:**
   ```rust
   /// Query params for POST /tama/v1/backends/:name/source
   #[derive(Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub struct SourceQuery {
       #[serde(default)]
       pub gpu_variant: Option<String>,
   }

   /// POST /tama/v1/backends/:name/source
   pub async fn update_backend_source(
       State(state): State<Arc<ProxyState>>,
       Path(name): Path<String>,
       axum::extract::Query(query): axum::extract::Query<SourceQuery>,
       Json(req): Json<UpdateSourceRequest>,
   ) -> impl IntoResponse
   ```
   - **Path traversal validation** on `name` (mandatory, matching existing pattern):
      ```rust
      if name.contains('/') || name.contains('\\') || name.contains("..") {
          return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid backend name"}))).into_response();
      }
      ```
    - Get `config_dir` from `state.config.read().await.loaded_from` (same pattern as `update_backend`)
    - Determine `gpu_variant`: use query param or auto-infer from manager (same pattern as `update_backend` lines 78-122)
    - **Validate resolved `gpu_variant`** for path traversal (same check as name):
      ```rust
      if gpu_variant.contains('/') || gpu_variant.contains('\\') || gpu_variant.contains("..") {
          return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Invalid gpu_variant"}))).into_response();
      }
      ```
    - **Check for active job conflict** (same pattern as `remove_backend_version` lines 500-517 in manage.rs):
      ```rust
      if let Some(jobs) = &state.web_jobs {
          if let Some(active_job) = jobs.active().await {
              if active_job.backend_type.as_ref().map(|b| b.to_string()) == Some(name.clone()) {
                  return (StatusCode::CONFLICT, Json(serde_json::json!({"error": "another backend job is already running"}))).into_response();
              }
          }
      }
      ```
    - Open `BackendManager` on blocking thread (same pattern as existing handlers)
    - Call `mgr.update_build_method(&name, &gpu_variant, req.build_from_source)`
    - Return `Json(UpdateSourceResponse { build_from_source: req.build_from_source })` on success
    - Error responses: 400 (path traversal), 404 (not found / config_dir missing), 409 (job conflict), 500 (DB error)

3. **In `manage.rs`, add tests** (follow the existing test pattern from `test_update_backend_path_traversal_rejected` at the bottom of manage.rs):
   - `test_update_backend_source_path_traversal_rejected`: POST to `/tama/v1/backends/foo../bar/source` expects 400
   - `test_update_backend_source_missing_backend`: POST to non-existent backend expects 404

4. **In `mod.rs`, re-export the new handler:**
   - The `pub use manage::*;` already handles re-exports, but verify `update_backend_source` is visible.

5. **In `router.rs`, add the route:**
   - Add inside `backend_routes` (before the CORS layer), following the existing pattern:
     ```rust
     .route(
         "/tama/v1/backends/:name/source",
         post(update_backend_source),
     )
     ```
   - Add `update_backend_source` to the import from `crate::api::backends` at line 16-20.

**Steps:**
- [ ] Add `UpdateSourceRequest` and `UpdateSourceResponse` to `types.rs`
- [ ] Implement `update_backend_source` handler in `manage.rs` with path traversal validation
- [ ] Add `SourceQuery` struct in `manage.rs`
- [ ] Add the route to `router.rs` in `backend_routes`
- [ ] Add `update_backend_source` to the imports in `router.rs`
- [ ] Write a test for path traversal rejection in `manage.rs`
- [ ] Run `cargo test --package tama-web api::backends::manage::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Run `cargo build --package tama-web`
- [ ] Commit with message: "feat: add POST /backends/:name/source endpoint for build method toggle"

**Acceptance criteria:**
- [ ] POST `/tama/v1/backends/:name/source` accepts `{ "build_from_source": bool }` and updates DB
- [ ] Path traversal names are rejected with 400 (both name and gpu_variant)
- [ ] Missing backends return 404
- [ ] Active job conflict returns 409
- [ ] Route is registered in `backend_routes` (CORS protected)
- [ ] All tests pass, clippy clean, formatted

---

### Task 3: Frontend — Toggle in BackendCard + callback in Backends page

**Context:**
The `BackendCard` component needs a checkbox toggle that shows for installed backends. The toggle reads its initial state from `backend.info.source` (already available on the DTO). When toggled, it fires a callback to the parent page, which makes the API call. This follows the existing callback pattern used by `on_install`, `on_update`, `on_delete`, etc.

For backends that must always build from source (ik_llama, or Linux+CUDA llama.cpp), the toggle is checked and disabled with a hint. For TTS and custom backends, the toggle is hidden.

**Files:**
- Modify: `crates/tama-web/src/components/backend_card.rs`
- Modify: `crates/tama-web/src/pages/backends.rs`

**What to implement:**

1. **In `backend_card.rs`, add a new callback prop:**
   ```rust
   /// Called with (backend_type, gpu_variant, build_from_source) when toggle changes.
   #[prop(optional)]
   on_build_method_change: Option<Callback<(String, String, bool)>>,
   ```

2. **In `backend_card.rs`, add `BackendSourceDto` helper method:**
   ```rust
   impl BackendSourceDto {
       pub fn is_source_code(&self) -> bool {
           matches!(self, BackendSourceDto::SourceCode { .. })
       }
   }
   ```

3. **In `backend_card.rs`, compute toggle state:**
   After the existing signal declarations (around line 162), add:
   ```rust
   // Build method toggle state
   let current_build_from_source = RwSignal::new(
       backend.info.as_ref()
           .and_then(|i| i.source.as_ref())
           .map(|s| s.is_source_code())
           .unwrap_or(false) // Default to prebuilt if no source recorded
   );

   // Whether toggle should be disabled (forced source)
   let force_source = {
       let bt = backend.r#type.clone();
       // ik_llama always source; tts_kokoro and custom have no toggle
       bt == "ik_llama"
   };

   // Whether to show the toggle at all (not for tts/custom)
   let show_toggle = {
       let bt = backend.r#type.clone();
       installed && bt != "tts_kokoro" && bt != "custom"
   };
   ```

4. **In `backend_card.rs`, add the toggle UI** — insert it before the action buttons div (before line 319). Use the existing `form-check` CSS pattern from `install_modal.rs`:
   ```rust
   {if show_toggle {
       let bt = backend.r#type.clone();
       let gv = gpu_variant.clone();
       let cb = on_build_method_change;
       let force = force_source;
       view! {
           <div style="margin-top:0.75rem;">
               <div class="form-check" style="display:flex;align-items:center;gap:0.5rem;">
                   <input
                       type="checkbox"
                       class="form-check-input"
                       prop:checked=move || current_build_from_source.get()
                       prop:disabled=move || force
                       on:change=move |e| {
                           let checked = e.target()
                               .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                               .map(|el| el.checked())
                               .unwrap_or(false);
                           current_build_from_source.set(checked);
                           if let Some(c) = &cb {
                               c.run((bt.clone(), gv.clone(), checked));
                           }
                       }
                   />
                   <span class="form-check-label" style="font-size:0.875rem;">"Build from source"</span>
               </div>
               {move || {
                   if force {
                       view! {
                           <div style="font-size:0.75rem;color:var(--muted,#666);margin-top:0.125rem;margin-left:1.5rem;">
                               "Always built from source — no prebuilt binaries"
                           </div>
                       }.into_any()
                   } else {
                       view! { <span/> }.into_any()
                   }
               }}
           </div>
       }.into_any()
   } else {
       view! { <span/> }.into_any()
   }}
   ```

5. **In `backends.rs`, add the callback handler:**
   After the existing callbacks (around line 150), add:
   ```rust
   let on_build_method_change = Callback::new(
       move |(backend_type, gpu_variant, build_from_source): (String, String, bool)| {
           action_error.set(None);
           wasm_bindgen_futures::spawn_local(async move {
               let url = format!(
                   "/tama/v1/backends/{}/source?gpu_variant={}",
                   backend_type, gpu_variant
               );
               let body = serde_json::json!({ "build_from_source": build_from_source });
               match post_request(&url).json(&body).unwrap().send().await {
                   Ok(resp) if resp.ok() => {
                       // Success — no need to refresh, toggle already reflects the change
                   }
                   Ok(resp) => {
                       let text = resp.text().await.unwrap_or_default();
                       action_error.set(Some(format!("Failed to update build method: {text}")));
                   }
                   Err(e) => action_error.set(Some(format!("Request failed: {e}"))),
               }
           });
       },
   );
   ```

6. **In `backends.rs`, wire the callback to BackendCard:**
   In the card rendering loop (around line 378), add:
   ```rust
   on_build_method_change=on_build_method_change
   ```

7. **In `backend_card.rs`, update tests:**
   - Add `test_backend_source_dto_is_source_code` verifying the helper method
   - Update existing serialization test to include the new prop (should compile without changes since it's optional)

**Steps:**
- [ ] Add `is_source_code()` method to `BackendSourceDto` in `backend_card.rs`
- [ ] Add `on_build_method_change` callback prop to `BackendCard` component
- [ ] Add toggle state signals (`current_build_from_source`, `force_source`, `show_toggle`)
- [ ] Add toggle UI with checkbox, label, and forced-source hint
- [ ] Add `on_build_method_change` callback in `backends.rs`
- [ ] Wire the callback prop in the `BackendCard` instantiation in `backends.rs`
- [ ] Add test for `is_source_code()` in `backend_card.rs`
- [ ] Run `cargo test --package tama-web components::backend_card::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Run `cargo build --package tama-web`
- [ ] Commit with message: "feat: add build-from-source toggle to backend cards with DB persistence"

**Acceptance criteria:**
- [ ] Toggle appears for installed llama_cpp and ik_llama backends
- [ ] Toggle is hidden for tts_kokoro and custom backends
- [ ] Toggle is hidden for uninstalled backends
- [ ] Toggle defaults to checked when source is SourceCode, unchecked for Prebuilt/None
- [ ] ik_llama toggle is checked + disabled with hint text
- [ ] Toggling fires POST to `/tama/v1/backends/:name/source`
- [ ] All tests pass, clippy clean, formatted

---

### Task 4: Integration test + verification

**Context:**
Verify the full stack works end-to-end. Run the full test suite, check formatting, and verify the feature works as expected.

**Files:**
- No new files

**What to implement:**
No new code — just verification.

**Steps:**
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures before proceeding.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo build --release --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Verify manually (if possible): start the server, navigate to backends page, toggle the checkbox, verify the DB source column changes
- [ ] Commit with message: "chore: verify build method toggle feature, run full check"

**Acceptance criteria:**
- [ ] All workspace tests pass
- [ ] Formatting clean across workspace
- [ ] Clippy clean across workspace
- [ ] Release build succeeds
