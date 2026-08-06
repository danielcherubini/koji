# Backend Args/Env Fix Plan

**Goal:** Fix docker backend args/env not saving and improve the UI from single-line inputs to side-by-side textareas with one-item-per-line format.

**Architecture:** 
1. Add `backend_name` field to `BackendCardDto` so the frontend can construct correct API URLs for docker backends (where `r#type` is always `"docker"` but the DB key is the actual name like `"vllm"`).
2. Replace the `<input>` elements with `<textarea>` elements, one item per line, side by side.

**Tech Stack:** Leptos (Rust WASM frontend), axum (backend API)

---

### Task 1: Fix docker backend save bug — add `backend_name` to DTO flow

**Context:**
Docker backends store config in the DB keyed by their actual name (e.g., `"vllm"`), but `BackendCardDto.r#type` is always set to `"docker"`. The frontend uses `r#type` to construct the save URL (`/tama/v1/backends/{bt}/default-args?...`), which for docker sends `"docker"` instead of the actual name. This means save hits a non-existent DB entry and silently does nothing (or returns empty). Native backends work because their `r#type` matches the DB key.

The fix adds a `backend_name` field to the DTO that carries the actual DB key for all backend types, and passes it through the card component so the save handler uses the correct URL path.

**Files:**
- Modify: `crates/tama/src/components/backend_card.rs` — Add `backend_name` to frontend mirror DTO, update props/callbacks, update test constructors
- Modify: `crates/tama/src/api/backends/types.rs` — Add `backend_name` to SSR-side wire DTO and `default_uninstalled()`
- Modify: `crates/tama/src/pages/backends.rs` — Accept `backend_name` from card callbacks (args/env/version), use in save URLs
- Modify: `crates/tama/src/api/backends/list.rs` — Set `backend_name` when building DTOs (both `list_backends` and `check_backend_updates`)

**What to implement:**

1. **`api/backends/types.rs` — Add `backend_name` to SSR-side DTO:**
   - Add `#[serde(default)] pub backend_name: String` field after `r#type` in `BackendCardDto`. The `#[serde(default)]` protects new WASM clients talking to an old server that doesn't emit the field.
   - In `default_uninstalled()`, add `backend_name: type_.to_string()` to the struct literal.

2. **`backend_card.rs` — Add `backend_name` to frontend mirror DTO:**
   - Add `#[serde(default)] pub backend_name: String` field after `r#type` in the frontend `BackendCardDto`.
   - Update all 7 `BackendCardDto` literal constructors in the `mod tests` block to include `backend_name: "<type_value>".to_string()`.

3. **`backend_card.rs` — Change callbacks to include `backend_name`:**
   - Change `on_default_args_change` and `on_default_env_change` from `Callback<(String, String)>` (key, value) to `Callback<(String, String, String)>` (backend_name, gpu_variant, value).
   - Change `on_version_change` from `Callback<(String, String, String)>` (backend_type, version, gpu_variant) to `Callback<(String, String, String)>` (backend_name, version, gpu_variant). The first element changes from backend_type to backend_name — same 3-tuple shape. This fixes the same silent-failure bug for docker version activation.
   - Extract `backend_name` from prop with fallback: `let backend_name = if backend.backend_name.is_empty() { backend.r#type.clone() } else { backend.backend_name.clone() };`
   - In the args and env input handlers, call `cb.run((backend_name.clone(), gpu_variant.clone(), value))` instead of `cb.run((bk_input.clone(), input.value()))`.
   - In the version dropdown handler, pass `(backend_name.clone(), ver, gv)` to `on_version_change` instead of `(vts.clone(), ver, gv)`.
   - Remove now-unused bindings: `backend_type`, `backend_key`, `bk_input`, and `bk_env` (they were only used for constructing the old key-based callbacks).

4. **`backend_card.rs` — Fix version dropdown `<select>` cast:**
   - The version dropdown's `on:change` handler currently casts to `HtmlInputElement`, but a `<select>` event target is an `HtmlSelectElement` — the cast fails silently at runtime and the handler never fires.
   - Replace `ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())` with `let value = crate::utils::target_value(&ev);` (which handles input/select/textarea uniformly). Then parse: `if let Ok(idx) = value.parse::<usize>() { ... }`.

5. **`backends.rs` — Update page callbacks:**
   - Change `on_default_args_change` and `on_default_env_change` to accept `(backend_name, gpu_variant, value): (String, String, String)` and store in edits keyed by `"backend_name:gpu_variant"`.
   - Change `on_version_change` to accept `(backend_name, version, gpu_variant): (String, String, String)`. Store in `version_edits` keyed by `"backend_name:gpu_variant"`, with value `(backend_name, version, gpu_variant)` — the first element is now the real DB name.
   - In the `save` closure, when iterating `ver_edits.values()`, use the first tuple element (backend_name) for the activate URL path `/tama/v1/backends/{backend_name}/activate`.

6. **`list.rs` — Set `backend_name` when building DTOs:
   - In `list_backends()`: For native backends, set `backend_name: type_.to_string()`. For custom and docker, set `backend_name: name.clone()`.
   - In `check_backend_updates()`: Same pattern.
   - There are two functions that build DTOs — update both.

**Steps:**
- [ ] Add `#[serde(default)] pub backend_name: String` to SSR-side `BackendCardDto` in `crates/tama/src/api/backends/types.rs` (after `r#type`). Add `backend_name: type_.to_string()` to `default_uninstalled()`.
- [ ] Add `#[serde(default)] pub backend_name: String` to frontend mirror `BackendCardDto` in `crates/tama/src/components/backend_card.rs` (after `r#type`).
- [ ] Update all 7 `BackendCardDto` literal constructors in `backend_card.rs`'s `mod tests` to include `backend_name`.
- [ ] Change `on_default_args_change` and `on_default_env_change` callback types from `Callback<(String, String)>` to `Callback<(String, String, String)>` (backend_name, gpu_variant, value).
- [ ] Change `on_version_change` callback: first element changes from backend_type to backend_name (remains 3-tuple `(backend_name, version, gpu_variant)`).
- [ ] In card component: extract `backend_name` with fallback to `r#type`, update all three callback invocations.
- [ ] Remove unused bindings `backend_type`, `backend_key`, `bk_input`, and `bk_env` from card component body.
- [ ] Fix version dropdown `<select>` handler: replace `dyn_into::<HtmlInputElement>()` with `let value = crate::utils::target_value(&ev);` — the cast silently fails for select elements.
- [ ] Update `backends.rs` callbacks to accept new tuple shapes and key edits by `"backend_name:gpu_variant"`.
- [ ] In `save` closure, change ver_edits iteration: use first tuple element (backend_name) for the activate URL path `/tama/v1/backends/{backend_name}/activate`.
- [ ] Update `list_backends()` in `list.rs` to set `backend_name` for native (`type_`), custom (`name`), and docker (`name`) sections.
- [ ] Update `check_backend_updates()` in `list.rs` with same `backend_name` values.
- [ ] Run `cargo check --package tama`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --package tama`
- [ ] Commit with message: "fix: add backend_name to DTO so docker backends save args/env correctly"

**Acceptance criteria:**
- [ ] Both `BackendCardDto` definitions (frontend mirror and SSR wire DTO) have a `backend_name` field with `#[serde(default)]`
- [ ] Docker backend save URLs use the correct name (e.g., `/tama/v1/backends/vllm/default-args?...`) instead of `/tama/v1/backends/docker/...`
- [ ] Docker version activation also uses the correct name (same fix pattern), and the select handler fires correctly via `target_value()`
- [ ] No unused-variable warnings (old bindings `backend_key`, `bk_input`, `bk_env`, `backend_type` removed from card)
- [ ] Native and custom backends continue to work unchanged
- [ ] All existing tests in `backend_card.rs` compile and pass with `backend_name` added
- [ ] Code compiles, passes clippy `--all-targets`, and passes all tests

---

### Task 2: Replace inputs with side-by-side textareas (one item per line)

**Context:**
The current UI uses single-line `<input>` elements — Default Args is space-separated and Environment Variables is a JSON array string. This is hard to edit, especially env vars with quotes and brackets. The model editor's advanced tab uses a cleaner one-item-per-line format in textareas. Apply the same pattern here with the two textareas side by side.

**Files:**
- Modify: `crates/tama/src/components/backend_card.rs` — Replace input elements with textareas, update parsing
- Modify: `crates/tama/src/pages/backends.rs` — Update save handler to split textarea content by newlines instead of JSON/space parsing

**What to implement:**

1. **`backend_card.rs` — Default Args textarea:**
   - Change initial value from `backend.default_args.join(" ")` to `backend.default_args.join("\n")`
   - Replace `<input type="text">` with `<textarea>` styled with monospace font, `rows=4`, `resize:vertical`
   - Placeholder: "One arg per line\n--max-num-seqs 4\n--enable-prefix-caching"

2. **`backend_card.rs` — Environment Variables textarea:**
   - Change initial value from `serde_json::to_string(&backend.default_env)` to raw lines: `backend.default_env.iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n")`
   - Replace `<input type="text">` with `<textarea>` same styling
   - Placeholder: "One variable per line\nKEY=value\nOTHER_VAR=123"

3. **`backend_card.rs` — Update `on:input` handlers for textarea compatibility:**
   - Replace `ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())` with `let value = crate::utils::target_value(&ev);` in both the args and env input handlers. `target_value` returns a `String` (not `Option`) and handles input/select/textarea uniformly.
   - The handler body becomes: `let value = crate::utils::target_value(&ev);` followed by using `value` directly.

4. **`backend_card.rs` — Layout:**
   - Wrap the two textarea blocks in a parent `<div style="display:flex;gap:1rem;">` so they sit side by side
   - Each textarea gets `style="flex:1;min-width:0;"` to share space equally
   - Add `resize:vertical` to textareas for user resizing

5. **`backends.rs` — Save handler parsing:**
   - For args: Change from `args_str.split_whitespace().map(String::from).collect()` to `args_str.lines().filter_map(|l| { let t = l.trim(); (!t.is_empty()).then(|| t.to_string()) }).collect()`. The `filter_map` with trim ensures no surrounding whitespace is preserved.
   - For env: Change from `serde_json::from_str(&env_str).unwrap_or_default()` to `env_str.lines().filter_map(|l| { let t = l.trim(); (!t.is_empty()).then(|| t.to_string()) }).collect()`.

**Steps:**
- [ ] In `backend_card.rs`, change default args initial value from space-joined to newline-joined (`backend.default_args.join("\n")`)
- [ ] In `backend_card.rs`, change env initial value from JSON string to newline-joined raw lines
- [ ] Replace both `<input type="text">` elements with `<textarea rows=4>` elements, styled with monospace font and `resize:vertical`
- [ ] Replace `dyn_into::<web_sys::HtmlInputElement>()` in both `on:input` handlers with `let value = crate::utils::target_value(&ev);` (returns `String`, not `Option`) for textarea compatibility
- [ ] Wrap both label+textarea blocks in a flex container (`display:flex;gap:1rem`) with `flex:1;min-width:0` on each child for side-by-side layout
- [ ] In `backends.rs` save handler, change args parsing from `split_whitespace()` to `lines().filter_map(|l| { let t = l.trim(); (!t.is_empty()).then(|| t.to_string()) })`
- [ ] In `backends.rs` save handler, change env parsing from `serde_json::from_str().unwrap_or_default()` to same `lines().filter_map(...)` pattern
- [ ] Run `cargo check --package tama`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --package tama`
- [ ] Commit with message: "ui: replace backend args/env inputs with side-by-side textareas"

**Acceptance criteria:**
- [ ] Default Args is a textarea with one arg per line (newline-separated)
- [ ] Environment Variables is a textarea with one `KEY=value` per line (no JSON wrapping)
- [ ] Both textareas are displayed side by side in a flex row
- [ ] Save correctly parses newline-separated content back into `Vec<String>` for both fields (using `filter_map` with trim)
- [ ] Empty lines and whitespace-only lines are filtered out on save
- [ ] The `on:input` handlers use `target_value()` (returning `String`) and fire correctly for textarea elements
- [ ] Code compiles, passes clippy, and passes all tests
