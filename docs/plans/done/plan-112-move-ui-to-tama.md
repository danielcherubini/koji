# Move Web UI from `/ui` to `/tama`

**Goal:** Consolidate all non-bearer-token endpoints under `/tama` — the web UI main page becomes `/tama`, and `/tama/v1/*` remains the management API.

**Architecture:** The Axum router already mounts `/tama/v1/*` API routes before the web UI catch-all. The web routes are composed into the proxy via `build_unified_router()` in `tama-core/src/proxy/server/router.rs`, where proxy-specific `/tama/v1/*` routes are merged first, guaranteeing priority over the web UI's `/tama/*path` catch-all. Changing the web UI from `/ui` to `/tama` requires only renaming the route paths and adding a 303 redirect from `/ui` → `/tama`. Axum's specificity matching (validated by `test_unified_router_route_priority`) ensures `/tama/v1/*` always matches before `/tama/*path`.

**Tech Stack:** Rust, Axum, Leptos Router

---

### Task 1: Server-side routing — add `/tama` routes and `/ui` redirect

**Context:**
The web UI is currently served at `/ui` and `/ui/*path` in `tama-web/src/router.rs`. We need to move these to `/tama` and `/tama/*path`, and add a 303 redirect from the old `/ui` paths to `/tama` so bookmarks and direct links continue working. The `/tama/v1/*` API routes are already defined in `backend_routes` and `csrf_routes` sub-routers that are `.merge()`d before the web UI routes — so Axum's specificity matching ensures they always take priority over the SPA catch-all.

**Files:**
- Modify: `crates/tama-web/src/router.rs`

**⚠️ Deployability Constraint:** This task must ship together with Task 2. After Task 1 alone, the server serves `/tama` correctly but the client-side Leptos routes still use `/ui` prefix — navigating to `/tama` would show the SPA fallback's "Page not found". The `/ui` → `/tama` redirect ensures old bookmarks work during any gap.

**What to implement:**

1. Add redirect handlers near the top of the file (after `serve_index`). These preserve query strings so bookmarked URLs like `/ui/logs?source=my_model` redirect to `/tama/logs?source=my_model`:

```rust
use axum::extract::Uri;

/// Redirect old /ui/* paths to /tama, preserving query strings.
async fn redirect_to_tama(path: Path<String>, uri: Uri) -> axum::response::Response {
    let query = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();
    let target = if path.is_empty() {
        format!("/tama{}", query)
    } else {
        format!("/tama/{}{}", path, query)
    };
    (
        StatusCode::SEE_OTHER,
        [(axum::http::header::LOCATION, target)],
    )
        .into_response()
}

/// Redirect /ui root to /tama, preserving query strings.
async fn redirect_ui_root(uri: Uri) -> axum::response::Response {
    let query = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();
    (
        StatusCode::SEE_OTHER,
        [(axum::http::header::LOCATION, format!("/tama{}", query))],
    )
        .into_response()
}
```

Also add `use axum::extract::Uri;` to the imports (it's already imported via `use axum::{...}` at the top — add `extract::Uri` to the existing import list if not already present).

2. In `build_web_routes()`, replace the web UI routes at the bottom (currently lines ~314-318):

**Replace:**
```rust
        // Web UI — mounted at /ui
        .route("/ui", get(serve_index))
        .route(
            "/ui/*path",
            get(|Path(p): Path<String>| async move { serve_static(Some(Path(p))).await }),
        )
```

**With:**
```rust
        // Redirect old /ui paths to /tama
        .route("/ui", get(redirect_ui_root))
        .route("/ui/*path", get(redirect_to_tama))
        // Web UI — mounted at /tama (SPA fallback, /tama/v1/* takes priority)
        .route("/tama", get(serve_index))
        .route(
            "/tama/*path",
            get(|Path(p): Path<String>| async move { serve_static(Some(Path(p))).await }),
        )
```

3. These routes must remain at the END of the router chain (after `.merge(csrf_routes)` and `.merge(backend_routes)`) so that `/tama/v1/*` API routes take priority. Do NOT reorder the merges.

**Steps:**
- [ ] Add the `redirect_to_tama` and `redirect_ui_root` handlers in `crates/tama-web/src/router.rs`
- [ ] Replace the `/ui` routes with `/tama` routes + `/ui` redirects
- [ ] Run `cargo build --package tama-web --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web --features ssr -- -D warnings`
  - Did it succeed? If not, fix clippy warnings and re-run.
- [ ] Commit with message: "feat: move web UI routes from /ui to /tama with redirect"

**Acceptance criteria:**
- [ ] `cargo build --package tama-web --features ssr` succeeds with no errors
- [ ] `cargo clippy --package tama-web --features ssr -- -D warnings` passes
- [ ] The `/tama` route serves index.html (same as `/ui` did)
- [ ] The `/tama/*path` route serves static files or SPA fallback
- [ ] The `/ui` route returns 303 with `Location: /tama`
- [ ] The `/ui/something` route returns 303 with `Location: /tama/something`

---

### Task 2: Client-side routing and component links

**Context:**
The Leptos SPA uses `/ui` prefix for all client-side routes and hardcoded links. After the server serves the UI at `/tama`, the client-side router must match so that browser navigation and `<A>` components work correctly. All `/ui` references in the frontend must change to `/tama`.

**⚠️ Deployability Constraint:** Tasks 1 and 2 must ship in the same build. After Task 1 alone, the server serves `/tama` correctly but the client-side Leptos routes still use `/ui` prefix — navigating to `/tama` would show the SPA fallback's "Page not found". The `/ui` → `/tama` redirect ensures old bookmarks work during any gap, but the client-side routes must match for proper SPA navigation.

**Files:**
- Modify: `crates/tama-web/src/lib.rs` (client router, 10 routes)
- Modify: `crates/tama-web/src/components/sidebar.rs` (9 `<A>` links)
- Modify: `crates/tama-web/src/components/model_card.rs` (2 `href` attributes)
- Modify: `crates/tama-web/src/components/pull_wizard/components/done_step.rs` (1 `<a>` link)
- Modify: `crates/tama-web/src/components/pull_wizard/components/download_step.rs` (2 `<a>` links)
- Modify: `crates/tama-web/src/pages/model_editor/mod.rs` (2 `<A>` links)
- Modify: `crates/tama-web/src/pages/updates.rs` (1 `href` attribute)

**What to implement:**

1. In `crates/tama-web/src/lib.rs`, replace all 10 `<Route path=path!("/ui...")>` with `path!("/tama...")`:

```rust
// Replace:
<Route path=path!("/ui") view=pages::dashboard::Dashboard />
<Route path=path!("/ui/models") view=pages::models::Models />
<Route path=path!("/ui/model/:id/edit") view=pages::model_editor::ModelEditor />
<Route path=path!("/ui/backends") view=pages::backends::Backends />
<Route path=path!("/ui/benchmarks") view=pages::benchmarks::Benchmarks />
<Route path=path!("/ui/aliases") view=pages::aliases::AliasesPage />
<Route path=path!("/ui/logs") view=pages::logs::Logs />
<Route path=path!("/ui/config") view=pages::config_editor::ConfigEditor />
<Route path=path!("/ui/updates") view=pages::updates::Updates />
<Route path=path!("/ui/downloads") view=pages::downloads::Downloads />

// With:
<Route path=path!("/tama") view=pages::dashboard::Dashboard />
<Route path=path!("/tama/models") view=pages::models::Models />
<Route path=path!("/tama/model/:id/edit") view=pages::model_editor::ModelEditor />
<Route path=path!("/tama/backends") view=pages::backends::Backends />
<Route path=path!("/tama/benchmarks") view=pages::benchmarks::Benchmarks />
<Route path=path!("/tama/aliases") view=pages::aliases::AliasesPage />
<Route path=path!("/tama/logs") view=pages::logs::Logs />
<Route path=path!("/tama/config") view=pages::config_editor::ConfigEditor />
<Route path=path!("/tama/updates") view=pages::updates::Updates />
<Route path=path!("/tama/downloads") view=pages::downloads::Downloads />
```

2. In `crates/tama-web/src/components/sidebar.rs`, replace all `<A href="/ui` with `<A href="/tama`:
   - `href="/ui"` → `href="/tama"` (sidebar header + Dashboard link)
   - `href="/ui/backends"` → `href="/tama/backends"`
   - `href="/ui/logs"` → `href="/tama/logs"`
   - `href="/ui/updates"` → `href="/tama/updates"`
   - `href="/ui/downloads"` → `href="/tama/downloads"`
   - `href="/ui/benchmarks"` → `href="/tama/benchmarks"`
   - `href="/ui/aliases"` → `href="/tama/aliases"`
   - `href="/ui/config"` → `href="/tama/config"`

3. In `crates/tama-web/src/components/model_card.rs`, replace:
   - `href=format!("/ui/logs?source={}", log_src)` → `href=format!("/tama/logs?source={}", log_src)`
   - `href=format!("/ui/model/{}/edit", edit_id)` → `href=format!("/tama/model/{}/edit", edit_id)`

4. In `crates/tama-web/src/components/pull_wizard/components/done_step.rs`, replace:
   - `href="/ui/models"` → `href="/tama/models"`

5. In `crates/tama-web/src/components/pull_wizard/components/download_step.rs`, replace:
   - `href="/ui/models"` → `href="/tama/models"` (both occurrences)

6. In `crates/tama-web/src/pages/model_editor/mod.rs`, replace:
   - `href="/ui/models"` → `href="/tama/models"` (both "Back to Models" links)

7. In `crates/tama-web/src/pages/updates.rs`, replace:
   - `format!("/ui/model/{}/edit", m.item_id)` → `format!("/tama/model/{}/edit", m.item_id)` (line ~532, "Edit" button link)

8. Verify build tooling does not inject `/ui` into generated HTML assets. After building, check `dist/` output for any hardcoded `/ui` references.

**DO NOT change:** Any `/tama/v1/*` API URLs in components — those are backend API calls and are already correct.

**Steps:**
- [ ] Update the 10 `<Route>` paths in `crates/tama-web/src/lib.rs`
- [ ] Update all `<A href="/ui` links in `crates/tama-web/src/components/sidebar.rs` (9 links)
- [ ] Update `href` attributes in `crates/tama-web/src/components/model_card.rs`
- [ ] Update `<a href="/ui/models">` in `crates/tama-web/src/components/pull_wizard/components/done_step.rs`
- [ ] Update `<a href="/ui/models">` in `crates/tama-web/src/components/pull_wizard/components/download_step.rs`
- [ ] Update `<A href="/ui/models">` in `crates/tama-web/src/pages/model_editor/mod.rs`
- [ ] Update `format!("/ui/model/...")` in `crates/tama-web/src/pages/updates.rs`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
  - Did it succeed? If not, fix clippy warnings and re-run.
- [ ] Verify no remaining `/ui` references exist in the frontend (excluding `/tama/v1/*` API URLs):
  - `grep -rn '"/ui' crates/tama-web/src/ --include="*.rs"` should return no results
- [ ] Commit with message: "feat: update client-side routes and links from /ui to /tama"

**Acceptance criteria:**
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo clippy --package tama-web -- -D warnings` passes
- [ ] All 10 Leptos routes use `/tama` prefix
- [ ] All component links use `/tama` prefix
- [ ] No remaining `/ui` references in frontend code (grep confirms)

---

### Task 3: Update tests and verify workspace

**Context:**
Any tests that reference `/ui` paths need updating. The workspace build and test suite must pass with the new routing.

**Files:**
- Modify: `crates/tama-web/tests/` (any files with `/ui` references)
- Verify: `crates/tama-core/src/proxy/server/router.rs` tests (should be unaffected)

**What to implement:**

**TDD Note:** This is a purely-renaming change — no new behavior is introduced. TDD is waived for Tasks 1–2 per the principle of not writing tests for mechanical renames. Task 3 verifies the existing test suite passes (regression check). If desired, an integration test could be added to `tama-core/src/proxy/server/router.rs` verifying `GET /ui` returns 303 and `GET /tama` returns 200, but this is optional.

1. Search for any `/ui` references in test files:
   ```bash
   grep -rn '"/ui\|'/ui crates/tama-web/tests/ --include="*.rs"
   ```
   Update any found references from `/ui` to `/tama`.

2. Run the full workspace test suite:
   ```bash
   cargo test --workspace
   ```
   All tests must pass. The proxy router tests (`test_proxy_router_serves_known_routes` and `test_unified_router_route_priority`) should be unaffected — they test `/tama/v1/*` route priority, not `/ui`.

3. Run the full workspace check:
   ```bash
   cargo build --workspace
   cargo fmt --all
   cargo clippy --workspace -- -D warnings
   ```

**Steps:**
- [ ] Search for `/ui` references in `crates/tama-web/tests/` and update any found
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "test: update test references from /ui to /tama, verify workspace"

**Acceptance criteria:**
- [ ] `cargo test --workspace` passes with zero failures
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] No remaining `/ui` references in test files

---

## Summary

| Task | File(s) | Scope |
|------|---------|-------|
| 1 | `tama-web/src/router.rs` | Server routes + redirect |
| 2 | `tama-web/src/lib.rs` + 6 component/page files | Client routes + links |
| 3 | `tama-web/tests/` + workspace verification | Tests + final check |

**Risk:** Low. Axum's route specificity ensures `/tama/v1/*` always matches before `/tama/*path`. The existing `test_unified_router_route_priority` test validates this pattern.
