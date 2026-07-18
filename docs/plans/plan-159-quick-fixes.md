# Quick Fixes Plan

**Goal:** Land five independent small fixes from the 2026-07-18 audit: break the config↔proxy module cycle (F3), stop the restore endpoint from returning false success (F6 stopgap), move the benchmark-history DELETE behind CSRF middleware + test the middleware (F21), route the orphan `handle_logout` (F25), and make the docs stop claiming SvelteKit (F35).

**Architecture:** All changes are surgical and local: one function move inside `tama-core`, one handler behavior change in `crates/tama/src/api/backup.rs`, one route move + one new test module in `crates/tama`, one route registration + test in `tama-core`, and doc edits. Tasks are fully independent — they can be committed in any order.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio, tower (tests)

---

### Task 1: Move `count_active_keys` into `db::queries` (break config↔proxy cycle)

**Context:**
`Config::from_db()` and `Config::to_db()` in `crates/tama-core/src/config/types/mod.rs` (lines 108 and 272) call `crate::proxy::api_keys::count_active_keys(&conn)` to derive `api_keys_enabled`, while `ProxyState` stores `Arc<RwLock<Config>>` — a true module-level import cycle (config → proxy::api_keys → config). `count_active_keys` is a pure SQL `COUNT(*)` over the `api_keys` table with no proxy dependencies, so it belongs in `db::queries`. Decision: move the function (do NOT keep a re-export in `proxy::api_keys` — only two call sites exist, both in `config/types/mod.rs`, so update them directly). The rest of `proxy/api_keys.rs` (key generation, hashing, CRUD) stays untouched.

**Files:**
- Create: `crates/tama-core/src/db/queries/api_key_queries.rs`
- Modify: `crates/tama-core/src/db/queries/mod.rs`
- Modify: `crates/tama-core/src/db/queries/tests.rs`
- Modify: `crates/tama-core/src/proxy/api_keys.rs`
- Modify: `crates/tama-core/src/config/types/mod.rs`

**What to implement:**

1. **`api_key_queries.rs`** — new file containing ONLY this function, moved verbatim from `crates/tama-core/src/proxy/api_keys.rs:180-192` (doc comment included):
   ```rust
   //! Queries for the `api_keys` table.

   use anyhow::Result;
   use rusqlite::Connection;

   /// Count the number of active (non-revoked, non-expired) API keys.
   ///
   /// Used to derive the `api_keys_enabled` flag on the `app_proxy` table so
   /// the flag can never drift from the actual key state. The flag is a
   /// derived value — the source of truth is the `api_keys` table.
   pub fn count_active_keys(conn: &Connection) -> Result<i64> {
       let count: i64 = conn.query_row(
           "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
           [],
           |row| row.get(0),
       )?;
       Ok(count)
   }
   ```

2. **`db/queries/mod.rs`** — add `mod api_key_queries;` to the private module list (alphabetical, before `mod active_model_queries;` — note: `api_key_queries` sorts before `active_model_queries`) and `pub use api_key_queries::*;` to the re-export list in the same position.

3. **`proxy/api_keys.rs`** — delete the `count_active_keys` function (lines 180–192 including its doc comment). Do NOT add a re-export. Leave everything else unchanged.

4. **`config/types/mod.rs`** — change line 108 (in `Config::from_db`) and line 272 (in `Config::to_db`) from `crate::proxy::api_keys::count_active_keys(&conn)?` to `crate::db::queries::count_active_keys(&conn)?`. Nothing else changes in this file.

5. **`db/queries/tests.rs`** — add a test using the existing `crate::db::open_in_memory()` helper (pattern already used at the top of this file). Insert rows with raw SQL (do NOT call `proxy::api_keys::create_key` — the db layer must not depend on the proxy layer):
   ```rust
   #[test]
   fn test_count_active_keys() {
       let OpenResult { conn, .. } = open_in_memory().unwrap();
       assert_eq!(count_active_keys(&conn).unwrap(), 0);

       // One active key
       conn.execute(
           "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, created_at, expires_at) \
            VALUES ('a', 'tama_aaa', 'h1', '[\"inference\"]', 'test', '2026-01-01T00:00:00Z', NULL)",
           [],
       )
       .unwrap();
       assert_eq!(count_active_keys(&conn).unwrap(), 1);

       // One revoked key — must NOT be counted
       conn.execute(
           "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, created_at, revoked_at) \
            VALUES ('b', 'tama_bbb', 'h2', '[\"inference\"]', 'test', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z')",
           [],
       )
       .unwrap();
       assert_eq!(count_active_keys(&conn).unwrap(), 1);

       // One expired key — must NOT be counted
       conn.execute(
           "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, created_at, expires_at) \
            VALUES ('c', 'tama_ccc', 'h3', '[\"inference\"]', 'test', '2026-01-01T00:00:00Z', '2020-01-01T00:00:00Z')",
           [],
       )
       .unwrap();
       assert_eq!(count_active_keys(&conn).unwrap(), 1);
   }
   ```
   (Verify the `api_keys` column set against `crates/tama-core/src/db/migrations/` before writing the INSERTs — adjust the column list if the migration defines more `NOT NULL` columns.)

**Steps:**
- [ ] Write the failing test `test_count_active_keys` in `crates/tama-core/src/db/queries/tests.rs` (it fails to compile until the function exists — that is the expected failure)
- [ ] Run `cargo nextest run --package tama-core -- db::queries` — verify the new test fails (unresolved import)
- [ ] Create `crates/tama-core/src/db/queries/api_key_queries.rs`, wire `mod` + `pub use` in `crates/tama-core/src/db/queries/mod.rs`
- [ ] Delete `count_active_keys` from `crates/tama-core/src/proxy/api_keys.rs` and update the two call sites in `crates/tama-core/src/config/types/mod.rs` (lines 108, 272)
- [ ] Run `cargo nextest run --package tama-core -- db::queries` — new test passes
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (catches any other `count_active_keys` caller)
- [ ] Run `rg "proxy::api_keys::count_active_keys" crates/` — zero hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: move count_active_keys to db::queries to break config<->proxy cycle"

**Acceptance criteria:**
- [ ] `crates/tama-core/src/config/types/mod.rs` contains no reference to `crate::proxy::api_keys` (verify with `rg "proxy::api_keys" crates/tama-core/src/config/`)
- [ ] `count_active_keys` is defined exactly once, in `crates/tama-core/src/db/queries/api_key_queries.rs`
- [ ] `cargo nextest run --package tama-core` passes including the new `test_count_active_keys`
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 2: Make `start_restore` return 501 instead of false success (F6 stopgap)

**Context:**
`POST /tama/v1/restore` in `crates/tama/src/api/backup.rs` (`start_restore`, line 227) submits a job and spawns a task whose body is `// TODO: Implement actual restore logic` + `let _ = (config_dir, temp_dir, job);` — it silently discards the uploaded backup, deletes the uploaded file from `<config_dir>/uploads/`, and returns a success JSON with a `job_id`. That is a dishonest API. Decision (already made): as a stopgap the handler must return `501 Not Implemented` with the canonical nested error shape via the existing `error_response` helper (`crates/tama/src/api/error.rs:28`), must NOT delete the uploaded file, and must NOT submit a job. The full restore implementation (wiring `tama_core::backup::extract_backup` + `merge`) is plan-163 — do NOT implement it here. Keep the request parsing (`RestoreRequest`, upload lookup in `WebState.upload_lock`) so the 501 fires AFTER the upload-id is validated (a bad `upload_id` still gets 404).

**Files:**
- Modify: `crates/tama/src/api/backup.rs`

**What to implement:**

1. In `start_restore` (line 227), keep everything up to and including the `upload_path` lookup (lines ~229–247: JSON body extraction and the `uploads.get(&body.upload_id)` match that returns 404 for unknown uploads). Then delete everything from the `jobs` lookup through the end of the function (the `let jobs = ...` block, the `jobs.submit(...)` call, and the whole `match job { ... }` including the `tokio::spawn` with the TODO and the `std::fs::remove_file` cleanup) and replace it with:
   ```rust
   let _ = upload_path; // validated above; restore implementation is plan-163
   error_response(
       StatusCode::NOT_IMPLEMENTED,
       "Backup restore is not yet implemented. The uploaded archive was kept on disk.",
       Some("NotImplementedError"),
   )
   ```
2. After the edit, remove now-unused imports if clippy flags them (likely candidates: `crate::web_types::JobKind` usage, `uuid::Uuid` if it was only used in the deleted block — check before deleting; `Uuid` is also used by `restore_preview`'s upload-id generation, so it probably stays). Do NOT touch `restore_preview`, `create_backup`, the DTO structs, or the existing `#[cfg(test)] mod tests` for the DTOs.
3. Add a route-level test at the bottom of the existing `#[cfg(test)] mod tests` in `crates/tama/src/api/backup.rs`. Follow the `tower::ServiceExt::oneshot` pattern from `crates/tama/src/api/backends/manage/tests.rs` (build `WebState` via a local helper, `Router::new().route("/tama/v1/restore", post(start_restore))`, `.layer(Extension(web_state))`, `.with_state(Arc<ProxyState>)`):
   - Seed `web_state.upload_lock` with one entry: key `"up-1"`, value `crate::api::backup::UploadEntry { path: <tempdir path joined with "up-1.tar.gz">, .. }` (check the `UploadEntry` field list in `crates/tama/src/web_types.rs` before constructing; write the file into a `tempfile::tempdir()` so it exists on disk).
   - POST `/tama/v1/restore` with JSON body `{"upload_id": "up-1"}` → assert status `501 NOT_IMPLEMENTED`; deserialize the body and assert it has the nested shape `{"error": {"message": ..., "type": "NotImplementedError"}}` (assert `body["error"]["type"] == "NotImplementedError"`).
   - Assert the uploaded file still exists on disk after the request (`std::path::Path::exists`).
   - POST `/tama/v1/restore` with `{"upload_id": "unknown"}` → assert status `404 NOT_FOUND` (unchanged behavior).

**Steps:**
- [ ] Write the failing test `test_start_restore_returns_501_and_keeps_upload` in `crates/tama/src/api/backup.rs`
- [ ] Run `cargo nextest run --package tama -- api::backup` — verify the new test fails (currently returns 200)
- [ ] Implement the 501 change in `start_restore` per above
- [ ] Run `cargo nextest run --package tama -- api::backup` — all pass
- [ ] Run `cargo nextest run --package tama` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean (fix unused-import fallout only)
- [ ] Commit with message: "fix: return 501 from POST /tama/v1/restore instead of false success"

**Acceptance criteria:**
- [ ] `start_restore` contains no `tokio::spawn`, no `jobs.submit`, and no `std::fs::remove_file`
- [ ] `rg "TODO: Implement actual restore logic" crates/` — zero hits
- [ ] The handler returns `501` with `{"error":{"message": ..., "type":"NotImplementedError"}}` and leaves the uploaded file on disk — proven by the new test
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 3: Move benchmark-history DELETE behind CSRF middleware + add middleware unit tests (F21)

**Context:**
`DELETE /tama/v1/benchmarks/history/:id` is mounted in `crates/tama/src/router.rs:308` in the unprotected root router under a "Benchmark GET routes (no CSRF needed)" comment — every other DELETE in the app sits behind `api::middleware::enforce_same_origin`. The middleware itself (`crates/tama/src/api/middleware.rs`, 154 lines) has no `#[cfg(test)]` module, and its POST branch has a real hole: the match arms at lines ~90–100 reject `(Some(cookie), None)` but let `(Some(cookie), Some(mismatched_header))` and `(None, None)` fall through to the catch-all `_ => Ok(...)` — so a mismatched token pair is ACCEPTED, and token-less POSTs are accepted. Decision (already made): tighten the POST/PUT/PATCH branch so any combination other than "cookie and header present and equal" returns 403 — the lenient "(None, None) → allow" arm is removed (the code comment documents it as a trade-off for localhost dev, but the audit's decided test contract is: POST without token → 403). The DELETE branch (Origin-vs-Host check) stays exactly as-is. The GET/HEAD/OPTIONS branch (token issuance) stays exactly as-is.

**Files:**
- Modify: `crates/tama/src/router.rs`
- Modify: `crates/tama/src/api/middleware.rs`

**What to implement:**

1. **Route move** in `crates/tama/src/router.rs`: delete line 308 (`.route("/tama/v1/benchmarks/history/:id", delete(delete_benchmark))`) from the final root `Router::new()`. Add the same line to `csrf_routes` immediately after the `/tama/v1/benchmarks/mtp-run` route (after line 259), so the benchmark routes there read: `run`, `spec-run`, `mtp-run`, `history/:id` (delete). Keep the GET routes (`jobs/:id`, `jobs/:id/events`, `history`) in the root router and adjust their comment to `// Benchmark GET routes (safe methods, no CSRF needed)`.

2. **Middleware tightening** in `crates/tama/src/api/middleware.rs`: in `enforce_same_origin`, replace the POST/PUT/PATCH match (currently the four-arm match ending in the commented `_ => Ok(next.run(req).await)` arm) with:
   ```rust
   match (cookie_token, header_token) {
       // Both present and matching — full double-submit verification
       (Some(cookie_val), Some(header_val)) if cookie_val == header_val => {
           Ok(next.run(req).await)
       }
       // Any other combination — missing cookie, missing header, or mismatch — reject.
       _ => Err((StatusCode::FORBIDDEN, "CSRF token validation failed")),
   }
   ```
   Delete the now-obsolete inline comments about the localhost trade-off. Update the doc comment on `enforce_same_origin` (lines ~43–47): change the `POST/PUT/PATCH: verify CSRF double-submit (cookie matches header)` line to state that requests without a matching pair are rejected with 403. Do NOT touch `should_set_secure`, `generate_csrf_token`, `extract_csrf_cookie`, the GET branch, or the DELETE branch.

3. **New `#[cfg(test)] mod tests`** at the bottom of `crates/tama/src/api/middleware.rs`. Use the `tower::ServiceExt::oneshot` pattern from `crates/tama-core/src/proxy/scope_middleware.rs:134-198` (`Router::new().route(...).layer(axum::middleware::from_fn(enforce_same_origin))`, then `app.oneshot(Request::builder()...body(Body::empty()).unwrap())`). The middleware takes no state, so `from_fn` (not `from_fn_with_state`) and no `.with_state(...)` is needed. Table:
   - `test_get_sets_csrf_cookie_and_header`: GET `/` → 200; response has `Set-Cookie` starting with `tama_csrf_token=` and NOT containing `Secure` (no `X-Forwarded-Proto` sent); response has an `x-csrf-token` header equal to the token inside the cookie.
   - `test_get_sets_secure_flag_behind_https_proxy`: GET `/` with header `x-forwarded-proto: https` → 200; `Set-Cookie` contains `; Secure`.
   - `test_post_without_token_rejected`: POST `/` with no cookie/header → 403.
   - `test_post_with_matching_tokens_passes`: POST `/` with cookie `tama_csrf_token=abc123` and header `x-csrf-token: abc123` → 200.
   - `test_post_with_mismatched_tokens_rejected`: POST `/` with cookie `tama_csrf_token=abc123` and header `x-csrf-token: different` → 403.
   - `test_post_cookie_without_header_rejected`: POST `/` with cookie only → 403.
   - `test_put_and_patch_also_enforced`: PUT and PATCH with no tokens → 403 each (two assertions in one test is fine).
   - `test_delete_with_matching_origin_passes`: DELETE `/` with `origin: http://example.com` and `host: example.com` → 200.
   - `test_delete_with_mismatched_origin_rejected`: DELETE `/` with `origin: http://evil.com` and `host: example.com` → 403.
   - `test_delete_without_origin_allowed`: DELETE `/` with only `host: example.com` → 200 (documents existing behavior — do not change it).
   - `test_should_set_secure`: unit-test the private fn directly: empty `HeaderMap` → false; map with `x-forwarded-proto: https` → true; `x-forwarded-proto: http` → false.
   - `test_generate_csrf_token_format`: two calls produce different tokens; each token is 32 lowercase hex chars (`token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit())`).
   - `test_extract_csrf_cookie`: parses `a=1; tama_csrf_token=xyz; b=2` → `Some("xyz")`; returns `None` when absent.
   Note for the response-cookie assertion: `Set-Cookie` is `tama_csrf_token=<token>; Path=/; SameSite=Lax` (+`; Secure` when secure) — parse by splitting on `;`.

**Steps:**
- [ ] Write the failing tests (`test_post_without_token_rejected`, `test_post_with_mismatched_tokens_rejected`, `test_put_and_patch_also_enforced` fail against the current lenient match; the rest pass immediately) in `crates/tama/src/api/middleware.rs`
- [ ] Run `cargo nextest run --package tama -- api::middleware` — verify the three tests above fail and the others pass
- [ ] Implement the match tightening in `crates/tama/src/api/middleware.rs`
- [ ] Move the DELETE route in `crates/tama/src/router.rs` per above
- [ ] Run `cargo nextest run --package tama -- api::middleware` — all pass
- [ ] Run `cargo nextest run --package tama` — whole crate passes; if any existing route test relied on token-less POSTs, fix the TEST by adding a matching `tama_csrf_token` cookie + `x-csrf-token` header pair (pattern: `crates/tama/src/api/backends/manage/tests.rs` "Valid CSRF token pair"), never by loosening the middleware
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: protect benchmark history DELETE with CSRF middleware and test it"

**Acceptance criteria:**
- [ ] `crates/tama/src/router.rs` mounts `delete(delete_benchmark)` inside `csrf_routes`; the root router contains only GET benchmark routes
- [ ] `crates/tama/src/api/middleware.rs` has a `#[cfg(test)] mod tests` with the 12 tests above, all passing
- [ ] POST/PUT/PATCH without a matching cookie+header pair → 403 (proven by tests)
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 4: Route `GET /logout` (F25)

**Context:**
`handle_logout` (`crates/tama-core/src/proxy/auth.rs:667`) builds an expired `tama_session` cookie and redirects to the configured OAuth2 `logout_url` (or `/tama`), but no router registers it — there is no way to log out when OAuth2 is enabled. The login flow routes are registered twice: in `build_router` (`crates/tama-core/src/proxy/server/router.rs:55-57`) and in `build_unified_router` (`crates/tama-core/src/proxy/server/router.rs:165-167`), both times as `/login`, `/login/callback`, `/login/error` under a `// OAuth2 login flow` comment. Decision: add `/logout` in BOTH builders, right after `/login/error`. Do NOT add `/logout` to the hardcoded `LOGIN_SKIP_PATHS` in `auth_middleware` (`crates/tama-core/src/proxy/auth.rs:69`) — logging out is an authenticated action; an already-expired session is effectively logged out, and the middleware's normal redirect covers that case. Do NOT touch `handle_logout` itself.

**Files:**
- Modify: `crates/tama-core/src/proxy/server/router.rs`
- Modify: `crates/tama-core/src/proxy/auth.rs` (tests only)

**What to implement:**

1. In `crates/tama-core/src/proxy/server/router.rs`, extend the use block at lines 8–10 to include `handle_logout`:
   ```rust
   use crate::proxy::auth::{
       auth_middleware, handle_login, handle_login_callback, handle_login_error, handle_logout,
   };
   ```
   In `build_router`, after `.route("/login/error", get(handle_login_error))` (line 57) add `.route("/logout", get(handle_logout))`. In `build_unified_router`, after the identical line (167) add the same route. Keep the `// OAuth2 login flow` comments covering all four routes in both places.

2. Add tests to the existing `#[cfg(test)] mod tests` at the bottom of `crates/tama-core/src/proxy/auth.rs`. Reuse the existing `make_app_oauth2(auth_url)` helper (line 1109) for state construction, but build a minimal router per test (the helper mounts `auth_middleware`, which would intercept the request; instead construct `Arc::new(crate::proxy::ProxyState::new(config, None))` with the same config shape and mount only the logout route):
   ```rust
   let app = Router::new()
       .route("/logout", get(handle_logout))
       .with_state(proxy_state);
   ```
   - `test_logout_clears_session_cookie_and_redirects_to_tama`: config with `oauth2.enabled = true` and `logout_url: None` → GET `/logout` → assert `StatusCode::FOUND`; `location` header == `/tama`; `set-cookie` header starts with `tama_session=` and contains `Max-Age=0` (expired cookie clears the session).
   - `test_logout_redirects_to_configured_logout_url`: same but with `logout_url: Some("https://auth.example.com/logout".to_string())` in the `OAuth2Config` → `location` header == `https://auth.example.com/logout`; `set-cookie` still expires the session cookie.
   Construct the `OAuth2Config` exactly as `make_app_oauth2` does (struct update syntax `..Default::default()`); the `handle_logout` handler reads only `config.proxy.oauth2.logout_url`.

**Steps:**
- [ ] Write the two failing tests in `crates/tama-core/src/proxy/auth.rs` (they fail to compile until `handle_logout` is routable/visible — it is already `pub`, so the compile succeeds and the assertions verify behavior; run them once BEFORE touching the router to confirm they pass against the handler, proving the tests target routing, not the handler)
- [ ] Run `cargo nextest run --package tama-core -- proxy::auth` — new tests green (handler-level baseline)
- [ ] Register `/logout` in both `build_router` and `build_unified_router` in `crates/tama-core/src/proxy/server/router.rs`
- [ ] Add a router-level assertion: extend the new tests (or add `test_logout_route_registered`) to build the app via `build_router(proxy_state).await` and `oneshot` a GET `/logout` → 302 with expired-cookie `set-cookie` — proving the route is actually mounted
- [ ] Run `cargo nextest run --package tama-core -- proxy` — all pass
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: route GET /logout so OAuth2 sessions can be ended"

**Acceptance criteria:**
- [ ] `rg '"/logout"' crates/tama-core/src/proxy/server/router.rs` — two hits (both builders)
- [ ] GET `/logout` returns 302, sets an expired `tama_session` cookie, and redirects to `logout_url` or `/tama` — proven by tests through `build_router`
- [ ] No change to `LOGIN_SKIP_PATHS`, `handle_logout`, or `auth_middleware`
- [ ] `cargo nextest run --package tama-core` passes; clippy clean

---

### Task 5: Fix SvelteKit doc drift (F35)

**Context:**
`CONTEXT.md:3` says Tama ships "a SvelteKit web control plane" and `docs/decisions/0003-use-sveltekit-over-leptos-wasm.md` claims a SvelteKit migration "removed Leptos, Trunk, and all WASM build infrastructure" — but the live UI is Leptos compiled to WASM via Trunk (`crates/tama/src/lib.rs` routes 11 Leptos pages; `crates/tama/dist/index.html` loads `tama-*_bg.wasm`; `AGENTS.md` documents `make dev` as the Leptos dev server). The migration was decided but never implemented, and the docs actively misdescribe the system. Decision: CONTEXT.md is corrected to describe the live system; ADR-0003 is annotated as proposed-but-not-implemented (do NOT delete it — it records a real decision that may still be executed later); the empty `crates/tama/ui/` directory (contains only an untracked, empty `node_modules/`) is deleted. The workspace version is 2.0.0 — use that in the annotation.

**Files:**
- Modify: `CONTEXT.md`
- Modify: `docs/decisions/0003-use-sveltekit-over-leptos-wasm.md`
- Delete: `crates/tama/ui/` (untracked; verify with `git ls-files crates/tama/ui` before deleting)

**What to implement:**

1. **`CONTEXT.md`** line 3: replace the sentence `A local AI server written in Rust that provides an OpenAI-compatible API on a single port. It manages backend lifecycles — starting models on demand, routing requests, and unloading idle models — with a SvelteKit web control plane.` with the same sentence but ending `— with a Leptos/WASM web control plane (compiled via Trunk and embedded in the binary).` Do not touch anything else in the file.

2. **`docs/decisions/0003-use-sveltekit-over-leptos-wasm.md`**: immediately under the `# Use SvelteKit over Leptos/WASM for the frontend` title, insert:
   ```markdown
   > **Status: proposed, not implemented — Leptos remains the live UI as of 2.0.0.**
   > The migration described below was never executed. The web control plane is
   > still Leptos compiled to WASM via Trunk and embedded in the `tama` binary
   > (`crates/tama/src/`, `crates/tama/dist/`). This document is retained as a
   > record of the decision; update or supersede it if the migration happens.
   ```
   Also change the `**Status:**` line if the file has one (it does not currently — the blockquote above is the only status marker to add). Do not alter the rest of the ADR.

3. **`crates/tama/ui/`**: verify nothing is tracked (`git ls-files crates/tama/ui` prints nothing), then `rm -rf crates/tama/ui`. No git commit entry is needed for the directory itself if untracked — include the deletion in the commit only if `git status` shows tracked content (it should not).

**Steps:**
- [ ] Edit `CONTEXT.md` line 3 per above
- [ ] Add the status blockquote to `docs/decisions/0003-use-sveltekit-over-leptos-wasm.md` per above
- [ ] Run `git ls-files crates/tama/ui` — confirm empty; then `rm -rf crates/tama/ui`
- [ ] Run `rg -i "sveltekit" CONTEXT.md docs/decisions/ AGENTS.md README.md` — the only remaining hits are inside ADR-0003 itself (now clearly marked as not implemented) and any historical entries under `docs/plans/done/` (leave those)
- [ ] Run `cargo check --workspace` — docs-only change, but confirm nothing referenced `crates/tama/ui` in the build (it should not)
- [ ] Commit with message: "docs: correct SvelteKit claims — Leptos/WASM is the live UI"

**Acceptance criteria:**
- [ ] `CONTEXT.md` describes a Leptos/WASM (Trunk-embedded) control plane, not SvelteKit
- [ ] ADR-0003 carries the "proposed, not implemented" status note naming version 2.0.0
- [ ] `crates/tama/ui/` no longer exists; no tracked files were deleted (`git status` clean apart from the two edited docs)
