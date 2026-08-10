# Centralized 401 Auth Handling Plan

**Goal:** Detect OIDC session expiry on the frontend and automatically redirect to `/login` instead of silently failing.

**Architecture:** Add a `handle_response()` utility function in `utils/mod.rs` that checks for 401 status and redirects. Every `gloo_net` fetch call site calls this after `.send().await`. SSE `onerror` handlers do a lightweight health check to detect auth failures.

**Tech Stack:** Rust, WASM, Leptos, `gloo_net::http::Response`, `web_sys::EventSource`

---

### Task 1: Add `handle_response` and `check_session_expired` to `utils/mod.rs`

**Context:**
The frontend currently has no centralized way to detect 401 responses. This task adds two utility functions: `handle_response()` that checks for 401 and redirects to `/login`, and `check_session_expired()` that does a lightweight health check to determine if the session has expired.

`handle_response()` also absorbs the existing `extract_and_store_csrf_token()` call — on non-401 responses it extracts and stores the CSRF token. This eliminates the need for separate CSRF extraction calls at every site.

**Convention:** `check_session_expired()` returns `true` when the session is **expired** (redirect was triggered). This is the opposite of "alive" — the name matches the return value to avoid inversion bugs.

**Files:**
- Modify: `crates/tama/src/utils/mod.rs`

**What to implement:**

Add two new public functions to `crates/tama/src/utils/mod.rs`:

```rust
/// Check response for auth failure and extract CSRF token on success.
/// Redirects to `/login` if status is 401.
/// On non-401 responses, extracts and stores the CSRF token from headers.
///
/// Returns `true` if a redirect was triggered (caller should short-circuit).
/// Returns `false` if the response is valid (caller should continue).
pub fn handle_response(resp: &Response) -> bool {
    if resp.status() == 401 {
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_href("/login");
        }
        return true;
    }
    extract_and_store_csrf_token(resp);
    false
}

/// Lightweight check: fetch a small endpoint to determine if the session has expired.
/// Returns `true` if the session appears expired (redirect was triggered by `handle_response`).
/// Returns `false` if the session is still valid or the request failed for non-auth reasons.
pub async fn check_session_expired() -> bool {
    match get_request("/tama/v1/system/health").send().await {
        Ok(resp) => handle_response(&resp), // true = 401 = expired
        Err(_) => false, // network error, not auth
    }
}
```

The `handle_response` function uses `web_sys::window().location().set_href("/login")` to redirect. This already works in the codebase (e.g., `pages/logs.rs:39` uses `window.location().href()`).

**Steps:**
- [ ] Add `handle_response(resp: &Response) -> bool` function to `crates/tama/src/utils/mod.rs`
- [ ] Add `check_session_expired() -> impl Future<Output = bool>` async function
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
  - Did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add handle_response and check_session_expired utilities"

**Acceptance criteria:**
- [ ] `handle_response` exists and is `pub` in `utils/mod.rs`
- [ ] `handle_response` redirects to `/login` on 401 and returns `true`
- [ ] `handle_response` calls `extract_and_store_csrf_token` on non-401 and returns `false`
- [ ] `check_session_expired` fetches `/tama/v1/system/health` and returns `true` when expired (401)
- [ ] Code compiles and passes clippy

---

### Task 2: Update all `gloo_net` call sites to use `handle_response`

**Context:**
Every place in the frontend that makes an API call via `gloo_net::http::Request` (using the helper functions `get_request()`, `post_request()`, etc.) needs to call `handle_response(&resp)` after a successful `.send().await`. This replaces the existing `extract_and_store_csrf_token(&resp)` calls and adds 401 detection to call sites that previously had no check.

**Short-circuit recipes by code shape:**

1. **`Result<T, String>` return:** Replace `extract_and_store_csrf_token(&resp)` with:
   ```rust
   if handle_response(&resp) { return Err("unauthorized".into()); }
   ```

2. **`Option<T>` return:** Same as above but `return None`.

3. **`spawn_local` closure (returns `()`):** `if handle_response(&resp) { return; }`.

4. **Fire-and-forget `let _ = ...send().await`:** Must be restructured to check:
   ```rust
   // Before: let _ = post_request(url).send().await;
   // After:
   if let Ok(resp) = post_request(url).send().await {
       let _ = handle_response(&resp);
   }
   ```

5. **`match resp` / `Ok(resp) if resp.ok()` guards:** Insert `handle_response` BEFORE the guard arm, so 401 is caught before falling through:
   ```rust
   // Before:
   match get_request(url).send().await {
       Ok(resp) if resp.ok() => { /* ... */ }
       Ok(resp) => { /* error path */ }
       Err(e) => { /* ... */ }
   }
   // After — convert guarded arm to unguarded, check first:
   match get_request(url).send().await {
       Ok(resp) => {
           if handle_response(&resp) { return; }
           if resp.ok() { /* ... */ }
           else { /* error path */ }
       }
       Err(e) => { /* ... */ }
   }
   ```
   This applies to all `Ok(r) if r.status() == 200`, `Ok(resp) if resp.ok()`, and `Ok(r) if (200..300).contains(&r.status())` patterns.

6. **Inverted guards (`Ok(resp) if !resp.ok()`):** The error-first arm. Add `handle_response` as the first line inside the guarded arm (the guard fires for 401, so the check is reachable):
   ```rust
   match post_request(url).send().await {
       Ok(resp) if !resp.ok() => {
           if handle_response(&resp) { return; }
           // existing error handling...
       }
       Ok(resp) => { /* success path */ }
       Err(e) => { /* ... */ }
   }
   ```

7. **`expect_status` helper (model_editor/api.rs):** Call `handle_response` BEFORE `expect_status`, because `expect_status` converts non-2xx to opaque error strings. Note: `expect_status` takes `resp` **by value** and is async:
   ```rust
   // Before:
   expect_status(resp, &[200, 201]).await?;
   // After:
   if handle_response(&resp) { return Err("unauthorized".into()); }
   expect_status(resp, &[200, 201]).await?;
   ```

**Import changes:** In each file, swap `extract_and_store_csrf_token` for `handle_response` in the existing `use crate::utils::{...}` list. Do NOT replace the entire `use` statement — just swap the item within the braces.

**Files and call sites (18 files, ~78 call sites total):**

**`pages/aliases/api.rs`** — 5 call sites, all `Result<T, String>`:
- `fetch_aliases()`: Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check
- `fetch_models()`: Same pattern
- `create_alias()`: Add `handle_response` check after `.send().await`
- `update_alias()`: Same as create_alias
- `delete_alias()`: Add check after `.send().await`

**`pages/keys/api.rs`** — 4 call sites, all `Result<T, String>`:
- `fetch_keys()`: Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check
- `create_key()`: Add `handle_response` check after `.send().await`
- `update_key_scopes()`: Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check
- `revoke_key()`: Add `handle_response` check after `.send().await` (before the existing manual 204 status check)

**`pages/model_editor/api.rs`** — 13 call sites:
- `fetch_model()` (line 25, `Option`): Replace `extract_and_store_csrf_token(&resp)` with `handle_response`. Return `None` on redirect.
- `save_model()` (line 174, `Result`): Add `handle_response` before `expect_status`.
- `rename_model()` (line 188, `Result`): Add `handle_response` before `expect_status`.
- `delete_model_api()` (line 199, `Result`): Add `handle_response` before `expect_status`.
- `delete_quant_api()` (line 213, `Result`): Add `handle_response` before `expect_status`.
- `refresh_model_api()` (line 225, `Result`): Add `handle_response` before `expect_status`.
- `verify_model_api()` (line 237, `Result`): Add `handle_response` before `expect_status`.
- `fetch_sampling_templates()` (line 248, `Option`): Replace `extract_and_store_csrf_token(&resp)` with `handle_response`.
- `fetch_gpu_devices()` (line 265, returns `Vec<GpuDeviceInfo>`): The `Ok(resp)` arm has a guarded match `Ok(r) if r.status() == 200`. Convert to unguarded: `Ok(r) => { if handle_response(&r) { return Vec::new(); } if r.status() == 200 { ... } else { ... } }`.
- `refresh_gpu_devices()` (line 285, returns `Vec<GpuDeviceInfo>`): Same guarded-arm restructure as `fetch_gpu_devices`.
- `save_sampling_template()` (lines 301, 342, `Result`, **two** `.send()` calls — GET + POST): Add `handle_response` to both.
- `fetch_model()` status-match arm (line 71): The `Ok(r) if r.status() == 200` arm — a 401 falls through to `_ => None`. Convert to unguarded: `Ok(r) => { if handle_response(&r) { return None; } if r.status() == 200 { ... } else { ... } }`.
- `fetch_model()` extract call (line 72): Inside the status-200 arm, remove `extract_and_store_csrf_token(&r)` (already handled by the outer `handle_response`).

**`pages/backends.rs`** — 12 call sites:
- All use `spawn_local` with `RwSignal` state. After each `Ok(resp)`, add `if handle_response(&resp) { return; }`.
- Replace any `extract_and_store_csrf_token(&resp)` with `handle_response(&resp)`.
- Sites include: initial backend fetch, install modal fetch, system capabilities fetch, and view closure fetches.

**`pages/downloads.rs`** — 4 call sites:
- Initial fetch in `Downloads` component: Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check.
- History load (`load_history`): Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check.
- `cancel_download()`: Add `handle_response` check on the POST response. On the GET refresh (nested `if let Ok(resp2)`), add `handle_response` check.

**`pages/config_editor/mod.rs`** — 2 call sites:
- Config load (line 71): Add `handle_response` check after `.send().await`.
- Config save (line 113): Add `handle_response` check in the `Ok(resp)` arm of the match.

**`pages/benchmarks/utils.rs`** — 3 call sites:
- `use_benchmark_form_state()` (line 101): Add `handle_response` check after `.send().await`.
- `fetch_installed_backend_variants()` (line 166): Add `handle_response` check.
- `submit_bench_job()` (line 248): Add `handle_response` check before the existing manual status check.

**`pages/updates.rs`** — 9 call sites:
- Mix of `match` statements (5 sites: lines 316, 395, 439, 514, 558) and `if let Ok(resp)` patterns (4 sites).
- Sites at lines 316, 395, 558 use `Ok(resp) if resp.ok()` — convert to unguarded per recipe #5.
- Sites at lines 439, 514 use `Ok(resp) if !resp.ok()` (inverted guard) — add `handle_response` as first line inside the guarded arm per recipe #6.
- For `if let` sites: add `handle_response` inside the `Ok(resp)` block.
- Includes nested `if let Ok(resp2)` at line 401 and fetch inside SSE fallback at line 569.
- Replace `extract_and_store_csrf_token` calls with `handle_response`.

**`pages/models.rs`** — 6 call sites:
- LocalResource closure (1): Add `handle_response` check, return `None` on redirect.
- `load_action`, `unload_action`, `cancel_action` (3): Fire-and-forget `let _ = ...`. Restructure to `if let Ok(resp) = ... { let _ = handle_response(&resp); }`.
- Check-all flow: list GET + per-model refresh POSTs. Add `handle_response` to the list GET and each POST.

**`components/sidebar.rs`** — 1 call site:
- Update badge `Effect::new` fetch: Add `handle_response` check after `.send().await`.

**`pages/dashboard/mod.rs`** — 4 call sites:
- `restart` action: Fire-and-forget `let _ = ...`. Restructure to check.
- `load_action`, `unload_action`, `cancel_action`: Same fire-and-forget restructure.
- Metrics SSE `on_error` handler: See Task 3 (SSE section).

**`lib.rs`** — 2 call sites (SSE progress refresh fetches only):
- "Queued" event handler fetches `/tama/v1/pulls/active`: Add `handle_response` check.
- Terminal event handlers fetch `/tama/v1/pulls/history`: Add `handle_response` check.

**`components/pull_quant_wizard.rs`** — 6 call sites:
- Initial quant fetch in Reset Effect (line 155, `get_request`): Add `handle_response` check.
- Repo search: quants + metadata are sent as futures at lines 243–244 and joined via `futures_util::join!` at line 246. After the join, add `handle_response` checks on both `quants_resp` and `metadata_resp`.
- Repo search: metadata guarded arm at line 251 (`Ok(r) if (200..300).contains(&r.status())`) — convert to unguarded per recipe #5.
- Repo search: stub model creation (line 275, `post_request`): The `Ok(r) if (200..300).contains(&r.status())` guard — convert to unguarded, add `handle_response` first.
- Pull request submission (line 377, `post_request`): Add `handle_response` check.
- Context step: settings save (line 454, `put_request`): Add `handle_response` check.

**`components/self_update_section.rs`** — 2 call sites:
- Check for updates (`get_request`, line ~45): Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check.
- Start update (`post_request`, line ~85): Add `handle_response` check.

**`components/docker_register_modal.rs`** — 1 call site:
- Register backend (`post_request`, line ~75): Add `handle_response` check.

**`pages/benchmarks/mod.rs`** — 1 call site:
- Benchmark history fetch (line 430): Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check.

**`pages/logs.rs`** — 1 call site:
- Logs fetch (line 71): Replace `extract_and_store_csrf_token(&resp)` with `handle_response` check.

**`utils/self_update.rs`** — 1 call site (line 137):
- Post-restart polling loop. This is a special case: the server may be mid-restart when this fires, so a 401 here could be transient. **Add `handle_response` check but do NOT redirect** — just log a warning and continue polling. The redirect will happen on the next normal API call.

**Steps:**
- [ ] Update `pages/aliases/api.rs` — 5 call sites
- [ ] Update `pages/keys/api.rs` — 4 call sites
- [ ] Update `pages/model_editor/api.rs` — 13 call sites
- [ ] Update `pages/backends.rs` — 12 call sites
- [ ] Update `pages/downloads.rs` — 4 call sites
- [ ] Update `pages/config_editor/mod.rs` — 2 call sites
- [ ] Update `pages/benchmarks/utils.rs` — 3 call sites
- [ ] Update `pages/updates.rs` — 9 call sites
- [ ] Update `pages/models.rs` — 6 call sites
- [ ] Update `components/sidebar.rs` — 1 call site
- [ ] Update `pages/dashboard/mod.rs` — 4 call sites (fetch only, SSE in Task 3)
- [ ] Update `lib.rs` — 2 call sites (SSE progress refresh fetches)
- [ ] Update `components/pull_quant_wizard.rs` — 6 call sites
- [ ] Update `components/self_update_section.rs` — 2 call sites
- [ ] Update `components/docker_register_modal.rs` — 1 call site
- [ ] Update `pages/benchmarks/mod.rs` — 1 call site
- [ ] Update `pages/logs.rs` — 1 call site
- [ ] Update `utils/self_update.rs` — 1 call site (warning only, no redirect)
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add handle_response check to all gloo_net call sites"

**Acceptance criteria:**
- [ ] Every `.send().await` that produces a `gloo_net::http::Response` calls `handle_response(&resp)`
- [ ] No remaining `extract_and_store_csrf_token` call sites in any file EXCEPT `utils/mod.rs` (where it's intentionally called by `handle_response`)
- [ ] Code compiles and passes clippy

---

### Task 3: Update SSE error handlers to detect auth failures

**Context:**
`EventSource` doesn't expose HTTP status codes — a 401 just fires `onerror` and closes the connection, indistinguishable from a network error. This task adds a lightweight auth check (`check_session_expired()`) in each SSE error handler to determine if the connection failure was due to session expiry.

**Important:** `gloo_net`'s `EventSource::new()` only fails on invalid URLs — it never fails for HTTP errors. A 401 surfaces as an `error` event on the already-open connection. Therefore, the interception point is the `onerror` handler on raw `web_sys::EventSource` instances.

For `SseConnection` (the wrapper in `sse_stream.rs`): gloo-net 0.6's `EventSource` type has no `set_onerror` method and keeps its inner `web_sys::EventSource` private. We cannot attach an error callback at the wrapper level. Instead, consumers of `SseConnection` should call `check_session_expired()` in their existing reconnect/error handling loops.

**Files:**
- Modify: `crates/tama/src/lib.rs` (pull events SSE)
- Modify: `crates/tama/src/pages/dashboard/mod.rs` (metrics stream SSE)
- Modify: `crates/tama/src/components/pull_quant_wizard.rs` (pull wizard SSE)

**Out of scope:** `pages/updates.rs` and `components/job_log_panel.rs` use `SseConnection` (gloo-net wrapper with no `set_onerror` access). Their fetch calls are covered by Task 2. The SSE stream termination on 401 is handled gracefully by existing code (the stream ends and the UI shows stale data until the next fetch, which Task 2's `handle_response` will redirect).

**What to implement:**

**`lib.rs` — Pull events SSE:**
Currently there is **no `onerror` handler** on the EventSource. The code only handles synchronous `EventSource::new()` failure. Add a new `set_onerror` handler:

```rust
// After creating the EventSource (both the initial one and the one in the retry loop):
let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
    sse_connected.set(false);
    wasm_bindgen_futures::spawn_local(async move {
        if crate::utils::check_session_expired().await {
            // Session expired — handle_response already redirected to /login
        }
        // Otherwise it was a network error; the retry loop (if any) handles it
    });
});
es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
on_error.forget();
```

Attach this handler to **both** the initial `es` (after the `match es_result` binding, ~line 87) and the `new_es` created inside the retry loop (inside the `Ok(new_es)` arm, ~line 105, alongside the existing listener attachment). The retry loop itself should remain as-is (it handles network errors with exponential backoff).

**`pages/dashboard/mod.rs` — Metrics stream SSE:**
The `on_error` closure currently does `fetch_failed.set(true)`. Augment it:

```rust
let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
    fetch_failed.set(true);
    wasm_bindgen_futures::spawn_local(async move {
        if crate::utils::check_session_expired().await {
            // Session expired — handle_response already redirected to /login
        }
    });
});
```

**`components/pull_quant_wizard.rs` — Pull wizard SSE:**
The `spawn_pull_events_listener` function (line 521) creates a raw `web_sys::EventSource` on `/tama/v1/pulls/events` (line 531) with no `onerror` handler. Add one after the EventSource is created:

```rust
let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
    wasm_bindgen_futures::spawn_local(async move {
        if crate::utils::check_session_expired().await {
            // Session expired — handle_response already redirected to /login
        }
    });
});
es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
on_error.forget();
```

**Steps:**
- [ ] Add `set_onerror` handler to `lib.rs` pull events EventSource (both initial and retry-loop instances)
- [ ] Augment `on_error` handler in `pages/dashboard/mod.rs` metrics SSE
- [ ] Add `set_onerror` handler to `components/pull_quant_wizard.rs` `spawn_pull_events_listener`
- [ ] Run `cargo check --package tama`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add auth detection to SSE error handlers"

**Acceptance criteria:**
- [ ] `lib.rs` pull events EventSource has `onerror` handler that calls `check_session_expired()`
- [ ] Both initial and retry-loop EventSource instances in `lib.rs` have the handler
- [ ] `dashboard/mod.rs` SSE `onerror` calls `check_session_expired()` on error
- [ ] `pull_quant_wizard.rs` SSE has `onerror` handler that calls `check_session_expired()`
- [ ] Code compiles and passes clippy

---

### Task 4: Run full validation gate

**Context:**
After all changes are committed, run the full CI validation gate to ensure nothing is broken.

**Steps:**
- [ ] Run `cargo fmt --all --check`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
- [ ] If any failures, fix and re-run

**Acceptance criteria:**
- [ ] All format checks pass
- [ ] All clippy checks pass (both workspace and SSR target)
- [ ] All tests pass
