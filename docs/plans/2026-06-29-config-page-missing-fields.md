# Config Page: Add Missing Fields

**Goal:** Add 4 missing config fields from `config/types.rs` to the web UI config editor (`config_editor.rs`).

**Architecture:** All changes are in a single file — `crates/tama-web/src/pages/config_editor.rs`. Each field follows the existing pattern: add the field to the WASM-safe mirror struct, add a form input in the appropriate `*Form` component, and wire it to the config signal with `config.update()`.

**Note on type duplication:** The WASM mirror types in `config_editor.rs` are a separate copy from `types/config.rs` (which already has all four fields). This plan only updates `config_editor.rs` — the file that's missing the fields. The duplication is a known issue; a future refactor should deduplicate by importing from `types/config.rs` instead of redefining structs.

**Tech Stack:** Leptos (Rust WASM), existing SectionCard + inline form pattern.

---

### Task 1: Add simple number inputs to General and Proxy sections

**Context:**
Two config fields are simple numeric inputs with no conditional logic: `update_check_interval` (General) and `download_queue_poll_interval_secs` (Proxy). These are straightforward additions that follow the exact same pattern as every other numeric field already in the file (e.g. `idle_timeout_secs`, `circuit_breaker_threshold`).

**Files:**
- Modify: `crates/tama-web/src/pages/config_editor.rs`

**What to implement:**

1. **Add `update_check_interval` to the `General` struct** (after `hf_token`, at the end of the struct):
   ```rust
   #[derive(Debug, Clone, Default, Serialize, Deserialize)]
   pub struct General {
       #[serde(default)]
       pub log_level: String,
       #[serde(default)]
       pub models_dir: Option<String>,
       #[serde(default)]
       pub logs_dir: Option<String>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub hf_token: Option<String>,
       #[serde(default)]
       pub update_check_interval: u32,  // NEW
   }
   ```

2. **Add a number input for `update_check_interval` in `GeneralForm`** — place it after the HuggingFace Token field, before the closing `</div>` of the form fields. Use the existing pattern:
   - Label: "Update Check Interval (hours)"
   - Type: `number`, min="1"
   - Help text: "How often to check for Tama updates (in hours). Default: 12."
   - Bind to `config.general.update_check_interval` via `config.update()`

3. **Add `download_queue_poll_interval_secs` to the `ProxyConfig` struct** (after `max_loaded_models`):
   ```rust
   #[derive(Debug, Clone, Default, Serialize, Deserialize)]
   pub struct ProxyConfig {
       // ... existing fields ...
       #[serde(default)]
       pub max_loaded_models: u32,
       #[serde(default)]
       pub download_queue_poll_interval_secs: u64,  // NEW
   }
   ```

4. **Add a number input for `download_queue_poll_interval_secs` in `ProxyForm`** — place it after the "Max Loaded Models (per GPU)" field. Use the existing pattern:
   - Label: "Download Queue Poll Interval (seconds)"
   - Type: `number`, min="1"
   - Help text: "How often the download queue checks for new items. Minimum: 1 second."
   - Bind to `config.proxy.download_queue_poll_interval_secs` via `config.update()`

**Steps:**
- [ ] Add `update_check_interval: u32` to the `General` struct
- [ ] Add the number input field in `GeneralForm` after the HuggingFace Token field
- [ ] Add `download_queue_poll_interval_secs: u64` to the `ProxyConfig` struct
- [ ] Add the number input field in `ProxyForm` after the Max Loaded Models field
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(web): add update_check_interval and download_queue_poll_interval to config page"

**Acceptance criteria:**
- [ ] `cargo build --workspace` succeeds with no errors
- [ ] `cargo clippy --workspace -- -D warnings` succeeds with no warnings
- [ ] The General section shows "Update Check Interval (hours)" with a number input
- [ ] The Proxy section shows "Download Queue Poll Interval (seconds)" with a number input (min 1)
- [ ] Both fields round-trip through the config API (edit → save → reload shows new value)

---

### Task 2: Add authentication fields to Proxy section with conditional rendering

**Context:**
The `authenticator_url` and `authenticator_skip_paths` fields control Authentik integration — an advanced feature most users won't need. The authenticator skip paths field is only relevant when an authenticator URL is set, so it should be conditionally rendered. The skip paths is a `Vec<String>` which needs a tag-style input (add/remove individual paths).

**Files:**
- Modify: `crates/tama-web/src/pages/config_editor.rs`

**What to implement:**

1. **Add `authenticator_url` and `authenticator_skip_paths` to the `ProxyConfig` struct** (after `download_queue_poll_interval_secs`):
   ```rust
   #[serde(default)]
   pub authenticator_url: Option<String>,
   #[serde(default)]
   pub authenticator_skip_paths: Vec<String>,
   ```

2. **Add the `authenticator_url` input** — place it after "Download Queue Poll Interval":
   - Label: "Authenticator URL"
   - Type: `text`
   - Placeholder: `https://auth.example.com`
   - Help text: "Authentik instance URL for bearer token validation. When set, all requests require auth. Leave empty to disable."
   - Use the exact same empty-to-None pattern as `models_dir`/`logs_dir` in `GeneralForm`:
   ```rust
   on:input=move |ev| {
       let v = target_value(&ev);
       config.update(|c| if let Some(c) = c {
           c.proxy.authenticator_url = if v.trim().is_empty() { None } else { Some(v) };
       });
   }
   ```

3. **Add the `authenticator_skip_paths` textarea** — conditionally rendered using `<Show>`. Use a `textarea` (one path per line) — this avoids adding a new shared tag component. The `<Show>` pattern (used elsewhere in the codebase, e.g. `lib.rs:292`, `pages/model_editor/spec_decoding_form.rs:120`):
   ```rust
   <Show when=move || config.get()
       .and_then(|c| c.proxy.authenticator_url.as_deref())
       .is_some_and(|u| !u.is_empty())>
       <div>
           <label>"Auth Skip Paths"</label>
           <textarea
               rows="3"
               placeholder="/health\n/metrics"
               prop:value=move || config.get()
                   .map(|c| c.proxy.authenticator_skip_paths.join("\n"))
                   .unwrap_or_default()
               on:input=move |ev| {
                   let v = target_value(&ev);
                   let paths: Vec<String> = v.lines()
                       .map(|l| l.trim().to_string())
                       .filter(|l| !l.is_empty())
                       .collect();
                   config.update(|c| if let Some(c) = c {
                       c.proxy.authenticator_skip_paths = paths;
                   });
               }
               class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
           />
           <p class="text-muted" style="font-size:0.85em;margin-top:0.25rem;">
               "Paths exempt from authentication, one per line. Default: /health, /metrics"
           </p>
       </div>
   </Show>
   ```
   - The `has_auth` local signal from step 2 is NOT needed — inline the condition directly in `<Show when=...>` as shown above (the `config.get()` call inside the closure subscribes to the signal automatically).

**What NOT to change:**
- Do not create a new shared tag input component — the textarea approach is sufficient and consistent with the file's inline style
- Do not add validation beyond trimming and filtering empty lines

**Steps:**
- [ ] Add `authenticator_url: Option<String>` and `authenticator_skip_paths: Vec<String>` to `ProxyConfig` struct
- [ ] Add the `authenticator_url` text input with help text and empty-to-None handler
- [ ] Add the conditionally-rendered `authenticator_skip_paths` textarea with `<Show>` and newline join/split logic
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(web): add authenticator_url and skip_paths to config page"

**Acceptance criteria:**
- [ ] `cargo build --workspace` succeeds with no errors
- [ ] `cargo clippy --workspace -- -D warnings` succeeds with no warnings
- [ ] The Proxy section shows "Authenticator URL" text input
- [ ] The "Auth Skip Paths" textarea is hidden when Authenticator URL is empty
- [ ] The "Auth Skip Paths" textarea appears when Authenticator URL has a value
- [ ] Paths are stored one-per-line in the textarea, round-trip correctly (edit → save → reload)
- [ ] Empty lines in the textarea are ignored

---

### Verification (after both tasks)

Run the full check:
```bash
cargo check --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

All four commands must pass with no errors or warnings.

**Manual verification:** Build and run the app, navigate to `/tama/config`, and confirm:
- General section shows "Update Check Interval (hours)" number input
- Proxy section shows "Download Queue Poll Interval (seconds)" number input (min 1)
- Proxy section shows "Authenticator URL" text input
- "Auth Skip Paths" textarea is hidden when Authenticator URL is empty
- "Auth Skip Paths" textarea appears when Authenticator URL has a value
- All fields persist through save → reload
