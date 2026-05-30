# Aliases Page Redesign Plan

**Goal:** Redesign the Aliases page to match the app's design system — compact card rows, enabled dot indicator, proper page header layout, and dedicated CSS.

**Architecture:** Replace the bulky `.card`-based alias blocks with compact `.alias-card` rows styled like `.model-list-card` (left accent border, tight padding, two-line layout). Add dedicated CSS file `18-aliases.css`. Fix the page header so the description sits below the header instead of floating in the center.

**Tech Stack:** Leptos/WASM, CSS

---

### Task 1: Create dedicated CSS file for aliases page

**Context:**
The aliases page currently has no dedicated CSS — it relies on generic `.card`, `.card-header`, and `.badge` classes that don't fit its needs. `.card-header` is styled for stat card labels (uppercase, small, muted), not for alias names. This task creates `18-aliases.css` with all alias-specific styles.

**Files:**
- Create: `crates/tama-web/css/18-aliases.css`
- Modify: `crates/tama-web/style.css`

**What to implement:**

Create `crates/tama-web/css/18-aliases.css` with the following styles:

```css
/* ============================================
 * 26. Aliases Page
 * ============================================ */

/* Alias card — compact horizontal row like model-list-card */
.alias-card {
  display: flex;
  flex-direction: column;
  gap: 0.375rem;
  padding: 0.6rem 0.75rem 0.6rem 0.5rem;
  margin-bottom: 0.25rem;
  border-radius: 0.5rem;
  border-left: 3px solid #374151;
  background-color: var(--bg-secondary);
  border-top: 1px solid var(--border-color);
  border-right: 1px solid var(--border-color);
  border-bottom: 1px solid var(--border-color);
  transition:
    border-color var(--transition-fast),
    box-shadow var(--transition-fast),
    background var(--transition-fast);
}

.alias-card:hover {
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
}

/* Enabled aliases get a green accent strip */
.alias-card--enabled {
  border-left-color: var(--accent-green);
  box-shadow: -2px 0 8px rgba(63, 185, 80, 0.15);
}

/* Disabled aliases get a muted accent strip */
.alias-card--disabled {
  border-left-color: var(--text-muted);
}

/* Line 1 — name, enabled dot, actions */
.alias-card__line1 {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  flex-wrap: nowrap;
}

/* Enabled dot indicator — ● for enabled, ○ for disabled */
.alias-card__dot {
  flex-shrink: 0;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  border: 1.5px solid var(--accent-green);
  background: transparent;
}

.alias-card__dot--enabled {
  background: var(--accent-green);
}

.alias-card__dot--disabled {
  border-color: var(--text-muted);
  background: transparent;
}

.alias-card__name {
  font-size: 0.875rem;
  font-weight: 600;
  color: var(--text-primary);
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex-grow: 1;
  font-family: var(--font-mono);
  text-transform: uppercase;
}

.alias-card__actions {
  display: flex;
  align-items: center;
  gap: 0.375rem;
  flex-shrink: 0;
  margin-left: auto;
}

/* Line 2 — model target, description, badges */
.alias-card__line2 {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  flex-wrap: wrap;
  padding-left: 1.125rem; /* align with text after dot */
}

/* Model target — monospace with arrow prefix */
.alias-card__target {
  font-size: 0.8rem;
  color: var(--text-secondary);
  font-family: var(--font-mono);
  display: inline-flex;
  align-items: center;
  gap: 0.35rem;
}

.alias-card__target-arrow {
  color: var(--text-muted);
  font-size: 0.75rem;
}

/* Description text — muted, small */
.alias-card__description {
  font-size: 0.75rem;
  color: var(--text-muted);
  margin: 0;
}

/* Default alias badge */
.badge-pill--default {
  background: rgba(88, 166, 255, 0.12);
  color: var(--accent-blue);
}

/* Alias list container — vertical stack with gap */
.aliases-list {
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
}

/* Page header subtitle — sits below the header */
.page-header__subtitle {
  font-size: 0.9rem;
  color: var(--text-secondary);
  margin-top: 0.125rem;
  margin-bottom: 1rem;
}

/* Icon-only action buttons (reused from model-list-card pattern) */
.alias-card .btn-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  padding: 4px;
  border: 1px solid var(--border-color);
  border-radius: 4px;
  background: transparent;
  color: var(--text-muted);
  cursor: pointer;
  transition:
    color var(--transition-fast),
    border-color var(--transition-fast),
    background var(--transition-fast);
}

.alias-card .btn-icon:hover {
  color: var(--text-primary);
  border-color: var(--border-hover);
  background: rgba(255, 255, 255, 0.04);
}

.alias-card .btn-icon--danger:hover {
  color: var(--accent-red);
  border-color: var(--accent-red);
}

/* Responsive — wrap line 1 on narrow screens */
@media (max-width: 900px) {
  .alias-card__line1 {
    flex-wrap: wrap;
  }

  .alias-card__name {
    min-width: 120px;
    max-width: 60%;
  }
}
```

Add the `badge-pill--default` class above to the CSS file (it's not in the existing badge CSS).

Modify `crates/tama-web/style.css` to add the import:
- Add `@import "./css/18-aliases.css";` after the `17-api-docs.css` import line
- Update the module index comment to include `18-aliases` under Pages

**Steps:**
- [ ] Create `crates/tama-web/css/18-aliases.css` with all styles above
- [ ] Add `@import "./css/18-aliases.css";` to `style.css` after `17-api-docs.css`
- [ ] Update the module index comment in `style.css` to include `18-aliases`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add dedicated CSS for aliases page"

**Acceptance criteria:**
- [ ] `18-aliases.css` exists with `.alias-card`, `.alias-card__line1`, `.alias-card__line2`, `.alias-card__dot`, `.alias-card__name`, `.alias-card__target`, `.alias-card__description`, `.badge-pill--default`, `.aliases-list`, `.page-header__subtitle`, `.alias-card .btn-icon` styles
- [ ] `style.css` imports `18-aliases.css`
- [ ] `cargo build --package tama-web` succeeds

---

### Task 2: Rewrite Aliases page component with new design

**Context:**
The current `mod.rs` uses bulky `.card` blocks with `.card-header` (meant for stat labels) for displaying alias names. The page header has the description text floating centered between title and button. This task rewrites the component to use the new compact card design and fix the header layout.

**Files:**
- Modify: `crates/tama-web/src/pages/aliases/mod.rs`

**What to implement:**

Rewrite `crates/tama-web/src/pages/aliases/mod.rs` with the following structural changes:

1. **Fix page header layout:** The header should have `h1` on the left and `+ New Alias` button on the right. The description subtitle goes BELOW the header (not inside it), using `.page-header__subtitle`.

```rust
<div class="page-header">
    <h1>"🏷️ Aliases"</h1>
    <button class="btn btn-primary" on:click=move |_| show_create.set(true)>
        "+ New Alias"
    </button>
</div>
<p class="page-header__subtitle">"Custom model aliases — point a friendly name to any loaded model."</p>
```

2. **Move save status alerts** below the subtitle (after `.page-header__subtitle`, before the card list).
   - **IMPORTANT:** Remove the `save_status` rendering from inside `.page-header-actions` (currently at line ~93 of `mod.rs`). The `page-header-actions` div should ONLY contain the "+ New Alias" button. Render `save_status` as its own standalone `<div>` (not nested in any other element) positioned between `.page-header__subtitle` and the card list.

3. **Replace AliasCard component** — the new card uses the compact two-line layout:

```rust
fn AliasCard(...) -> impl IntoView {
    view! {
        <div class=("alias-card", if alias.enabled { "alias-card--enabled" } else { "alias-card--disabled" })>
            // Line 1: dot, name, actions
            <div class="alias-card__line1">
                <span class=("alias-card__dot", if alias.enabled { "alias-card__dot--enabled" } else { "alias-card__dot--disabled" })></span>
                <span class="alias-card__name">{alias.name.clone()}</span>
                <div class="alias-card__actions">
                    // Edit button — icon-only
                    <button class="btn-icon" title="Edit" on:click=move |_| show_edit.set(true)>
                        "✏️"
                    </button>
                    // Toggle enable/disable — icon-only
                    <button
                        class="btn-icon"
                        title=if alias.enabled { "Disable" } else { "Enable" }
                        on:click=/* toggle handler */
                    >
                        {if alias.enabled { "👁️" } else { "🚫" }}
                    </button>
                    // Delete button — icon-only with danger hover
                    <button class="btn-icon btn-icon--danger" title="Delete" on:click=/* copy existing delete handler from current AliasCard */>
                        "🗑️"
                    </button>
                </div>
            </div>
            // Line 2: target model, description, default badge
            <div class="alias-card__line2">
                <span class="alias-card__target">
                    <span class="alias-card__target-arrow">"→"</span>
                    {alias.model_name.clone()}
                </span>
                {alias.description.as_ref().map(|d| {
                    if d.is_empty() { view! { <span/> } } else { view! { <span class="alias-card__description">{d}</span> } }
                }).unwrap_or_else(|| view! { <span/> }.into_any())}
                // Default alias badge (if description contains "Default alias")
                {if alias.description.as_deref() == Some("Default alias — routes to this model") {
                    view! { <span class="badge-pill badge-pill--default">"Default alias"</span> }
                } else {
                    view! { <span/> }
                }}
            </div>
        </div>
    }
}
```

4. **Keep all existing logic unchanged** — the create/edit/delete handlers, modal forms, validation, API calls should all remain the same. Only the view/template changes.

5. **Keep the CreateAliasForm and EditAliasForm components unchanged** — they live in modals and already use the correct form styles.

6. **Empty state** — keep the existing empty state card but simplify the text to match the spec: `"No aliases yet. Click + New to create one."`

7. **Loading state** — keep existing loading spinner card.

8. **Aliases list container** — wrap cards in `<div class="aliases-list">` instead of the current wrapper.

**Specific things NOT to change:**
- Don't modify `types.rs` or `api.rs` — no type or API changes needed
- Don't modify the CreateAliasForm or EditAliasForm components (they're in modals and work fine)
- Don't modify the sidebar or routing
- Don't modify the `validate_alias_name` function

**Known limitations (not addressed in this plan):**
- The `.btn-icon` CSS in `18-aliases.css` duplicates `.model-list-card .btn-icon` from `06-badges-model-list.css`. A future refactor should extract `.btn-icon` into a shared location (e.g., `05-buttons-forms-progress.css`).
- The "default alias" detection uses exact string match on description (`"Default alias — routes to this model"`). This is fragile — if the user edits the description the badge disappears. A future improvement would add `is_default: bool` to the API response.
- `.card--centered` has no CSS definition anywhere in the project (pre-existing issue, out of scope).

**Steps:**
- [ ] Rewrite the `AliasesPage` component's view section: fix header layout (h1 left, button right, no description in header), move subtitle below header using `.page-header__subtitle`, render save_status as standalone `<div>` between subtitle and card list
- [ ] Remove `save_status` rendering from inside `.page-header-actions` — only the "+ New Alias" button should remain there
- [ ] Rewrite the `AliasCard` component: replace `.card` with `.alias-card`, use two-line layout with dot indicator, icon-only action buttons, styled target model, default alias badge
- [ ] Copy the existing toggle enable/disable handler block from the current AliasCard into the new toggle button's `on:click` — do NOT lose this logic
- [ ] Copy the existing delete handler block from the current AliasCard into the new delete button's `on:click` — do NOT lose this logic
- [ ] Update empty state text to `"No aliases yet. Click + New to create one."`
- [ ] Wrap card list in `<div class="aliases-list">`
- [ ] Verify no stale `.badge--enabled`/`.badge--disabled` class strings remain in the component
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it pass? If not, fix and re-run.
- [ ] Commit with message: "feat: redesign aliases page with compact card layout"

**Acceptance criteria:**
- [ ] Page header has `h1` left, `+ New Alias` button right, no description in header row
- [ ] Description subtitle renders below header using `.page-header__subtitle`
- [ ] Alias cards use `.alias-card` with left accent border (green for enabled, muted for disabled)
- [ ] Enabled dot indicator: filled green ● for enabled, hollow ○ for disabled
- [ ] Alias name in monospace uppercase in line 1
- [ ] Action buttons are icon-only `.btn-icon` (edit ✏️, toggle 👁️/🚫, delete with danger hover)
- [ ] Line 2 shows `→ model-name` in monospace with `.alias-card__target`
- [ ] Description shown as muted text in line 2
- [ ] "Default alias" shown as `.badge-pill--default` badge when description matches
- [ ] Cards wrapped in `.aliases-list` container
- [ ] Create/Edit/Delete logic works unchanged
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo clippy --package tama-web -- -D warnings` passes

---

### Task 3: Build verification

**Context:**
Final verification that the full workspace builds and the web UI compiles for deployment.

**Files:**
- None (verification only)

**What to implement:**

Run the full build pipeline and verify no regressions:

**Steps:**
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it pass? If not, fix and re-run.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: "chore: verify aliases redesign builds cleanly"

**Acceptance criteria:**
- [ ] `cargo fmt --all` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes

---

## Task Dependencies

```
Task 1 (CSS) ──> Task 2 (component rewrite) ──> Task 3 (verification)
```

Task 1 must come first since Task 2 references the new CSS classes.
Task 3 is a final verification pass.

---

## Verification Checklist

- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all` passes
- [ ] Page header layout matches other pages (h1 left, actions right, subtitle below)
- [ ] Alias cards are compact with left accent border
- [ ] Enabled dot indicator works (● enabled, ○ disabled)
- [ ] Model target displayed in monospace with arrow
- [ ] Default alias badge shown as blue badge-pill
- [ ] Icon-only action buttons with hover states
- [ ] Create/Edit/Delete functionality unchanged
