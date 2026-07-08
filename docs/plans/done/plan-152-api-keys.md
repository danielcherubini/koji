# API Keys Plan

**Goal:** Add named, scoped API keys (`tama_XXXX`) stored as SHA-256 hashes in SQLite, with three scopes (`inference`, `management:read`, `management:write`) that coexist with OAuth2/Authentik auth.

**Architecture:** Two-layer middleware — `auth_middleware` answers "who are you?" (attaches `AuthSubject` to request), `scope_middleware` answers "what can you do?" (checks scopes against route). OAuth2 users bypass scope checks; API keys are enforced.

**Tech Stack:** SQLite migration, rusqlite, SHA-256 (via `sha2` crate), `subtle` for constant-time comparison, base62 random generation, axum middleware layers.

---

### Task 1: Database migration, types, and key utilities

**Context:**
This task establishes the foundation — the DB table, config flag, Rust types, and key generation/hashing utilities. All subsequent tasks depend on these types and DB queries. This is purely internal plumbing with no handler or middleware changes.

**Files:**
- Create: `crates/tama-core/src/db/migrations/_0036_create_api_keys.rs`
- Create: `crates/tama-core/src/proxy/api_keys.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (add `mod api_keys` + `pub use api_keys::{AuthSubject, Scope, ApiKeyRecord}`)
- Modify: `crates/tama-core/src/db/migrations.rs` (add `mod _0036_create_api_keys;`, append `_0036_create_api_keys::MIGRATION` to array, bump `LATEST_VERSION` to 36)
- Modify: `crates/tama-core/Cargo.toml` (add `subtle = "2"` — `sha2 = "0.10"` already present, `rand = "0.9"` already in workspace for randomness)

**What to implement:**

1. **Migration `_0036_create_api_keys.rs`:**
   ```sql
   CREATE TABLE api_keys (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       name TEXT NOT NULL DEFAULT '',
       key_prefix TEXT NOT NULL,
       key_hash TEXT NOT NULL UNIQUE,
       scopes TEXT NOT NULL,
       created_by TEXT NOT NULL DEFAULT '',
       created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
       last_used_at TEXT,
       revoked_at TEXT,
       expires_at TEXT
   );
   CREATE INDEX idx_api_keys_active_created ON api_keys (revoked_at, created_at DESC);
   ALTER TABLE app_proxy ADD COLUMN api_keys_enabled INTEGER NOT NULL DEFAULT 0;
   ```

2. **Types in `api_keys.rs`:**
   ```rust
   /// Scopes that can be assigned to API keys.
   #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
   #[serde(rename_all = "kebab-case")]
   pub enum Scope {
       Inference,
       ManagementRead,
       ManagementWrite,
   }

   /// The authenticated identity attached to requests.
   /// Send + Sync + 'static are automatic (contains only String, i64, Vec).
   #[derive(Debug, Clone)]
   pub enum AuthSubject {
       User { username: String },
       Key { key_id: i64, scopes: Vec<Scope> },
   }
   ```

3. **Key generation:**
   - `pub fn generate_key() -> String` — returns `tama_` + 32 chars of base62 (`a-zA-Z0-9`) random. Use existing workspace `rand = "0.9"`: `rand::Rng::sample_iter(&mut rand::rng(), rand::distr::Alphanumeric).take(32).collect()`. No new dependency needed.
   - `pub fn hash_key(key: &str) -> String` — SHA-256 of the exact bytes of the full key (including `tama_` prefix), hex-encoded. Uses existing `sha2 = "0.10"` dep.
   - `pub fn extract_prefix(key: &str) -> String` — returns first 8 chars of the random portion (after `tama_`), prefixed with `tama_` for display (e.g. `tama_aB3dEfGh`).

4. **DB queries (all take `&rusqlite::Connection`):**
   - `pub fn validate_key(conn: &Connection, raw_key: &str) -> Result<Option<(i64, Vec<Scope>)>, anyhow::Error>` — hashes the key, looks up by hash, checks `revoked_at IS NULL` and `expires_at` (if set, must be in the future). Uses constant-time comparison via `subtle::ConstantTimeEq` on the hash string bytes. Returns `(key_id, scopes)`. Also updates `last_used_at` on success.
   - `pub fn create_key(conn: &Connection, name: &str, raw_key: &str, scopes: &[Scope], created_by: &str, expires_at: Option<&str>) -> Result<i64, anyhow::Error>` — inserts a new row. Validates scopes are non-empty and known values. Sets `api_keys_enabled = 1` on `app_proxy`.
   - `pub fn list_keys(conn: &Connection) -> Result<Vec<ApiKeyRecord>, anyhow::Error>` — returns all keys (active and revoked) ordered by `created_at DESC`.
   - `pub fn revoke_key(conn: &Connection, key_id: i64) -> Result<(), anyhow::Error>` — sets `revoked_at = now()`. If this was the last active key, sets `api_keys_enabled = 0` on `app_proxy`.
   - `pub fn update_key_scopes(conn: &Connection, key_id: i64, scopes: &[Scope]) -> Result<(), anyhow::Error>` — updates scopes on an existing key. Validates scopes.
   - `pub fn get_key(conn: &Connection, key_id: i64) -> Result<Option<ApiKeyRecord>, anyhow::Error>` — single key lookup.

5. **`ApiKeyRecord` struct** (for list/get responses):
   ```rust
   #[derive(Debug, Clone, Serialize)]
   pub struct ApiKeyRecord {
       pub id: i64,
       pub name: String,
       pub key_prefix: String,
       pub scopes: Vec<Scope>,
       pub created_by: String,
       pub created_at: String,
       pub last_used_at: Option<String>,
       pub revoked_at: Option<String>,
       pub expires_at: Option<String>,
   }
   ```

6. **Register migration** in `db/migrations.rs` — follow the existing pattern:
   - Add `mod _0036_create_api_keys;` to the module declarations
   - The migration file exports `pub const MIGRATION: (i32, bool, &str) = (36, false, r#"...SQL..."#);`
   - Append `_0036_create_api_keys::MIGRATION` to the `MIGRATIONS` array
   - Bump `LATEST_VERSION` from 35 to 36

7. **Export from `proxy/mod.rs`** — `pub mod api_keys;` and re-export `AuthSubject`, `Scope`, `ApiKeyRecord`.

**Steps:**
- [ ] Create `api_keys.rs` with `#[cfg(test)] mod tests` and a test helper: `fn test_conn() -> Connection { crate::db::open_in_memory().unwrap().conn }`
- [ ] Write failing test: `test_generate_key_format` — verify key starts with `tama_`, is 37 chars total (5 prefix + 32 random), contains only base62 chars
- [ ] Run `cargo nextest run --package tama-core -- proxy::api_keys::tests::test_generate_key_format`
  - Did it fail with compilation error (module doesn't exist or function missing)? If not, investigate.
- [ ] Implement `generate_key()`, `hash_key()`, `extract_prefix()` in `api_keys.rs`
- [ ] Run `cargo nextest run --package tama-core -- proxy::api_keys::tests::test_generate_key_format`
  - Did it pass? If not, fix and re-run.
- [ ] Write test: `test_hash_key_deterministic` — same input produces same hash, different input produces different hash, hash is 64 hex chars
- [ ] Write test: `test_extract_prefix` — `tama_aB3dEfGhIjKl...` → `tama_aB3dEfGh`
- [ ] Implement DB schema types (`Scope`, `AuthSubject`, `ApiKeyRecord`) in `api_keys.rs`
- [ ] Create migration file `_0036_create_api_keys.rs` with `pub const MIGRATION: (i32, bool, &str) = ...`
- [ ] Register migration in `db/migrations.rs` (add mod, append to MIGRATIONS array, bump LATEST_VERSION to 36)
- [ ] Implement `validate_key()` with constant-time comparison (`subtle::ConstantTimeEq` on hash bytes)
- [ ] Implement all DB query functions (`create_key`, `list_keys`, `revoke_key`, `update_key_scopes`, `get_key`)
- [ ] Write test: `test_create_and_validate_key_roundtrip` — create key, validate with raw key, get back same key_id and scopes
- [ ] Write test: `test_validate_revoked_key_returns_none` — create, revoke, validate → None
- [ ] Write test: `test_validate_expired_key_returns_none` — create with past expires_at, validate → None
- [ ] Write test: `test_api_keys_enabled_flag_toggled` — create first key → flag=1, revoke last key → flag=0
- [ ] Write test: `test_scope_validation_rejects_empty` — create with empty scopes → error
- [ ] Write test: `test_scope_validation_rejects_unknown` — create with unknown scope → error
- [ ] Run `cargo nextest run --package tama-core -- proxy::api_keys`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add API key database schema, types, and utilities"

**Acceptance criteria:**
- [ ] `generate_key()` produces `tama_` + 32 base62 chars
- [ ] `hash_key()` produces deterministic 64-char hex SHA-256
- [ ] `validate_key()` returns None for revoked/expired/nonexistent keys
- [ ] `validate_key()` uses constant-time comparison for hash
- [ ] `create_key()` sets `api_keys_enabled = 1` on app_proxy
- [ ] `revoke_key()` sets `api_keys_enabled = 0` when last active key is revoked
- [ ] Scopes validated at creation (non-empty, known values only)
- [ ] All tests pass, clippy clean, fmt clean

---

### Task 2: Auth middleware integration — `tama_` prefix check + `AuthSubject`

**Context:**
The existing `auth_middleware` in `proxy/auth.rs` needs to recognize `tama_` prefixed bearer tokens and validate them against the DB. On success, it attaches `AuthSubject::Key` to the request. On failure, it falls through to existing auth (session cookie, Authentik, Caddy). The middleware also needs to know about the `api_keys_enabled` config flag for the "auth configured" check.

**Files:**
- Modify: `crates/tama-core/src/proxy/auth.rs`
- Modify: `crates/tama-core/src/config/types/proxy.rs` (add `api_keys_enabled: bool` to `ProxyConfig` + Default)
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs` (add `api_keys_enabled` to `ProxyRecord`, `upsert_proxy`, `get_proxy`, `seed_defaults`)
- Modify: `crates/tama-core/src/config/types/mod.rs` (add `api_keys_enabled` to `Config::from_db` ProxyConfig construction, and `Config::to_db` upsert_proxy call)
- Modify: `crates/tama-core/src/proxy/mod.rs` (re-export `AuthSubject`, `Scope` from `api_keys`)

**DB access from async middleware:**
`ProxyState::open_db()` opens a fresh `rusqlite::Connection` per call (not Sync). All DB calls from the async `auth_middleware` must be wrapped in `tokio::task::spawn_blocking(move || { ... })` to avoid blocking the tokio runtime. This pattern is already used elsewhere in the codebase for DB access from async contexts.

**What to implement:**

1. **Add `api_keys_enabled` to config:**
   - Add `pub api_keys_enabled: bool` to `ProxyConfig` in `config/types/proxy.rs`
   - Default: `false`
   - Add to `Default` impl
   - Add to `resolve_env_vars()` — no env var resolution needed (it's a boolean from DB)

2. **Add `api_keys_enabled` to DB queries:**
   - Add `pub api_keys_enabled: bool` to `ProxyRecord` in `app_config_queries.rs`
   - Add to `upsert_proxy()` parameter and SQL
   - Add to `get_proxy()` SELECT and deserialization
   - Add to `seed_defaults()` — default `0`

3. **Auth middleware changes in `auth.rs`:**
   - Import `AuthSubject`, `Scope`, and `validate_key` from `crate::proxy::api_keys`
   - Modify `auth_middleware` flow:
     ```
     1. Skip paths → pass through (no AuthSubject attached)
     2. Session cookie (OIDC) → req.extensions_mut().insert(AuthSubject::User), pass
     3. Bearer token starts with "tama_" (case-sensitive, exact lowercase):
        a. If api_keys_enabled is false → 401
        b. spawn_blocking(move || { let conn = state.open_db().unwrap(); validate_key(&conn, &raw_token) })
        c. If found → req.extensions_mut().insert(AuthSubject::Key), pass
        d. If not found → 401 with JSON {"error": "unauthorized", "message": "invalid API key"}
     4. Other bearer token (only if authenticator_url set) → validate against Authentik → AuthSubject::User or 401
     5. Caddy X-Authentik-Username header → AuthSubject::User, pass
     6. No valid auth:
        a. If auth_configured → 401 JSON (or redirect to /login for browser + OAuth2)
        b. If not auth_configured → pass through (open mode)
     ```
   - `auth_configured = oauth2_enabled || authenticator_url.is_some() || api_keys_enabled`

4. **How to attach/retrieve AuthSubject to request:**
   - Attach: `req.extensions_mut().insert(subject)` before each `next.run(req).await`
   - Retrieve (in scope_middleware): `req.extensions().get::<AuthSubject>().cloned()`
   - `AuthSubject` is `Clone + Send + Sync + 'static` (contains only String, i64, Vec)

5. **Audit logging:**
   - On successful key validation: `tracing::info!(key_id, key_prefix, remote_addr, "API key authenticated")`
   - On failed key validation: `tracing::warn!(key_prefix_attempted, remote_addr, reason, "API key validation failed")`
   - Never log the plaintext key

**Steps:**
- [ ] Add `api_keys_enabled` to `ProxyConfig`, `ProxyRecord`, `upsert_proxy`, `get_proxy`, `seed_defaults`
- [ ] Run `cargo build --package tama-core` — verify config changes compile
- [ ] Modify `auth_middleware` to add `tama_` prefix check (step 3 in flow above)
- [ ] Write test: `test_tama_key_auth_passes` — mock DB returns valid key, bearer `tama_XXXX` → 200, AuthSubject::Key attached
- [ ] Write test: `test_tama_key_auth_invalid_returns_401` — mock DB returns None → 401 JSON
- [ ] Write test: `test_tama_key_disabled_returns_401` — `api_keys_enabled = false`, `tama_XXXX` → 401
- [ ] Write test: `test_non_tama_bearer_still_validates_authentik` — non-`tama_` bearer → goes to Authentik (mock)
- [ ] Write test: `test_auth_not_configured_open_mode` — none of oauth2/authentik/api_keys → pass through
- [ ] Write test: `test_auth_configured_with_api_keys_only` — only `api_keys_enabled = true`, no auth → 401
- [ ] Write test: `test_session_cookie_still_works` — existing session cookie auth unchanged
- [ ] Write test: `test_tama_prefix_case_sensitive` — `Tama_XXXX` → does NOT match as API key, falls through to other auth
- [ ] Run `cargo nextest run --package tama-core -- proxy::auth::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: integrate API key validation into auth middleware"

**Acceptance criteria:**
- [ ] `tama_` bearer tokens validated against DB (case-sensitive prefix)
- [ ] `AuthSubject::Key` attached on success, `AuthSubject::User` for OAuth2/Authentik
- [ ] Non-`tama_` bearer tokens still go to Authentik
- [ ] `api_keys_enabled = false` rejects `tama_` tokens
- [ ] Open mode (no auth configured) passes through
- [ ] Session cookie auth unchanged
- [ ] Audit logging for key validation (success/failure, no plaintext)
- [ ] All existing auth tests still pass

---

### Task 3: Scope middleware

**Context:**
A new middleware layer that runs after `auth_middleware` and enforces scope-based authorization. It reads `AuthSubject` from request extensions and checks scopes against the route pattern. OAuth2 users (`AuthSubject::User`) always bypass. API keys (`AuthSubject::Key`) must have the required scope for the route.

**Files:**
- Create: `crates/tama-core/src/proxy/scope_middleware.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (add `mod scope_middleware`)
- Modify: `crates/tama-core/src/proxy/server/router.rs` (apply scope middleware to route groups)

**What to implement:**

1. **`scope_middleware.rs`:**
   ```rust
   pub async fn scope_middleware(
       req: Request,
       next: Next,
   ) -> Response {
       // 1. Extract AuthSubject from request extensions
       // 2. If AuthSubject::User → bypass (full access)
       // 3. If AuthSubject::Key → check scopes against path + method:
       //    - /v1/* → requires "inference"
       //    - GET /tama/v1/* → requires "management:read"
       //    - POST/PUT/DELETE /tama/v1/* → requires "management:write"
       //    - Missing scope → 403 JSON
       // 4. If no AuthSubject (skip path that somehow reached here) → pass through
   }
   ```

   The 403 response body:
   ```json
   {
     "error": "forbidden",
     "message": "missing required scope: management:write",
     "required_scope": "management:write"
   }
   ```

2. **Router changes in `router.rs`:**
   - Apply `scope_middleware` as a layer to the routes it protects
   - The middleware should be applied AFTER `auth_middleware` (auth first, then authorization)
   - Routes that need scope checking: `/v1/*` (inference) and `/tama/v1/*` (management)
   - Routes that should NOT have scope checking: `/health`, `/metrics`, `/login*` (these are skip paths that bypass auth entirely)
   - In the unified router (`build_unified_router`), the scope middleware is already applied globally via the `.layer()` call — ensure it's ordered after auth

   The layer ordering in the router should be:
   ```rust
   Router::new()
       .route(...)
       .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
       .layer(middleware::from_fn(scope_middleware))
       .with_state(state)
   ```

3. **Helper function for determining required scope:**
   ```rust
   fn required_scope(path: &str, method: &Method) -> Option<Scope> {
       // All /v1/* routes (chat, models, audio, compaction, opencode, forward wildcards)
       // require the "inference" scope. This is intentional — any /v1/ consumer
       // needs an inference-scoped key.
       if path.starts_with("/v1/") || path == "/v1" {
           return Some(Scope::Inference);
       }
       if path.starts_with("/tama/v1/") {
           return if matches!(method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE) {
               Some(Scope::ManagementWrite)
           } else {
               Some(Scope::ManagementRead)
           };
       }
       None // No scope required for other paths (forwarded llama.cpp routes, etc.)
   }
   ```
   **Scope routing note:** `/v1/audio/*` (TTS), `/v1/compaction`, `/v1/opencode/models` all start with `/v1/` and require `inference` scope. This is intentional — any API key calling inference-adjacent endpoints needs the `inference` scope. Forwarded llama.cpp routes (e.g., `/completion`, `/tokenize`) do NOT start with `/v1/` or `/tama/v1/`, so they return `None` and pass through without scope checks.

**Steps:**
- [ ] Write failing test: `test_scope_middleware_key_without_inference_rejected` — Key with `management:read` scope → POST /v1/chat/completions → 403
- [ ] Run `cargo nextest run --package tama-core -- proxy::scope_middleware::tests`
  - Did it fail? If not, investigate.
- [ ] Implement `scope_middleware` and `required_scope` helper
- [ ] Write test: `test_scope_middleware_user_bypasses` — AuthSubject::User → any route → 200
- [ ] Write test: `test_scope_middleware_key_with_inference_passes` — Key with `inference` → POST /v1/chat/completions → 200
- [ ] Write test: `test_scope_middleware_key_management_read_get_passes` — Key with `management:read` → GET /tama/v1/models → 200
- [ ] Write test: `test_scope_middleware_key_management_read_post_rejected` — Key with `management:read` → POST /tama/v1/keys → 403 with `required_scope: management:write`
- [ ] Write test: `test_scope_middleware_key_management_write_get_passes` — Key with `management:write` → GET /tama/v1/models → 200 (write implies read)
- [ ] Write test: `test_scope_middleware_no_subject_passes` — No AuthSubject → pass through (skip path leakage)
- [ ] Wire scope_middleware into `router.rs` (both `build_router` and `build_unified_router`)
- [ ] Write integration test: `test_full_auth_then_scope_flow` — auth with valid key → scope check → handler
- [ ] Run `cargo nextest run --package tama-core -- proxy::scope_middleware`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add scope middleware for API key authorization"

**Acceptance criteria:**
- [ ] `AuthSubject::User` bypasses all scope checks
- [ ] `AuthSubject::Key` checked against route pattern
- [ ] `/v1/*` requires `inference` scope
- [ ] `GET /tama/v1/*` requires `management:read`
- [ ] `POST/PUT/DELETE /tama/v1/*` requires `management:write`
- [ ] `management:write` implies `management:read` (GET works with write scope)
- [ ] Missing scope returns 403 with required_scope in body
- [ ] Middleware applied after auth_middleware in router
- [ ] All tests pass, clippy clean, fmt clean

---

### Task 4: CRUD handlers and routes

**Context:**
The final task wires up the management API endpoints for creating, listing, updating, and revoking API keys. These handlers live in `tama-core` (following the existing pattern for tama management handlers) and are routed in the existing router.

**Files:**
- Create: `crates/tama-core/src/proxy/tama_handlers/api_keys.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/mod.rs` (add `mod api_keys` and re-exports)
- Modify: `crates/tama-core/src/proxy/server/router.rs` (add routes, add `patch` to `axum::routing` import)
- **CSRF note:** The existing `enforce_same_origin` middleware in `crates/tama/src/api/middleware.rs` already lets through requests with neither CSRF cookie nor header (lines 113-117), so API-key bearer clients work without changes. Cookie-based sessions need the CSRF header (same as existing `/tama/v1/models` routes). No modification needed.

**What to implement:**

1. **Request/Response types:**
   ```rust
   #[derive(Debug, Deserialize)]
   pub struct CreateApiKeyRequest {
       pub name: String,
       pub scopes: Vec<Scope>,
       pub expires_at: Option<String>, // ISO 8601 or null
   }

   #[derive(Debug, Deserialize)]
   pub struct UpdateApiKeyRequest {
       pub scopes: Vec<Scope>,
   }

   #[derive(Debug, Serialize)]
   pub struct CreateApiKeyResponse {
       pub id: i64,
       pub name: String,
       pub key: String,         // Plaintext — returned ONCE
       pub scopes: Vec<Scope>,
       pub expires_at: Option<String>,
       pub created_at: String,
   }

   #[derive(Debug, Serialize)]
   pub struct ListApiKeyResponse {
       pub id: i64,
       pub name: String,
       pub key_prefix: String,   // Never the full key
       pub scopes: Vec<Scope>,
       pub created_by: String,
       pub created_at: String,
       pub last_used_at: Option<String>,
       pub revoked_at: Option<String>,
       pub expires_at: Option<String>,
   }
   ```

2. **Handlers (all take `State<Arc<ProxyState>>` and `axum::Json` bodies):**

   - `pub async fn handle_create_key(State(state): State<Arc<ProxyState>>, Json(body): Json<CreateApiKeyRequest>) -> impl IntoResponse`
     - Extract `AuthSubject` from request (must be present — auth_middleware ensures this)
     - Determine `created_by`: username for `User`, key_prefix for `Key`
     - Validate scopes (non-empty, known values)
     - Generate key, hash it, insert into DB
     - Log: `tracing::info!(key_id, key_prefix, creator, "API key created")`
     - Return 201 with `CreateApiKeyResponse` (includes plaintext key)

   - `pub async fn handle_list_keys(State(state): State<Arc<ProxyState>>) -> impl IntoResponse`
     - Query all keys from DB
     - Return 200 with `Vec<ListApiKeyResponse>` (no plaintext keys)

   - `pub async fn handle_update_key(Path(key_id_str): Path<String>, State(state): State<Arc<ProxyState>>, Json(body): Json<UpdateApiKeyRequest>) -> impl IntoResponse`
     - Parse `key_id_str` to `i64` (return 400 on parse failure)
     - Validate key exists
     - Validate scopes
     - Update in DB via `spawn_blocking`
     - Log: `tracing::info!(key_id, "API key scopes updated")`
     - Return 200 with `ListApiKeyResponse`

   - `pub async fn handle_revoke_key(Path(key_id_str): Path<String>, State(state): State<Arc<ProxyState>>) -> impl IntoResponse`
     - Parse `key_id_str` to `i64` (return 400 on parse failure)
     - Validate key exists
     - Revoke in DB via `spawn_blocking` (set `revoked_at`)
     - Log: `tracing::info!(key_id, revoker, "API key revoked")`
     - Return 204 No Content

   All handlers that access the DB must use `tokio::task::spawn_blocking(move || { let conn = state.open_db().unwrap(); ... })` — same pattern as Task 2.

3. **Route registration in `router.rs`:**
   ```rust
   // In both build_router() and build_unified_router(), add to the import from crate::proxy::tama_handlers:
   // handle_tama_api_keys_list, handle_tama_api_keys_create, handle_tama_api_keys_update, handle_tama_api_keys_revoke
   
   // Add routes (follow existing pattern — handler names prefixed with handle_tama_):
   .route("/tama/v1/keys", get(handle_tama_api_keys_list).post(handle_tama_api_keys_create))
   .route("/tama/v1/keys/:id", patch(handle_tama_api_keys_update).delete(handle_tama_api_keys_revoke))
   ```
   Handler names follow the existing `handle_tama_*` convention used by all `tama_handlers/` (e.g., `handle_tama_load_model`, `handle_tama_pull_model`).

4. **Error responses:**
   - 400 for invalid scopes: `{"error": "invalid_request", "message": "unknown scope: foo"}`
   - 404 for nonexistent key: `{"error": "not_found", "message": "key not found"}`
   - All errors return JSON with appropriate status code

**Steps:**
- [ ] Write failing test: `test_create_key_returns_201_with_plaintext` — POST /tama/v1/keys with valid body → 201, response includes `key` field
- [ ] Run `cargo nextest run --package tama-core -- proxy::tama_handlers::api_keys::tests`
  - Did it fail? If not, investigate.
- [ ] Implement request/response types
- [ ] Implement `handle_tama_api_keys_create`
- [ ] Run `cargo nextest run --package tama-core -- proxy::tama_handlers::api_keys::tests::test_create_key_returns_201_with_plaintext`
  - Did it pass? If not, fix and re-run.
- [ ] Implement `handle_tama_api_keys_list`
- [ ] Write test: `test_list_keys_excludes_plaintext` — GET /tama/v1/keys → response has `key_prefix`, not `key`
- [ ] Implement `handle_tama_api_keys_update`
- [ ] Write test: `test_update_key_scopes` — PATCH /tama/v1/keys/1 → scopes updated
- [ ] Write test: `test_update_key_invalid_scopes_returns_400` — PATCH with unknown scope → 400
- [ ] Implement `handle_tama_api_keys_revoke`
- [ ] Write test: `test_revoke_key_returns_204` — DELETE /tama/v1/keys/1 → 204
- [ ] Write test: `test_revoke_nonexistent_key_returns_404` — DELETE /tama/v1/keys/999 → 404
- [ ] Write test: `test_create_key_empty_scopes_returns_400` — POST with `scopes: []` → 400
- [ ] Write test: `test_create_key_unknown_scope_returns_400` — POST with `scopes: ["foo"]` → 400
- [ ] Register routes in `router.rs` (both `build_router` and `build_unified_router`)
- [ ] Add `patch` to `use axum::routing::{get, patch, post};` import
- [ ] Write integration test: `test_key_crud_full_flow` — create → list → update → validate with new scopes → revoke → validate fails
- [ ] Run `cargo nextest run --package tama-core -- proxy::tama_handlers::api_keys`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: "feat: add API key CRUD handlers and routes"

**Acceptance criteria:**
- [ ] POST /tama/v1/keys creates key, returns 201 with plaintext (once only)
- [ ] GET /tama/v1/keys lists keys with `key_prefix` (no plaintext)
- [ ] PATCH /tama/v1/keys/:id updates scopes
- [ ] DELETE /tama/v1/keys/:id revokes key (soft delete)
- [ ] 400 for invalid/empty/unknown scopes
- [ ] 404 for nonexistent key
- [ ] Audit logging on create/revoke
- [ ] Routes registered in both `build_router` and `build_unified_router`
- [ ] Full workspace tests pass, clippy clean, fmt clean

---

### Verification (after all tasks)

Run the full gate:
```bash
cargo check --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace
```

**End-to-end manual test scenarios:**
1. Create a key with `["inference"]` → use it on `/v1/chat/completions` → 200
2. Same key on `POST /tama/v1/keys` → 403 (missing `management:write`)
3. Create a key with `["management:read"]` → `GET /tama/v1/models` → 200
4. Same key on `POST /tama/v1/keys` → 403 (missing `management:write`)
5. Revoke key → any request with that key → 401
6. OAuth2 session user → any route → 200 (bypass)
7. No auth, no keys configured → open mode, pass through
8. `api_keys_enabled = true`, no auth token → 401
