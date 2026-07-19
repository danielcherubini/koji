# Router Consolidation Plan

**Goal:** Collapse the two copy-synced `/tama/v1` route tables in `crates/tama-core/src/proxy/server/router.rs` into a single-source proxy-exclusive table, give the `tama` crate sole ownership of management CRUD routes (explicitly mounting the three core handlers it needs), and move the generic process helpers from `proxy/process.rs` up to `crate::process`.

**Architecture:** Audit finding F33 (`docs/reviews/2026-07-18-codebase-improvement.md` #33). Today `build_router` (standalone, 118 lines) and `build_unified_router` (production, 100 lines) duplicate ~90% of one route table, kept consistent by comments. Verification while writing this plan surfaced a LIVE shadow-route bug: `GET /tama/v1/system/health` is excluded from `build_unified_router` ("web UI re-exports proxy handler") but `crates/tama/src/router.rs` never mounts it — in production the request falls into the `/tama/*path` SPA wildcard and returns `index.html`. Also verified: the production binary (`crates/tama/src/main.rs:126-128`) ALWAYS builds the unified router; standalone mode (`ProxyServer::into_router`/`run`) is exercised only by tama-core tests (`proxy/server/tests.rs` ×6, `proxy/handlers/compaction.rs:357,383`), so removing management CRUD from core's table breaks no production path. After this plan: one `proxy_routes()` list (inference + lifecycle + auth + proxy-ops + POST forwarding) feeds both `build_router` (adds GET wildcard + fallback for standalone) and `build_unified_router` (merges web routes), and a `proxy_route_paths()` export powers a cross-crate ownership test.

**Tech Stack:** Rust, Axum 0.8, tower-http, tokio

---

### Task 1: Move generic process helpers into `crate::process` (shim re-export)

**Context:**
Generic POSIX process utilities live in `crates/tama-core/src/proxy/process.rs` (9 functions: `override_arg`, `is_process_alive`, `kill_process`, `force_kill_process`, `configure_process_group`, `kill_process_group`, `force_kill_process_group`, `is_process_group_alive`, `check_health` + 3 tests) while `crates/tama-core/src/process.rs` holds the sibling `configure_backend_command` — causing the wrong-direction import `bench/runner.rs:18` (`use crate::proxy::process::…`; bench is a leaf, proxy is not). Decisions: move ALL 9 functions and the 3 inline tests (`test_kill_process_group_nonexistent_pid_returns_ok`, `test_force_kill_process_group_nonexistent_pid_returns_ok`, `test_process_group_kills_children`) into `crate::process`; `proxy/process.rs` becomes a pure re-export shim so every existing caller path keeps compiling in this commit. `ProcessSupervisor` (`process.rs:88`) stays untouched — plan-173 deletes it as dead code.

**Files:**
- Modify: `crates/tama-core/src/process.rs`
- Modify: `crates/tama-core/src/proxy/process.rs`

**What to implement:**

1. `crates/tama-core/src/process.rs`: append the 9 functions verbatim from `proxy/process.rs` (keep their doc comments and the `anyhow::{anyhow, Context, Result}` usage — `process.rs` already imports `{Context, Result}` at :1; add `anyhow` to that import), and append the 3 tests to the existing `mod tests` (:306), replacing their `use super::*;` assumptions with the merged module's (they keep working — same module now).
2. `crates/tama-core/src/proxy/process.rs`: replace the entire body with:
   ```rust
   //! Re-exports — the generic process helpers moved to `crate::process`.
   //! This module exists only for path compatibility; new code should import
   //! from `crate::process` directly. Removed once callers migrate (this plan).
   pub use crate::process::{
       check_health, configure_process_group, force_kill_process, force_kill_process_group,
       is_process_alive, is_process_group_alive, kill_process, kill_process_group, override_arg,
   };
   ```
3. Do NOT touch `proxy/mod.rs:31` (`pub use process::{check_health, force_kill_process, is_process_alive, kill_process, override_arg};`) — it resolves through the shim unchanged.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- process` — green baseline (incl. the 3 group-kill tests)
- [ ] Move the functions + tests; convert `proxy/process.rs` to the shim
- [ ] Run `cargo check --package tama-core` — compiles with zero caller edits
- [ ] Run `cargo nextest run --package tama-core -- process` — same tests pass from their new home
- [ ] Run `cargo nextest run --package tama-core` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: move generic process helpers from proxy::process to crate::process (shim re-export)"

**Acceptance criteria:**
- [ ] All 9 helpers are defined in `crate::process`; `proxy/process.rs` contains only the re-export block
- [ ] Zero caller-file edits; `cargo nextest run --package tama-core` passes with the same test count
- [ ] Clippy clean

---

### Task 2: Migrate process-helper callers to `crate::process`; delete `proxy/process.rs`

**Context:**
With the shim in place, switch every caller to the canonical path and delete the shim module. Verified caller list (complete — from rg): `proxy/tama_handlers/models/handlers.rs:13`, `proxy/lifecycle/compaction.rs:5`, `proxy/lifecycle/tts.rs:6`, `proxy/lifecycle/idle_timeout.rs:5`, `proxy/lifecycle/mod.rs:10` (`use super::process::{…}`), `proxy/forward/request.rs:39` and `:585` (`crate::proxy::process::is_process_alive` inline paths), and the wrong-direction `bench/runner.rs:18`. `proxy/mod.rs:6` (`pub mod process;`) is removed; `proxy/mod.rs:31`'s re-export is redirected to `crate::process` (kept pub — it is workspace-public API even though no external callers exist today; the dead-code sweep can remove it later).

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/handlers.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/{mod,compaction,tts,idle_timeout}.rs`
- Modify: `crates/tama-core/src/proxy/forward/request.rs`
- Modify: `crates/tama-core/src/bench/runner.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs`
- Delete: `crates/tama-core/src/proxy/process.rs`

**What to implement:**

1. Import swaps (function lists unchanged): `use crate::proxy::process::{…}` → `use crate::process::{…}` in handlers.rs, compaction.rs, tts.rs, idle_timeout.rs, runner.rs; `use super::process::{…}` → `use crate::process::{…}` in lifecycle/mod.rs; the two inline `crate::proxy::process::is_process_alive(pid)` paths in forward/request.rs → `crate::process::is_process_alive(pid)`.
2. Delete `crates/tama-core/src/proxy/process.rs`.
3. `proxy/mod.rs`: delete `pub mod process;` (:6); change :31 to `pub use crate::process::{check_health, force_kill_process, is_process_alive, kill_process, override_arg};`.

**Steps:**
- [ ] Apply the swaps; delete the shim; run `cargo check --package tama-core`
- [ ] Run `rg "proxy::process" crates/` — zero hits
- [ ] Run `cargo nextest run --package tama-core` — all pass
- [ ] Run `cargo nextest run --package tama` — downstream crate unaffected
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: callers import process helpers from crate::process; drop proxy::process shim"

**Acceptance criteria:**
- [ ] `rg "proxy::process" crates/` — zero hits; `bench/runner.rs` no longer imports from `proxy` for process utilities
- [ ] `tama_core::proxy::{check_health, force_kill_process, is_process_alive, kill_process, override_arg}` still resolves (re-export redirected)
- [ ] Workspace tests pass; clippy clean

---

### Task 3: Single-source the core route table; drop management routes from core

**Context:**
`router.rs` currently maintains TWO tables by comment. This task replaces both with one data-driven list, `proxy_routes() -> Vec<(&'static str, &'static str, MethodRouter<Arc<ProxyState>>)>` (method-label, path, router), folded by both `build_router` and `build_unified_router`. The method label exists only for the test export `proxy_route_paths()` — `MethodRouter` can't introspect its methods. Routes REMOVED from core (management, owned by the `tama` table — Task 4 mounts the three that must keep working): `GET /tama/v1/models` (`handle_tama_list_models`), `GET /tama/v1/models/:id` (`handle_tama_get_model`, killing the gratuitous `as handle_tama_get_model_fn` alias — F40), `GET /tama/v1/hf/*repo_id` (`handle_hf_list_quants`), `GET /tama/v1/system/health` (`handle_tama_system_health`), `GET /tama/v1/logs` (`handle_all_logs`), `GET /tama/v1/logs/:backend/events` (`handle_backend_log_sse`). The three unrouted handlers (`handle_tama_list_models`, `handle_tama_get_model`, `handle_hf_list_quants` — verified zero callers outside router.rs) are LEFT IN PLACE with a `// TODO(plan-172): unrouted — delete` comment; the dead-code sweep owns deletion. The stale NOTE comment block claiming `/tama/v1/logs/:backend` overlap (a route core never had) is deleted.

**Final core route ownership table** (the `proxy_routes()` list — method label / path / handler):

| Methods | Path | Handler | Group |
|---|---|---|---|
| POST | `/v1` | `handle_chat_completions` | inference |
| POST | `/v1/chat/completions` | `handle_chat_completions` | inference |
| POST | `/v1/chat/completions/stream` | `handle_stream_chat_completions` | inference |
| GET | `/v1/models` | `handle_list_models` | inference |
| GET | `/v1/models/:model_id` | `handle_get_model` | inference |
| GET | `/v1/opencode/models` | `handle_opencode_list_models` | inference |
| GET | `/v1/audio/models` | `handle_audio_models` | inference (TTS) |
| POST | `/v1/audio/speech` | `handle_audio_speech` | inference (TTS) |
| POST | `/v1/audio/speech/stream` | `handle_audio_stream` | inference (TTS) |
| GET | `/v1/audio/voices` | `handle_audio_voices` | inference (TTS) |
| POST | `/v1/compaction` | `handle_compaction` | inference |
| GET | `/status` | `handle_status` | ops |
| GET | `/health` | `handle_health` | ops |
| GET | `/metrics` | `handle_metrics` | ops |
| GET | `/login` | `handle_login` | auth |
| GET | `/login/callback` | `handle_login_callback` | auth |
| GET | `/login/error` | `handle_login_error` | auth |
| POST | `/tama/v1/models/:id/load` | `handle_tama_load_model` | lifecycle |
| POST | `/tama/v1/models/:id/unload` | `handle_tama_unload_model` | lifecycle |
| POST | `/tama/v1/models/:id/cancel` | `handle_tama_cancel_load` | lifecycle |
| POST | `/tama/v1/pulls` | `handle_tama_pull_model` | lifecycle (pulls) |
| GET | `/tama/v1/pulls/:job_id` | `handle_tama_get_pull_job` | lifecycle (pulls) |
| GET | `/tama/v1/pulls/:job_id/stream` | `handle_pull_job_stream` | lifecycle (pulls) |
| GET+POST | `/tama/v1/keys` | `handle_tama_api_keys_list` / `handle_tama_api_keys_create` | auth (API keys) |
| PATCH+DELETE | `/tama/v1/keys/:id` | `handle_tama_api_keys_update` / `handle_tama_api_keys_revoke` | auth (API keys) |
| POST | `/tama/v1/system/reload-configs` | `handle_reload_configs` | proxy ops |
| GET | `/tama/v1/system/metrics/stream` | `handle_system_metrics_stream` | proxy ops |
| GET | `/tama/v1/system/gpu-devices` | `handle_tama_system_gpu_devices` | proxy ops |
| POST | `/tama/v1/system/gpu-devices/refresh` | `handle_tama_system_gpu_devices_refresh` | proxy ops |
| POST | `/tama/v1/system/restart` | `handle_tama_system_restart` | proxy ops |
| POST | `/*path` | `handle_forward_post` | forwarding |

Standalone-only additions (in `build_router` after the fold, NOT in the shared list): `GET /*path` → `handle_forward_get`, `.fallback(handle_fallback)`. If another plan has added `/logout` (audit F25) it goes in the auth group.

**Files:**
- Modify: `crates/tama-core/src/proxy/server/router.rs`

**What to implement:**

1. Replace both table bodies with:
   ```rust
   type ProxyRoute = (&'static str, &'static str, MethodRouter<Arc<ProxyState>>);

   /// The single source of truth for proxy-owned routes: OpenAI-compatible
   /// inference, model lifecycle, auth, and proxy ops. Management CRUD routes
   /// live in the `tama` crate's router (`crates/tama/src/router.rs`) — do NOT
   /// add them here. The ownership test in `crates/tama/tests/router_ownership_test.rs`
   /// enforces the boundary.
   fn proxy_routes() -> Vec<ProxyRoute> { vec![ /* the 31 entries from the table above, verbatim handler references */ ] }

   /// (method-label, path) pairs from `proxy_routes()` — for the cross-crate
   /// ownership test. Labels for multi-method routes are "GET+POST" style.
   pub fn proxy_route_paths() -> Vec<(&'static str, &'static str)> {
       proxy_routes().into_iter().map(|(m, p, _)| (m, p)).collect()
   }

   fn fold_proxy_routes() -> Router<Arc<ProxyState>> {
       let mut router = Router::new();
       for (_, path, method_router) in proxy_routes() {
           router = router.route(path, method_router);
       }
       router
   }

   fn apply_shared_layers(router: Router<Arc<ProxyState>>, state: Arc<ProxyState>) -> Router {
       router
           .layer(middleware::from_fn(scope_middleware))
           .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
           .with_state(state)
           .layer(CorsLayer::permissive())
   }
   ```
   `build_router(state)` = `apply_shared_layers(fold_proxy_routes().route("/*path", get(handle_forward_get)).fallback(handle_fallback), state)` — preserving today's standalone layer order (scope → auth → with_state → cors). `build_unified_router(state, extra)` = `apply_shared_layers(Router::new().merge(fold_proxy_routes()).merge(extra), state).layer(CatchPanicLayer::new())` — preserving merge order (proxy first) and the unified-only CatchPanic. Both keep their current signatures and `#[cfg]` gates (`build_unified_router` stays `#[cfg(feature = "web-ui")]`). Update the module's `use` block: drop `handle_tama_get_model as handle_tama_get_model_fn`, `handle_tama_list_models`, `handle_hf_list_quants`, `handle_tama_system_health`, `handle_all_logs`, `handle_backend_log_sse` from the `tama_handlers` import; add `axum::routing::MethodRouter`.
2. Update the module doc/`build_unified_router` doc comment: delete the "Routes that overlap with the web UI … intentionally excluded" NOTE block; state the new rule (proxy-exclusive only; management lives in `crates/tama/src/router.rs`; the ownership test enforces it).
3. Update `test_proxy_router_serves_known_routes` (router.rs tests): the third probe hits `/tama/v1/system/health` — that route MOVED. Replace it with a `/status` probe (200, same pattern as `/health`); keep `/health` and `/v1/models`. `test_unified_router_route_priority` needs NO changes (it probes load/unload/cancel + `/health` + `/v1/models` — all still core-owned).
4. Add the `// TODO(plan-172): unrouted after plan-178 — delete` comment above `handle_tama_list_models`/`handle_tama_get_model` (`tama_handlers/models/handlers.rs`) and `handle_hf_list_quants` (`tama_handlers/system.rs`) — comment only, do not delete code or their tests.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- proxy::server` — green baseline (incl. both router tests)
- [ ] Rewrite `router.rs` per above (single list + two thin builders + updated docs/imports/tests)
- [ ] Run `cargo check --package tama-core --features web-ui` AND `cargo check --package tama-core` (without the feature — `build_unified_router` is gated; both must compile)
- [ ] Run `cargo nextest run --package tama-core -- proxy::server` — updated router test passes; `test_unified_router_route_priority` unchanged-green
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — all pass (tama's production behavior is verified end-to-end in Task 4/5)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: single-source proxy route table; management routes owned by tama router"

**Acceptance criteria:**
- [ ] Exactly one route list in `router.rs`; `build_router`/`build_unified_router` are ≤ 15-line wrappers over it
- [ ] The 6 management routes are gone from core's table; zero "intentionally excluded" coordination comments remain
- [ ] Both feature configurations compile; all router/server tests pass
- [ ] `proxy_route_paths()` returns 31 (method, path) pairs

---

### Task 4: Mount the 3 moved routes in the `tama` router (fixes the `system/health` production shadow)

**Context:**
`build_web_routes` (`crates/tama/src/router.rs`) already owns all management CRUD. It must now explicitly mount the three core handlers whose routes were removed in Task 3 and are still part of the served API: `GET /tama/v1/system/health` (currently BROKEN in production — falls through to the SPA wildcard and returns HTML), `GET /tama/v1/logs`, and `GET /tama/v1/logs/:backend/events`. All three take `State<Arc<ProxyState>>`, which the web router already provides (`build_web_routes` returns `Router<Arc<ProxyState>>`). They are GET/SSE endpoints, so they join the no-CSRF root section next to the existing `/tama/v1/logs/:backend` route (:284). `GET /tama/v1/hf/*repo_id` is already served by tama's own `api::hf::hf_metadata` (:260) — nothing to do there.

**Files:**
- Modify: `crates/tama/src/router.rs`

**What to implement:**

1. Add to the imports:
   ```rust
   use tama_core::proxy::tama_handlers::{
       backend_logs::handle_all_logs, handle_backend_log_sse, handle_tama_system_health,
   };
   ```
2. In the root router (next to `/tama/v1/logs/:backend`, :284), add:
   ```rust
   // System health + backend logs: core proxy handlers mounted explicitly
   // (they are part of the management API surface but implemented in tama-core).
   .route("/tama/v1/system/health", get(handle_tama_system_health))
   .route("/tama/v1/logs", get(handle_all_logs))
   .route(
       "/tama/v1/logs/:backend/events",
       get(handle_backend_log_sse),
   )
   ```
3. Add a short ownership comment at the top of `build_web_routes`: "This table owns ALL `/tama/v1` management routes. Core's router (`tama_core::proxy::server::router`) owns only inference/lifecycle/auth — `crates/tama/tests/router_ownership_test.rs` asserts the two tables stay disjoint."

**Steps:**
- [ ] Add the import + 3 routes + ownership comment
- [ ] Run `cargo check --package tama`
- [ ] Run `cargo nextest run --package tama` — all pass (existing server_test.rs/config_structured_test.rs boot this router and must stay green)
- [ ] Manual smoke (optional but recommended): `cargo run` and `curl -s -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/tama/v1/system/health" | head -c 200` — JSON, not HTML
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: mount system/health + logs routes in tama router (was SPA-shadowed in unified mode)"

**Acceptance criteria:**
- [ ] `GET /tama/v1/system/health`, `GET /tama/v1/logs`, `GET /tama/v1/logs/:backend/events` are served by core handlers in the production unified router
- [ ] `rg "handle_tama_system_health|handle_all_logs|handle_backend_log_sse" crates/tama/src/router.rs` — 3 route entries
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 5: Router ownership test (no duplicate paths, no SPA shadows)

**Context:**
The F33 failure mode — adding a route to one crate's table and forgetting the other — needs a tripwire. Axum panics at merge time for exact path+method conflicts (loud), but silently shadows when one table has a path the other matches via wildcard (the `system/health` → SPA bug) and silently diverges when a management route is re-added to core. Decisions: an ssr integration test in `crates/tama/tests/` (the only place both tables are visible) with two halves: (a) static disjointness between `tama_core::proxy::server::router::proxy_route_paths()` (Task 3) and a hard-coded list of the `tama` table's `/tama/v1` paths — the hard-coded list is deliberate, with a maintenance comment: it is the tripwire that fires when EITHER table gains a route the other already owns; (b) behavioral probes against the real unified app asserting the sensitive endpoints answer with JSON, not SPA HTML.

**Files:**
- Create: `crates/tama/tests/router_ownership_test.rs`
- Modify: `crates/tama/Cargo.toml` (register the `[[test]]` target with `required-features = ["ssr"]`, same pattern as `server_test` at :70-73)

**What to implement:**

1. `crates/tama/tests/router_ownership_test.rs`:
   ```rust
   #![cfg(feature = "ssr")]

   use std::sync::Arc;

   /// Paths owned by the tama (web) route table under /tama/v1.
   /// MAINTENANCE: when you add a /tama/v1 route to `crates/tama/src/router.rs`,
   /// add its path here. This list is the tripwire that keeps core's proxy
   /// router and this crate's management router disjoint (audit F33).
   const TAMA_MANAGED_PATHS: &[&str] = &[
       "/tama/v1/system/capabilities",
       "/tama/v1/backends",
       "/tama/v1/backends/install",
       "/tama/v1/backends/:name/update",
       "/tama/v1/backends/:name",
       "/tama/v1/backends/:name/default-args",
       "/tama/v1/backends/:name/default-env",
       "/tama/v1/backends/:name/versions/:version",
       "/tama/v1/backends/check-updates",
       "/tama/v1/backends/:name/versions",
       "/tama/v1/backends/:name/activate",
       "/tama/v1/backends/:name/source",
       "/tama/v1/backends/jobs/:id",
       "/tama/v1/backends/jobs/:id/events",
       "/tama/v1/backends/compaction",
       "/tama/v1/restore/preview",
       "/tama/v1/restore",
       "/tama/v1/self-update/update",
       "/tama/v1/self-update/check",
       "/tama/v1/self-update/events",
       "/tama/v1/updates/check",
       "/tama/v1/updates/check/:item_type/:item_id",
       "/tama/v1/updates/events",
       "/tama/v1/updates/apply/backend/:name",
       "/tama/v1/updates/apply/model/:id",
       "/tama/v1/updates",
       "/tama/v1/config",
       "/tama/v1/config/structured",
       "/tama/v1/models",
       "/tama/v1/models/:id",
       "/tama/v1/models/:id/rename",
       "/tama/v1/models/:id/refresh",
       "/tama/v1/models/:id/verify",
       "/tama/v1/models/:id/quants/:quant_key",
       "/tama/v1/benchmarks/run",
       "/tama/v1/benchmarks/spec-run",
       "/tama/v1/benchmarks/mtp-run",
       "/tama/v1/benchmarks/jobs/:id",
       "/tama/v1/benchmarks/jobs/:id/events",
       "/tama/v1/benchmarks/history",
       "/tama/v1/benchmarks/history/:id",
       "/tama/v1/downloads/:job_id/cancel",
       "/tama/v1/downloads/active",
       "/tama/v1/downloads/history",
       "/tama/v1/downloads/events",
       "/tama/v1/aliases",
       "/tama/v1/aliases/:id",
       "/tama/v1/hf/*repo_id",
       "/tama/v1/docs",
       "/tama/v1/logs",
       "/tama/v1/logs/:backend",
       "/tama/v1/logs/:backend/events",
       "/tama/v1/system/health",
   ];

   /// Core's proxy table and tama's management table must be disjoint —
   /// a path in both is a shadow-route bug (audit F33).
   #[test]
   fn test_proxy_and_management_tables_are_disjoint() {
       let proxy_paths: std::collections::HashSet<&str> =
           tama_core::proxy::server::router::proxy_route_paths()
               .into_iter()
               .map(|(_, p)| p)
               .collect();
       for path in TAMA_MANAGED_PATHS {
           assert!(
               !proxy_paths.contains(path),
               "route {path} exists in BOTH routers — pick exactly one owner"
           );
       }
   }

   /// The unified (production) app must serve API routes as API responses,
   /// never as the SPA's index.html (the system/health shadow bug).
   #[tokio::test]
   async fn test_unified_app_serves_api_not_spa_html() {
       // Boot the production-style unified router (pattern from server_test.rs).
       let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
       let addr = listener.local_addr().unwrap();
       tokio::spawn(async move {
           let config = tama_core::config::Config::default();
           let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));
           let web_state = Arc::new(tama_web::web_types::WebState { /* same literal as server_test.rs::test_web_state */ });
           let web_routes = tama_web::router::build_web_routes(web_state);
           let server = tama_core::proxy::ProxyServer::new(state).await;
           let app = server.into_unified_router(web_routes).await;
           axum::serve(listener, app).await.unwrap();
       });
       tokio::time::sleep(std::time::Duration::from_millis(50)).await;
       let client = reqwest::Client::new();

       for path in ["/tama/v1/system/health", "/tama/v1/logs", "/tama/v1/models"] {
           let resp = client.get(format!("http://{addr}{path}")).send().await.unwrap();
           let content_type = resp
               .headers()
               .get(reqwest::header::CONTENT_TYPE)
               .and_then(|v| v.to_str().ok())
               .unwrap_or("")
               .to_string();
           assert!(
               !content_type.contains("text/html"),
               "GET {path} returned SPA HTML (shadowed by the web UI fallback); content-type: {content_type}"
           );
       }
   }
   ```
   Notes for the executing agent: `proxy_route_paths` must be re-exported visibly — import it as `tama_core::proxy::server::router::proxy_route_paths` (verify `server` and `router` modules are pub along that path; `proxy/mod.rs:11` has `pub mod server;` — check `server/mod.rs` re-exports `router` or import the full path; adjust the use statement to whatever compiles). The auth middleware may 401 these probes — that is FINE for this test: a 401 has a JSON body, not `text/html`, which is exactly the shadow-signature we assert against. Do NOT assert status codes except "not HTML". If `WebState` gains a `repository` field from plan-160, copy the current `test_web_state` literal from `server_test.rs` — do not invent fields.
2. Register in `Cargo.toml`:
   ```toml
   [[test]]
   name = "router_ownership_test"
   path = "tests/router_ownership_test.rs"
   required-features = ["ssr"]
   ```

**Steps:**
- [ ] Write the test file + Cargo.toml registration
- [ ] Run `cargo nextest run --package tama --test router_ownership_test` — both tests pass (if the disjointness test fails, a Task 3/4 edit double-owned a route — fix the router, not the test; if the SPA-shadow test fails on content-type, check whether the auth middleware sets HTML on 401 — it doesn't, it returns JSON per `auth.rs`)
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: router ownership — proxy/management tables disjoint, no SPA shadows"

**Acceptance criteria:**
- [ ] `test_proxy_and_management_tables_are_disjoint` passes over 53 tama-owned paths vs core's 31
- [ ] `test_unified_app_serves_api_not_spa_html` proves `/tama/v1/system/health`, `/tama/v1/logs`, `/tama/v1/models` no longer fall through to the SPA in the production router
- [ ] `cargo nextest run --workspace` passes; clippy clean
