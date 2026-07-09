# API Keys Web UI Plan

**Goal:** Add a web UI page at `/tama/keys` for managing API keys (create, view, edit scopes, revoke).
**Architecture:** New `pages/keys/` module following the aliases page pattern (mod.rs + api.rs + types.rs). The backend API (`tama-core` at `/tama/v1/keys`) is already implemented with full CRUD. This plan only covers the frontend.
**Tech Stack:** Leptos (WASM), gloo-net HTTP, existing shared components (ListCard, Modal, AlertBanner), dedicated CSS file.

---

### Task 1: Types + API Layer

**Context:**
The backend already exposes `/tama/v1/keys` with GET (list), POST (create), PATCH (update scopes), and DELETE (revoke). This task creates the frontend types and API wrapper functions that the page components will call. The backend returns kebab-case scope strings (`"inference"`, `"management-read"`, `"management-write"`) and stores keys as SHA-256 hashes — the frontend only ever sees `key_prefix` (e.g. `tama_aB3dEfGh`) except on create, where the full plaintext key is returned once.

**Files:**
- Create: `crates/tama/src/pages/keys/types.rs`
- Create: `crates/tama/src/pages/keys/api.rs`

**What to implement:**

In `types.rs`, define:

```rust
use serde::{Deserialize, Serialize};

/// API key record returned by GET /tama/v1/keys and PATCH /tama/v1/keys/:id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,       // e.g. "tama_aB3dEfGh"
    pub scopes: Vec<String>,      // e.g. ["inference", "management-read"]
    pub created_by: String,
    pub created_at: String,       // RFC 3339
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub expires_at: Option<String>,
}

/// Response from POST /tama/v1/keys — includes the plaintext key (returned once).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKeyResponse {
    pub id: i64,
    pub name: String,
    pub key: String,              // Plaintext — returned ONCE
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// All available scopes (used for checkbox labels).
pub const AVAILABLE_SCOPES: &[(&str, &str)] = &[
    ("inference", "Allow making inference requests"),
    ("management-read", "Allow reading management endpoints"),
    ("management-write", "Allow writing management endpoints"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_deserialization() {
        let json = r#"{"id":1,"name":"k","key_prefix":"tama_aB3dEfGh","scopes":["inference","management-read"],"created_by":"admin","created_at":"2024-01-01T00:00:00Z","last_used_at":null,"revoked_at":null,"expires_at":null}"#;
        let key: ApiKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.id, 1);
        assert_eq!(key.name, "k");
        assert_eq!(key.scopes, vec!["inference", "management-read"]);
        assert!(key.revoked_at.is_none());
    }

    #[test]
    fn test_create_key_response_deserialization() {
        let json = r#"{"id":2,"name":"new-key","key":"tama_abcdefghijklmnopqrstuvwxyz123456","scopes":["inference"],"expires_at":null,"created_at":"2024-01-01T00:00:00Z"}"#;
        let resp: CreateKeyResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.name, "new-key");
        assert!(resp.key.starts_with("tama_"));
        assert!(resp.expires_at.is_none());
    }
}
```

In `api.rs`, implement:

```rust
use super::types::{ApiKey, CreateKeyResponse};
use crate::utils::{
    delete_request, extract_and_store_csrf_token, get_request, post_request,
};

/// Fetch all API keys from the backend.
pub async fn fetch_keys() -> Result<Vec<ApiKey>, String> {
    let resp = get_request("/tama/v1/keys")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    extract_and_store_csrf_token(&resp);
    resp.json().await.map_err(|e| e.to_string())
}

/// Create a new API key. Returns the response including the plaintext key.
pub async fn create_key(
    name: &str,
    scopes: &[String],
    expires_at: Option<String>,
) -> Result<CreateKeyResponse, String> {
    let body = serde_json::json!({
        "name": name,
        "scopes": scopes,
        "expires_at": expires_at,  // None serializes as null — backend accepts this
    });

    let resp = post_request("/tama/v1/keys")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

/// Update an API key's scopes.
pub async fn update_key_scopes(id: i64, scopes: &[String]) -> Result<ApiKey, String> {
    let body = serde_json::json!({
        "scopes": scopes,
    });

    // gloo-net provides `Request::patch()` — use it directly with CSRF.
    let token = crate::utils::get_csrf_token().unwrap_or_default();
    let resp = gloo_net::http::Request::patch(&format!("/tama/v1/keys/{}", id))
        .header("Content-Type", "application/json")
        .header("X-CSRF-Token", &token)
        .body(serde_json::to_string(&body).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json().await.map_err(|e| e.to_string())
}

/// Revoke an API key (soft delete).
pub async fn revoke_key(id: i64) -> Result<(), String> {
    delete_request(&format!("/tama/v1/keys/{}", id))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

**Steps:**
- [ ] Create `crates/tama/src/pages/keys/types.rs` with the three types, `AVAILABLE_SCOPES`, and `#[cfg(test)]` deserialization tests
- [ ] Run `cargo nextest run --package tama -- test_api_key_deserialization`
  - Did it fail (module not yet wired)? If it passes unexpectedly, stop and investigate.
- [ ] Wire the module by creating `crates/tama/src/pages/keys/mod.rs` with just `mod api; mod types;` and a stub `pub fn KeysPage() -> impl IntoView { view! { <div>"Keys page" </div> } }` — enough to compile
- [ ] Run `cargo nextest run --package tama -- test_api_key_deserialization`
  - Did it pass? If not, fix and re-run.
- [ ] Create `crates/tama/src/pages/keys/api.rs` with the four API functions
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add API keys page types and API layer"

**Acceptance criteria:**
- [ ] `types.rs` compiles with `ApiKey`, `CreateKeyResponse`, and `AVAILABLE_SCOPES`
- [ ] `api.rs` compiles with `fetch_keys`, `create_key`, `update_key_scopes`, `revoke_key`
- [ ] `update_key_scopes` uses `gloo_net::http::Request::patch()` (not `Request::builder()`)
- [ ] Deserialization tests pass for both `ApiKey` and `CreateKeyResponse`

---

### Task 2: Page Component (KeysPage, KeyCard, Modals)

**Context:**
This is the main task — the page UI itself. It follows the aliases page pattern: a page with a header, filter toggle, list of cards, and modals for create/edit. The key difference from aliases is: (a) the one-time key reveal modal after creation, (b) the "Active only" filter, (c) scopes are checkboxes instead of a dropdown, and (d) revoked/expired keys have visual indicators.

**Files:**
- Modify: `crates/tama/src/pages/keys/mod.rs` — replace stub with full implementation

**What to implement:**

The module exports `KeysPage` (the main page component) and internally defines:
- `KeyCard` — individual key card using `ListCard`
- `CreateKeyForm` — form inside a `Modal` for creating keys
- `KeyCreatedModal` — one-time reveal modal showing the plaintext key with copy button
- `EditKeyForm` — form inside a `Modal` for editing scopes

State management (all `RwSignal` on `KeysPage`):
- `keys: RwSignal<Vec<ApiKey>>` — all fetched keys
- `show_revoked: RwSignal<bool>` — filter toggle (default `false`, i.e., "Active only" checked)
- `loading: RwSignal<bool>`
- `error: RwSignal<Option<String>>`
- `save_status: RwSignal<Option<(bool, String)>>` — success/error banner
- `show_create: RwSignal<bool>` — create modal visibility
- `show_key_created: RwSignal<bool>` — key reveal modal visibility
- `new_key: RwSignal<Option<CreateKeyResponse>>` — the just-created key for display

On mount, load keys via `fetch_keys()` (same pattern as aliases page).

Filtering logic (extract as `pub(crate) fn` so it can be unit-tested):
```rust
/// Determine if a key is active (not revoked and not expired).
/// Malformed timestamps are treated as active (don't silently dim a key).
pub(crate) fn is_active(key: &ApiKey) -> bool {
    if key.revoked_at.is_some() {
        return false;
    }
    if let Some(ref ts) = key.expires_at {
        // Malformed timestamp → treat as active (don't silently dim)
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
            if chrono::Utc::now() >= dt {
                return false;
            }
        }
    }
    true
}
```

Unit tests for `is_active()` (in `mod.rs` under `#[cfg(test)]`):
```rust
#[test]
fn test_is_active_no_revocation_no_expiry() {
    let key = ApiKey { id: 1, name: "k".into(), key_prefix: "tama_x".into(), scopes: vec!["inference".into()], created_by: "admin".into(), created_at: "2024-01-01T00:00:00Z".into(), last_used_at: None, revoked_at: None, expires_at: None };
    assert!(is_active(&key));
}

#[test]
fn test_is_active_revoked() {
    let mut key = ApiKey { id: 1, name: "k".into(), key_prefix: "tama_x".into(), scopes: vec!["inference".into()], created_by: "admin".into(), created_at: "2024-01-01T00:00:00Z".into(), last_used_at: None, revoked_at: None, expires_at: None };
    key.revoked_at = Some("2024-06-01T00:00:00Z".into());
    assert!(!is_active(&key));
}

#[test]
fn test_is_active_expired() {
    let mut key = ApiKey { id: 1, name: "k".into(), key_prefix: "tama_x".into(), scopes: vec!["inference".into()], created_by: "admin".into(), created_at: "2024-01-01T00:00:00Z".into(), last_used_at: None, revoked_at: None, expires_at: None };
    key.expires_at = Some("2020-01-01T00:00:00Z".into());
    assert!(!is_active(&key));
}

#[test]
fn test_is_active_future_expiry() {
    let mut key = ApiKey { id: 1, name: "k".into(), key_prefix: "tama_x".into(), scopes: vec!["inference".into()], created_by: "admin".into(), created_at: "2024-01-01T00:00:00Z".into(), last_used_at: None, revoked_at: None, expires_at: None };
    key.expires_at = Some("2099-12-31T23:59:59Z".into());
    assert!(is_active(&key));
}

#[test]
fn test_is_active_malformed_expiry_treats_as_active() {
    let mut key = ApiKey { id: 1, name: "k".into(), key_prefix: "tama_x".into(), scopes: vec!["inference".into()], created_by: "admin".into(), created_at: "2024-01-01T00:00:00Z".into(), last_used_at: None, revoked_at: None, expires_at: None };
    key.expires_at = Some("not-a-date".into());
    assert!(is_active(&key)); // Malformed → treat as active
}
```

`KeysPage` view structure:
```rust
view! {
    <div class="page">
        <div class="page-header">
            <h1>"🔑 API Keys"</h1>
            <button class="btn btn-primary" on:click=move |_| show_create.set(true)>
                "+ New Key"
            </button>
        </div>
        <p class="page-header__subtitle">"Manage API keys for programmatic access."</p>

        // Save status alerts (same pattern as aliases)
        {move || save_status.get().map(|(ok, msg)| {
            let variant = if ok { AlertVariant::Success } else { AlertVariant::Error };
            view! { <AlertBanner variant=variant>{msg}</AlertBanner> }
        })}

        // Loading state
        {move || loading.get().then(|| view! {
            <div class="card card--centered">
                <span class="spinner">"Loading keys..."</span>
            </div>
        }.into_any())}

        // Error state
        {move || error.get().map(|e| view! {
            <AlertBanner variant=AlertVariant::Error>{e}</AlertBanner>
        }.into_any())}

        // Filter toggle
        <div class="keys-filter-row">
            <label>
                <input
                    type="checkbox"
                    prop:checked=move || !show_revoked.get()
                    on:change=move |ev| {
                        use wasm_bindgen::JsCast;
                        if let Some(checked) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                            show_revoked.set(!checked.checked());
                        }
                    }
                />
                "Active only"
            </label>
        </div>

        // Empty state
        {move || {
            let keys = keys.get();
            let filtered: Vec<_> = keys.iter().filter(|k| show_revoked.get() || is_active(k)).collect();
            (!loading.get() && filtered.is_empty()).then(|| view! {
                <div class="card card--centered">
                    <p class="text-muted">"No API keys yet. Click + New Key to create one."</p>
                </div>
            }.into_any())
        }}

        // Key card list
        {move || {
            let keys = keys.get();
            let filtered: Vec<_> = keys.iter().filter(|k| show_revoked.get() || is_active(k)).collect();
            if !filtered.is_empty() {
                view! {
                    <div class="keys-list">
                        {filtered.into_iter().map(|key| view! {
                            <KeyCard
                                key=key.clone()
                                keys=keys
                                save_status=save_status
                            />
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // Create modal
        <Modal
            open=rw_signal_to_signal(show_create)
            on_close=Callback::new(move |_| show_create.set(false))
            title="Create API Key".to_string()
        >
            <CreateKeyForm
                keys=keys
                save_status=save_status
                on_close=Callback::new(move |_| show_create.set(false))
                on_key_created=Callback::new(move |resp| {
                    show_key_created.set(Some(resp));
                })
            />
        </Modal>

        // Key created reveal modal
        <KeyCreatedModal
            show=show_key_created
            keys=keys
            save_status=save_status
        />
    </div>
}
```

`KeyCard` design:
- Uses `ListCard` component (same as aliases)
- **Children** (primary): Key name
- **line2** (secondary): Monospace key prefix + scope badges (small inline `<span class="badge-pill">`) + status badge if revoked (`badge-pill--danger` "Revoked") or expired (`badge-pill--warning` "Expired")
- **Actions**: ✏️ Edit button (opens edit modal), 🗑️ Revoke button (browser `confirm()` then `revoke_key()`)
- For dimming revoked/expired keys, wrap explicitly:
  ```rust
  view! {
      <div class=if is_active(&key) { "" } else { "key-card--dimmed" }>
          <ListCard ...>
              // children, line2, actions
          </ListCard>
      </div>
  }
  ```

`CreateKeyForm` design:
- Name input (required, non-empty validation)
- Three scope checkboxes using `AVAILABLE_SCOPES` (at least one required)
- Expires at: `<input type="datetime-local">` (optional, empty means no expiry)
- **Expiry conversion**: `<input type="datetime-local">` returns `"YYYY-MM-DDTHH:mm"` (no timezone). The backend requires RFC 3339 (`"YYYY-MM-DDTHH:mm:ssZ"`). When serializing a non-empty value, append `":00Z"`: `format!("{}:00Z", raw_value)`. If empty, pass `None` which serializes as `null`.
- On success: close create modal, fire `on_key_created` callback with the `CreateKeyResponse`

`KeyCreatedModal` design:
- Uses `Modal` component with `open` signal derived from `show_key_created.get().is_some()`
- Title: "🔑 Key Created"
- Warning text: "Copy this key now — it will never be shown again!"
- Monospace box with the full plaintext key + Copy button
  - **Copy button**: Use the existing clipboard pattern from `job_log_panel.rs`:
    ```rust
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(navigator) = web_sys::window().map(|w| w.navigator()) {
            let _ = navigator.clipboard().write_text(&key_text).await;
        }
    });
    ```
  - "Copied!" feedback: Use a `RwSignal<bool>` (`copied`, default `false`). On copy, set `copied.set(true)`, then use `gloo_timers::future::TimeoutFuture::new(2_000).await` to reset `copied.set(false)` after 2 seconds. The button text toggles between "Copy" and "Copied!".
- Summary lines: Name, Scopes, Expires (if set)
- "Done" button: closes modal, refreshes key list via `fetch_keys()`

`EditKeyForm` design:
- Name (read-only `<span>`)
- Key prefix (read-only monospace `<span>`)
- Scope checkboxes pre-checked with current scopes (at least one required)
- Metadata: Created by, Created at, Last used (read-only)
- On success: update key in list, close modal, show save status

**Steps:**
- [ ] Write `#[cfg(test)]` unit tests for `is_active()` (5 tests: active, revoked, expired, future expiry, malformed expiry)
- [ ] Run `cargo nextest run --package tama -- is_active`
  - Did it fail (function not yet implemented)? If it passes unexpectedly, stop.
- [ ] Implement `is_active()` to make tests pass
- [ ] Run `cargo nextest run --package tama -- is_active`
  - Did it pass? If not, fix and re-run.
- [ ] Implement `KeysPage` with loading, error, empty states, filter toggle, and card list
- [ ] Implement `KeyCard` using `ListCard` with scope badges and revoke/edit actions (with dimming wrapper div)
- [ ] Implement `CreateKeyForm` with name, scope checkboxes, and datetime-local expiry (with `:00Z` conversion)
- [ ] Implement `KeyCreatedModal` with plaintext key display, copy-to-clipboard (using `web_sys` clipboard from `job_log_panel.rs`), and Done button
- [ ] Implement `EditKeyForm` with read-only name/prefix, editable scope checkboxes, and metadata
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: implement API keys page with create/edit/revoke modals"

**Acceptance criteria:**
- [ ] `KeysPage` loads keys on mount, shows loading spinner, then list or empty state
- [ ] "Active only" filter toggles visibility of revoked/expired keys
- [ ] `is_active()` has 5 unit tests covering all branches
- [ ] Creating a key opens the one-time reveal modal with copy button
- [ ] Copy button copies plaintext key to clipboard and shows "Copied!" feedback
- [ ] Editing scopes via modal updates the key in the list
- [ ] Revoking a key shows confirm dialog, then removes/dims the key in the list
- [ ] Scope validation requires at least one scope selected (in both create and edit)
- [ ] Expiry from `datetime-local` is converted to RFC 3339 (`:00Z` appended) before sending

---

### Task 3: Sidebar, Route, CSS, Module Registration

**Context:**
Wire the new page into the navigation (sidebar entry + route registration) and add page-specific CSS for key cards, scope badges, and the key reveal box. This follows the exact same pattern as the aliases page integration.

**Files:**
- Modify: `crates/tama/src/components/sidebar.rs` — add sidebar entry
- Modify: `crates/tama/src/pages/mod.rs` — register `keys` module
- Modify: `crates/tama/src/lib.rs` — add route
- Modify: `crates/tama/style.css` — add `@import` for the new CSS file
- Create: `crates/tama/css/20-api-keys.css` — page-specific styles

**What to implement:**

In `sidebar.rs`, add a new `<A>` entry between the Aliases item and the footer Config section:

```rust
<A href="/tama/keys" attr:class="sidebar-item" attr:data-tooltip="Keys" on:click=move |_| mobile_open.set(false)>
    <span class="sidebar-item__icon">"🔑"</span>
    <span class="sidebar-item__text">"Keys"</span>
</A>
```

In `pages/mod.rs`, add:
```rust
pub mod keys;
```

In `lib.rs`, add the route (find the existing `/tama/aliases` route and add after it). Use the `path!()` macro (already imported):
```rust
<Route path=path!("/tama/keys") view=pages::keys::KeysPage />
```

In `style.css`, add at the end (after `@import "./css/19-gpu-device-card.css";`):
```css
@import "./css/20-api-keys.css";
```

In `css/20-api-keys.css`, add styles for:
- `.key-card--dimmed` — `opacity: 0.5` for revoked/expired keys
- `.key-card__prefix` — monospace key prefix styling (matching existing code font patterns, e.g. `font-family: var(--font-mono)`)
- `.keys-list` — container for key cards (same pattern as `.aliases-list` in `18-aliases.css`)
- `.keys-filter-row` — the "Active only" checkbox row (margin, font-size consistent with other filter rows)
- `.key-card__reveal-box` — the monospace box in KeyCreatedModal with border, background, padding, and flex layout for the copy button

Reference existing CSS patterns:
- `18-aliases.css` for card-specific styles (`.aliases-list`, `.alias-card__name`)
- `06-badges-list-card.css` for badge-pill base styles
- `05-buttons-forms-progress.css` for form styling

**Steps:**
- [ ] Add sidebar entry in `sidebar.rs` between Aliases and footer Config
- [ ] Add `pub mod keys;` in `pages/mod.rs`
- [ ] Add `<Route path=path!("/tama/keys") view=pages::keys::KeysPage />` in `lib.rs` after the aliases route (use `path!()` macro — every other route uses it)
- [ ] Create `css/20-api-keys.css` with page-specific styles
- [ ] Add `@import "./css/20-api-keys.css";` to `style.css` (after the `19-gpu-device-card.css` import) — Trunk does NOT use globs, it relies on `style.css` `@import` statements
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama` (full build to verify Trunk processes the new CSS import)
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: wire API keys page into sidebar, routes, and CSS"

**Acceptance criteria:**
- [ ] Sidebar shows "🔑 Keys" entry between Aliases and Config
- [ ] Navigating to `/tama/keys` renders the KeysPage
- [ ] `style.css` includes `@import "./css/20-api-keys.css";` (verify in the file, not Trunk.toml)
- [ ] Dimmed keys have reduced opacity
- [ ] Scope badges are styled as small inline pills
- [ ] Key reveal box in modal has monospace font, border, and copy button

---

### Task 4: Verification

**Context:**
Final verification that the full feature works end-to-end. This task runs the full build, checks formatting and linting, and verifies the page renders correctly.

**Files:**
- No new files — verification only

**Steps:**
- [ ] Run `cargo fmt --all` — did it succeed?
- [ ] Run `cargo clippy --package tama -- -D warnings` — did it succeed? If not, fix warnings.
- [ ] Run `cargo build --package tama` — did it succeed?
- [ ] Run `cargo nextest run --package tama` — did tests pass? (includes the new deserialization + is_active tests)
- [ ] Run `cargo nextest run --package tama-core` — ensure no regressions in core API key tests
- [ ] Commit with message: "ci: verify API keys web UI builds and tests pass"

**Acceptance criteria:**
- [ ] `cargo fmt` passes with no changes
- [ ] `cargo clippy` passes with no warnings
- [ ] `cargo build --package tama` succeeds
- [ ] All existing tests pass (no regressions)
- [ ] New tests pass: `test_api_key_deserialization`, `test_create_key_response_deserialization`, `test_is_active_*`
