# Consolidate Models Page Plan

**Goal:** Merge Aliases and Installations into a three-tab Models page, add Models to sidebar, fix model edit URL to plural.

**Architecture:** The single-file `models.rs` becomes a module (`models/`) with pill-style tab navigation matching the model editor's `model-editor-pills` pattern. Three tabs: Models (existing), Aliases (from aliases page), Providers (from installations page renamed). Each tab owns its own page-header actions (no prop wiring needed). Sidebar gains Models entry, loses Installations and Aliases. Route `/tama/model/:id/edit` pluralizes to `/tama/models/:id/edit`; `/tama/aliases` and `/tama/installations` routes removed (content lives as tabs on `/tama/models`).

---

### Task 1: Convert models.rs to a module with tab infrastructure

**Context:** The current `models.rs` is a single-file page component. To add tabs, it needs to become a directory module with a tab enum, pill navigation, and the existing content rendered as the default "Models" tab. This establishes the shell that tasks 2 and 3 fill in.

**Files:**
- Modify: `crates/tama/src/pages/models.rs` → `crates/tama/src/pages/models/mod.rs`
- Create: `crates/tama/src/pages/models/tab.rs`

**What to implement:**

1. Create `crates/tama/src/pages/models/` directory and move `models.rs` content into `models/mod.rs`.

2. Create `crates/tama/src/pages/models/tab.rs` with a tab enum matching the model editor's `Section` pattern:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Models,
    Aliases,
    Providers,
}

impl Tab {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Models => "Models",
            Self::Aliases => "Aliases",
            Self::Providers => "Providers",
        }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Self::Models => "📦",
            Self::Aliases => "🏷️",
            Self::Providers => "🔌",
        }
    }
}
```

3. In `models/mod.rs`, add:
   - `mod tab;` and `use self::tab::Tab;`
   - `let active_tab = RwSignal::new(Tab::Models);`
   - Pill navigation bar (using `.model-editor-pills` and `.model-editor-pill` classes) right after `<div class="page-header">`:
   ```rust
   <div class="model-editor-pills">
       <button class="model-editor-pill" class:model-editor-pill--active=move || active_tab.get() == Tab::Models on:click=move |_| active_tab.set(Tab::Models)>
           <span>{Tab::Models.icon()}</span>
           <span>{Tab::Models.name()}</span>
       </button>
       // ... Aliases and Providers pills (same pattern)
   </div>
   ```

4. Wrap the existing content (model card list, pull wizard modal, alert banners, suspense) inside a `match active_tab.get()` block:
   - `Tab::Models` → existing content
   - `Tab::Aliases` → `<p class="text-muted">Coming soon.</p>` placeholder
   - `Tab::Providers` → `<p class="text-muted">Coming soon.</p>` placeholder

   Each match arm must end with `.into_any()` so Leptos unifies the different view types (the existing `models.rs` already uses this pattern).

5. The page header `<h1>Models</h1>` and action buttons stay as-is for now (they only apply to the Models tab; Aliases and Providers will provide their own headers inside tab content).

**Steps:**
- [ ] Create directory `crates/tama/src/pages/models/`
- [ ] Move `crates/tama/src/pages/models.rs` → `crates/tama/src/pages/models/mod.rs`
- [ ] Create `crates/tama/src/pages/models/tab.rs` with the `Tab` enum
- [ ] Add `mod tab;` and `use self::tab::Tab;` at top of `mod.rs`
- [ ] Add `let active_tab = RwSignal::new(Tab::Models);` in the `Models` component
- [ ] Add pill navigation div after the page-header div
- [ ] Wrap existing content in `match active_tab.get()` with placeholders for Aliases and Providers (each arm ends with `.into_any()`)
- [ ] Run `cargo build --package tama --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: add tab infrastructure to models page"

**Acceptance criteria:**
- [ ] `cargo build --package tama --features ssr` succeeds
- [ ] Models page renders with three pill buttons (Models, Aliases, Providers)
- [ ] Models tab shows existing model card list
- [ ] Aliases and Providers tabs show "Coming soon" placeholders
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes

---

### Task 2: Integrate Aliases into Models page tab

**Context:** The Aliases page at `/tama/aliases` is a standalone page with full CRUD. Move its content into the Aliases tab of the Models page. The API module (`aliases/api.rs` and `aliases/types.rs`) stays in place as imports — only the view logic moves.

**Files:**
- Create: `crates/tama/src/pages/models/aliases_tab.rs`
- Modify: `crates/tama/src/pages/models/mod.rs`
- Modify: `crates/tama/src/pages/aliases/mod.rs` (make submodules pub(crate), remove page component)
- Modify: `crates/tama/src/lib.rs` (remove `/tama/aliases` route)

**What to implement:**

1. Create `crates/tama/src/pages/models/aliases_tab.rs` containing the Aliases tab content component:
   - Extract all the stateful logic from `AliasesPage()` (aliases signal, models signal, loading, error, create modal, etc.) into a new `#[component] pub fn AliasesTab() -> impl IntoView` function
   - Include the card list, create modal, empty state, loading state, and error state
   - Import from the existing aliases module: `use crate::pages::aliases::api::*;` and `use crate::pages::aliases::types::{Alias, ModelOption};`
   - Include the `AliasCard`, `CreateAliasForm`, and `EditAliasForm` components (copy them into the new file)
   - Keep the `page-header` div (with `<h1>"Aliases"</h1>` and the "+ New Alias" button) and `page-header__subtitle` inside the tab content — this keeps the tab self-contained with no parent prop wiring needed
   - Remove only the outer `<div class="page">` wrapper (the parent page already provides the page-level layout)

2. In `models/mod.rs`:
   - Add `mod aliases_tab;` and `use self::aliases_tab::AliasesTab;`
   - Replace the Aliases placeholder with `<AliasesTab />` in the match block
   - The Aliases tab provides its own `<h1>` and action buttons, so the parent header remains static

3. In `pages/aliases/mod.rs`:
   - Change `mod api;` to `pub(crate) mod api;` and `mod types;` to `pub(crate) mod types;` (needed so `aliases_tab.rs` can import from them)
   - Keep `api.rs` and `types.rs` as-is (they are imported by the aliases tab)
   - Remove the `AliasesPage` component, `validate_alias_name` function, and all Leptos imports (the file is reduced to just the two module declarations)

4. Copy `validate_alias_name` from `aliases/mod.rs` into `aliases_tab.rs` (it's a small pure function, no need for a shared module).

5. In `lib.rs`, remove the `<Route path=path!("/tama/aliases") view=pages::aliases::AliasesPage />` line — this must be done now (not deferred) because removing `AliasesPage` would break the build if the route still references it.

**What NOT to change:**
- Do NOT change the API endpoints or their paths
- Do NOT change the aliases database schema
- Do NOT modify `aliases/api.rs` or `aliases/types.rs`

**Steps:**
- [ ] Create `crates/tama/src/pages/models/aliases_tab.rs` with `AliasesTab` component
- [ ] Copy `validate_alias_name`, `AliasCard`, `CreateAliasForm`, `EditAliasForm` into the new file
- [ ] Keep the `page-header` div (with `<h1>` and "+ New Alias" button) and subtitle inside the tab — remove only the outer `<div class="page">` wrapper
- [ ] Wire up imports from `crate::pages::aliases::api` and `crate::pages::aliases::types`
- [ ] In `pages/aliases/mod.rs`, change `mod api;` → `pub(crate) mod api;` and `mod types;` → `pub(crate) mod types;`
- [ ] Remove `AliasesPage`, `validate_alias_name`, and all Leptos imports from `pages/aliases/mod.rs` (leaving only the two module declarations)
- [ ] Add `mod aliases_tab;` to `models/mod.rs`
- [ ] Replace Aliases placeholder with `<AliasesTab />` in the match block
- [ ] Remove `<Route path=path!("/tama/aliases") ...>` from `lib.rs`
- [ ] Run `cargo build --package tama --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: move aliases into models page as tab"

**Acceptance criteria:**
- [ ] Aliases tab shows full CRUD functionality (list, create, edit, delete, toggle) with its own page-header and "+ New Alias" button
- [ ] Aliases tab subtitle ("Custom model aliases - point a friendly name to any loaded model.") is preserved
- [ ] `/tama/aliases` route removed from `lib.rs`
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes
- [ ] `cargo build --package tama --features ssr` succeeds

---

### Task 3: Integrate Providers (Installations) into Models page tab

**Context:** The Installations page at `/tama/installations` (component name `Backends`) manages backend installations. Move it into the Providers tab, renaming UI labels from "Backends" to "Providers" and "Backend" to "Provider". The API endpoints remain `/tama/v1/backends/...` (not renamed).

**Files:**
- Create: `crates/tama/src/pages/models/providers_tab.rs`
- Modify: `crates/tama/src/pages/models/mod.rs`
- Modify: `crates/tama/src/pages/mod.rs` (remove `pub mod installations;`)
- Modify: `crates/tama/src/lib.rs` (remove `/tama/installations` route)

**What to implement:**

1. Create `crates/tama/src/pages/models/providers_tab.rs` containing the Providers tab content:
   - Copy the entire content of `installations.rs` into the new file, including both `#[cfg(test)]` modules (`backend_url_tests` and `newline_parsing_tests`)
   - Update the module doc comment from `//! Backends page` to `//! Providers tab content`
   - Rename the top-level component from `pub fn Backends()` to `pub fn ProvidersTab()`
   - Remove only the outer `<div class="page">` wrapper — keep the `page-header` div (with `<h1>` and action buttons) inside the tab content so it stays self-contained with no parent prop wiring needed
   - Rename user-facing strings in the providers tab: "Backends" → "Providers", "Backend" → "Provider", "+ Add Backend" → "+ Add Provider" (in user-facing strings only, NOT in variable names, API URLs, struct field names, or function names)
   - Keep all API URLs as-is (they still call `/tama/v1/backends/...`)
   - The "+ Add Provider" dropdown retains all four options: `llama_cpp`, `ik_llama`, `tts_kokoro`, and Docker (which opens `DockerRegisterModal`). Only the button label changes.
   - The "Save Changes" button and `save_status` span remain inside the tab's page-header — they apply default args/env/version edits.

2. In `models/mod.rs`:
   - Add `mod providers_tab;` and `use self::providers_tab::ProvidersTab;`
   - Replace the Providers placeholder with `<ProvidersTab />` in the match block

3. Delete `crates/tama/src/pages/installations.rs` (all content moved to providers_tab.rs)

4. In `pages/mod.rs`, remove `pub mod installations;`

5. In `lib.rs`, remove the `<Route path=path!("/tama/installations") view=pages::installations::Backends />` line — this must be done now (not deferred) because removing the `installations` module would break the build if the route still references it.

**What NOT to change:**
- Do NOT rename any API endpoints (`/tama/v1/backends/...` stays)
- Do NOT rename variable names, struct fields, or function parameter names that reference "backend" (those are domain terms matching the API)
- Only change user-facing display text in the providers tab file

**Steps:**
- [ ] Create `crates/tama/src/pages/models/providers_tab.rs` from installations.rs content
- [ ] Rename component `Backends` → `ProvidersTab`
- [ ] Move both `#[cfg(test)]` modules verbatim; update the module doc comment
- [ ] Remove outer `<div class="page">` wrapper only — keep `page-header` div with `<h1>` and action buttons inside the tab
- [ ] Rename UI text in providers_tab.rs only: "Backend"/"Backends" → "Provider"/"Providers", "+ Add Backend" → "+ Add Provider"
- [ ] Keep the "+ Add Provider" dropdown with all four options (llama_cpp, ik_llama, tts_kokoro, Docker)
- [ ] Keep the "Save Changes" button and `save_status` span inside the tab header
- [ ] Add `mod providers_tab;` to `models/mod.rs`
- [ ] Replace Providers placeholder with `<ProvidersTab />`
- [ ] Delete `crates/tama/src/pages/installations.rs`
- [ ] Remove `pub mod installations;` from `pages/mod.rs`
- [ ] Remove `<Route path=path!("/tama/installations") ...>` from `lib.rs`
- [ ] Run `cargo build --package tama --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: move providers into models page as tab"

**Acceptance criteria:**
- [ ] Providers tab shows full backend management (install, update, delete) with its own page-header
- [ ] UI labels in the providers tab say "Provider"/"Providers" and "+ Add Provider" (not "Backend"/"Backends")
- [ ] "+ Add Provider" dropdown retains all four options including Docker
- [ ] "Save Changes" button and status remain functional
- [ ] API calls still target `/tama/v1/backends/...`
- [ ] Test modules (`backend_url_tests`, `newline_parsing_tests`) preserved and pass
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes
- [ ] `cargo build --package tama --features ssr` succeeds

---

### Task 4: Update sidebar — add Models, remove Installations and Aliases

**Context:** The sidebar currently has entries for Installations and Aliases that point to now-removed routes. Add a Models entry and remove the obsolete ones.

**Files:**
- Modify: `crates/tama/src/components/sidebar.rs`

**What to implement:**

1. Add a Models sidebar entry immediately after Dashboard:
```rust
<A href="/tama/models" attr:class="sidebar-item" attr:data-tooltip="Models" on:click=move |_| mobile_open.set(false)>
    <span class="sidebar-item__icon">"📦"</span>
    <span class="sidebar-item__text">"Models"</span>
</A>
```

2. Remove the Installations sidebar entry:
```rust
// Remove this block:
<A href="/tama/installations" attr:class="sidebar-item" ...>
    <span class="sidebar-item__icon">"🔧"</span>
    <span class="sidebar-item__text">"Installations"</span>
</A>
```

3. Remove the Aliases sidebar entry:
```rust
// Remove this block:
<A href="/tama/aliases" attr:class="sidebar-item" ...>
    <span class="sidebar-item__icon">"🏷️"</span>
    <span class="sidebar-item__text">"Aliases"</span>
</A>
```

4. Final sidebar order: Dashboard → Models → Logs → Updates → Downloads → Benchmarks → Keys → Config (footer)

**What NOT to change:**
- Do NOT change any other sidebar entries
- Do NOT change the collapse/toggle behavior

**Steps:**
- [ ] Add Models `<A>` element after Dashboard in `sidebar.rs`
- [ ] Remove Installations `<A>` element
- [ ] Remove Aliases `<A>` element
- [ ] Verify sidebar order is correct
- [ ] Run `cargo build --package tama --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: update sidebar — add Models, remove Installations and Aliases"

**Acceptance criteria:**
- [ ] Sidebar shows: Dashboard → Models → Logs → Updates → Downloads → Benchmarks → Keys → Config
- [ ] No "Installations" or "Aliases" entries in sidebar
- [ ] Models link navigates to `/tama/models`
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes

---

### Task 5: Pluralize model edit route and fix href references

**Context:** The model edit route uses singular `/tama/model/:id/edit` which should be plural `/tama/models/:id/edit` for consistency with the `/tama/models` listing route. Two components hardcode the old path in hrefs.

**Files:**
- Modify: `crates/tama/src/lib.rs`
- Modify: `crates/tama/src/components/model_card.rs`
- Modify: `crates/tama/src/pages/updates.rs`

**What to implement:**

1. In `lib.rs` (Routes section):
   - Change `path!("/tama/model/:id/edit")` → `path!("/tama/models/:id/edit")`

2. In `model_card.rs` (line ~302):
   - Change `href=format!("/tama/model/{}/edit", edit_id_clone)` → `href=format!("/tama/models/{}/edit", edit_id_clone)`

3. In `updates.rs` (line ~842):
   - Change `href=format!("/tama/model/{}/edit", m_item_id_for_actions)` → `href=format!("/tama/models/{}/edit", m_item_id_for_actions)`

**What NOT to change:**
- Do NOT change any API endpoint paths (only frontend routes)
- Do NOT add redirect routes for the old path

**Steps:**
- [ ] In `lib.rs`, change `/tama/model/:id/edit` → `/tama/models/:id/edit`
- [ ] In `model_card.rs`, fix href to `/tama/models/{}/edit`
- [ ] In `updates.rs`, fix href to `/tama/models/{}/edit`
- [ ] Run `cargo build --package tama --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "fix: pluralize model edit route to /tama/models/:id/edit"

**Acceptance criteria:**
- [ ] `/tama/models/:id/edit` route works (old `/tama/model/:id/edit` removed)
- [ ] Model card "Edit" link navigates to correct URL
- [ ] Updates page "Edit" link navigates to correct URL
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes
- [ ] `cargo build --package tama --features ssr` succeeds

---

### Task 6: Final verification — full gate

**Context:** All individual tasks pass their local checks. This task runs the full validation gate matching CI.

**Files:**
- No files to modify — verification only.

**Steps:**
- [ ] Run `cargo fmt --all --check`
  - Did it pass? If not, run `cargo fmt --all` and re-check.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
  - Did it pass? If not, fix clippy errors and re-run.
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
  - Did it pass? If not, fix clippy errors and re-run.
- [ ] Run `cargo nextest run --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Commit with message: "chore: verification gate for models page consolidation"

**Acceptance criteria:**
- [ ] All four gate commands pass with zero errors
- [ ] Workspace builds and tests cleanly
