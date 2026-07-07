# Model Editor Redesign Plan

**Goal:** Redesign the model editor page with pill-style tabs, sticky save bar, compact sampling, and reorganized sections for better usability.

**Architecture:** Replace the side-nav + stacked-sections layout with horizontal pill-style tabs (only active tab renders). Consolidate 5 sections into 5 focused tabs: Settings, Hardware, Sampling, Files, Advanced. Add sticky bottom save bar with unsaved changes indicator.

**Tech Stack:** Leptos (WASM + SSR), Rust, CSS

---

### Task 1: Pill-style tab navigation + sticky save bar

**Context:**
The current model editor uses a sticky side navigation (200px) with all 5 sections always visible in the main content area. This wastes vertical space and makes the page feel long. The Save/Delete buttons live in the page header and disappear on scroll. This task replaces the side nav with pill-style tabs and adds a sticky bottom save bar.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/mod.rs`
- Modify: `crates/tama/src/pages/model_editor/sections.rs`

**What to implement:**

1. In `sections.rs`, update the `Section` enum with new names:
   - Old enum: `General, Sampling, SpecDecoding, QuantsVision, ExtraArgs`
   - New enum: `Settings, Hardware, Sampling, Files, Advanced`
   - Update `name()` method: Settings, Hardware, Sampling, Files, Advanced
   - Update `icon()` method: Settings → "⚙️", Hardware → "🖥️", Sampling → "🎲", Files → "📁", Advanced → "🔧"

2. In `mod.rs`, update the `active_section` initialization:
   - Change `RwSignal::new(Section::General)` → `RwSignal::new(Section::Settings)`

3. In `mod.rs`, replace the side nav (`model-editor-nav` div with `nav-btn` buttons) with pill-style tabs:
   - Render a horizontal row of `<button>` elements with class `model-editor-pill` (one per Section variant)
   - Active pill gets class `model-editor-pill--active`
   - Each pill shows icon + name (e.g., "⚙️ Settings")
   - Clicking a pill sets `active_section.set(Section::X)` — **do NOT include `scroll_into_view_with_bool` calls** (those were for the side nav's scroll-to-section behavior, not needed for tabs)
   - Only render the content for the active tab (use `match active_section.get()`) — do NOT render all tabs and hide with CSS
   - **Map new Section variants to existing components** (Tasks 2, 4, 5 will replace these with new components):
     - `Section::Settings` → render existing `ModelEditorGeneralForm` (Task 2 replaces with `ModelEditorSettingsForm` + `ModelEditorHardwareForm`)
     - `Section::Hardware` → render existing `ModelEditorGeneralForm` as well (both Settings and Hardware come from the same form initially; Task 2 splits them)
     - `Section::Sampling` → render existing `ModelEditorSamplingForm` (Task 3 redesigns)
     - `Section::Files` → render existing `ModelEditorQuantsVisionForm` (Task 4 replaces with `ModelEditorFilesForm`)
     - `Section::Advanced` → render existing `ModelEditorSpecDecodingForm` + `ModelEditorExtraArgsForm` stacked (Task 5 replaces with `ModelEditorAdvancedForm`)

4. Replace the page header actions (Save/Delete/Back buttons) with a sticky bottom bar:
   - New div with class `model-editor-save-bar` positioned sticky at bottom
   - Left side: `← Back to Models` link (`<A href="/tama/models">`)
   - Center: unsaved changes indicator — see "Dirty tracking" below
   - Right side: `Save Model` (primary) + `Delete Model` (danger, hidden for new models via `{move || (!is_new()).then(...)}`)
   - Save/Delete actions remain the same — just moved to the bar

5. **Dirty tracking** — Use snapshot comparison (NOT per-handler signals):
   - Add `last_saved_form: RwSignal<Option<String>>` — stores `serde_json::to_string(&form.get()).ok()` at last save
   - Derive dirty: `let is_dirty = Signal::derive(move || { let current = serde_json::to_string(&form.get()).ok(); current != last_saved_form.get(); })`
   - On successful save: `last_saved_form.set(serde_json::to_string(&form.get()).ok())`
   - This requires zero changes to individual form handlers

6. **save_status migration** — The `save_status` signal is written by `save_action`, `delete_action`, and `delete_quant_action`. Keep the signal but move its rendering:
   - Remove `{move || save_status.get().map(|(_, msg)| view! { <span class="text-muted">{msg}</span> })}` from the page header
   - Add the same rendering inside the save bar's center area (next to or instead of the dirty indicator)
   - The save bar center shows: dirty indicator (● unsaved) OR save_status message — use `save_status.get().or_else(|| is_dirty.get().then(|| ...))` pattern
   - After successful save, `save_status` is set to `Some((true, "✅ Saved"))` which naturally overrides the dirty state

7. **Do NOT change module declarations in this task** — Each subsequent task (2, 4, 5) handles its own module rename/import changes. This task only changes `sections.rs` and the view/layout in `mod.rs`.

**Steps:**
- [ ] Update `sections.rs` with new Section enum variants and names/icons
- [ ] In `mod.rs`, update `active_section` init to `Section::Settings`
- [ ] In `mod.rs`, replace side nav HTML with pill-style tab buttons (no scroll_into_view)
- [ ] Wire tab switching: `match active_section.get()` mapping to existing components (Settings→GeneralForm, Hardware→GeneralForm, Sampling→SamplingForm, Files→QuantsVisionForm, Advanced→SpecDecodingForm+ExtraArgsForm)
- [ ] Add `last_saved_form` signal and derived `is_dirty` signal
- [ ] Move Save/Delete/Back from page header to sticky bottom bar div
- [ ] Move `save_status` rendering from header to save bar
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: replace side nav with pill tabs and sticky save bar in model editor"

**Acceptance criteria:**
- [ ] Pill-style tabs rendered horizontally below the page title
- [ ] Clicking a tab switches visible content (only active tab renders)
- [ ] Active tab highlighted with accent color
- [ ] Sticky save bar at bottom with Back, unsaved indicator, Save, Delete
- [ ] Delete hidden for new models
- [ ] Unsaved indicator appears after any form change (via snapshot comparison), clears on save
- [ ] save_status messages (save success/failure, delete failure, quant delete) displayed in save bar
- [ ] No side navigation visible
- [ ] All existing form components still render (no module declarations changed yet)
- [ ] Build passes with no warnings

---

### Task 2: Settings + Hardware tabs

**Context:**
The current "General" form crams 15+ fields into one section — backend, GPU, modalities, KV cache, port, enabled, etc. This task splits it into two focused tabs: Settings (identity) and Hardware (runtime configuration). It also adds a HuggingFace repo link.

**Files:**
- Create: `crates/tama/src/pages/model_editor/settings_form.rs`
- Create: `crates/tama/src/pages/model_editor/hardware_form.rs`
- Modify: `crates/tama/src/pages/model_editor/general_form.rs` (delete or repurpose)
- Modify: `crates/tama/src/pages/model_editor/mod.rs`
- Modify: `crates/tama/src/pages/model_editor/sections.rs` (if not done in Task 1)
- Modify: `crates/tama/dist/css/14-model-editor.css`

**What to implement:**

1. Create `settings_form.rs` with component `ModelEditorSettingsForm`:
   - Props: `form: RwSignal<Option<ModelForm>>`, `backends: RwSignal<Vec<BackendOption>>`
   - Fields (in order), using `form-grid` layout:
     - **Display Name** — text input, placeholder "Auto-generated from HF repo name"
     - **Model (HF repo)** — text input, placeholder "e.g. unsloth/gemma-4-26B-A4B-it-GGUF". Add an external link (`<a href="https://huggingface.co/{repo}" target="_blank" rel="noopener" class="hf-repo-link">`) that opens the HF repo page when the repo field is non-empty. Show a ↗ icon.
     - **API Name** — disabled text input (auto-derived)
     - **Backend** — select dropdown (same as current, parsing "name:variant" format)
     - **Enabled** — checkbox
     - **Port Override** — number input, placeholder "leave blank for default"

2. Create `hardware_form.rs` with component `ModelEditorHardwareForm`:
   - Props: `form: RwSignal<Option<ModelForm>>`
   - **Move the following from `general_form.rs`** (these are deleted in step 4):
     - `const MODALITY_OPTIONS: &[(&str, &str)]` — the modality label pairs
     - `const KV_QUANT_OPTIONS: &[&str]` — the KV quant value options
     - `enum KvQuantField { K, V }` — discriminant for KV cache fields
     - `fn KvQuantCustomInput(form, field)` — the custom KV quant text input component
     - GPU device fetching logic: `gpu_devices` signal, `gpu_fetching` signal, `fetch_devices_for_backend` callback, `refresh_devices` callback, and the `Effect` that fetches devices when backend changes
   - Fields (in order):
     - **GPU Layers** — number input, placeholder "e.g. 999"
     - **GPU Isolation** — select dropdown with refresh button (moved from general_form.rs, including all GPU device fetching logic)
     - **Context Length** — `ContextLengthSelector` component (import from `crate::components::context_length_selector`)
     - **Num Parallel Slots** — number input, min="0", placeholder "0 = auto"
     - **Unified KV Cache** — checkbox with hint "All parallel slots share a single context pool. Better for agent+subagent workflows."
     - **KV Cache Type K** — select dropdown with `KV_QUANT_OPTIONS` + "Custom…" option + `KvQuantCustomInput` for custom values
     - **KV Cache Type V** — same pattern as K
     - **Input Modalities** — compact row of checkboxes using `MODALITY_OPTIONS` and `form-check-group` with `modality-row` CSS class
     - **Output Modalities** — same compact row layout
   - Use `form-grid` for the top fields, `modality-row` CSS class for the modalities (horizontal flex layout)

3. **Regarding `set_input_value` and the initialization Effect from `general_form.rs`**: The current `general_form.rs` uses DOM manipulation (`set_input_value` via `document.get_element_by_id`) to initialize input values when the form loads. The new forms use signal-driven binding (`prop:value=move || form.get()...`) which automatically reacts to signal changes. The `set_input_value` helper and its Effect are **not needed** — the `prop:value` bindings handle initialization. Do NOT migrate `set_input_value`.

4. Delete `general_form.rs` (all needed content moved to settings_form.rs and hardware_form.rs)

5. Update `mod.rs`:
   - Change module declaration: remove `mod general_form;`, add `mod settings_form;` and `mod hardware_form;`
   - Update imports: remove `use self::general_form::ModelEditorGeneralForm;`, add `use self::settings_form::ModelEditorSettingsForm;` and `use self::hardware_form::ModelEditorHardwareForm;`
   - In the tab content rendering (established by Task 1), replace:
     - `Section::Settings` → render `ModelEditorSettingsForm` (was `ModelEditorGeneralForm`)
     - `Section::Hardware` → render `ModelEditorHardwareForm` (was `ModelEditorGeneralForm`)
   - Delete `general_form.rs`

5. CSS additions in `14-model-editor.css`:
   - `.modality-row` — horizontal flex layout for modality checkboxes
   - `.modality-row .form-check` — inline flex, no grid alignment
   - `.hf-repo-link` — external link styling (subtle, inline with the input)

**Steps:**
- [ ] Create `settings_form.rs` with 6 fields + HF repo link
- [ ] Create `hardware_form.rs` with GPU fields + modalities (move GPU fetching logic from general_form.rs)
- [ ] Update `mod.rs` to import and render new forms in Settings/Hardware tabs
- [ ] Delete `general_form.rs`
- [ ] Add CSS for `.modality-row` and `.hf-repo-link`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: split General into Settings and Hardware tabs, add HF repo link"

**Acceptance criteria:**
- [ ] Settings tab shows: Display Name, Model (HF repo) + HF link, API Name, Backend, Enabled, Port
- [ ] HF link opens `huggingface.co/{repo}` in new tab when repo is set
- [ ] Hardware tab shows: GPU Layers, GPU Isolation (with refresh), Context Length, Num Parallel, Unified KV, KV Cache K/V, Modalities
- [ ] Modalities rendered as compact horizontal rows (not stacked)
- [ ] GPU device fetching works when backend changes (moved from general_form)
- [ ] `general_form.rs` deleted
- [ ] Build passes with no warnings

---

### Task 3: Sampling tab with expandable fields + preset management

**Context:**
The current Sampling tab uses a checkbox + full-width input for each of 7 fields, taking significant vertical space. Most models only use 1-2 sampling parameters. This task implements an expandable pattern where disabled fields show as compact `[off] (+)` rows, and enabled fields expand to show the input. It also adds preset management: showing the active preset and a "Save as preset" button.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/sampling_form.rs`
- Modify: `crates/tama/src/pages/model_editor/api.rs` (frontend API function — uses existing config/structured endpoint)
- Modify: `crates/tama/src/pages/model_editor/mod.rs` (wiring for preset save)
- Modify: `crates/tama/dist/css/14-model-editor.css`

**What to implement:**

1. **Preset save via existing config API** — Use `POST /tama/v1/config/structured` (no new endpoint needed):
   - Sampling templates are already persisted through `POST /tama/v1/config/structured` (the `StructuredConfigBody` includes `sampling_templates: BTreeMap<String, SamplingParams>`)
   - The frontend flow: GET `/tama/v1/config/structured` → add/update the template in `sampling_templates` → POST the full config back
   - Add a frontend helper in `model_editor/api.rs`: `async fn save_sampling_template(name: &str, params: &serde_json::Value) -> Result<(), String>` that:
     - GETs `/tama/v1/config/structured` to fetch current config
     - Inserts/updates the template in the `sampling_templates` field
     - POSTs the full config back to `/tama/v1/config/structured`
     - Uses `get_request()` and `post_request()` from `crate::utils` (which handle CSRF automatically)
     - Returns `Ok(())` on 200, or `Err(text)` on non-200
   - After successful save, refresh the `templates` `LocalResource` by dispatching a refresh trigger (see step 4 below)

2. **Frontend API function** — in `model_editor/api.rs`:
   - Add `async fn save_sampling_template(name: &str, params: &serde_json::Value) -> Result<(), String>`:
     - GET `/tama/v1/config/structured` to fetch current config as JSON
     - Insert/update the template in the `sampling_templates` field of the response
     - POST the full config back to `/tama/v1/config/structured`
     - Uses `get_request()`/`post_request()` from `crate::utils` (handle CSRF automatically)
     - Returns `Ok(())` on 200, or `Err(text)` on non-200
   - The params format: `{ "name": String, "params": serde_json::Value }`
   - Load current config, insert/update the template in `cfg.sampling_templates`, save to DB
   - Return 200 OK with `{ "ok": true }` or 500 on error
   - The params should be the same format as existing sampling templates (e.g., `{ "temperature": 0.3, "top_k": 40 }`)
   - Register the route in `router.rs` under `csrf_routes` (POST, with JSON body limit)

3. **Expandable sampling fields** — redesign `sampling_form.rs`:
   - Each sampling parameter (Temperature, Top K, Top P, Min P, Presence Penalty, Frequency Penalty, Repeat Penalty) renders as:
     - When **disabled**: compact row showing label + `[off]` text + `(+)` button. Clicking `(+)` enables the field and expands it.
     - When **enabled**: expanded card showing checkbox (to disable), input field, and `(×)` button to collapse/disable.
   - Use a `Show` component or conditional rendering based on `field.enabled`
   - Default values: Temperature 0.3, Top K 40, Top P 0.9, Min P 0.05, Presence Penalty 0.1, Frequency Penalty 0.1, Repeat Penalty 1.1 (use these as placeholders)

4. **Preset management** — signals and actions in `mod.rs`:
   - Add `active_preset: RwSignal<String>` in `mod.rs` — tracks which preset was last loaded
   - In `load_preset_action`, after loading preset values, also set `active_preset.set(preset_name.clone())`
   - Add `save_preset_action: Action<String, (), LocalStorage>` in `mod.rs`:
     - Collects all enabled sampling values from `form.get()` into a `serde_json::Map`
     - Calls `save_sampling_template(&name, &serde_json::Value::Object(map))`
     - On success: sets `save_status.set(Some((true, "✅ Preset saved".into())))`, sets `active_preset.set(name)`, and triggers templates refresh
     - On failure: sets `save_status.set(Some((false, format!("❌ Preset save failed: {}", e))))`
   - **Templates refresh after preset save**: The `templates` is a `LocalResource`. To refresh, create a `templates_refresh: RwSignal::new(0u32)` trigger signal and include it in the LocalResource closure: `let _ = templates_refresh.get();`. After successful preset save, increment: `templates_refresh.update(|n| *n += 1);`
   - Pass `active_preset` and `save_preset_action` as props to `ModelEditorSamplingForm`

5. **Preset UI in `sampling_form.rs`**:
   - Props: add `active_preset: RwSignal<String>`, `save_preset_action: Action<String, (), LocalStorage>`
   - Preset bar layout:
     - Dropdown (existing `field-profile` select)
     - "Save as preset" button (`btn btn-secondary btn-sm`) — when clicked, shows an inline text input + confirm button (NOT `web_sys::window().prompt()` which blocks the WASM main thread)
     - Active preset label: `Currently using: "{name}"` rendered below the dropdown when `active_preset.get()` is non-empty
   - **Inline preset name input**: When "Save as preset" is clicked, toggle a `show_preset_input` signal that reveals a text input + "Save" button inline. On Save, dispatch `save_preset_action` with the input value. On cancel, hide the input.

5. **CSS additions**:
   - `.sampling-field-row` — compact disabled row: label + [off] + (+)
   - `.sampling-field-expanded` — expanded enabled field: checkbox + input + (×)
   - `.sampling-preset-bar` — preset dropdown + save button + active preset label
   - `.sampling-toggle-btn` — (+) and (×) buttons

**Steps:**
- [ ] Add `save_sampling_template` function in `model_editor/api.rs` — uses existing `POST /tama/v1/config/structured` endpoint (GET config → modify sampling_templates → POST back). Uses `get_request()`/`post_request()` from `crate::utils` for CSRF handling.
- [ ] Add `active_preset: RwSignal<String>` and `templates_refresh: RwSignal<u32>` signals in `mod.rs`
- [ ] Add `save_preset_action: Action<String, (), LocalStorage>` in `mod.rs` — collects enabled sampling values from form, calls `save_sampling_template`, on success sets `save_status` and increments `templates_refresh`
- [ ] Update `load_preset_action` in `mod.rs` to also set `active_preset.set(preset_name)` after loading
- [ ] Update `templates` LocalResource closure to track `templates_refresh.get()` for refetch capability
- [ ] Pass `active_preset` and `save_preset_action` as props to `ModelEditorSamplingForm` in the tab rendering
- [ ] Redesign `sampling_form.rs` with expandable field pattern (compact `[off] (+)` / expanded card)
- [ ] Add inline preset name input (text input + Save button — NOT `web_sys::window().prompt()` which blocks WASM main thread)
- [ ] Add CSS for `.sampling-field-row`, `.sampling-field-expanded`, `.sampling-toggle-btn`, `.sampling-preset-bar`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: expandable sampling fields with preset management"

**Acceptance criteria:**
- [ ] Disabled sampling fields show as compact `[off] (+)` rows
- [ ] Clicking `(+)` expands field with input
- [ ] Clicking `(×)` or unchecking collapses field back to `[off]`
- [ ] Preset dropdown loads presets as before
- [ ] Active preset name displayed below dropdown after loading a preset
- [ ] "Save as preset" shows inline text input + Save button (not blocking prompt)
- [ ] Saved preset appears in dropdown after save (templates resource refreshed)
- [ ] Preset persisted via existing `POST /tama/v1/config/structured` endpoint
- [ ] Build passes with no warnings

---

### Task 4: Files tab (Quants, Vision, MTP)

**Context:**
The current "Quants & Vision" section mixes model quants, vision projectors, and repo metadata in a cluttered layout. It also includes per-quant context length overrides that are not needed. This task reorganizes into three clear subsections and removes unnecessary columns.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/quants_vision_form.rs`
- Modify: `crates/tama/dist/css/14-model-editor.css`

**What to implement:**

1. Rename the file from `quants_vision_form.rs` to `files_form.rs` and rename the component to `ModelEditorFilesForm`

2. Reorganize into three subsections with `<h3 class="form-section-title">` headers:

   **Subsection 1: "Model Quants"**
   - Table columns: Active (checkbox) | Name | Size | SHA | Verified | Delete (button)
   - **Remove** the "Context length" column and the `ContextLengthSelector` per-row
   - Keep all other behavior (active checkbox, delete action, refresh/verify merge)
   - The table should filter for `QuantKind::Model` only (same as current)

   **Subsection 2: "Vision Projector"**
   - Table columns: Active (checkbox) | Name | Size | SHA | Verified
   - **Remove** the "File" column
   - **Add** SHA column (show short SHA or "—")
   - Keep the empty state message when no mmproj files exist
   - Filter for `QuantKind::Mmproj` only (same as current)

   **Subsection 3: "MTP Draft Model"**
   - Dropdown selector for MTP draft model (same as current Spec Decoding form's MTP selector)
   - Only show when MTP quants exist (`QuantKind::Mtp`)
   - Options: `(none)` + list of MTP quant files
   - This replaces the MTP Draft Model selector that was in the Spec Decoding form

3. Keep the repo metadata bar (commit SHA, pulled at) and action buttons (Check for updates, Verify files, + Pull Quant) at the top — these apply to all subsections.

4. Update `mod.rs` to import `ModelEditorFilesForm` and render in the Files tab.

5. Update imports in `mod.rs` to use the renamed module.

**Steps:**
- [ ] Rename `quants_vision_form.rs` to `files_form.rs`, rename component to `ModelEditorFilesForm`
- [ ] Remove ContextLengthSelector from model quants table (remove "Context length" column)
- [ ] Remove "File" column from vision projector table
- [ ] Add SHA column to vision projector table
- [ ] Add MTP Draft Model subsection with dropdown selector
- [ ] Update `mod.rs` imports and tab rendering
- [ ] Update `sections.rs` if needed (QuantsVision → Files)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: reorganize Files tab with Quants, Vision, MTP subsections"

**Acceptance criteria:**
- [ ] Model Quants table: Active | Name | Size | SHA | Verified | Delete (no Context length)
- [ ] Vision Projector table: Active | Name | Size | SHA | Verified (no File column)
- [ ] MTP Draft Model dropdown shown when MTP quants exist
- [ ] Repo metadata bar + action buttons at top
- [ ] Three clear subsections with headers
- [ ] Build passes with no warnings

---

### Task 5: Advanced tab (Spec Decoding + Extra Args)

**Context:**
Spec Decoding and Extra Args are rarely-used settings. This task merges them into a single "Advanced" tab, removing the MTP Draft Model selector (moved to Files tab in Task 4).

**Files:**
- Create: `crates/tama/src/pages/model_editor/advanced_form.rs`
- Modify: `crates/tama/src/pages/model_editor/spec_decoding_form.rs` (delete — content moved)
- Modify: `crates/tama/src/pages/model_editor/extra_args_form.rs` (delete — content moved)
- Modify: `crates/tama/src/pages/model_editor/mod.rs`
- Modify: `crates/tama/src/pages/model_editor/sections.rs` (if not done earlier)

**What to implement:**

1. Create `advanced_form.rs` with component `ModelEditorAdvancedForm`:
   - Props: `form: RwSignal<Option<ModelForm>>`
   - Two subsections:

   **Subsection: "Speculative Decoding"**
   - Checkboxes for spec types: `draft-mtp` and `ngram-simple` (with hints)
   - When any type is checked, show:
     - Draft Max — select dropdown (1-8)
     - Draft Min — select dropdown (1-8)
   - When `draft-mtp` is checked, additionally show:
     - Draft GPU Layers — number input, placeholder "e.g. 99", hint "99 = all layers"
   - **Remove** the MTP Draft Model selector (moved to Files tab)

   **Subsection: "Extra Args"**
   - Textarea (6 rows), placeholder "One flag per line, e.g. -fa 1, -b 4096, --mlock"
   - Hint text below: "One flag per line, e.g. -fa 1, --mlock, or -b 4096. Quote values containing spaces"

2. Delete `spec_decoding_form.rs` and `extra_args_form.rs` (all content moved to advanced_form.rs)

3. Update `mod.rs`:
   - Import `ModelEditorAdvancedForm`
   - Remove imports of `ModelEditorSpecDecodingForm` and `ModelEditorExtraArgsForm`
   - Render Advanced form in the Advanced tab

4. Update module declarations in `mod.rs`:
   - Remove `mod spec_decoding_form;` and `mod extra_args_form;`
   - Add `mod advanced_form;`

**Steps:**
- [ ] Create `advanced_form.rs` with Spec Decoding + Extra Args subsections
- [ ] Remove MTP Draft Model selector from Spec Decoding (moved to Files tab)
- [ ] Delete `spec_decoding_form.rs` and `extra_args_form.rs`
- [ ] Update `mod.rs` imports, module declarations, and tab rendering
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: merge Spec Decoding and Extra Args into Advanced tab"

**Acceptance criteria:**
- [ ] Advanced tab shows Spec Decoding subsection (types + conditional fields)
- [ ] Advanced tab shows Extra Args subsection (textarea)
- [ ] MTP Draft Model selector removed from Advanced (present in Files tab instead)
- [ ] `spec_decoding_form.rs` and `extra_args_form.rs` deleted
- [ ] Build passes with no warnings

---

### Task 6: CSS polish (pill tabs, save bar, expandable sampling, modalities)

**Context:**
All the structural changes are in place. This task adds the CSS needed for the new layout patterns: pill-style tabs, sticky save bar, expandable sampling fields, compact modality rows, and the HF repo link.

**Files:**
- Modify: `crates/tama/dist/css/14-model-editor.css`

**What to implement:**

Add/update the following CSS rules in `14-model-editor.css`:

0. **Update existing layout classes** (critical — without these, the layout breaks):
   - `.model-editor-layout` — change from `display: flex; gap: 1.5rem; align-items: flex-start;` (two-column side nav layout) to `display: flex; flex-direction: column;` (single-column tab layout)
   - `.model-editor-main` — remove `flex: 1; min-width: 0;` (no longer needed without side nav). Keep any other properties.
   - Remove old side nav classes: `.model-editor-nav`, `.nav-btn`, `.nav-btn--active`, `.nav-btn__icon`, `.nav-btn__text`

1. **Pill-style tabs** (`.model-editor-pills`):
   ```css
   .model-editor-pills {
     display: flex;
     gap: 0.5rem;
     margin-bottom: 1.5rem;
     flex-wrap: wrap;
   }
   .model-editor-pill {
     display: inline-flex;
     align-items: center;
     gap: 0.4rem;
     padding: 0.4rem 0.9rem;
     border: 1px solid var(--border-color);
     border-radius: 999px;
     background: var(--bg-secondary);
     color: var(--text-secondary);
     cursor: pointer;
     font-size: 0.85rem;
     font-weight: 500;
     transition: all var(--transition-fast);
   }
   .model-editor-pill:hover {
     background: var(--bg-tertiary);
     color: var(--text-primary);
     border-color: var(--border-hover);
   }
   .model-editor-pill--active {
     background: var(--accent-blue);
     color: #fff;
     border-color: var(--accent-blue);
   }
   .model-editor-pill--active:hover {
     background: var(--accent-blue);
     color: #fff;
   }
   ```

2. **Sticky save bar** (`.model-editor-save-bar`):
   ```css
   .model-editor-save-bar {
     position: sticky;
     bottom: 0;
     z-index: 50;
     display: flex;
     align-items: center;
     justify-content: space-between;
     gap: 1rem;
     padding: 0.75rem 1rem;
     background: var(--bg-secondary);
     border-top: 1px solid var(--border-color);
     margin-top: 2rem;
   }
   .model-editor-save-bar__status {
     display: flex;
     align-items: center;
     gap: 0.35rem;
     font-size: 0.8rem;
     color: var(--text-muted);
   }
   .model-editor-save-bar__status--dirty {
     color: var(--accent-yellow);
   }
   .model-editor-save-bar__status--saved {
     color: var(--accent-green);
   }
   ```

3. **Expandable sampling fields**:
   ```css
   .sampling-field-row {
     display: flex;
     align-items: center;
     justify-content: space-between;
     padding: 0.35rem 0;
   }
   .sampling-field-expanded {
     display: flex;
     align-items: center;
     gap: 0.5rem;
     padding: 0.5rem 0.75rem;
     background: var(--bg-tertiary);
     border-radius: var(--radius-sm);
     margin-bottom: 0.35rem;
   }
   .sampling-toggle-btn {
     background: none;
     border: 1px solid var(--border-color);
     border-radius: var(--radius-sm);
     color: var(--text-muted);
     cursor: pointer;
     font-size: 0.8rem;
     padding: 0.15rem 0.4rem;
     line-height: 1;
   }
   .sampling-toggle-btn:hover {
     color: var(--text-primary);
     border-color: var(--border-hover);
   }
   ```

4. **Compact modality rows**:
   ```css
   .modality-row {
     display: flex;
     flex-wrap: wrap;
     gap: 1rem;
     align-items: center;
   }
   .modality-row .form-check {
     padding: 0.2rem 0;
   }
   ```

5. **HF repo link**:
   ```css
   .hf-repo-link {
     display: inline-flex;
     align-items: center;
     gap: 0.25rem;
     margin-left: 0.5rem;
     color: var(--accent-blue);
     text-decoration: none;
     font-size: 0.85rem;
     vertical-align: middle;
   }
   .hf-repo-link:hover {
     text-decoration: underline;
   }
   ```

6. **Remove old styles** that are no longer needed:
   - `.model-editor-nav`, `.nav-btn`, `.nav-btn--active`, `.nav-btn__icon`, `.nav-btn__text` — can be removed or left as dead CSS (prefer removing)

**Steps:**
- [ ] Add all new CSS rules to `14-model-editor.css`
- [ ] Remove old side nav CSS (`.model-editor-nav`, `.nav-btn`, etc.)
- [ ] Verify pill tabs look correct (rounded, active state)
- [ ] Verify save bar sticks to bottom
- [ ] Verify expandable sampling fields render correctly
- [ ] Verify modality rows are horizontal
- [ ] Run `cargo build --workspace` (CSS is embedded, so build is needed)
- [ ] Commit with message: "style: CSS for pill tabs, save bar, expandable sampling, modalities"

**Acceptance criteria:**
- [ ] Pill tabs rendered as rounded buttons with active state
- [ ] Save bar sticks to bottom of viewport
- [ ] Expandable sampling fields show compact/expanded states
- [ ] Modalities rendered as horizontal rows
- [ ] HF repo link styled as inline link
- [ ] Old side nav CSS removed
- [ ] Build passes

---

## Execution Order

**Strictly sequential** — Tasks 1-6 must be executed in order.

1. **Task 1** (tabs + save bar) — establishes the new layout shell, updates Section enum, replaces side nav with pill tabs, adds save bar. Maps new Section variants to existing components (builds successfully with existing form files).
2. **Task 2** (Settings + Hardware) — creates `settings_form.rs` and `hardware_form.rs`, deletes `general_form.rs`, updates `mod.rs` module declarations and tab rendering.
3. **Task 4** (Files) — renames `quants_vision_form.rs` to `files_form.rs`, reorganizes content, updates `mod.rs` module declarations and tab rendering.
4. **Task 5** (Advanced) — creates `advanced_form.rs`, deletes `spec_decoding_form.rs` and `extra_args_form.rs`, updates `mod.rs` module declarations and tab rendering.
5. **Task 3** (Sampling) — most complex (expandable UI + preset management via existing config API), redesigns `sampling_form.rs`, adds signals/actions to `mod.rs`.
6. **Task 6** (CSS) — polish everything together (pill tabs, save bar, expandable sampling, modalities, layout fixes).

Tasks 2, 4, 5 could theoretically be reordered (4 before 2, etc.) since they modify independent form files, but the recommended order groups the simpler tasks (2, 4, 5) before the complex Task 3.
