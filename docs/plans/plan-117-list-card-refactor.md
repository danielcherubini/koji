# Shared Components Consolidation

**Goal:** Consolidate duplicated UI patterns across the Tama web UI into shared components: `ListCard` (two-line cards), `SectionCard` (form section shells), `AlertBanner` (status alerts), and `TabButtons` (tab navigation).

**Architecture:** Four new shared components replace duplicated patterns:
1. `ListCard` — generic two-line card with accent strip, used by ModelCard, AliasCard, and Updates page items
2. `SectionCard` — card wrapper with title/description, used by Config Editor's 4 form sections
3. `AlertBanner` — colored status banner (success/error/warning/info), used across 6+ pages
4. `TabButtons` — tab navigation buttons with active/inactive styling, used by Downloads and Benchmarks

**Tech Stack:** Rust, Leptos 0.7 (WASM), CSS

---

### Task 1: Foundation — Create ListCard component and generalize CSS

**Context:**
This task creates the shared infrastructure that all subsequent tasks depend on. The `ListCard` component handles the structural HTML (accent strip div, two-line flex containers) and the CSS provides the shared layout styles. Without this task, no other task can proceed.

**Files:**
- Create: `crates/tama-web/src/components/list_card.rs`
- Rename: `crates/tama-web/css/06-badges-model-list.css` → `crates/tama-web/css/06-badges-list-card.css`
- Modify: `crates/tama-web/src/components/mod.rs`
- Modify: `crates/tama-web/style.css`
- Modify: `crates/tama-web/tests/css_test.rs`

**What to implement:**

1. **Create `list_card.rs`** — A new Leptos component with the following exact signature:

```rust
use leptos::prelude::*;

/// Generic two-line list card with left accent strip.
///
/// Shared structural component used by ModelCard, AliasCard, and Updates page items.
/// Handles: accent strip (left border), two-line flex layout, icon prefix,
/// content area, actions area, and optional line-2 metadata.
#[component]
pub fn ListCard(
    /// Accent strip state suffix. Controls the left border color.
    /// Pass e.g. "ready", "enabled", "update-available" for colored strips.
    /// When None, the default gray strip is used (no state class appended).
    ///
    /// Accepts static values (`Some("ready".into())`) or reactive signals
    /// (`Signal::derive(move || if condition { Some("enabled".into()) } else { None })`).
    #[prop(default = None)]
    #[prop(into)]
    state: Signal<Option<String>>,

    /// Leading icon or indicator (e.g. server icon, dot, chevron).
    /// Rendered at the start of line 1, before the content area.
    /// Pass as: `icon=Some(|| view! { ... }.into_any())`
    #[prop(default = None)]
    icon: Option<Children>,

    /// Line 1 content — name, inline badges, action buttons.
    /// Rendered inside `<span class="list-card__content">`.
    /// The content wrapper has `display: flex; gap: 0.5rem; flex: 1;` so children
    /// flow inline (name + badges + buttons all on one line).
    children: Children,

    /// Action buttons or icons on the far right of line 1.
    /// Rendered inside `<div class="list-card__actions">`.
    /// Typically icon-only buttons (edit, logs, toggle) or small action buttons.
    /// Pass as: `actions=Some(|| view! { ... }.into_any())`
    #[prop(default = None)]
    actions: Option<Children>,

    /// Line 2 content — badge pills, metadata, expandable content.
    /// Rendered inside `<div class="list-card__line2">`.
    /// When None, no second line is rendered at all (no empty div in DOM).
    /// Pass as: `line2=Some(move || view! { ... }.into_any())` (use `move ||` for reactive content).
    #[prop(default = None)]
    line2: Option<Children>,
) -> impl IntoView
```

2. **Rendered HTML structure** — The component renders exactly this (inside `view!` macro):

```rust
let card_class = move || {
    match state.get() {
        Some(ref s) => format!("list-card list-card--{s}"),
        None => "list-card".to_string(),
    }
};

view! {
    <div class=card_class>
        <div class="list-card__line1">
            {icon.map(|i| i())}
            <span class="list-card__content">{children()}</span>
            {actions.map(|a| a())}
        </div>
        {line2.map(|l| view! { <div class="list-card__line2">{l()}</div> })}
    </div>
}
```

**Important**: All three optional slots (`icon`, `actions`, `line2`) use `.map()` (not `.as_ref().map()`) because `Children` is `Box<dyn FnOnce()>` — it must be consumed by value, not called through a reference. The `line2` slot does NOT need a `move ||` wrapper — it's statically set at component creation and the returned view's content reacts to signals through captured `RwSignal`/`ReadSignal` references.

When `state` is `None`, the outer div is just `class="list-card"` (no trailing `--`).
When `state` is `Some("ready")`, the outer div is `class="list-card list-card--ready"`.
Reactive states (via `Signal::derive`) automatically update the class when the signal changes.

3. **Rename CSS file:** `css/06-badges-model-list.css` → `css/06-badges-list-card.css`

4. **Generalize CSS classes** in the renamed file — rename ALL occurrences:

| Old selector | New selector |
|---|---|
| `.model-list-card` | `.list-card` |
| `.model-list-card:hover` | `.list-card:hover` |
| `.model-list-card--ready` | `.list-card--ready` |
| `.model-list-card--loading` | `.list-card--loading` |
| `.model-list-card--unloading` | `.list-card--unloading` |
| `.model-list-card--failed` | `.list-card--failed` |
| `.model-list-card__line1` | `.list-card__line1` |
| `.model-list-card--ready .model-list-card__icon` | `.list-card--ready .list-card__icon` |
| `.model-list-card--loading .model-list-card__icon` | `.list-card--loading .list-card__icon` |
| `.model-list-card--failed .model-list-card__icon` | `.list-card--failed .list-card__icon` |
| `.model-list-card__icon` | `.list-card__icon` |
| `.model-list-card__name` | `.list-card__name` |
| `.model-list-card__actions` | `.list-card__actions` |
| `.model-list-card__line2` | `.list-card__line2` |
| `.model-list-card .btn-icon` | `.list-card .btn-icon` |
| `.model-list-card .btn-icon svg` | `.list-card .btn-icon svg` |
| `.model-list-card .btn-icon:hover` | `.list-card .btn-icon:hover` |
| `.model-list-card .btn-icon:disabled` | `.list-card .btn-icon:disabled` |
| `@media ... .model-list-card__line1` | `@media ... .list-card__line1` |
| `@media ... .model-list-card__name` | `@media ... .list-card__name` |

5. **Add new CSS rules** to the renamed file:

```css
/* Content wrapper — flex container for line 1 children (name + badges + buttons) */
.list-card__content {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  flex: 1 1 0;
  min-width: 0;
  overflow: hidden;
}

/* New state classes for non-model consumers */
.list-card--enabled {
  border-left-color: var(--accent-green);
  box-shadow: -2px 0 8px rgba(63, 185, 80, 0.15);
}

.list-card--disabled {
  border-left-color: var(--text-muted);
}

.list-card--update-available {
  border-left-color: var(--accent-yellow);
  box-shadow: -2px 0 6px rgba(210, 153, 34, 0.2);
}
```

6. **Update `components/mod.rs`** — add `pub mod list_card;` to the module declarations.

7. **Update `style.css`** — change the `@import` path from `06-badges-model-list.css` to `06-badges-list-card.css`. Update the module index comment from `06-badges-model-list` to `06-badges-list-card`.

8. **Update `tests/css_test.rs`** — change the `include_str!` path from `06-badges-model-list.css` to `06-badges-list-card.css`. Rename the const from `CSS_06` reference if needed (the const name can stay `CSS_06`, just the path changes).

**Steps:**
- [ ] Create `crates/tama-web/src/components/list_card.rs` with the `ListCard` component
- [ ] Rename `css/06-badges-model-list.css` to `css/06-badges-list-card.css`
- [ ] Rename all `.model-list-card*` selectors to `.list-card*` in the renamed file
- [ ] Add `.list-card__content`, `.list-card--enabled`, `.list-card--disabled`, `.list-card--update-available` rules
- [ ] Add `pub mod list_card;` to `src/components/mod.rs`
- [ ] Update `style.css` @import path and module index comment
- [ ] Update `tests/css_test.rs` include_str! path
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add generic ListCard component and generalize CSS classes"

**Acceptance criteria:**
- [ ] `ListCard` component compiles with the exact signature above
- [ ] CSS file renamed to `06-badges-list-card.css` with no remaining `.model-list-card*` selectors
- [ ] New state classes (`.list-card--enabled`, `--disabled`, `--update-available`) present in CSS
- [ ] `.list-card__content` class present with flex layout
- [ ] `style.css` imports the renamed file
- [ ] `tests/css_test.rs` references the renamed file
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo test --package tama-web` passes

---

### Task 2: Refactor ModelCard to use ListCard

**Context:**
ModelCard is the most complex consumer — it has status badges, load/unload buttons, icon actions (logs, edit), and line-2 badge pills. This task replaces its inline HTML structure with `ListCard` while keeping ALL existing helper functions and the exact same public API. Dashboard and models page callers must work without any changes.

**Files:**
- Modify: `crates/tama-web/src/components/model_card.rs`

**What to implement:**

1. **Add import:** `use crate::components::list_card::ListCard;`

2. **Keep ALL existing helper functions** with one change: update `server_icon()`'s SVG class from `model-list-card__icon` to `list-card__icon` (the CSS class was renamed in Task 1).

```rust
fn server_icon() -> impl IntoView {
    view! {
        <svg viewBox="0 0 16 16" fill="currentColor" xmlns="http://www.w3.org/2000/svg" class="list-card__icon">
            // ... rest unchanged
        </svg>
    }
}
```

3. **Keep the exact same `ModelCard` component signature** — do not change any props.

4. **Replace the inline view template** with a `ListCard` wrapper. The key mapping:

- `state` prop: Map `effective_state` to `Option<String>` (static — no `Signal::derive` needed):
  - `"ready"` → `Some("ready".into())`
  - `"loading"` → `Some("loading".into())`
  - `"unloading"` → `Some("unloading".into())`
  - `"failed"` → `Some("failed".into())`
  - anything else (idle, unknown) → `None` (NOT `Some("idle")`)
  - Pass as: `state=card_state_option.into()` where `card_state_option: Option<String>`

- `icon` slot: `icon=Some(|| view! { {server_icon()} }.into_any())`

- `children` (line 1 content): `<span class="list-card__name">{display_name}</span>` + enabled badge + status badge + load/unload button. All these flow inline via the `__content` flex wrapper.

- `actions` slot: `actions=Some(|| view! { /* logs link + edit link */ }.into_any())`

- `line2` slot: `line2=Some(|| view! { /* badge pills */ }.into_any())`

5. **The load/unload button** sits inside `children` (not in `actions`). It flows inline after the status badge because `__content` has `display: flex; gap: 0.5rem;`. The button rendering logic (ready → unload, idle → load, failed → retry, loading/unloading → disabled) stays exactly the same.

6. **The edit link** uses the same `edit_id` logic (db_id when Some, id string otherwise).

7. **The logs link** is only rendered when `log_source` is Some.

8. **Keep ALL existing tests** in the `#[cfg(test)]` module unchanged. They test helper functions, not rendering.

**Steps:**
- [ ] Add `use crate::components::list_card::ListCard;` import
- [ ] Update `server_icon()` SVG class from `model-list-card__icon` to `list-card__icon`
- [ ] Replace the `view!` block in `ModelCard` with `ListCard` wrapper
- [ ] Map `effective_state` to `Option<String>` for the `state` prop (None for idle), pass via `.into()` for `#[prop(into)]` conversion
- [ ] Use `actions=Some(|| view! { ... }.into_any())` prop syntax for all optional slots (not `<#actions>` tags)
- [ ] Move server icon to `icon` slot, name+badges+button to `children`, icon links to `actions`, badge pills to `line2`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-web -- model_card`
  - Did all model_card tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: ModelCard uses generic ListCard component"

**Acceptance criteria:**
- [ ] `ModelCard` has the exact same public API (all props unchanged)
- [ ] All existing `model_card` tests pass
- [ ] `cargo build --package tama-web` succeeds
- [ ] Dashboard page still renders model cards (visual verification via trunk serve)
- [ ] Models page still renders model cards with load/unload functionality

---

### Task 3: Refactor AliasCard to use ListCard

**Context:**
AliasCard currently renders its own `.alias-card` div with structural styles (padding, border-left, background, flex layout). After this task, it uses `ListCard` for the structure and only provides alias-specific content. The CSS file is also cleaned up to remove duplicated structural styles.

**Files:**
- Modify: `crates/tama-web/src/pages/aliases/mod.rs`
- Modify: `crates/tama-web/css/18-aliases.css`

**What to implement:**

1. **Add import:** `use crate::components::list_card::ListCard;` in `aliases/mod.rs`.

2. **Replace the `AliasCard` view template** — the current `<div class="alias-card ...">` becomes `<ListCard>`:

```rust
<ListCard
    state=some_state.into()   // Option<String> converted via #[prop(into)]
    icon=Some(|| view! { <span class="alias-card__dot ..."></span> }.into_any())
    actions=Some(|| view! { /* edit, toggle, delete btn-icons */ }.into_any())
    line2=Some(|| view! { /* target model, description, default badge */ }.into_any())
>
    <span class="alias-card__name">{alias_name}</span>
</ListCard>
```

Where `some_state` is `Some("enabled".into())` when `alias_enabled`, `Some("disabled".into())` when not.

3. **The edit modal** stays exactly the same — it's rendered after the card, not inside it.

4. **Update `css/18-aliases.css`** — remove structural styles from `.alias-card`:

Remove these properties from `.alias-card` rule (they come from `.list-card` now):
- `display: flex; flex-direction: column; gap: 0.375rem;`
- `padding: 0.6rem 0.75rem 0.6rem 0.5rem;`
- `margin-bottom: 0.25rem;`
- `border-radius: 0.5rem;`
- `border-left: 3px solid #374151;`
- `background-color: var(--bg-secondary);`
- `border-top`, `border-right`, `border-bottom` (1px solid var(--border-color))
- `transition: ...`
- The `.alias-card:hover` rule (comes from `.list-card:hover`)

Remove these properties from `.alias-card--enabled` (comes from `.list-card--enabled`):
- `border-left-color: var(--accent-green);`
- `box-shadow: -2px 0 8px rgba(63, 185, 80, 0.15);`

Remove these properties from `.alias-card--disabled`:
- `border-left-color: var(--text-muted);`

**Delete the following CSS rules** — they no longer correspond to any DOM element:
- `.alias-card__line1 { ... }` — replaced by `.list-card__line1`
- `.alias-card__line2 { ... }` — replaced by `.list-card__line2`
- `.alias-card__actions { ... }` — replaced by `.list-card__actions`

These selectors targeted DOM structure that now lives inside `ListCard`. Delete the entire rule blocks, not just individual properties.

**Delete the `.alias-card .btn-icon` and `.alias-card .btn-icon:hover` rules** — they are now covered by `.list-card .btn-icon` and `.list-card .btn-icon:hover` in `06-badges-list-card.css`. Leaving them would create dead CSS and potential specificity conflicts.

**Rename orphaned descendant selector:** Replace `.alias-card .btn-icon--danger:hover` with `.list-card .btn-icon--danger:hover` (the parent `.alias-card` class no longer exists in the DOM, so the selector must target `.list-card` instead):
```css
.list-card .btn-icon--danger:hover {
  color: var(--accent-red);
  border-color: var(--accent-red);
}
```

Keep `.alias-card` as an empty class declaration only if needed for other descendant selectors.

5. **Keep in `18-aliases.css`:** Alias-specific content classes (`.alias-card__dot`, `.alias-card__name`, `.alias-card__target`, `.alias-card__target-arrow`, `.alias-card__description`, `.badge-pill--default`, `.aliases-list`, `.page-header__subtitle`).

6. **Remove the responsive media query** from `18-aliases.css` — the responsive rules (wrap line1, min-width on name) are generic and already exist in `06-badges-list-card.css` as `.list-card__line1` and `.list-card__name`.

**Steps:**
- [ ] Add `use crate::components::list_card::ListCard;` import
- [ ] Replace `AliasCard`'s `<div class="alias-card ...">` with `<ListCard>` wrapper
- [ ] Map enabled/disabled to state prop, dot to icon, name to children, buttons to actions, metadata to line2
- [ ] Remove structural styles from `.alias-card` in `18-aliases.css`
- [ ] Delete orphaned CSS rules: `.alias-card__line1`, `.alias-card__line2`, `.alias-card__actions`, `.alias-card .btn-icon`, `.alias-card .btn-icon:hover`
- [ ] Rename `.alias-card .btn-icon--danger:hover` → `.list-card .btn-icon--danger:hover`
- [ ] Remove duplicate responsive media query from `18-aliases.css` (already in `06-badges-list-card.css`)
- [ ] Add `CSS_18` constant to `tests/css_test.rs`: `const CSS_18: &str = include_str!("../css/18-aliases.css");` and include it in `combined_css()`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: AliasCard uses generic ListCard component"

**Acceptance criteria:**
- [ ] AliasCard renders with `ListCard` structural wrapper
- [ ] `.alias-card` CSS has no structural styles (padding, border, background, flex)
- [ ] Alias-specific styles (dot, name, target, description) are preserved
- [ ] `cargo build --package tama-web` succeeds
- [ ] Aliases page renders correctly with enabled/disabled accent strips

---

### Task 4: Refactor Updates page items to use ListCard

**Context:**
The Updates page has three types of items: backend updates, model updates (with expandable quants), and the SelfUpdateSection. All three currently use `.update-item` with `__info` / `__actions` sub-classes. This task converts all three to use `ListCard`, and removes the duplicated structural CSS from `.update-item`.

**Files:**
- Modify: `crates/tama-web/src/pages/updates.rs`
- Modify: `crates/tama-web/src/components/self_update_section.rs`
- Modify: `crates/tama-web/css/08-updates.css`

**What to implement:**

1. **Add import** in both files: `use crate::components::list_card::ListCard;`

2. **Refactor backend update items** in `updates.rs` — the section that renders `updates.backends`:

Replace `<div class="update-item" class:update-available=b.update_available>` with:

```rust
<ListCard
    state=if b.update_available { Some("update-available".into()) } else { None }
    actions=Some(|| view! {
        {/* Update button (when available) + Refresh button */}
    }.into_any())
>
    <span class="update-item__name">{b.item_id.clone()}</span>
    {/* variant badge */}
    <span class="update-item__version">{current_version}</span>
    {/* update/up-to-date badge */}
</ListCard>
```

Where:
- `children`: item_id name, optional variant badge, current_version, update/up-to-date badge
- `actions`: Update button (when available) + Refresh button (always)
- No `icon` slot, no `line2` slot

3. **Refactor model update items** in `updates.rs` — the section that renders `updates.models`:

Replace `<div class="update-item" class:update-available=has_updates>` with:

```rust
<ListCard
    state=if has_updates { Some("update-available".into()) } else { None }
    icon=Some(|| view! { /* chevron toggle span */ }.into_any())
    line2=Some(move || view! {
        {/* expandable quant list */}
        {/* action buttons (Refresh Metadata + Edit) — kept in line2 to preserve visual order */}
    }.into_any())
>
    <span class="update-item__name">{display_name}</span>
    {/* version info + update/up-to-date badge */}
</ListCard>
```

Where:
- `icon`: The chevron toggle (expand/collapse)
- `children`: display_name, current_version (short sha), update/up-to-date badge
- `line2`: The expandable quant list (when expanded) + action buttons ("Refresh Metadata" + "Edit")
  - **Important**: Action buttons are inside `line2` (not `actions`) to preserve the existing visual order where buttons appear below the quant list, not above it
  - **Note**: Wrap the action buttons in `<div style="display:flex;gap:0.5rem;">` inside the `line2` closure to maintain the original `0.5rem` gap (`.list-card__line2` has `gap: 0.25rem` which is tighter)
- No `actions` slot

4. **Refactor SelfUpdateSection** in `self_update_section.rs`:

Replace `<div class="update-item" class:update-available=...>` with:

```rust
<ListCard
    state=Signal::derive(move || {
        if update_available.get() { Some("update-available".into()) } else { None }
    })
    actions=Some(move || view! {
        {/* Check for updates / Update / Refresh buttons — conditional based on state */}
    }.into_any())
>
    <span class="update-item__name">"Tama"</span>
    {/* version + update badge + progress/error states */}
</ListCard>
```

Where:
- `state` uses `Signal::derive()` because `update_available` is a `RwSignal<bool>` that changes over time
- `children`: "Tama" name, version display, update available badge, up-to-date badge, loading state, progress state, error state
- `actions`: Check for updates / Update / Refresh buttons (conditional based on state)
- No `icon` slot, no `line2` slot

5. **Update `css/08-updates.css`** — remove structural styles from `.update-item`:

Remove from `.update-item` rule:
- `display: flex; justify-content: space-between; align-items: center;`
- `padding: 1rem;`
- `background-color: var(--bg-secondary);`
- `border: 1px solid var(--border-color);`
- `border-radius: var(--radius-md);`

Remove from `.update-item.update-available`:
- `border-color: var(--accent-yellow);`

Remove `.update-item__info` rule (replaced by `.list-card__content`).

Remove `.update-item__actions` rule (replaced by `.list-card__actions`).

Keep `.update-item` as an empty class declaration.

**Keep in `08-updates.css`:** `.updates-page`, `.last-checked`, `.error-banner`, `.updates-section`, `.section__title`, `.updates-list`, `.update-item__name`, `.update-item__version`, `.update-item__variant`, `.update-badge`, `.up-to-date-badge`, `.sidebar-badge`.

6. **Note:** `CSS_18` was already added to `tests/css_test.rs` in Task 3. Verify it's present before proceeding.

**Steps:**
- [ ] Add `use crate::components::list_card::ListCard;` to `updates.rs` and `self_update_section.rs`
- [ ] Replace backend update items with `ListCard` wrapper — use `actions=Some(|| view! { ... }.into_any())` prop syntax
- [ ] Replace model update items with `ListCard` wrapper — icon in `icon` slot, quants + action buttons in `line2` slot (to preserve visual order)
- [ ] Replace SelfUpdateSection's `.update-item` div with `ListCard` wrapper — use `Signal::derive()` for reactive state
- [ ] Remove structural styles from `.update-item`, `.update-item__info`, `.update-item__actions` in `08-updates.css`
- [ ] Add `CSS_18` constant to `tests/css_test.rs` and include it in `combined_css()`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: Updates page items use generic ListCard component"

**Implementation note:** The `Children` closures use `.into_any()` to convert `view!` output. If the compiler rejects `|| view! { ... }.into_any()` for `Option<Children>`, try without `.into_any()` first — Leptos 0.7 may auto-wrap `AnyView` in `Fragment`. If neither compiles, use `Children` directly: `Some(Box::new(|| view! { ... }))`.

**Acceptance criteria:**
- [ ] Backend update items render with `ListCard` (name + version + badges in children, buttons in actions)
- [ ] Model update items render with `ListCard` (chevron in icon, name + version in children, quants + buttons in line2)
- [ ] SelfUpdateSection renders with `ListCard` (name + version + badges in children, buttons in actions)
- [ ] `.update-item` CSS has no structural styles
- [ ] `.update-item__info` and `.update-item__actions` CSS rules removed
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo test --package tama-web` passes
- [ ] Updates page renders correctly with update-available accent strips
- [ ] Visual verification via `trunk serve`: model update action buttons appear below quant list (not above)

---

### Task 5: Final verification and cleanup

**Context:**
After all eight refactoring tasks, this final pass ensures everything works together, runs clippy, and verifies no regressions.

**Files:**
- No new files. Verification only.

**What to implement:**

1. **Run full workspace checks:**
   - `cargo check --workspace` — no errors
   - `cargo fmt --all` — no formatting changes needed
   - `cargo clippy --workspace -- -D warnings` — no warnings
   - `cargo test --workspace` — all tests pass

2. **Verify CSS has no orphaned selectors:**
   - Grep for `.model-list-card` across all CSS files — should find zero matches
   - Grep for `.update-item__info` across all CSS files — should find zero matches
   - Grep for `.update-item__actions` across all CSS files — should find zero matches

3. **Verify no dead code:**
   - Check that `list_card` module is properly exported from `components/mod.rs`
   - Check that `ListCard` is imported in all three consumer files

4. **Build release:**
   - `cargo build --release --package tama-web` — succeeds

**Steps:**
- [ ] Run `cargo check --workspace` — no errors
- [ ] Run `cargo fmt --all` — no changes
- [ ] Run `cargo clippy --workspace -- -D warnings` — no warnings
- [ ] Run `cargo test --workspace` — all pass
- [ ] Grep for orphaned `.model-list-card` selectors — zero matches
- [ ] Grep for orphaned `.update-item__info` / `.update-item__actions` — zero matches
- [ ] Run `cargo build --release --package tama-web` — succeeds
- [ ] Commit with message: "chore: verify ListCard refactor — clippy clean, all tests pass"

**Acceptance criteria:**
- [ ] `cargo check --workspace` passes
- [ ] `cargo fmt --all` makes no changes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] No orphaned `.model-list-card*` CSS selectors
- [ ] `cargo build --release --package tama-web` succeeds

---

### Task 6: SectionCard — Shared Form Section Shell

**Context:**
The Config Editor page has 4 form sections (General, Proxy, Supervisor, Sampling) that all share the same HTML shell: `<div class="card"><h2>...</h2><p class="text-muted">...</p>...</div>`. This task extracts that pattern into a reusable `SectionCard` component, eliminating 4× duplication. The component is simple — no reactivity, just structural dedup.

**Files:**
- Create: `crates/tama-web/src/components/section_card.rs`
- Modify: `crates/tama-web/src/components/mod.rs`
- Modify: `crates/tama-web/src/pages/config_editor.rs`

**What to implement:**

1. **Create `section_card.rs`:**

```rust
use leptos::prelude::*;

/// Card wrapper for form sections with title and optional description.
///
/// Replaces the repeated `<div class="card"><h2>...</h2><p class="text-muted">...</p>`
/// pattern in config_editor.rs and similar form pages.
#[component]
pub fn SectionCard(
    /// Section title rendered as <h2>.
    title: String,
    /// Optional description rendered as <p class="text-muted"> below the title.
    #[prop(default = None)]
    description: Option<String>,
    /// Form fields or other content.
    children: Children,
) -> impl IntoView {
    view! {
        <div class="card">
            <h2>{title}</h2>
            {description.map(|d| view! { <p class="text-muted">{d}</p> })}
            {children()}
        </div>
    }
}
```

2. **Update `components/mod.rs`** — add `pub mod section_card;`

3. **Refactor `config_editor.rs`** — replace all 4 form sections:

Before:
```rust
<div class="card">
    <h2>"General Settings"</h2>
    <p class="text-muted">"Global Tama settings."</p>
    <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
        <!-- form fields -->
    </div>
</div>
```

After:
```rust
<SectionCard title="General Settings" description=Some("Global Tama settings.")>
    <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
        <!-- form fields -->
    </div>
</SectionCard>
```

Apply to all 4 sections: General Settings, Proxy Settings, Supervisor, Sampling Templates.

4. **Keep the individual form components** (`GeneralForm`, `ProxyForm`, `SupervisorForm`, `SamplingForm`) — only the outer shell in `ConfigEditor` changes. The form components themselves still render their own `<div class="card">` which is now wrapped by `SectionCard`. Actually, the forms ARE the cards — so the refactoring is:

- Remove the `<div class="card"><h2>...</h2><p class="text-muted">...</p>` wrapper from each form component
- Wrap each form component's content in `<SectionCard>` in the main `ConfigEditor` view

Actually, looking at the code more carefully: each form component (`GeneralForm`, etc.) renders its own `<div class="card"><h2>...</h2>...`. The simplest refactor is to replace the card wrapper in each form component with `SectionCard`.

**Steps:**
- [ ] Create `crates/tama-web/src/components/section_card.rs` with the `SectionCard` component
- [ ] Add `pub mod section_card;` to `src/components/mod.rs`
- [ ] In `config_editor.rs`, replace each form's `<div class="card"><h2>...</h2><p class="text-muted">...</p>` with `<SectionCard title="..." description=Some("...")>`
  - GeneralForm: title="General Settings", description="Global Tama settings."
  - ProxyForm: title="Proxy Settings", description="Configure the proxy server that routes OpenAI/Ollama-compatible requests."
  - SupervisorForm: title="Supervisor", description="Process restart and health-check behavior for managed models."
  - SamplingForm: title="Sampling Templates", description="Reusable named sets of LLM sampling parameters."
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add SectionCard component and refactor config editor forms"

**Acceptance criteria:**
- [ ] `SectionCard` component compiles and exports correctly
- [ ] All 4 config editor form sections use `SectionCard`
- [ ] No `<div class="card"><h2>` pattern remains in config_editor.rs
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo test --package tama-web` passes

---

### Task 7: AlertBanner — Shared Status Alerts

**Context:**
Alert banners are scattered across 6+ pages with inconsistent implementations: some use `.alert alert--success/error` classes, some use inline styled divs (`background:#fee2e2;border:1px solid #ef4444`), and some use `.error-banner`. This task creates a shared `AlertBanner` component with a typed variant enum, and replaces all inline implementations.

**Files:**
- Create: `crates/tama-web/src/components/alert_banner.rs`
- Modify: `crates/tama-web/src/components/mod.rs`
- Modify: `crates/tama-web/src/pages/backends.rs`
- Modify: `crates/tama-web/src/pages/updates.rs`
- Modify: `crates/tama-web/src/pages/models.rs`
- Modify: `crates/tama-web/src/pages/aliases/mod.rs`
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs`
- Modify: `crates/tama-web/css/12-forms-alerts.css`

**What to implement:**

1. **Create `alert_banner.rs`:**

```rust
use leptos::prelude::*;

/// Alert variant — determines colors and default icon.
#[derive(Debug, Clone, Copy, Default)]
pub enum AlertVariant {
    Success,  // green
    Error,    // red
    Warning,  // amber/yellow
    #[default]
    Info,     // blue
}

impl AlertVariant {
    fn css_class(self) -> &'static str {
        match self {
            AlertVariant::Success => "alert alert--success",
            AlertVariant::Error => "alert alert--error",
            AlertVariant::Warning => "alert alert--warning",
            AlertVariant::Info => "alert alert--info",
        }
    }

    fn default_icon(self) -> &'static str {
        match self {
            AlertVariant::Success => "✓",
            AlertVariant::Error => "✗",
            AlertVariant::Warning => "⚠",
            AlertVariant::Info => "ℹ",
        }
    }
}

/// Colored alert banner for displaying status messages.
///
/// Replaces inline styled error/success divs across multiple pages.
#[component]
pub fn AlertBanner(
    /// Alert type that determines colors and default icon.
    #[prop(default = AlertVariant::Info)]
    variant: AlertVariant,
    /// Optional custom icon. Defaults to variant-specific icon (✓, ✗, ⚠, ℹ).
    #[prop(default = None)]
    icon: Option<String>,
    /// Alert message content.
    children: Children,
) -> impl IntoView {
    let icon_text = icon.unwrap_or_else(|| variant.default_icon().to_string());
    view! {
        <div class={variant.css_class()}>
            <span class="alert__icon">{icon_text}</span>
            <span>{children()}</span>
        </div>
    }
}
```

2. **Update `components/mod.rs`** — add `pub mod alert_banner;`

3. **Ensure CSS has all variant classes** in `12-forms-alerts.css`. Check for and add if missing:
- `.alert--success` (green) — should already exist
- `.alert--error` (red) — should already exist
- `.alert--warning` (amber) — may need to be added
- `.alert--info` (blue) — may need to be added
- `.alert__icon` — may need to be added (spacing between icon and text)

If `.alert--warning` or `.alert--info` are missing, add them following the existing pattern:
```css
.alert--warning {
  background-color: rgba(210, 153, 34, 0.1);
  border: 1px solid var(--accent-yellow);
  color: var(--accent-yellow);
}

.alert--info {
  background-color: rgba(88, 166, 255, 0.1);
  border: 1px solid var(--accent-blue);
  color: var(--accent-blue);
}

.alert__icon {
  margin-right: 0.5rem;
}
```

4. **Refactor each page:**

**backends.rs** — Replace the inline styled error div:
```rust
// Before:
<div style="background:#fee2e2;border:1px solid #ef4444;color:#b91c1c;padding:0.75rem;border-radius:4px;margin-bottom:1rem;font-size:0.875rem;">
    {err}
</div>

// After:
<AlertBanner variant=AlertVariant::Error>{err}</AlertBanner>
```

**updates.rs** — Replace `<div class="error-banner">`:
```rust
// Before:
<div class="error-banner">{e}</div>

// After:
<AlertBanner variant=AlertVariant::Error>{e}</AlertBanner>
```

**models.rs** — Replace `<div class="alert alert--success/error">`:
```rust
// Before:
<div class=cls>{msg}</div>  // where cls = "alert alert--success" or "alert alert--error"

// After:
<AlertBanner variant={if ok { AlertVariant::Success } else { AlertVariant::Error }}>{msg}</AlertBanner>
```

**aliases/mod.rs** — Same pattern as models.rs for save_status alerts.

**dashboard/mod.rs** — Replace `<p class="text-error">`:
```rust
// Before:
<p class="text-error">"Failed to load metrics stream. Is Tama running?"</p>

// After:
<AlertBanner variant=AlertVariant::Error>"Failed to load metrics stream. Is Tama running?"</AlertBanner>
```

5. **Note:** The `config_editor.rs` save status (`<span class="text-muted">{save_status}</span>`) is inline text next to a button — NOT a banner. Leave it as-is.

**Steps:**
- [ ] Create `crates/tama-web/src/components/alert_banner.rs` with `AlertBanner` and `AlertVariant`
- [ ] Add `pub mod alert_banner;` to `src/components/mod.rs`
- [ ] Check `css/12-forms-alerts.css` for `.alert--warning`, `.alert--info`, `.alert__icon` — add if missing
- [ ] Refactor `backends.rs` — replace inline styled error div
- [ ] Refactor `updates.rs` — replace `.error-banner`
- [ ] Refactor `models.rs` — replace conditional `.alert` div
- [ ] Refactor `aliases/mod.rs` — replace conditional `.alert` div
- [ ] Refactor `dashboard/mod.rs` — replace `.text-error` paragraph
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add AlertBanner component and replace inline alerts"

**Acceptance criteria:**
- [ ] `AlertBanner` component compiles with `AlertVariant` enum
- [ ] All 6 pages use `AlertBanner` (no inline styled error/success divs)
- [ ] CSS has `.alert--success`, `.alert--error`, `.alert--warning`, `.alert--info`, `.alert__icon`
- [ ] `config_editor.rs` inline save status left unchanged (different use case)
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo test --package tama-web` passes

---

### Task 8: TabButtons — Shared Tab Navigation

**Context:**
Both Downloads and Benchmarks pages have identical tab switching logic: an `active_tab` signal, buttons with conditional active/inactive classes, and conditional content rendering. This task extracts the button rendering into a shared `TabButtons` component. The caller manages the `active_tab` signal and content conditionals (presenter-controlled pattern).

**Files:**
- Create: `crates/tama-web/src/components/tab_buttons.rs`
- Modify: `crates/tama-web/src/components/mod.rs`
- Modify: `crates/tama-web/src/pages/downloads.rs`
- Modify: `crates/tama-web/src/pages/benchmarks/mod.rs`

**What to implement:**

1. **Create `tab_buttons.rs`:**

```rust
use leptos::prelude::*;

/// Tab button definition — key and display label.
#[derive(Debug, Clone)]
pub struct TabButton {
    pub key: String,
    pub label: String,
}

/// Tab navigation buttons with active/inactive styling.
///
/// Presenter-controlled: the caller manages the active tab signal and
/// renders tab content conditionally. This component only renders the buttons.
#[component]
pub fn TabButtons(
    /// Currently active tab key.
    active: Signal<String>,
    /// Tab definitions — key and display label.
    tabs: Vec<TabButton>,
    /// Called with the tab key when clicked.
    on_select: Callback<String>,
    /// CSS class for active button. Default: "btn btn-sm btn-primary".
    #[prop(default = "btn btn-sm btn-primary".into())]
    active_class: String,
    /// CSS class for inactive button. Default: "btn btn-sm btn-outline-secondary".
    #[prop(default = "btn btn-sm btn-outline-secondary".into())]
    inactive_class: String,
) -> impl IntoView {
    view! {
        <div class="tab-buttons">
            {tabs.into_iter().map(|tab| {
                let key = tab.key.clone();
                let label = tab.label.clone();
                let active_clone = active.clone();
                let ac = active_class.clone();
                let ic = inactive_class.clone();
                view! {
                    <button
                        class=move || if active_clone.get() == key { &ac } else { &ic }
                        on:click=move |_| on_select.run(key.clone())
                    >
                        {label}
                    </button>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}
```

2. **Update `components/mod.rs`** — add `pub mod tab_buttons;`

3. **Refactor `downloads.rs`:**

Before:
```rust
<div class="downloads-tabs">
    <button class=move || format!("tab-btn {}", if active_tab.get() == "active" { "active" } else { "" })>
        "Active"
    </button>
    <button class=move || format!("tab-btn {}", if active_tab.get() == "history" { "active" } else { "" })>
        "History"
    </button>
</div>
```

After:
```rust
<TabButtons
    active=Signal::derive(move || active_tab.get())
    tabs=vec![
        TabButton { key: "active".into(), label: "Active".into() },
        TabButton { key: "history".into(), label: "History".into() },
    ]
    on_select=Callback::new(move |key| active_tab.set(key))
    active_class="tab-btn active".into()
    inactive_class="tab-btn".into()
/>
```

Note: Remove the `<div class="downloads-tabs">` wrapper — `TabButtons` renders its own `<div class="tab-buttons">`.

4. **Refactor `benchmarks/mod.rs`:**

Before:
```rust
<div class="tab-buttons">
    <button class=move || if active_tab.get() == "llama-bench" { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline-secondary" }>
        "LLaMA-Bench"
    </button>
    <button class=move || if active_tab.get() == "spec-decode" { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline-secondary" }>
        "Spec Decoding"
    </button>
    <button class=move || if active_tab.get() == "mtp-testing" { "btn btn-sm btn-primary" } else { "btn btn-sm btn-outline-secondary" }>
        "MTP Testing"
    </button>
</div>
```

After:
```rust
<TabButtons
    active=Signal::derive(move || active_tab.get())
    tabs=vec![
        TabButton { key: "llama-bench".into(), label: "LLaMA-Bench".into() },
        TabButton { key: "spec-decode".into(), label: "Spec Decoding".into() },
        TabButton { key: "mtp-testing".into(), label: "MTP Testing".into() },
    ]
    on_select=Callback::new(move |key| active_tab.set(key))
/>
```

Uses default active/inactive classes.

**Steps:**
- [ ] Create `crates/tama-web/src/components/tab_buttons.rs` with `TabButtons` and `TabButton`
- [ ] Add `pub mod tab_buttons;` to `src/components/mod.rs`
- [ ] Refactor `downloads.rs` — replace inline tab buttons with `TabButtons` (custom active/inactive classes)
- [ ] Refactor `benchmarks/mod.rs` — replace inline tab buttons with `TabButtons` (default classes)
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add TabButtons component and refactor tab navigation"

**Acceptance criteria:**
- [ ] `TabButtons` component compiles with `TabButton` struct
- [ ] Downloads page uses `TabButtons` with custom active/inactive classes
- [ ] Benchmarks page uses `TabButtons` with default classes
- [ ] No inline `class=move || if active_tab.get()` pattern remains in downloads.rs or benchmarks/mod.rs
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo test --package tama-web` passes

---

### Task 9: Final verification and cleanup

**Context:**
After all eight tasks, this final pass ensures everything works together, runs clippy, and verifies no regressions.

**Files:**
- No new files. Verification only.

**What to implement:**

1. **Run full workspace checks:**
   - `cargo check --workspace` — no errors
   - `cargo fmt --all` — no formatting changes needed
   - `cargo clippy --workspace -- -D warnings` — no warnings
   - `cargo test --workspace` — all tests pass

2. **Verify CSS has no orphaned selectors:**
   - Grep for `.model-list-card` across all CSS files — should find zero matches
   - Grep for `.update-item__info` across all CSS files — should find zero matches
   - Grep for `.update-item__actions` across all CSS files — should find zero matches

3. **Verify no dead code:**
   - Check that all 4 new modules are properly exported from `components/mod.rs`
   - Check that each component is imported by its consumers

4. **Build release:**
   - `cargo build --release --package tama-web` — succeeds

**Steps:**
- [ ] Run `cargo check --workspace` — no errors
- [ ] Run `cargo fmt --all` — no changes
- [ ] Run `cargo clippy --workspace -- -D warnings` — no warnings
- [ ] Run `cargo test --workspace` — all pass
- [ ] Grep for orphaned `.model-list-card` selectors — zero matches
- [ ] Grep for orphaned `.update-item__info` / `.update-item__actions` — zero matches
- [ ] Verify `components/mod.rs` exports: `list_card`, `section_card`, `alert_banner`, `tab_buttons`
- [ ] Run `cargo build --release --package tama-web` — succeeds
- [ ] Commit with message: "chore: verify shared components refactor — clippy clean, all tests pass"

**Acceptance criteria:**
- [ ] `cargo check --workspace` passes
- [ ] `cargo fmt --all` makes no changes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] No orphaned `.model-list-card*` CSS selectors
- [ ] All 4 new components exported and used
- [ ] `cargo build --release --package tama-web` succeeds
