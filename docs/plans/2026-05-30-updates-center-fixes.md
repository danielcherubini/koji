# Updates Center Fixes Plan

**Goal:** Fix 5 issues with the Updates Center page — inconsistent UI layout, missing variant info, stale entries for deleted items, and outdated data after backend updates.

**Architecture:** Frontend CSS/Leptos changes for UI fixes, backend API bug fixes for stale entry cleanup, and post-update refresh logic.

**Tech Stack:** Rust, Leptos (Web UI), CSS, SQLite

---

## Issue Summary

| # | Issue | Root Cause |
|---|-------|------------|
| 1 | Tama "Check for updates" looks different from backend/model entries | `SelfUpdateSection` uses its own `.self-update-section` styling instead of `.update-item` card |
| 2 | "Check Now" button is left-aligned instead of top-right | Uses `<h1 class="page__title">` instead of standard `<div class="page-header">` pattern |
| 3 | llama\_cpp entries don't show which variant (cpu/rocm/vulkan) | API returns `variant` field but frontend only renders `item_id` (name without variant) |
| 4 | Deleted models/backends leave stale entries in updates table | Backend delete uses `name` instead of `name:variant`; Model delete uses `repo_id` instead of integer ID |
| 5 | Updating a backend from the backends page doesn't refresh the updates table | No update check is triggered after backend update completes |

---

### Task 1: Fix page header layout and Tama section consistency

**Context:**
The Updates Center page has two layout problems. First, the "Check Now" button and "Last checked" timestamp are left-aligned below the title, while all other pages (Models, Backends, Downloads) use a `<div class="page-header">` with `justify-content: space-between` to put actions in the top-right. Second, the Tama self-update section uses its own `.self-update-section` styling with a `<h2>` title and inline buttons, which looks visually inconsistent with the `.update-item` cards used for backends and models.

The fix aligns the page header with the standard pattern and restructures `SelfUpdateSection` to render as an `.update-item` card matching the backend/model entries.

**Design decisions (approved):**
- Page header: "Updates Center" left, "Last checked" + "Check Now" right (timestamp before button)
- Tama gets its own section with header "Application" (not "Tama")
- Tama card layout: `[Tama] [v{current} or v{current} → v{latest}] [✓ Up to date] ......... [Update/Refresh]`
- The component keeps its spinner, error, and confirmation dialog behavior — just wrapped in card format

**Files:**
- Modify: `crates/tama-web/src/pages/updates.rs`
- Modify: `crates/tama-web/src/components/self_update_section.rs`
- Modify: `crates/tama-web/css/08-updates.css`

**What to implement:**

1. In `updates.rs`, replace the page header:
   ```rust
   // OLD:
   <div class="page updates-page">
       <h1 class="page__title">"Updates Center"</h1>
       <div class="updates-header">
           <button class="btn btn-primary" ...>"Check Now"</button>
           {last_checked ...}
       </div>
   ```
   ```rust
   // NEW:
   <div class="page updates-page">
       <div class="page-header">
           <h1>"Updates Center"</h1>
           <div class="page-header-actions">
               <span class="last-checked">{last_checked ...}</span>
               <button class="btn btn-primary" ...>"Check Now"</button>
           </div>
       </div>
   ```

2. In `updates.rs`, wrap `SelfUpdateSection` in an "Application" section:
   ```rust
   <section class="updates-section">
       <h2 class="section__title">"Application"</h2>
       <div class="updates-list">
           <SelfUpdateSection />
       </div>
   </section>
   ```

3. In `self_update_section.rs`, restructure the component to render as an `.update-item` card:
   - Remove the `<h2 class="section__title">"Tama"</h2>` header (the section header provides it)
   - Wrap everything in `<div class="update-item">` with `.update-item__info` and `.update-item__actions`
   - Show "Tama" as the item name in `.update-item__name`
   - Show version in `.update-item__version`: `v{current}` or `v{current} → v{latest}` when update available
   - Show "✓ Up to date" badge when current, or nothing extra when update available (the arrow implies it)
   - Action buttons in `.update-item__actions`:
     - Initial state: single "Check for updates" button (btn-primary)
     - Checking state: spinner inline in the info area
     - Error state: error message + "Retry" button
     - Normal state: "Update" button (when available) + "Refresh" button (btn-ghost, always)
   - Keep the confirmation dialog overlay (page-scoped, shown before update starts)
   - Keep SSE streaming for update progress. After SSE completes, `stream_update_events` already sets `update_in_progress = false` and updates `current_version`, so the card will automatically show the correct post-update state ("✓ Up to date" with the new version).

4. In `css/08-updates.css`:
   - Remove `.updates-header` styles (replaced by standard `.page-header`)
   - Remove `.self-update-section` wrapper styles (replaced by `.update-item`)
   - Keep `.self-update-progress`, `.self-update-spinner`, `.self-update-error`, `.update-confirm-overlay`, `.update-confirm-dialog`, `.update-confirm-note`, `.update-confirm-actions` styles (still used internally)

**Steps:**
- [ ] Modify `updates.rs` to use `<div class="page-header">` pattern with right-aligned actions
- [ ] Modify `self_update_section.rs` to render as `.update-item` card — remove the internal `<h2 class="section__title">"Tama"</h2>` and `.self-update-section` wrapper div; the component's root should be the `.update-item` card itself
- [ ] In `updates.rs`, wrap `<SelfUpdateSection />` in a new `<section class="updates-section">` with `<h2 class="section__title">"Application"</h2>` and `<div class="updates-list">` (replaces the bare `<SelfUpdateSection />` call)
- [ ] Clean up `css/08-updates.css` — remove `.updates-header` styles, remove `.self-update-section` wrapper styles; keep internal styles (`.self-update-progress`, `.self-update-spinner`, `.self-update-error`, `.update-confirm-*`). Note: CSS changes require visual verification — `cargo build` only validates Rust changes.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
  - Note: This validates Rust changes (Steps 1-3). CSS cleanup (Step 4) requires visual verification.
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "fix(updates): use standard page-header layout and consistent Tama card style"

**Acceptance criteria:**
- [ ] "Updates Center" title is left-aligned, "Check Now" button is in the top-right corner
- [ ] "Last checked" timestamp appears next to the "Check Now" button in the top-right
- [ ] Tama section renders as a card visually identical to backend/model cards (same border, padding, layout)
- [ ] Tama card shows "Tama" as the item name, version as the version, and appropriate badges/buttons
- [ ] No visual regression on backend or model entries

---

### Task 2: Show backend variant in updates table

**Context:**
The API already returns the `variant` field in `UpdateCheckDto` (parsed from `item_id` format `name:variant` in `get_updates`). However, the frontend only renders `b.item_id` which is the parsed name without the variant. So both `llama_cpp:cpu` and `llama_cpp:rocm` show as just "llama\_cpp" with no way to distinguish them.

The fix displays the variant as a badge or label next to the backend name, similar to how the version is displayed.

**Files:**
- Modify: `crates/tama-web/src/pages/updates.rs`
- Modify: `crates/tama-web/css/08-updates.css`

**What to implement:**

1. **Add the missing `variant` field to the frontend struct.** The API returns `variant: Option<String>` but the frontend's `UpdateCheckDto` in `pages/updates.rs` doesn't have it (serde silently drops unknown fields by default). Add it:
   ```rust
   // In pages/updates.rs, inside UpdateCheckDto:
   #[derive(Debug, Clone, Deserialize, Serialize)]
   pub struct UpdateCheckDto {
       pub item_type: String,
       pub item_id: String,
       #[serde(default)]
       pub variant: Option<String>,     // <-- ADD THIS
       pub repo_id: Option<String>,
       // ... rest unchanged
   }
   ```
   Note: `#[serde(default)]` ensures backward compatibility — if the API ever returns a record without `variant`, it defaults to `None` rather than failing deserialization.

2. In `updates.rs`, in the backend rendering section, add the variant display after the item name:

```rust
// In the backend .map() callback, inside .update-item__info:
<span class="update-item__name">{b.item_id.clone()}</span>
{b.variant.as_ref().map(|v| {
    view! { <span class="update-item__variant">{v}</span> }
})}
<span class="update-item__version">
    {b.current_version.clone().unwrap_or_else(|| "—".to_string())}
</span>
```

Result: `[llama_cpp] [cpu] [b9415] [✓ Up to date] ........................ [Refresh]`

Add CSS for `.update-item__variant` in `css/08-updates.css` — blue pill badge:
```css
.update-item__variant {
  background-color: rgba(96, 165, 250, 0.15);
  color: #60a5fa;
  padding: 0.1rem 0.5rem;
  border-radius: 999px;
  font-size: 0.75rem;
  font-weight: 500;
  font-family: var(--font-mono);
}
```

This will show variants like "cpu", "rocm", "vulkan", "cuda" as blue badges between the name and version, making it clear which variant each entry represents.

**Steps:**
- [ ] Add variant display in backend entries in `updates.rs`
- [ ] Add `.update-item__variant` CSS styles in `css/08-updates.css`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "fix(updates): display backend variant badge in updates table"

**Acceptance criteria:**
- [ ] Each backend entry shows its variant (cpu, rocm, vulkan, etc.) as a blue badge
- [ ] The variant badge appears between the backend name and the version
- [ ] Backends without a variant (legacy or non-GPU) don't show an empty badge
- [ ] The variant badge styling is consistent with other badges on the page

---

### Task 3: Fix stale entries for deleted backends and models

**Context:**
Two bugs prevent stale update check records from being cleaned up:

**Backend bug:** In `install.rs` (line ~662) and `manage.rs` (line ~536), the delete uses just `name` (e.g., `"llama_cpp"`), but the update checker stores records with `item_id = "name:variant"` (e.g., `"llama_cpp:cpu"`). So `delete_update_check(conn, "backend", "llama_cpp")` never matches `"llama_cpp:cpu"`.

**Model bug:** In `delete.rs` (line ~195), the delete uses `repo_id` (e.g., `"unsloth/Qwen3.6-35B-A3B-GGUF"`), but the update checker stores records with `item_id = model_id.to_string()` (e.g., `"42"`). So `delete_update_check(conn, "model", "unsloth/Qwen3.6-35B-A3B-GGUF")` never matches `"42"`.

**Fix strategy:**
- For backends: Delete ALL variants of the backend (use `LIKE 'name:%'` pattern or iterate through variants)
- For models: The delete already has `model_id` available — use `model_id.to_string()` instead of `repo_id`

**Files:**
- Modify: `crates/tama-web/src/api/backends/install.rs`
- Modify: `crates/tama-web/src/api/backends/manage.rs`
- Modify: `crates/tama-web/src/api/models/crud/delete.rs`
- Modify: `crates/tama-core/src/db/queries/update_check_queries.rs`

**What to implement:**

1. In `update_check_queries.rs`, add a new function to delete by pattern. Escape `_` and `%` in the name to avoid SQL LIKE wildcard issues (backend names like `llama_cpp` contain `_` which would match any single character):
```rust
pub fn delete_update_checks_by_pattern(conn: &Connection, item_type: &str, item_id_pattern: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM update_checks WHERE item_type = ?1 AND item_id LIKE ?2 ESCAPE '\\'",
        (item_type, item_id_pattern),
    )?;
    Ok(())
}
```

2. In `install.rs`, replace:
```rust
// OLD (line ~662):
let _ = tama_core::db::queries::delete_update_check(&open.conn, "backend", &name);
```
with:
```rust
// NEW — escape _ and % in name, then delete all variants (e.g., "llama_cpp:cpu", "llama_cpp:rocm")
let escaped_name = name.replace('\\', "\\\\").replace('_', "\\_").replace('%', "\\%");
let pattern = format!("{}:%", escaped_name);
let _ = tama_core::db::queries::delete_update_checks_by_pattern(&open.conn, "backend", &pattern);
// Also delete legacy format (no variant separator)
let _ = tama_core::db::queries::delete_update_check(&open.conn, "backend", &name);
```

2. In `install.rs`, replace:
```rust
// OLD (line ~662):
let _ = tama_core::db::queries::delete_update_check(&open.conn, "backend", &name);
```
with:
```rust
// NEW — delete all variants (e.g., "llama_cpp:cpu", "llama_cpp:rocm")
let _ = tama_core::db::queries::delete_update_checks_by_pattern(&open.conn, "backend", &format!("{}:%", name));
// Also delete legacy format (no variant separator)
let _ = tama_core::db::queries::delete_update_check(&open.conn, "backend", &name);
```

3. In `manage.rs`, apply the same fix as install.rs.

4. In `delete.rs`, replace:
```rust
// OLD (line ~195):
let _ = tx.execute(
    "DELETE FROM update_checks WHERE item_type = ?1 AND item_id = ?2",
    rusqlite::params!["model", &repo_id],
);
```
with:
```rust
// NEW — use model_id (integer) as string, matching what update checker stores
let _ = tx.execute(
    "DELETE FROM update_checks WHERE item_type = ?1 AND item_id = ?2",
    rusqlite::params!["model", model_id.to_string()],
);
```

**Steps:**
- [ ] Add `delete_update_checks_by_pattern` function in `update_check_queries.rs` (with `ESCAPE '\\'` for LIKE pattern)
- [ ] Write a unit test in `update_check_queries.rs` `#[cfg(test)]` module for `delete_update_checks_by_pattern` covering:
  - Deleting all variants of a backend (`name:%` pattern with escaped `_`)
  - Verifying other backends' records are unaffected
  - Edge case: pattern that matches no records (should not error, just 0 rows)
- [ ] Fix backend cleanup in `install.rs` to use escaped pattern match + legacy format
- [ ] Fix backend cleanup in `manage.rs` to use escaped pattern match + legacy format (same pattern as install.rs)
- [ ] Fix model cleanup in `delete.rs` to use `model_id.to_string()` instead of `repo_id`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "fix(updates): clean stale update check records when deleting backends and models"

**Acceptance criteria:**
- [ ] Deleting a backend removes all its variant entries from the updates table (e.g., both `llama_cpp:cpu` and `llama_cpp:rocm`)
- [ ] Deleting a model removes its entry from the updates table (matched by integer ID)
- [ ] No regression in existing delete functionality
- [ ] All existing tests pass

---

### Task 4: Refresh update check after backend update

**Context:**
When a backend is updated from the Backends page (via `POST /tama/v1/backends/{name}/update`), the update completes successfully but the update check record in the DB is not refreshed. This means the Updates Center still shows the old version/status until the user manually clicks "Refresh" or "Check Now."

The endpoint is in `manage.rs` (not `install.rs`). The spawned async task at ~line 240 calls `update_backend_with_progress` and finishes the job — but never triggers an update check. The `apply_backend_update` endpoint in `api/updates.rs` handles this differently (the frontend polls and refreshes), but the backends page's update flow doesn't.

**Fix strategy:** After a backend update completes successfully in the spawned task, call `UpdateChecker::check_backend()` to refresh the DB record. This requires passing `web_update_checker` from `ProxyState` into the spawned task.

**Files:**
- Modify: `crates/tama-web/src/api/backends/manage.rs`

**What to implement:**

In `manage.rs`, in the `update_backend` endpoint's spawned task (~line 240), add:

1. Before `tokio::spawn`, clone the **newly needed** variables. Note: `name_clone`, `gpu_variant_clone`, `jobs_clone`, `job_clone`, `latest_version_clone` are **already cloned** in the existing code — only add these three:
   ```rust
   // Clone variables needed for the post-update check (add BEFORE tokio::spawn)
   let checker = state.web_update_checker.clone();
   let config_dir_clone = config_dir.clone();
   let backend_type_clone = backend_type.clone();
   ```

2. Inside the spawned task, after the `Ok(_)` branch that finishes the job:
   ```rust
   match result {
       Ok(_) => {
           let _ = jobs_clone
               .finish(&job_clone, tama_core::web_types::JobStatus::Succeeded, None)
               .await;
           // Refresh the update check record so the Updates Center reflects the new version
           let _ = checker.check_backend(
               &config_dir_clone,
               &name_clone,
               &backend_type_clone,
               &gpu_variant_clone,
           ).await;
       }
       Err(e) => {
           let _ = jobs_clone
               .finish(&job_clone, tama_core::web_types::JobStatus::Failed, Some(e))
               .await;
       }
   }
   ```

The `check_backend` call is fire-and-forget (result ignored with `let _`) — if it fails, the update still succeeded, the user just won't see the refreshed record until next check.

**Steps:**
- [ ] In `manage.rs`, in the `update_backend` endpoint's spawned task, add the update check after success
- [ ] After successful update completion, call `checker.check_backend()` to refresh the DB record
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "fix(updates): refresh update check record after backend update"

**Acceptance criteria:**
- [ ] After updating a backend from the Backends page, the Updates Center shows the new version without requiring a manual refresh
- [ ] No regression in the update flow from the Updates Center itself
- [ ] No regression in existing update functionality

---

## Verification

After all tasks are complete:

1. **Visual check:** Open the Updates Center page and verify:
   - Title is left-aligned, "Check Now" is top-right
   - Tama card looks identical to backend/model cards
   - Backend entries show variant badges (cpu, rocm, etc.)

2. **Functional check:**
   - Delete a model → verify its entry disappears from the updates table
   - Delete a backend variant → verify its entry disappears
   - Update a backend from the Backends page → verify the Updates Center shows the new version

3. **Build check:**
   ```bash
   cargo check --workspace
   cargo fmt --all
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   ```
