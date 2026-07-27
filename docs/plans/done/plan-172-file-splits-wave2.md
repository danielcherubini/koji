# File Splits Wave 2 Plan

**Goal:** Split the three remaining god files (`pull_queue.rs` 1,756 lines, `api/updates.rs` 1,022 lines, `auth.rs` 1,526 lines) into focused module directories without changing any public API or behavior.

**Architecture:** Each task converts one file into a same-named module directory (`foo.rs` → `foo/{mod.rs, ...}.rs`) with `pub use` re-exports in `mod.rs`, so every existing import path (`tama_core::proxy::pull_queue::PullQueueService`, `crate::api::updates::trigger_check`, `crate::proxy::auth::auth_middleware`, …) keeps compiling unchanged. Sibling submodules share items via `pub(super)` visibility. This mirrors the already-landed plan-145 (`docs/plans/done/plan-145-file-splits.md`).

**Tech Stack:** Rust, Axum, tokio

---

### Task 1: Split `proxy/pull_queue.rs` (1,756 lines) into 5 modules

**Context:**
`crates/tama-core/src/proxy/pull_queue.rs` carries four responsibilities: the `PullEvent` enum (lines 18–54), the `PullQueueService` DB/event struct (lines 56–349), the async queue-processor lifecycle with dead-task detection and startup recovery (lines 351–524), and ~1,230 lines of inline tests (lines 525–1756). Decisions: `on_startup_recovery` and `try_mark_running` stay `pub` methods on `PullQueueService` (moved into a second `impl` block inside `recovery.rs` — inherent impls may live in any module of the same crate) so the public API is preserved exactly; tests keep calling `svc.on_startup_recovery()`. Do NOT change any logic, SQL, or test bodies — this is a pure move. The 24 hand-rolled error sites mentioned in the audit belong to `api/updates.rs`, not this file.

**Files:**
- Create: `crates/tama-core/src/proxy/pull_queue/mod.rs`
- Create: `crates/tama-core/src/proxy/pull_queue/events.rs`
- Create: `crates/tama-core/src/proxy/pull_queue/service.rs`
- Create: `crates/tama-core/src/proxy/pull_queue/recovery.rs`
- Create: `crates/tama-core/src/proxy/pull_queue/tests.rs`
- Delete: `crates/tama-core/src/proxy/pull_queue.rs`
- Modify: nothing else (`crates/tama-core/src/proxy/mod.rs:8` already has `pub mod pull_queue;`)

**What to implement:**

1. **`events.rs`** — the `PullEvent` enum exactly as at lines 18–54 (`Started`, `Progress`, `Verifying`, `Completed`, `Failed`, `Cancelled`, `Queued`, all struct variants, `#[derive(Debug, Clone)]`). No imports needed.

2. **`service.rs`** — `PullQueueService` struct (line 58) and its impl block from lines 64–324, EXCLUDING `on_startup_recovery` and `try_mark_running` (those go to `recovery.rs`): `new`, `#[cfg(test)] test_model_mgr`, `enqueue`, `dequeue`, `update_status`, `update_progress`, `cancel`, `get_active_items`, `get_active_items_dto`, `get_history_items`, `get_history_items_dto`, `count_history_items`, `get_queue_item`, `subscribe_events`. Also the private free fn `item_to_dto` (lines 326–349). Imports: `std::sync::Arc` is NOT needed here; use `anyhow::{anyhow, Result}`, `tokio::sync::broadcast`, `crate::db::repository::PullQueueDto`, `crate::db::queries::PullQueueItem`, `crate::models::ModelManager`, `super::events::PullEvent`. Change field visibility: `model_mgr` and `poll_interval_secs` become `pub(super)` (they are accessed from `recovery.rs::queue_processor_loop` at current lines 388, 408, 479 and from `tests.rs` at current line 663 etc.); `events_tx` stays private.

3. **`recovery.rs`** — a second `impl PullQueueService` block containing `on_startup_recovery` (lines 298–310) and `try_mark_running` (lines 315–324) unchanged; the private `async fn start_pull_from_queue` (lines 351–376); and `pub(crate) async fn queue_processor_loop` (lines 379–524). Fix paths that used `super::` in the flat file: `super::ProxyState` → `crate::proxy::ProxyState`, `super::tama_handlers::QuantDownloadSpec` and `super::tama_handlers::start_pull_from_queue` → `crate::proxy::tama_handlers::{QuantDownloadSpec, start_pull_from_queue}`. Imports: `std::sync::Arc`, `anyhow::Result`, `super::service::PullQueueService`.

4. **`mod.rs`** — module doc comment (keep the file's current `//!` doc), then:
   ```rust
   mod events;
   mod recovery;
   mod service;
   #[cfg(test)]
   mod tests;

   pub use events::PullEvent;
   pub use service::PullQueueService;
   pub(crate) use recovery::queue_processor_loop;
   ```
   This preserves every existing path: `tama_core::proxy::pull_queue::{PullEvent, PullQueueService}` (used by `crates/tama/src/api/downloads.rs:203-263`, `crates/tama-core/src/proxy/types.rs:8`, `crates/tama-core/src/proxy/state.rs:5`, `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs:16`, `crates/tama/tests/downloads_api.rs:14`) and `queue_processor_loop` (used by `proxy/state.rs:5,64`).

5. **`tests.rs`** — the full body of the current `#[cfg(test)] mod tests` (lines 526–1756), verbatim, minus the `mod tests {` wrapper. Replace `use super::*;` with explicit imports: `use super::{PullEvent, PullQueueService};` plus what the tests actually reference: `use crate::config::Config;`, `use crate::models::ModelManager;`, `use crate::proxy::ProxyState;`, `use std::time::Instant;` (already imported in the test module today at lines 533–535 — keep those). Direct field accesses `svc.model_mgr.lock()` (e.g. current line 663) keep compiling because `model_mgr` is now `pub(super)`. Do NOT rename, reorder, or edit any test.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- proxy::pull_queue` — confirm the existing 25 tests are green BEFORE moving anything (baseline)
- [ ] Create `crates/tama-core/src/proxy/pull_queue/` with `events.rs`, `service.rs`, `recovery.rs`, `mod.rs`, `tests.rs` per above; delete `crates/tama-core/src/proxy/pull_queue.rs`
- [ ] Run `cargo check --package tama-core` — compiles (fix only import/visibility mistakes, never logic)
- [ ] Run `cargo nextest run --package tama-core -- proxy::pull_queue` — all 25 tests pass, zero test-body edits
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (catches path breakage in `proxy/state.rs`, `proxy/types.rs`, `tama_handlers/pull/verify.rs`)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: split proxy/pull_queue.rs into events, service, recovery, tests"

**Acceptance criteria:**
- [ ] `crates/tama-core/src/proxy/pull_queue.rs` no longer exists; no file under `proxy/pull_queue/` exceeds 450 lines except `tests.rs`
- [ ] `PullEvent`, `PullQueueService` (with all 15 pub methods), and `pub(crate) queue_processor_loop` are reachable at the same paths as before — no edits to any caller file
- [ ] `cargo nextest run --package tama-core` passes with the same test count as the baseline
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 2: Split `api/updates.rs` (1,022 lines) into 4 modules

**Context:**
`crates/tama/src/api/updates.rs` mixes three concerns: update checking (`get_updates` lines 79–184, `trigger_check` lines 185–217, `CheckSingleQuery` + `check_single` lines 218–340), SSE streaming (`update_events_sse` lines 341–417), and apply flows (`apply_backend_update` lines 418–648, `apply_model_update` lines 649–815), plus DTOs (lines 24–77) and DTO-serialization tests (lines 816–1022). Decisions: this is a pure move — do NOT fix the 24 hand-rolled error sites (that is plan-161's job), do NOT change the SSE event mapping in `update_events_sse` (that is plan-168's job). `QuantDetailJson` belongs in `check.rs` because its only use is inside `get_updates` (line 114).

**Files:**
- Create: `crates/tama/src/api/updates/mod.rs`
- Create: `crates/tama/src/api/updates/check.rs`
- Create: `crates/tama/src/api/updates/events.rs`
- Create: `crates/tama/src/api/updates/apply.rs`
- Delete: `crates/tama/src/api/updates.rs`
- Modify: nothing else (`crates/tama/src/api.rs:21` already has `pub mod updates;`)

**What to implement:**

1. **`check.rs`** — DTOs `UpdateCheckDto` (lines 25–39), `UpdatesListResponse` (41–45), `CheckResponse` (47–52), `QuantDetailJson` (69–77); handlers `get_updates` (79–184), `trigger_check` (185–217); `CheckSingleQuery` (218–225) + `check_single` (227–340). Keep the `#[cfg(test)] mod tests` from lines 816–1022 at the bottom of this file — all 8 tests (`test_update_check_dto_serialization`, `test_update_check_dto_model_type`, `test_update_check_dto_with_error`, `test_update_check_dto_with_details_json`, `test_updates_list_response_serialization`, `test_updates_list_response_empty`, `test_check_response_serialization`, `test_check_response_serialization_false`) only exercise DTOs that live in `check.rs`, so `use super::*;` keeps working. Imports needed: `axum::{extract::State, http::StatusCode, Json}` (+ `extract::Path`/`Extension` only if the moved code uses them — copy exactly what each moved function's signature requires from the file's current `use` block at lines 1–22), `serde::{Deserialize, Serialize}`, `std::sync::Arc`, `crate::web_types::WebState`, `tama_core::db::repository::Repository`, `tama_core::proxy::ProxyState`, `tama_core::updates::UpdateEvent` (only if referenced by moved code — check before importing; unused imports fail clippy).

2. **`events.rs`** — `update_events_sse` (lines 341–417) unchanged, including its current `Sse::new(...)` construction and KeepAlive behavior (plan-168 touches this later). Imports: `async_stream::stream`, `axum::response::sse::{Event, KeepAlive}`, `axum::response::Sse`, `axum::extract::State`, `futures_util::Stream`, `std::sync::Arc`, `tama_core::proxy::ProxyState`, `tama_core::updates::UpdateEvent` — again, copy only what the function actually uses.

3. **`apply.rs`** — `ModelUpdateRequest` (54–58), `ModelUpdateResponse` (60–67), `apply_backend_update` (418–648), `apply_model_update` (649–815), verbatim. Imports include `tama_core::backends::{check_latest_version, get_backend_install_path, BackendManager, BackendSource, BackendType, InstallOptions}` (verify against actual usage in the moved range; `check_single` in `check.rs` may use some of these too — each submodule imports only what it needs).

4. **`mod.rs`** —
   ```rust
   mod apply;
   mod check;
   mod events;

   pub use apply::{apply_backend_update, apply_model_update, ModelUpdateRequest, ModelUpdateResponse};
   pub use check::{
       check_single, get_updates, trigger_check, CheckResponse, CheckSingleQuery, QuantDetailJson,
       UpdateCheckDto, UpdatesListResponse,
   };
   pub use events::update_events_sse;
   ```
   This preserves the six handler paths used by `crates/tama/src/router.rs:167-184` (`api::updates::{trigger_check, check_single, update_events_sse, apply_backend_update, apply_model_update, get_updates}`) and every DTO path. Do NOT change `crates/tama/src/api/openapi.rs` — it references these DTOs only by string name.

**Steps:**
- [ ] Run `cargo nextest run --package tama -- api::updates` — confirm the 8 DTO tests are green BEFORE moving (baseline)
- [ ] Create `crates/tama/src/api/updates/` with `check.rs` (+ tests at bottom), `events.rs`, `apply.rs`, `mod.rs` per above; delete `crates/tama/src/api/updates.rs`
- [ ] Run `cargo check --package tama` — compiles (fix only import mistakes; per-submodule `use` lists must contain no unused imports)
- [ ] Run `cargo nextest run --package tama -- api::updates` — 8 tests pass, zero test-body edits
- [ ] Run `cargo nextest run --package tama` — whole crate passes (catches path breakage in `router.rs`)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: split api/updates.rs into check, events, apply"

**Acceptance criteria:**
- [ ] `crates/tama/src/api/updates.rs` no longer exists; `check.rs` < 550 lines (incl. tests), `events.rs` < 100 lines, `apply.rs` < 450 lines
- [ ] All 6 handlers and all 8 DTOs/types are reachable at `crate::api::updates::*` exactly as before — no edits to `router.rs`, `openapi.rs`, or any page/component
- [ ] `cargo nextest run --package tama` passes with the same test count as the baseline
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 3: Split `proxy/auth.rs` (1,526 lines) into 6 modules

**Context:**
`crates/tama-core/src/proxy/auth.rs` mixes four auth mechanisms: the Axum middleware (`auth_middleware` lines 45–213 with helpers at 215–310), signed session cookies (`SessionClaims`/`extract_session`/`session_cookie` lines 306–394), the OAuth2 login flow (`build_oauth2_client_from_config`, `fetch_userinfo`, `handle_login`, `handle_login_callback`, `handle_logout`, `handle_login_error` lines 396–705), and ~820 lines of tests (706–1526). The API-key branch (middleware step 4a, lines 88–154) is currently inline in the middleware — extracting it into `api_key.rs` as a `pub(super)` helper is the one small code-motion refactor in this split; behavior must stay identical (same logs, same 401 bodies, same spawn_blocking validation). Decisions: `AuthConfig` (lines 29–31) stays in `mod.rs` — it is an empty compat marker, not middleware. `SESSION_COOKIE_NAME` (line 306) goes to `session.rs` as `pub(super)` (used by `session.rs`, `oauth2.rs::handle_logout` at line 675, and `tests.rs`); `CSRF_STATE_COOKIE_NAME` (line 308) goes to `oauth2.rs` private (only used by the login flow, lines 541–598). Do NOT route `handle_logout` anywhere (F25 is a different plan) and do NOT add new tests (F8 is a different plan).

**Files:**
- Create: `crates/tama-core/src/proxy/auth/mod.rs`
- Create: `crates/tama-core/src/proxy/auth/middleware.rs`
- Create: `crates/tama-core/src/proxy/auth/api_key.rs`
- Create: `crates/tama-core/src/proxy/auth/session.rs`
- Create: `crates/tama-core/src/proxy/auth/oauth2.rs`
- Create: `crates/tama-core/src/proxy/auth/tests.rs`
- Delete: `crates/tama-core/src/proxy/auth.rs`
- Modify: nothing else (`crates/tama-core/src/proxy/mod.rs:2` already has `pub mod auth;`)

**What to implement:**

1. **`session.rs`** — `pub(super) const SESSION_COOKIE_NAME: &str = "tama_session";`; `SessionClaims` struct (lines 312–323) with ALL fields `pub(super)` (tests construct it with struct literals at current lines 919–926, 978–985) and its impl (`new`, `is_valid`) as `pub(super)` methods; `pub(super) fn extract_session(req: &Request, state: &crate::proxy::ProxyState) -> Option<SessionClaims>` (lines 352–370); `pub(super) fn session_cookie(claims: &SessionClaims, is_secure: bool, state: &crate::proxy::ProxyState) -> String` (lines 372–394). Imports: `axum::{extract::Request, http::header}`, `serde::{Deserialize, Serialize}`.

2. **`api_key.rs`** — move `json_unauthorized_invalid_key` (lines 269–281) and `json_unauthorized_api_keys` (lines 283–295) here as `pub(super) fn`. Extract middleware step 4a (lines 88–154, the whole `if bearer_token.starts_with("tama_") { … }` block) into:
   ```rust
   pub(super) enum ApiKeyAuthOutcome {
       Authenticated(AuthSubject),
       Rejected(Response),
   }

   pub(super) async fn authenticate_api_key(
       proxy_state: &Arc<crate::proxy::ProxyState>,
       bearer_token: &str,
       api_keys_enabled: bool,
       req: &Request,
   ) -> ApiKeyAuthOutcome
   ```
   Body = the existing logic verbatim (disabled-check → `json_unauthorized_api_keys`; `spawn_blocking` + `proxy_state.open_db()` + `validate_key`; the five match arms mapping to `info!`/`warn!` logs and `json_unauthorized*` responses), with `Ok(AuthSubject::Key { key_id, scopes })` returned as `Authenticated(...)` instead of inserting into extensions. Clone the `Arc` for the `spawn_blocking` closure. Imports: `std::sync::Arc`, `axum::{extract::Request, response::Response}`, `tracing::{info, warn}`, `crate::proxy::api_keys::{self, validate_key, AuthSubject}`.

3. **`middleware.rs`** — `pub async fn auth_middleware` (lines 45–213) with two mechanical changes: (a) step 4a becomes
   ```rust
   if bearer_token.starts_with("tama_") {
       match super::api_key::authenticate_api_key(&proxy_state, &bearer_token, api_keys_enabled, &req).await {
           super::api_key::ApiKeyAuthOutcome::Authenticated(subject) => {
               let mut req = req;
               req.extensions_mut().insert(subject);
               return next.run(req).await;
           }
           super::api_key::ApiKeyAuthOutcome::Rejected(resp) => return resp,
       }
   } else { /* existing 4b Authentik branch unchanged */ }
   ```
   (b) `extract_session` is now `super::session::extract_session`. Also move here: `extract_bearer_token` (215–219, private), `validate_token_against_authentik` (221–253, private — it is only called from the middleware), `json_unauthorized` (255–267, private), `extract_remote_addr` (297–310, stays here ONLY if still used after the 4a extraction — it is used by the API-key branch's disabled-reject log, so MOVE it to `api_key.rs` as `pub(super)` and delete from here if no remaining use; decide by grepping the final middleware.rs). `AuthentikUserResponse`/`AuthentikUser` (34–42) stay here since `validate_token_against_authentik` uses them.

4. **`oauth2.rs`** — `const CSRF_STATE_COOKIE_NAME: &str = "tama_oauth2_state";` (private); `build_oauth2_client_from_config` (396–411), `fetch_userinfo` (413–461), `redirect_to_login_error` (463–471), `html_escape` (473–482) as private fns; `pub async fn handle_login_error` (484–507), `pub async fn handle_login` (509–566), `pub async fn handle_login_callback` (568–665), `pub async fn handle_logout` (667–705). Uses `super::session::{session_cookie, SessionClaims, SESSION_COOKIE_NAME}`. Imports: the `oauth2::{basic::BasicClient, AuthUrl, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl, TokenResponse, TokenUrl}` block (only the items actually used by moved code — `TokenResponse` is needed for `.access_token()` in the callback), `serde::Deserialize` (only if a moved struct needs it — `AuthentikUser*` stays in middleware.rs), `std::collections::HashMap`, `std::sync::Arc`, `crate::config::types::OAuth2Config`, axum extract/response items, `tracing::warn`.

5. **`mod.rs`** — keep the file's `//!` doc comment; then:
   ```rust
   mod api_key;
   mod middleware;
   mod oauth2;
   mod session;
   #[cfg(test)]
   mod tests;

   /// AuthConfig is no longer used; auth settings are read live from ProxyState.
   /// The type is retained for backward compatibility but is empty.
   #[derive(Clone, Debug)]
   pub struct AuthConfig;

   pub use middleware::auth_middleware;
   pub use oauth2::{handle_login, handle_login_callback, handle_login_error, handle_logout};
   ```
   This preserves the paths used by `crates/tama-core/src/proxy/server/router.rs:8-10` (`auth_middleware, handle_login, handle_login_callback, handle_login_error`), `proxy/scope_middleware.rs:566,620,681`, and `proxy/tama_handlers/api_keys.rs:565`.

6. **`tests.rs`** — full body of the current `mod tests` (lines 707–1526) verbatim, minus the wrapper. Replace `use super::*;` with: `use super::middleware::auth_middleware;`, `use super::session::{SessionClaims, SESSION_COOKIE_NAME};`, plus the existing test-module imports (`crate::proxy::api_keys::Scope`, `axum::middleware`, `axum::{routing::get, Router}`, `std::sync::Arc`, `std::time::Duration`, `tower::util::ServiceExt`). Field-level struct-literal construction of `SessionClaims` compiles because the fields are `pub(super)`. Do NOT rename, reorder, or edit any test.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- proxy::auth` — confirm the existing tests are green BEFORE moving (baseline; currently 17+ tests covering middleware/cookies/keys)
- [ ] Create `crates/tama-core/src/proxy/auth/` with `session.rs`, `api_key.rs`, `middleware.rs`, `oauth2.rs`, `mod.rs`, `tests.rs` per above; delete `crates/tama-core/src/proxy/auth.rs`
- [ ] Run `cargo check --package tama-core` — compiles (fix only import/visibility mistakes)
- [ ] Run `cargo nextest run --package tama-core -- proxy::auth` — all tests pass, zero test-body edits; verify the API-key tests (`test_tama_key_auth_passes`, `test_tama_key_auth_invalid_returns_401`, `test_tama_key_disabled_returns_401`, `test_non_tama_bearer_still_validates_authentik`) still exercise the extracted `authenticate_api_key`
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (catches path breakage in `server/router.rs`, `scope_middleware.rs`, `tama_handlers/api_keys.rs`)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean (watch for unused imports left over from the move, e.g. `oauth2::TokenResponse` in middleware.rs)
- [ ] Commit with message: "refactor: split proxy/auth.rs into middleware, api_key, session, oauth2, tests"

**Acceptance criteria:**
- [ ] `crates/tama-core/src/proxy/auth.rs` no longer exists; no file under `proxy/auth/` exceeds 500 lines except `tests.rs`
- [ ] `auth_middleware`, `handle_login`, `handle_login_callback`, `handle_login_error`, `handle_logout`, `AuthConfig` reachable at `tama_core::proxy::auth::*` exactly as before — no edits to any caller file
- [ ] Middleware behavior identical: same 401 bodies (`json_unauthorized*` moved verbatim), same log messages, same spawn_blocking validation path
- [ ] `cargo nextest run --package tama-core` passes with the same test count as the baseline
- [ ] `cargo clippy --workspace -- -D warnings` is clean
