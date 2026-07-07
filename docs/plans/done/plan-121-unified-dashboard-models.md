# Unified Dashboard Models Plan

**Goal:** Merge the "Active Models" and "Inactive Models" sections on the Dashboard into a single "Models" section.
**Architecture:** Replace the two separate `<section>` blocks in the Dashboard view with one unified section that renders all models. Each model card retains its state badge (Loaded, Idle, Failed, etc.) so status visibility is preserved.
**Tech Stack:** Leptos (Rust/WASM), existing `ModelCard` component

---

### Task 1: Merge Active/Inactive into Single Models Section

**Context:**
The Dashboard page currently renders models in two separate sections: "Active Models" (ready/loading/unloading) and "Inactive Models" (idle/failed). This creates visual fragmentation. The goal is to combine them into a single "Models" section where all models appear together, each showing its own state badge. The model order should remain as provided by the backend (no re-sorting).

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs`
- No changes to: `crates/tama-web/src/pages/dashboard/metrics.rs` (helper functions remain, still tested)

**What to implement:**
In `crates/tama-web/src/pages/dashboard/mod.rs`, within the `Dashboard` component's view:

1. **Remove** lines 165-166:
   ```rust
   let active = active_models(&all_models);
   let inactive = inactive_models(&all_models);
   ```
   Verify no remaining code references `active` or `inactive` variables after removal. The `active_models()` and `inactive_models()` functions remain in `metrics.rs` but are no longer called from `mod.rs` — this is acceptable since they're `pub` (no dead-code warnings).

2. **Remove** lines 401-531 (both `<section class="dashboard-models">` blocks — Active Models and Inactive Models). **Do NOT remove line 532** (`}.into_any()`) — it closes the outer `view!` block.

3. **Add** a single `<section class="dashboard-models">` block inserted between the inference-stats closure and line 532's `}.into_any()`:
   - Heading: `<h2>"Models"</h2>`
   - Count badge: `<span class="text-muted">{format!("{} models", all_models.len())}</span>`
   - Empty state when `all_models.is_empty()` — use this exact markup:
     ```rust
     if all_models.is_empty() {
         view! {
             <div class="card card--centered">
                 <p class="text-muted">"No models configured yet."</p>
             </div>
         }.into_any()
     }
     ```
   - Single `<div class="models-list">` iterating over `all_models` directly (no filtering)
   - **Do NOT sort `all_models`** — iterate in backend-provided order. Omit the `model_sort_key` calls from the old code.
   - Render each model with `ModelCard` using the same props as before (id, db_id, display_name, quant, context_length, hf_architecture_type, hf_base_model, pips, backend, log_source, state, loaded=None, enabled=None, on_load, on_unload, load_busy, unload_busy)
   - Use the same `on_load_cb` and `on_unload_cb` Callback patterns

**Steps:**
- [ ] Modify `crates/tama-web/src/pages/dashboard/mod.rs`:
  - Remove `active` and `inactive` variable bindings (lines 165-166)
  - Remove both Active/Inactive `<section>` blocks (lines 401-531) — preserve line 532 (`}.into_any()`)
  - Add single "Models" `<section>` with unified rendering logic (no sorting, no filtering)
- [ ] Run `cargo check --package tama-web`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --package tama-web`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat: merge active/inactive models into single dashboard section"

**Acceptance criteria:**
- [ ] Dashboard shows a single "Models" section instead of two separate sections
- [ ] All models (active and inactive) appear in one list
- [ ] Each model card still shows its state badge (Loaded, Idle, Failed, etc.)
- [ ] Load/Unload buttons, Logs/Edit links, and all card metadata still work
- [ ] Empty state displays "No models configured yet." when no models exist
- [ ] `cargo clippy` passes with no warnings
- [ ] All existing tests pass (tests in `metrics.rs` remain unchanged)
