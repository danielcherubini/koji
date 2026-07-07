# Model Sort + Group Plan

**Goal:** Add sort and group controls to the Models page so users can organize models by GPU, family, vendor, status, or name, with optional grouping into visual sections.

**Architecture:** Client-side sort + group in `models.rs` with `RwSignal` state persisted to `localStorage`. The backend `GET /tama/v1/models` response is enriched with `hf_architecture_type` and `hf_base_model` fields (currently missing) to enable family/vendor sorting. Group headers render as subtle inline separators between model sections.

**Tech Stack:** Leptos (Rust), CSS (existing `11-models.css` and `15-dashboard.css`), `localStorage` via `web-sys`

---

### Task 1: Add missing fields to the API response

**Context:**
The `GET /tama/v1/models` endpoint (served by `list_models` in `info.rs`) does not include `hf_architecture_type` or `hf_base_model` in its JSON response. These fields exist in `ModelConfigRecord` (the DB row type) and are used by the dashboard via the SSE stream, but the REST API omits them. Without these fields, the frontend cannot sort or group by model family or vendor. This task adds them to the API response and the frontend `ModelEntry` struct.

**Files:**
- Modify: `crates/tama/src/api/models/info.rs` — add 2 fields to `model_entry_json`
- Modify: `crates/tama/src/pages/models.rs` — add 2 fields to `ModelEntry` struct
- Modify: `crates/tama/dist/css/11-models.css` — no changes needed here

**What to implement:**

1. In `crates/tama/src/api/models/info.rs`, inside the `model_entry_json` function, add two new fields to the `serde_json::json!` macro:
   - `"hf_architecture_type": record.hf_architecture_type,`
   - `"hf_base_model": record.hf_base_model,`
   Place these near the existing `"hf_context_length"` field for consistency. Note: `gpu_device` and `gpu_variant` are **already** sent by the API — only these two fields are missing.

2. In `crates/tama/src/pages/models.rs`, add four new fields to the `ModelEntry` struct:
   ```rust
   #[serde(default)]
   gpu_device: Option<String>,
   #[serde(default)]
   gpu_variant: Option<String>,
   #[serde(default)]
   hf_architecture_type: Option<String>,
   #[serde(default)]
   hf_base_model: Option<String>,
   ```
   Place these near the end of the struct, after `display_name`. The `gpu_device` field is required for GPU sort/group. The `gpu_variant` field is needed to populate `ModelPips.gpu_variant` in the `ModelCard`.

**Steps:**
- [ ] In `info.rs`, add `"hf_architecture_type": record.hf_architecture_type,` and `"hf_base_model": record.hf_base_model,` to the `model_entry_json` function's `serde_json::json!` block
- [ ] In `models.rs`, add `hf_architecture_type: Option<String>` and `hf_base_model: Option<String>` fields to `ModelEntry` with `#[serde(default)]`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Commit with message: "feat: expose hf_architecture_type and hf_base_model in /v1/models API"

**Acceptance criteria:**
- [ ] `GET /tama/v1/models` response includes `hf_architecture_type` and `hf_base_model` fields for each model
- [ ] Frontend `ModelEntry` struct deserializes all four new fields (`gpu_device`, `gpu_variant`, `hf_architecture_type`, `hf_base_model`) without error
- [ ] `cargo build --workspace` succeeds with no warnings

---

### Task 2: Sort + group state, UI controls, and localStorage persistence

**Context:**
This task adds the core sort and group logic to the Models page. Two `RwSignal` signals track the current sort and group preferences, persisted to `localStorage` so they survive page refresh. A toolbar row with two `<select>` dropdowns sits between the page header and the models list. The models array is sorted and grouped before rendering.

**Files:**
- Modify: `crates/tama/src/pages/models.rs` — add enums, signals, helpers, toolbar UI, sort/group logic
- Modify: `crates/tama/dist/css/11-models.css` — add `.models-toolbar` styles

**What to implement:**

1. **Define enums** at the top of `models.rs` (before the `ModelEntry` struct):
   ```rust
   #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
   enum SortBy { Name, Gpu, Family, Vendor, Status }

   #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
   enum GroupBy { Gpu, Family, Vendor, Status }
   ```

2. **Define localStorage keys** as constants:
   ```rust
   const SORT_KEY: &str = "tama-models-sort-by";
   const GROUP_KEY: &str = "tama-models-group-by";
   ```

3. **Add helper functions** for extracting sort keys and group labels:

   `fn extract_sort_key(entry: &ModelEntry, sort_by: SortBy) -> String` — returns a comparable string for all non-GPU sorts:
   - `SortBy::Name` → `model_display_name(entry)` (reuse existing function)
   - `SortBy::Family` → `entry.hf_architecture_type.clone().unwrap_or_default()`
   - `SortBy::Vendor` → result of `extract_vendor(entry)` (see below)
   - `SortBy::Status` → `entry.state.clone()`
   - `SortBy::Gpu` — **NOT handled here** (see `sort_models` below for GPU-specific sorting)

   `fn extract_vendor(entry: &ModelEntry) -> String` — vendor extraction chain:
   1. Try `display_name` — split on `:`, take prefix and trim (e.g., "Unsloth: Qwen3.6 27B" → "Unsloth")
   2. Try `api_name` — same split on `:`
   3. Try `hf_base_model` — split on `/`, take first segment (e.g., "Qwen/Qwen3.6-27B" → "Qwen")
   4. Fallback: `"other"`
   Implementation:
   ```rust
   fn extract_vendor(entry: &ModelEntry) -> String {
       if let Some(ref name) = entry.display_name {
           if let Some(vendor) = name.split(':').next() {
               let vendor = vendor.trim();
               if !vendor.is_empty() { return vendor.to_string(); }
           }
       }
       if let Some(ref name) = entry.api_name {
           if let Some(vendor) = name.split(':').next() {
               let vendor = vendor.trim();
               if !vendor.is_empty() { return vendor.to_string(); }
           }
       }
       if let Some(ref base) = entry.hf_base_model {
           if let Some(vendor) = base.split('/').next() {
               let vendor = vendor.trim();
               if !vendor.is_empty() { return vendor.to_string(); }
           }
       }
       "other".to_string()
   }
   ```

   `fn extract_gpu_sort_key(gpu_device: &Option<String>) -> (bool, u32)` — returns `(has_gpu, numeric_index)` for sorting:
   - Extract trailing digits from the string using a regex-like approach: iterate chars from the end, collect digits, reverse, parse as u32 (e.g., "CUDA10" → 10, "ROCm0" → 0)
   - If no digits found, use 0
   - `has_gpu` is `true` if `gpu_device` is `Some`, `false` otherwise
   - Rust tuple comparison: `(bool, u32)` sorts `true` before `false` — but we want GPU models FIRST. So use `(has_gpu, index)` and reverse the bool: sort by `(!has_gpu, index)` so GPU models (`has_gpu=true` → `!has_gpu=false`) sort first.
   - Simpler approach: return `(if has_gpu { 0 } else { 1 }, index)` — GPU models get priority 0, non-GPU get priority 1

   `fn gpu_group_label(gpu_device: &Option<String>) -> String` — human-readable GPU label:
   - If `Some(device)` → extract the numeric index and format as "GPU {index}" (e.g., "CUDA1" → "GPU 1", "ROCm0" → "GPU 0")
   - If no numeric index found → use the raw device string (e.g., "GPU")
   - If `None` → "No GPU"

   `fn capitalize_first(s: &str) -> String` — capitalizes the first letter of a string:
   ```rust
   fn capitalize_first(s: &str) -> String {
       let mut chars = s.chars();
       match chars.next() {
           None => String::new(),
           Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
       }
   }
   ```

   `fn extract_group_key(entry: &ModelEntry, group_by: GroupBy) -> String` — returns the grouping key:
   - `GroupBy::Gpu` → `gpu_group_label(&entry.gpu_device)`
   - `GroupBy::Family` → `entry.hf_architecture_type.clone().unwrap_or_else(|| String::from("Unknown"))`
   - `GroupBy::Vendor` → `extract_vendor(entry)`
   - `GroupBy::Status` → match `entry.state`: "ready" → "Loaded", "loading" → "Loading", "unloading" → "Unloading", "failed" → "Failed", _ → "Idle"

   `fn group_display_order(group_by: GroupBy, key: &str) -> u32` — defines display order for group headers:
   - `GroupBy::Gpu` → if key == "No GPU" return `u32::MAX` (sorts last), else extract numeric index from key
   - All others → 0 (alphabetical is fine)

   `fn sort_models(models: &mut Vec<ModelEntry>, sort_by: SortBy)` — sorts the models in place:
   - For `SortBy::Gpu`: use `models.sort_by(|a, b| { let ka = extract_gpu_sort_key(&a.gpu_device); let kb = extract_gpu_sort_key(&b.gpu_device); ka.cmp(&kb) })`
   - For all other sorts: use `models.sort_by(|a, b| extract_sort_key(a, sort_by).cmp(&extract_sort_key(b, sort_by)))`

   `fn parse_sort_by(s: &str) -> SortBy` — parses string to enum:
   ```rust
   fn parse_sort_by(s: &str) -> SortBy {
       match s {
           "gpu" => SortBy::Gpu,
           "family" => SortBy::Family,
           "vendor" => SortBy::Vendor,
           "status" => SortBy::Status,
           _ => SortBy::Name,
       }
   }
   ```

   `fn parse_group_by(s: &str) -> Option<GroupBy>` — parses string to enum:
   ```rust
   fn parse_group_by(s: &str) -> Option<GroupBy> {
       match s {
           "gpu" => Some(GroupBy::Gpu),
           "family" => Some(GroupBy::Family),
           "vendor" => Some(GroupBy::Vendor),
           "status" => Some(GroupBy::Status),
           _ => None,
       }
   }
   ```

4. **Add `RwSignal` state** in the `Models` component:

   Add imports at the top of `models.rs`:
   ```rust
   use web_sys::window;
   use wasm_bindgen::JsCast;
   ```

   Helper to read from localStorage (follow the pattern from `sidebar.rs`):
   ```rust
   fn read_local_storage(key: &str) -> Option<String> {
       window()
           .and_then(|w| w.local_storage().ok())
           .flatten()
           .and_then(|ls| ls.get(key).ok())
           .flatten()
   }

   fn write_local_storage(key: &str, value: &str) {
       if let Some(ls) = window().and_then(|w| w.local_storage().ok()).flatten() {
           let _ = ls.set(key, value);
       }
   }
   ```

   Initialize signals with localStorage values:
   ```rust
   let sort_by = RwSignal::new({
       let stored = read_local_storage(SORT_KEY);
       stored.as_deref().map(parse_sort_by).unwrap_or(SortBy::Name)
   });
   let group_by = RwSignal::new({
       let stored = read_local_storage(GROUP_KEY);
       stored.as_deref().map(parse_group_by).unwrap_or(None)
   });
   ```

   Persist on change using `Effect` (follow the pattern from `sidebar.rs`):
   ```rust
   Effect::new(move || {
       let val = sort_by.get();
       let key_str = match val {
           SortBy::Name => "name",
           SortBy::Gpu => "gpu",
           SortBy::Family => "family",
           SortBy::Vendor => "vendor",
           SortBy::Status => "status",
       };
       write_local_storage(SORT_KEY, key_str);
   });

   Effect::new(move || {
       let val = group_by.get();
       let key_str = match val {
           Some(GroupBy::Gpu) => "gpu",
           Some(GroupBy::Family) => "family",
           Some(GroupBy::Vendor) => "vendor",
           Some(GroupBy::Status) => "status",
           None => "none",
       };
       write_local_storage(GROUP_KEY, key_str);
   });
   ```

5. **Add toolbar UI** — a `<div class="models-toolbar">` between the page header and the Suspense block.

   The `on:change` handlers use `web_sys::HtmlSelectElement` (NOT `event_target_value` which only works with `HtmlInputElement`). Follow the pattern from `benchmarks/mod.rs`:
   ```rust
   use wasm_bindgen::JsCast;
   ```

   Sort select handler:
   ```rust
   on:change=move |e| {
       let val = e.target()
           .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
           .map(|s| s.value())
           .unwrap_or_default();
       sort_by.set(parse_sort_by(&val));
   }
   ```

   Group select handler:
   ```rust
   on:change=move |e| {
       let val = e.target()
           .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
           .map(|s| s.value())
           .unwrap_or_default();
       group_by.set(parse_group_by(&val));
   }
   ```

   The toolbar HTML:
   ```html
   <div class="models-toolbar">
       <div class="models-toolbar__controls">
           <select class="btn btn-secondary btn-sm" on:change={sort handler}>
               <option value="name">Sort: Name</option>
               <option value="gpu">Sort: GPU</option>
               <option value="family">Sort: Family</option>
               <option value="vendor">Sort: Vendor</option>
               <option value="status">Sort: Status</option>
           </select>
           <select class="btn btn-secondary btn-sm" on:change={group handler}>
               <option value="none">Group: None</option>
               <option value="gpu">Group: GPU</option>
               <option value="family">Group: Family</option>
               <option value="vendor">Group: Vendor</option>
               <option value="status">Group: Status</option>
           </select>
       </div>
       <span class="models-toolbar__count">{move || models.get().map(|m| m.models.len()).unwrap_or(0)}</span>
   </div>
   ```

   Note: The select dropdowns should reflect the current `sort_by`/`group_by` values. In Leptos, this can be done by setting the `selected` attribute on the matching `<option>`:
   ```rust
   <option value="name" selected=move || sort_by.get() == SortBy::Name>Sort: Name</option>
   ```

6. **Add sort/group logic** in the rendering section:

   Inside the `Some(data) => { ... }` branch (where models are rendered), replace the current render logic:

   ```rust
   // Clone, sort, and optionally group the models
   let mut sorted_models = data.models.clone();
   sort_models(&mut sorted_models, sort_by.get());

   // Build grouped output: Vec<(Option<String>, Vec<ModelEntry>)>
   // Each tuple is (group_label, models_in_group)
   // When no grouping: single entry (None, all_models)
   let groups: Vec<(Option<String>, Vec<ModelEntry>)> = {
       let group_by_val = group_by.get();
       if let Some(group_by_type) = group_by_val {
           // Partition into groups using a BTreeMap (preserves insertion order)
           let mut groups_map: std::collections::BTreeMap<String, Vec<ModelEntry>> = std::collections::BTreeMap::new();
           let mut group_order: Vec<String> = Vec::new();
           for m in &sorted_models {
               let key = extract_group_key(m, group_by_type);
               if !groups_map.contains_key(&key) {
                   group_order.push(key.clone());
               }
               groups_map.entry(key).or_default().push(m.clone());
           }
           // Sort group keys by display order
           group_order.sort_by(|a, b| {
               let oa = group_display_order(group_by_type, a.as_str());
               let ob = group_display_order(group_by_type, b.as_str());
               oa.cmp(&ob).then_with(|| a.cmp(b))
           });
           group_order.into_iter()
               .map(|key| {
                   let models = groups_map.remove(&key).unwrap();
                   (Some(capitalize_first(&key)), models)
               })
               .collect()
       } else {
           vec![(None, sorted_models)]
       }
   };
   ```

   Then render groups with headers. **Important**: Use `if let` branching (NOT `Option<AnyView>` + `chain`) to avoid type mismatch:
   ```rust
   groups.into_iter().flat_map(|(label, models_in_group)| {
       let cards: Vec<AnyView> = models_in_group.into_iter().map(|m| {
           // ... existing ModelCard rendering, but updated to pass new fields (see Task 2, step 7)
       }).collect();
       if let Some(l) = label {
           let count = models_in_group.len(); // capture before move
           let header: AnyView = view! {
               <div class="model-section__title">
                   {l} ({count} {if count == 1 { "model" } else { "models" }})
               </div>
           }.into_any();
           std::iter::once(header).chain(cards.into_iter())
       } else {
           cards.into_iter()
       }
   }).collect::<Vec<_>>()
   ```

7. **Update `ModelCard` calls** to pass the new fields. The current code passes `pips=ModelPips::default()` — update to:
   ```rust
   pips=ModelPips {
       gpu_variant: m.gpu_variant.clone(),
       gpu_label: Some(gpu_group_label(&m.gpu_device)),
       ..Default::default()
   }
   ```
   And pass the architecture and base model fields to the existing `ModelCard` props (these props already exist on `ModelCard` with `#[prop(default = None)]` — just wire them through from `ModelEntry`):
   ```rust
   hf_architecture_type=m.hf_architecture_type.clone()
   hf_base_model=m.hf_base_model.clone()
   ```

8. **Add CSS** in `crates/tama/dist/css/11-models.css` (append at end — keeps all Models page styles in one file):
   ```css
   /* Models sort/group toolbar */
   .models-toolbar {
       display: flex;
       justify-content: space-between;
       align-items: center;
       margin-bottom: 1rem;
   }
   .models-toolbar__controls {
       display: flex;
       gap: 0.5rem;
   }
   .models-toolbar__count {
       font-size: 0.85rem;
       color: var(--text-muted);
   }
   ```

   No changes needed for `.model-section__title` — it already exists in `11-models.css` with appropriate styling (font-size 1.1rem, font-weight 600, border-bottom, padding-bottom).

**Steps:**
- [ ] Write the enum definitions and localStorage constants in `models.rs`
- [ ] Implement all helper functions (`extract_sort_key`, `extract_vendor`, `extract_gpu_sort_key`, `gpu_group_label`, `capitalize_first`, `extract_group_key`, `group_display_order`, `sort_models`, `parse_sort_by`, `parse_group_by`)
- [ ] Add `RwSignal` state with localStorage load/save using `web_sys::Storage`
- [ ] Add the toolbar UI with two `<select>` dropdowns
- [ ] Add sort/group logic in the rendering section (clone → sort → partition → render with group headers)
- [ ] Add CSS for `.models-toolbar` in `11-models.css`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run
- [ ] Commit with message: "feat: add sort and group controls to Models page with localStorage persistence"

**Acceptance criteria:**
- [ ] Two dropdowns appear in a toolbar row between the page header and models list
- [ ] Selecting a sort option reorders the models list
- [ ] Selecting a group option adds section headers with model counts
- [ ] Sort and group preferences persist across page refresh (localStorage)
- [ ] GPU sort orders by numeric index (CUDA0 < CUDA1), non-GPU models sort last
- [ ] Vendor extraction falls back through display_name → api_name → hf_base_model → "other"
- [ ] `cargo build --workspace` succeeds with no warnings

---

### Task 3: Polish — edge cases, group header ordering, and tests

**Context:**
This task handles edge cases and adds unit tests for the sort/group helper functions. It ensures robust behavior for empty data, missing fields, and unusual GPU device names.

**Files:**
- Modify: `crates/tama/src/pages/models.rs` — add tests, handle edge cases

**What to implement:**

1. **Edge case handling:**
   - Empty model list → toolbar still renders, shows "0 models"
   - All models have same group key → single group, no separator needed
   - `hf_architecture_type` is `None` → groups under "Unknown" for family grouping
   - GPU device names without numeric suffix (e.g., "GPU") → sort index 0
   - GPU device names with multi-digit numbers (e.g., "CUDA10") → numeric sort (10, not lexicographic)

2. **Unit tests** in a `#[cfg(test)] mod tests` block at the bottom of `models.rs`:

   For `extract_vendor`:
   - `test_extract_vendor_from_display_name` — `display_name: Some("Unsloth: Qwen3.6 27B")` → "Unsloth"
   - `test_extract_vendor_from_api_name` — `display_name: None, api_name: Some("vendor:model-name")` → "vendor" (split on `:`)
   - `test_extract_vendor_from_hf_base_model` — `hf_base_model: Some("Qwen/Qwen3.6-27B")` → "Qwen"
   - `test_extract_vendor_fallback_other` — all None → "other"

   For `extract_gpu_sort_key`:
   - `test_extract_gpu_sort_key_cuda` — `Some("CUDA1")` → `(0, 1)` (priority 0 for GPU, index 1)
   - `test_extract_gpu_sort_key_rocm` — `Some("ROCm0")` → `(0, 0)`
   - `test_extract_gpu_sort_key_none` — `None` → `(1, 0)` (priority 1 = sorts last)
   - `test_extract_gpu_sort_key_multidigit` — `Some("CUDA10")` → `(0, 10)`
   - `test_extract_gpu_sort_key_no_number` — `Some("GPU")` → `(0, 0)`

   For `gpu_group_label`:
   - `test_gpu_group_label_cuda` — `Some("CUDA0")` → "GPU 0"
   - `test_gpu_group_label_rocm` — `Some("ROCm1")` → "GPU 1"
   - `test_gpu_group_label_none` — `None` → "No GPU"
   - `test_gpu_group_label_no_number` — `Some("GPU")` → "GPU"

   For `capitalize_first`:
   - `test_capitalize_first` — "qwen35" → "Qwen35", "" → ""

   For `parse_sort_by` and `parse_group_by`:
   - `test_parse_sort_by` — "name" → Name, "gpu" → Gpu, "unknown" → Name (default)
   - `test_parse_group_by` — "gpu" → Some(Gpu), "none" → None, "unknown" → None

**Steps:**
- [ ] Handle all edge cases in the helper functions
- [ ] Write unit tests for `extract_vendor`, `extract_gpu_sort_key`, `gpu_group_label`
- [ ] Run `cargo test --package tama`
  - Did all tests pass? If not, fix failures and re-run
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "test: add unit tests for model sort/group helpers, handle edge cases"

**Acceptance criteria:**
- [ ] All unit tests pass
- [ ] Edge cases handled gracefully (empty lists, missing fields, unusual GPU names)
- [ ] `cargo test --package tama` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` succeeds

---

## Verification

After all tasks complete:
1. `cargo build --release --workspace` — clean build
2. `cargo test --workspace` — all tests pass
3. `cargo clippy --workspace -- -D warnings` — no warnings
4. Manual test: Load the Models page, verify sort/group controls work, refresh page and verify preferences persist
