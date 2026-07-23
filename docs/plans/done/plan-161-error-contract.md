# Error Contract Plan

**Goal:** Unify the management API on one error wire format — the nested `{"error":{"message","type"}}` shape from `crates/tama/src/api/error.rs` (canonical per `docs/api/errors.md`) — by migrating 54 flat sites in `crates/tama`, adding a shared structured-error helper in tama-core, aligning the third shape in the keys API, fixing the OpenAPI schema, and locking the shape in with per-module tests.

**Architecture:** Audit finding F4. Three wire shapes exist today: (1) the canonical nested shape produced by `error_response`/`error_body` in `crates/tama/src/api/error.rs`; (2) a flat `{"error":"..."}` at 54 sites in 12 files (verified with `rg -U -c 'json!\(\s*\{\s*"error"\s*:\s*[^\{]' crates/tama/src`); (3) a code-first `{"error":code,"message":msg}` from the private `json_error` in `crates/tama-core/src/proxy/tama_handlers/api_keys.rs:66`. The OpenAPI spec (`crates/tama/src/api/openapi.rs:645-648`) codifies the FLAT shape, contradicting both the helper and the docs. Decision: the nested shape wins everywhere; error `type` values follow the `docs/api/errors.md` table (404→`NotFoundError`, 400/422→`ValidationError`, 409→`ConflictError`, 503→`ServiceUnavailableError`, 500→no type field); tama-core gets one shared helper in `proxy/handlers/mod.rs` rather than hand-rolled `json!` blocks per handler.

**Tech Stack:** Rust, Axum, serde_json

---

### Task 1: Migrate the 54 flat error sites in `crates/tama` to `error_response`/`error_body`

**Context:**
54 sites in 12 files emit flat `{"error":"..."}` JSON (verified counts; re-verify with `rg -U -c 'json!\(\s*\{\s*"error"\s*:\s*[^\{]' crates/tama/src --type rust` before starting): `api/updates.rs` ×23 (never imports the helper), `api/backends/install.rs` ×6, `api/backends/manage/remove.rs` ×5, `api/backends/manage/activate.rs` ×5, `api/backends/list.rs` ×4, `api/hf.rs` ×3 (never imports the helper), `api/self_update.rs` ×2, `api/backends/manage/update.rs` ×2, `api/models/files.rs` ×1, `api/benchmarks/history.rs` ×1, `api/backends/jobs.rs` ×1, `api/backends/capabilities.rs` ×1. Decision: pure mechanical migration — messages and status codes stay byte-identical; only the body shape changes. The `type` field is assigned by STATUS CODE using the `docs/api/errors.md` table below, not by guessing per-site semantics. Two helper forms exist (error.rs:28-65): `error_response(status, message, error_type) -> Response` for handler returns, and `error_body(message, error_type) -> serde_json::Value` for closures returning `(StatusCode, serde_json::Value)` tuples (the `spawn_model_crud` pattern). Do NOT change any success-path code, any status code, or any message text.

**Files:**
- Modify: `crates/tama/src/api/updates.rs`
- Modify: `crates/tama/src/api/backends/install.rs`
- Modify: `crates/tama/src/api/backends/manage/remove.rs`
- Modify: `crates/tama/src/api/backends/manage/activate.rs`
- Modify: `crates/tama/src/api/backends/list.rs`
- Modify: `crates/tama/src/api/hf.rs`
- Modify: `crates/tama/src/api/self_update.rs`
- Modify: `crates/tama/src/api/backends/manage/update.rs`
- Modify: `crates/tama/src/api/models/files.rs`
- Modify: `crates/tama/src/api/benchmarks/history.rs`
- Modify: `crates/tama/src/api/backends/jobs.rs`
- Modify: `crates/tama/src/api/backends/capabilities.rs`

**What to implement:**

1. **Type mapping by status code** (apply uniformly at every site):
   - `StatusCode::NOT_FOUND` → `Some("NotFoundError")`
   - `StatusCode::BAD_REQUEST` / `UNPROCESSABLE_ENTITY` → `Some("ValidationError")`
   - `StatusCode::CONFLICT` → `Some("ConflictError")`
   - `StatusCode::SERVICE_UNAVAILABLE` → `Some("ServiceUnavailableError")`
   - `StatusCode::INTERNAL_SERVER_ERROR` / `BAD_GATEWAY` / anything else → `None`

2. **Replacement patterns:**
   - Handler-return position: `(StatusCode::X, Json(serde_json::json!({ "error": <expr> }))).into_response()` → `error_response(StatusCode::X, <expr>, <type>)`.
   - Early-return position: `return (StatusCode::X, Json(json!({ "error": <expr> }))).into_response();` → `return error_response(StatusCode::X, <expr>, <type>);`
   - Closure error position (tuple of `(StatusCode, serde_json::Value)`, e.g. inside `spawn_model_crud` closures): `(StatusCode::X, serde_json::json!({ "error": <expr> }))` → `(StatusCode::X, error_body(<expr>, <type>))`.
   - Multi-line `json!({ "error": format!(...) })` blocks collapse the same way — preserve the exact `format!` expression.

3. **Imports:** add `use crate::api::error::{error_body, error_response};` (or only the used one — clippy forbids unused imports) to files that lack it: `updates.rs`, `hf.rs`, `self_update.rs`, `manage/remove.rs`, `manage/activate.rs`, `manage/update.rs`, `models/files.rs`, `benchmarks/history.rs`, `backends/jobs.rs`, `backends/capabilities.rs`. `install.rs:11` and `list.rs:12` already import `error_response` — extend those `use` lines if `error_body` is also needed. After migration, check for now-unused `serde_json::json` imports (`Json` often stays for success bodies; `serde_json::json!` may become unused in files like `self_update.rs` — remove only if clippy flags it).

4. **Do NOT touch:** `crates/tama/src/api/error.rs` itself, the OpenAPI spec (Task 3), any tama-core file (Task 2), and any site that already uses the helper.

**Steps:**
- [ ] Run `rg -U -c 'json!\(\s*\{\s*"error"\s*:\s*[^\{]' crates/tama/src --type rust` — record the baseline (54 sites / 12 files)
- [ ] Migrate `api/updates.rs` (23 sites) first — it is the worst offender and has an existing `#[cfg(test)]` module to catch regressions
- [ ] Run `cargo nextest run --package tama -- api::updates` — pass
- [ ] Migrate the remaining 11 files (backends cluster, hf.rs, self_update.rs, models/files.rs, benchmarks/history.rs)
- [ ] Run the rg command again — zero hits in `crates/tama/src`
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: migrate flat error bodies to the canonical nested error shape"

**Acceptance criteria:**
- [ ] `rg -U -c 'json!\(\s*\{\s*"error"\s*:\s*[^\{]' crates/tama/src --type rust` returns zero hits
- [ ] Every migrated site keeps its original status code and message text; `type` values follow the status-code table
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 2: Shared structured-error helper in tama-core; migrate tts.rs, pull/handlers.rs, api_keys.rs

**Context:**
tama-core handlers hand-roll error JSON: `crates/tama-core/src/proxy/handlers/tts.rs` has 17 verbatim `json!({"error":{"message":…,"type":…}})` blocks (types: `NotFoundError` ×4, `ServerError` ×13), and `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs` has 12 error sites — 10 nested (`UpstreamError` ×3, `ValidationError` ×6, `NotFoundError` ×1) plus 2 FLAT ones at lines 42–44 and 165–167. Separately, `crates/tama-core/src/proxy/tama_handlers/api_keys.rs:66` defines a private `json_error(status, error_code, message)` emitting the third shape `{"error":"not_found","message":"…"}` at 22 call sites (codes in use: `not_found` ×3, `forbidden` ×1, `invalid_request` ×1, `internal_error` ×17). `proxy/handlers/mod.rs:16` already has `json_error_response()` — but it is a zero-arg helper returning a FIXED 400 "Request body too large" body, used by `chat.rs:71,86,124` and `compaction.rs:119,126`. Decision: add a general `json_error(status, message, error_type)` in `proxy/handlers/mod.rs`, reimplement `json_error_response()` in terms of it (behavior identical), migrate tts.rs and pull/handlers.rs mechanically (preserve each site's exact `type` string), and align api_keys.rs by deleting its private helper and mapping its snake_case codes to canonical types: `not_found`→`Some("NotFoundError")`, `forbidden`→`Some("ForbiddenError")`, `invalid_request`→`Some("ValidationError")`, `internal_error`→`None`. This CHANGES the keys API wire shape from `{"error":code,"message":msg}` to `{"error":{"message":msg,"type":Type}}` — that is the intended alignment; the Leptos UI does not structurally parse error bodies (request helpers in `crates/tama/src/utils/mod.rs` use gloo builders and surface status/text), but verify with `rg '"error"' crates/tama/src/pages/keys/ crates/tama/src/utils/` before finalizing.

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs`
- Modify: `crates/tama-core/src/proxy/handlers/tts.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/api_keys.rs`

**What to implement:**

1. **New helper** in `crates/tama-core/src/proxy/handlers/mod.rs`, directly above the existing `json_error_response`:
   ```rust
   /// Build a structured JSON error response:
   /// `{"error": {"message": "...", "type": "..."}}` (type omitted when None).
   pub fn json_error(
       status: StatusCode,
       message: impl Into<String>,
       error_type: Option<&str>,
   ) -> Response {
       let mut detail = serde_json::Map::new();
       detail.insert("message".to_string(), serde_json::Value::String(message.into()));
       if let Some(ty) = error_type {
           detail.insert("type".to_string(), serde_json::Value::String(ty.to_string()));
       }
       let mut body = serde_json::Map::new();
       body.insert("error".to_string(), serde_json::Value::Object(detail));
       (status, Json(serde_json::Value::Object(body))).into_response()
   }
   ```
   Reimplement the existing `json_error_response()` as `json_error(StatusCode::BAD_REQUEST, "Request body too large", Some("BadRequestError"))` — its wire output must stay byte-identical (chat.rs/compaction.rs callers unchanged).
   Note: `crates/tama/src/api/error.rs::error_response` builds the same shape via serde structs — this duplication is intentional (tama-core must not depend on the `tama` crate); keep both.

2. **Export** in `crates/tama-core/src/proxy/mod.rs:23`: change `pub use handlers::json_error_response;` to `pub use handlers::{json_error, json_error_response};`.

3. **Migrate `tts.rs`** (17 sites): each `Json(serde_json::json!({"error": {"message": <msg>, "type": "<T>"}}))` tuple becomes `json_error(<status>, <msg>, Some("<T>"))` with the SAME status and type string. Import: `use super::json_error;` (tts.rs sits in `proxy/handlers/`). Do not touch success paths.

4. **Migrate `pull/handlers.rs`** (12 sites): the 10 nested sites → `crate::proxy::handlers::json_error(...)` preserving type strings; the 2 flat sites (lines 42–44 "Too many files requested…" 400, and 165–167 "Too many quants requested…" 400) → `json_error(StatusCode::BAD_REQUEST, format!(…), Some("ValidationError"))`. pull/handlers.rs imports `reqwest::StatusCode` (line 7) — the helper takes `axum::http::StatusCode`; these are the SAME type re-exported (`reqwest::StatusCode` IS `http::StatusCode`), so no conversion is needed, but do not add a conflicting import — call the helper as `crate::proxy::handlers::json_error(...)`.

5. **Align `api_keys.rs`:** delete the private `fn json_error` (lines 64–75). Rewrite each of the 22 call sites: `json_error(StatusCode::NOT_FOUND, "not_found", "key not found")` → `crate::proxy::handlers::json_error(StatusCode::NOT_FOUND, "key not found", Some("NotFoundError"))`, applying the code map: `not_found`→`Some("NotFoundError")`, `forbidden`→`Some("ForbiddenError")`, `invalid_request`→`Some("ValidationError")`, `internal_error`→`None`. NOTE the parameter order changes from (status, code, message) to (status, message, type) — do not blindly swap; rewrite each call. The existing tests at the bottom of api_keys.rs (e.g. asserting on error bodies — check `rg '"error"' crates/tama-core/src/proxy/tama_handlers/api_keys.rs` in the test module) must be updated to the nested shape: e.g. a body assertion `body["error"] == "not_found"` becomes `body["error"]["type"] == "NotFoundError"` and `body["error"]["message"] == "key not found"`.

**Steps:**
- [ ] Write failing tests first: add shape assertions to the existing test modules — in `api_keys.rs` tests assert one 404 body deserializes as `{"error":{"message":"key not found","type":"NotFoundError"}}`; in a new `#[cfg(test)]` block in `proxy/handlers/mod.rs` assert `json_error_response()` still yields `{"error":{"message":"Request body too large","type":"BadRequestError"}}` and `json_error(StatusCode::NOT_FOUND, "x", None)` yields `{"error":{"message":"x"}}` (no `type` key)
- [ ] Run `cargo nextest run --package tama-core -- proxy` — verify the api_keys shape test fails (old shape) and the helper tests fail (helper doesn't exist)
- [ ] Implement the helper + re-export, migrate tts.rs, pull/handlers.rs, api_keys.rs
- [ ] Run `rg -U 'json!\(\s*\{\s*"error"' crates/tama-core/src/proxy/handlers/tts.rs crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs crates/tama-core/src/proxy/tama_handlers/api_keys.rs` — zero hits
- [ ] Run `cargo nextest run --package tama-core` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: unify tama-core error bodies behind proxy::handlers::json_error"

**Acceptance criteria:**
- [ ] `proxy::handlers::json_error(status, message, type)` exists and is re-exported from `tama_core::proxy`; `json_error_response()` is implemented via it with identical output
- [ ] tts.rs (17), pull/handlers.rs (12), api_keys.rs (22) error sites all go through the helper; the private `json_error` in api_keys.rs is deleted
- [ ] Keys API emits the nested shape with the mapped type values — proven by updated tests
- [ ] `cargo nextest run --package tama-core` passes; clippy clean

---

### Task 3: Fix the `ErrorResponse` schema in openapi.rs to the nested shape

**Context:**
`crates/tama/src/api/openapi.rs:645-648` registers the `ErrorResponse` schema as `{"type":"object","required":["error"],"properties":{"error":{"type":"string"}}}` — the FLAT shape — contradicting `crates/tama/src/api/error.rs` and `docs/api/errors.md`. After Tasks 1–2 every endpoint emits the nested shape, so the spec becomes honest by changing exactly this one schema entry. Decision: match the serde implementation in error.rs precisely — `message` is required, `type` is optional (`skip_serializing_if = "Option::is_none"`), so `required` lists only `message`. Do not regenerate or restructure anything else in the 1,123-line spec (F19 owns the broader spec-drift problem).

**Files:**
- Modify: `crates/tama/src/api/openapi.rs`

**What to implement:**

1. Replace the `ErrorResponse` schema entry (openapi.rs:645-648) with:
   ```rust
   map.insert(
       "ErrorResponse".into(),
       serde_json::json!({
           "type": "object",
           "required": ["error"],
           "properties": {
               "error": {
                   "type": "object",
                   "required": ["message"],
                   "properties": {
                       "message": {"type": "string"},
                       "type": {"type": "string"}
                   }
               }
           }
       }),
   );
   ```
2. Check whether `docs/api/errors.md` shows an example schema — it shows the JSON structure only (verified lines 1–12); no doc edit needed. Do NOT touch any other schema entry.

**Steps:**
- [ ] Write a failing test: in `crates/tama/src/api/openapi.rs` add `#[cfg(test)] mod tests` (check whether one exists first — `rg "mod tests" crates/tama/src/api/openapi.rs`) with `test_error_response_schema_is_nested`: call the function that builds the spec (`schemas()` is private — test within the same module so it is reachable), navigate `spec["ErrorResponse"]["properties"]["error"]["properties"]["message"]["type"]` and assert `== "string"`, and assert `spec["ErrorResponse"]["properties"]["error"]["type"] == "object"`
- [ ] Run `cargo nextest run --package tama -- api::openapi` — verify it fails (current schema has `"error": {"type": "string"}`)
- [ ] Apply the schema fix
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: align OpenAPI ErrorResponse schema with the nested error shape"

**Acceptance criteria:**
- [ ] The `ErrorResponse` schema describes `{"error":{"message":string,"type":string?}}` with `message` required
- [ ] `GET /tama/v1/docs` output validates against an actual error body from `error_response` (asserted in the test)
- [ ] `cargo nextest run --package tama` passes; clippy clean

---

### Task 4: Shape-assertion tests per API module

**Context:**
Tasks 1–3 migrate the code but nothing prevents the next handler from re-introducing a flat body. Decision: make `ErrorResponse`/`ErrorDetail` in `crates/tama/src/api/error.rs` deserializable (add `#[derive(Deserialize)]` — currently only `Serialize`; additive, no wire change) and add one route-level shape test per migrated module that triggers an error path and deserializes the body into `ErrorResponse`. For tama-core, the Task-2 tests in `api_keys.rs` and `proxy/handlers/mod.rs` already cover the helper and the keys handlers. Do NOT try to cover all 54 sites — one representative error path per file, preferring the path that needs the lightest fixture (validation errors that fire before any DB access).

**Files:**
- Modify: `crates/tama/src/api/error.rs`
- Modify: `crates/tama/src/api/updates.rs` (tests)
- Modify: `crates/tama/src/api/hf.rs` (tests — new `#[cfg(test)] mod tests`)
- Modify: `crates/tama/src/api/backends/manage/tests.rs`
- Modify: `crates/tama/src/api/backends/jobs.rs` (tests — new `#[cfg(test)] mod tests`)
- Modify: `crates/tama/src/api/benchmarks/history.rs` (tests — new `#[cfg(test)] mod tests`)

**What to implement:**

1. In `crates/tama/src/api/error.rs`: add `Deserialize` to the derives of `ErrorResponse` and `ErrorDetail` (`use serde::{Deserialize, Serialize};`). Add a `#[cfg(test)] pub(crate) mod tests` with a shared assertion helper other test modules reuse:
   ```rust
   /// Assert a response body matches the canonical nested error shape and
   /// return the parsed detail for further assertions.
   pub(crate) async fn assert_error_shape(response: axum::response::Response) -> ErrorDetail {
       let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
           .await
           .unwrap();
       let parsed: ErrorResponse = serde_json::from_slice(&bytes)
           .unwrap_or_else(|e| panic!("body is not the canonical error shape: {e}; body={}", String::from_utf8_lossy(&bytes)));
       assert!(!parsed.error.message.is_empty());
       parsed.error
   }
   ```
   (ErrorDetail needs `pub` visibility of fields — it already has pub fields; the test module is `pub(crate)` so sibling api modules can use the helper as `crate::api::error::tests::assert_error_shape`.)

2. **Per-module tests** (build routers by mounting the handler directly with `Router::new().route(...)` + `.with_state(Arc<ProxyState>)` + `Extension<WebState>` where the handler needs it — the `tower::ServiceExt::oneshot` pattern from `crates/tama/src/api/backends/manage/tests.rs`; do NOT mount the CSRF middleware, these tests target the error body):
   - `updates.rs` (existing test module): `check_single` with `item_type = "bogus"` → 400 — the validation at updates.rs:322 fires before any DB access; assert status + `assert_error_shape` + `detail.r#type == Some("ValidationError")`.
   - `hf.rs` (new test module): `hf_metadata` with a repo id failing its validator (hf.rs:27-29, e.g. `"../evil"` — check the exact rule first) → 400 → shape + `ValidationError`.
   - `backends/manage/tests.rs` (existing): extend `test_update_backend_path_traversal_rejected`-style coverage — add `test_remove_backend_error_shape`: DELETE `/tama/v1/backends/..%2Fevil` (or the traversal form the handler rejects) → shape assertion; and one activate-shape test. Follow the file's existing CSRF-cookie+header request-building pattern (it DOES exercise the full router — that is fine and already set up).
   - `backends/jobs.rs` (new test module): `get_job` with a `WebState` whose `jobs` manager has no such job id → the handler's error path (check its actual not-found behavior — if it returns a flat/`"Jobs not configured"` string body instead of JSON, that is ALSO a contract violation: migrate it to `error_response` as part of this test and note the extra fix in the commit).
   - `benchmarks/history.rs` (new test module): `get_benchmark_result` for a nonexistent job id with an empty `WebState.jobs` → error path → shape assertion.
   - For the remaining migrated files without an obvious no-DB error path (`install.rs`, `list.rs`, `self_update.rs`, `manage/update.rs`, `models/files.rs`, `capabilities.rs`): rely on the manage/tests.rs coverage plus one rule — if you find ANY of these files returning a non-shaped body while writing the above tests, migrate it immediately; otherwise leave their coverage to plan-level regression tests (do not build heavy fixtures just for shape).

3. Also add a compile-time guard: in `error.rs` tests, a static assertion test that `ErrorResponse` serializes to `{"error":{"message":...}}` without `type` when `None` (locks the `skip_serializing_if` behavior the OpenAPI schema now documents).

**Steps:**
- [ ] Add `Deserialize` derives + the test module with `assert_error_shape` in `crates/tama/src/api/error.rs`
- [ ] Write the five per-module tests (updates, hf, manage, jobs, benchmarks/history)
- [ ] Run `cargo nextest run --package tama -- api::` — all pass; if a test exposes a non-shaped body (jobs.rs is the prime suspect), migrate that handler to `error_response` in this commit
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo nextest run --package tama-core` — confirm Task-2 tests still green
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: assert canonical error shape per API module"

**Acceptance criteria:**
- [ ] `ErrorResponse`/`ErrorDetail` derive `Deserialize`; `assert_error_shape` helper exists and is used by ≥5 test modules
- [ ] Each new test deserializes a real handler error body into `ErrorResponse` (fails on any future flat regression)
- [ ] Any additionally-discovered non-shaped bodies (e.g. jobs.rs) are migrated to `error_response` in the same commit
- [ ] `cargo nextest run --workspace` passes; clippy clean
