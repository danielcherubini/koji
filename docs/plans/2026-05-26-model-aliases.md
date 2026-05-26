# Model Aliases Plan

**Goal:** Replace the hardcoded `WILDCARD_MODEL_NAME` (`"whatevers-hot-n-fresh"`) with a user-managed global alias registry — users create aliases via the web UI that map a custom name to a target model, and the proxy resolves aliases on every request.

**Architecture:** A `model_aliases` SQLite table (v27 migration) stores `name → model_id` mappings with an enabled toggle. `ProxyState` caches enabled aliases as `HashMap<String, String>` (`alias_name → resolved_model_name`) loaded via a JOIN with `model_configs`. All request handlers call `state.resolve_alias(model_name)` which is O(1) and pass-through for non-aliases. The wildcard code (`resolve_wildcard_model`, `last_used_model`, etc.) is entirely removed.

**Tech Stack:** Rust (tama-core, tama-web), SQLite, Leptos/WASM, Axum

---

### Task 1: Database schema, queries, and migration v27

**Context:**
This task creates the foundation — the SQLite table, Rust query functions, and migration. Migration v27 both creates `model_aliases` AND drops `last_used_model` (no longer needed). It also seeds a default `whatevers-hot-n-fresh` alias pointing to the first enabled model for backward compatibility.

**Files:**
- Create: `crates/tama-core/src/db/queries/alias_queries.rs`
- Modify: `crates/tama-core/src/db/queries/mod.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`

**Note:** v27 does NOT need to be added to `FK_OFF_MIGRATIONS` because `last_used_model` has no incoming foreign key references from other tables; a simple `DROP TABLE IF EXISTS` is safe.

**What to implement:**

1. **Add `ModelAliasRecord` to `db/queries/types.rs`:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAliasRecord {
    pub id: i64,
    pub name: String,
    pub model_id: i64,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}
```

2. **Create `db/queries/alias_queries.rs` with these functions:**
```rust
/// Load all aliases joined with model_configs to get the resolved model name.
/// Used by ProxyState to populate the in-memory cache.
/// Returns (alias_name, resolved_model_name) pairs.
/// resolved_model_name = COALESCE(api_name, repo_id)
pub fn load_aliases_for_cache(conn: &Connection) -> Result<Vec<(String, String)>>

/// Load all aliases with model names for the web API.
pub fn get_all_aliases(conn: &Connection) -> Result<Vec<AliasResponse>>

/// Get a single alias by integer id.
pub fn get_alias_by_id(conn: &Connection, id: i64) -> Result<Option<AliasResponse>>

/// Insert a new alias. Returns the new row's id.
pub fn insert_alias(conn: &Connection, name: &str, model_id: i64, description: Option<&str>) -> Result<i64>

/// Update an existing alias. Only updates fields that are Some.
pub fn update_alias(conn: &Connection, id: i64, name: Option<&str>, model_id: Option<i64>, description: Option<Option<&str>>, enabled: Option<bool>) -> Result<()>

/// Delete an alias by id.
pub fn delete_alias(conn: &Connection, id: i64) -> Result<()>
```

The `AliasResponse` struct (defined in the same file or `types.rs`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasResponse {
    pub id: i64,
    pub name: String,
    pub model_id: i64,
    pub model_name: String,       // COALESCE(api_name, repo_id)
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}
```

3. **Update `db/queries/mod.rs`:**
- Add `pub mod alias_queries;`
- Add `pub use alias_queries::*;`

4. **Add migration v27 to `db/migrations.rs`:**
- Update `LATEST_VERSION` from 26 to 27
- Add migration tuple `(27, SQL)` with the following exact SQL:
```sql
DROP TABLE IF EXISTS last_used_model;

CREATE TABLE IF NOT EXISTS model_aliases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    model_id INTEGER NOT NULL REFERENCES model_configs(id) ON DELETE CASCADE,
    description TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_model_aliases_model_id ON model_aliases(model_id);
CREATE INDEX IF NOT EXISTS idx_model_aliases_enabled ON model_aliases(enabled);

-- Seed default alias for backward compatibility (only if enabled models exist)
INSERT OR IGNORE INTO model_aliases (name, model_id, description, enabled)
SELECT 'whatevers-hot-n-fresh', id, 'Default alias — routes to this model', 1
FROM model_configs
WHERE enabled = 1
ORDER BY id ASC
LIMIT 1;
```
- When no enabled models exist, no default alias is created (graceful skip via SELECT...LIMIT 1).

5. **Add tests in `alias_queries.rs`:**
- `test_load_aliases_for_cache_empty` — empty table returns empty vec
- `test_load_aliases_for_cache_with_data` — correct JOIN with COALESCE
- `test_insert_and_get_alias` — round-trip insert + get by id
- `test_update_alias` — partial update works
- `test_delete_alias` — delete removes row
- `test_duplicate_name_rejected` — UNIQUE constraint fires (case-insensitive via NOCASE)

**Steps:**
- [ ] Write failing tests for alias queries in `crates/tama-core/src/db/queries/alias_queries.rs`
- [ ] Run `cargo test --package tama-core alias_queries`
  - Did they fail? If not, investigate why.
- [ ] Implement `ModelAliasRecord` in `types.rs` and `AliasResponse` in `alias_queries.rs`
- [ ] Implement all query functions in `alias_queries.rs`
- [ ] Update `mod.rs` to expose the new module
- [ ] Add migration v27 to `migrations.rs` (update `LATEST_VERSION` to 27)
- [ ] Run `cargo test --package tama-core alias_queries`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo test --package tama-core migration`
  - Did migration tests pass? If not, fix and re-run.
- [ ] Run `cargo test --package tama-core migration_v27`
  - Add `test_migration_v27` to `crates/tama-core/src/db/migrations.rs` in the existing `#[cfg(test)] mod tests` block (follows the pattern of `test_migration_v26` and other migration tests). Verify: (a) `model_aliases` table exists with correct columns, (b) `last_used_model` table does not exist, (c) default `whatevers-hot-n-fresh` alias seeded when models exist, (d) graceful skip when no models exist
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add model_aliases table, queries, and migration v27"

**Acceptance criteria:**
- [ ] `model_aliases` table created by migration v27 with correct schema
- [ ] `last_used_model` table dropped by migration v27
- [ ] Default `whatevers-hot-n-fresh` alias seeded when enabled models exist
- [ ] When no enabled models exist, no default alias is created (graceful skip)
- [ ] All 6 query functions work correctly with tests
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

### Task 2: ProxyState alias cache and resolution

**Context:**
This task adds the in-memory alias cache to `ProxyState` and implements `resolve_alias` (O(1) HashMap lookup) and `reload_aliases` (DB → cache sync). This follows the exact same pattern as `model_configs` caching.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

1. **Add field to `ProxyState` in `types.rs`:**
```rust
/// alias_name → resolved model name (api_name or repo_id)
/// Only enabled aliases are cached. Populated from DB on init and reload.
pub aliases: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
```

2. **Initialize in `ProxyState::new()` in `state.rs`:**
```rust
aliases: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
```

**Do NOT call `reload_aliases()` inside `ProxyState::new()`.** The existing pattern is that `reload_model_configs()` is called from the proxy server startup code (see `crates/tama-web/src/api.rs:98` in `trigger_proxy_reload`). `reload_aliases()` will be called alongside `reload_model_configs()` from the same location.

3. **Update `trigger_proxy_reload` in `crates/tama-web/src/api.rs` (line ~98):**
After `state.reload_model_configs().await`, add `state.reload_aliases().await`. This ensures both model configs and aliases are reloaded together after any mutation.

3. **Implement `reload_aliases` method:**
```rust
pub async fn reload_aliases(&self) -> Result<()> {
    let mgr = self.model_mgr().context("No DB configured")?;
    let pairs = crate::db::load_aliases_for_cache(mgr.conn())?;
    let mut aliases = self.aliases.write().await;
    *aliases = pairs.into_iter().collect();
    Ok(())
}
```

4. **Implement `resolve_alias` method:**
```rust
/// Resolve a model name through the alias registry.
/// - If `name` is an alias → returns the resolved model name (api_name or repo_id)
/// - If `name` is not an alias → returns `name` unchanged (pass-through)
pub async fn resolve_alias(&self, name: &str) -> String {
    let aliases = self.aliases.read().await;
    if let Some(resolved) = aliases.get(name) {
        return resolved.clone();
    }
    name.to_string()
}
```

5. **Add tests in `state.rs` `#[cfg(test)]` module:**
- `test_resolve_alias_pass_through` — non-alias name returns unchanged
- `test_resolve_alias_resolves` — alias name returns resolved model name
- `test_reload_aliases_populates_cache` — after reload, cache has correct entries
- `test_reload_aliases_filters_disabled` — disabled aliases not in cache

**Steps:**
- [ ] Add `aliases` field to `ProxyState` in `types.rs`
- [ ] Initialize `aliases` in `ProxyState::new()` in `state.rs`
- [ ] Implement `reload_aliases` and `resolve_alias` methods
- [ ] Write tests for resolve_alias and reload_aliases
- [ ] Run `cargo test --package tama-core proxy::state::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add alias cache and resolution to ProxyState"

**Acceptance criteria:**
- [ ] `ProxyState.aliases` field exists and is initialized as empty HashMap
- [ ] `resolve_alias` returns pass-through for non-aliases
- [ ] `resolve_alias` returns resolved name for aliases
- [ ] `reload_aliases` populates cache from DB (enabled only)
- [ ] Tests pass for all 4 test cases

---

### Task 3: Remove wildcard code

**Context:**
This task removes ALL wildcard-related code — the constant, the resolution method, the guard mutex, the last_used tracking, and the DB queries. This is a pure deletion task that must compile cleanly.

**Files:**
- Modify: `crates/tama-core/src/proxy/types.rs` — remove `WILDCARD_MODEL_NAME`
- Modify: `crates/tama-core/src/proxy/state.rs` — remove `resolve_wildcard_model`, `try_db_fallback`, `WildcardDecision`, `wildcard_resolve_guard` field + init, all wildcard tests
- Modify: `crates/tama-core/src/proxy/handlers/chat.rs` — remove wildcard checks from `handle_chat_completions` and `handle_stream_chat_completions`, remove `update_last_used_best_effort` import and calls
- Modify: `crates/tama-core/src/proxy/handlers/forward.rs` — remove wildcard check from `handle_forward_post`, remove `update_last_used_best_effort` calls
- Modify: `crates/tama-core/src/proxy/handlers/models.rs` — remove wildcard entry from `handle_list_models` Phase 5, remove `has_available_llm` variable
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs` — remove `update_last_used_best_effort` function and exports
- Modify: `crates/tama-core/src/models/manager.rs` — remove `get_last_used()` and `set_last_used()` methods, remove `LastUsedModelRecord` import
- Modify: `crates/tama-core/src/db/queries/mod.rs` — remove `pub mod last_used_model_queries` and `pub use last_used_model_queries::*`
- Delete: `crates/tama-core/src/db/queries/last_used_model_queries.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs` — remove `LastUsedModelRecord` struct
- Modify: `crates/tama-core/src/proxy/mod.rs` — remove `pub use types::WILDCARD_MODEL_NAME` (or wherever it's re-exported)

**What to implement:**

For each file, read the current content, identify the wildcard-related code, and remove it:

1. **`proxy/types.rs`:** Remove the line `pub const WILDCARD_MODEL_NAME: &str = "whatevers-hot-n-fresh";`

2. **`proxy/state.rs`:**
   - Remove `wildcard_resolve_guard` field from `ProxyState` struct
   - Remove `wildcard_resolve_guard` initialization in `ProxyState::new()`
   - Remove `resolve_wildcard_model()` method and `try_db_fallback()` method
   - Remove `WildcardDecision` enum
   - Remove all tests in the `#[cfg(test)]` module that test wildcard functionality (tests that reference `WILDCARD_MODEL_NAME`, `resolve_wildcard_model`, or `WildcardDecision`)

3. **`proxy/handlers/chat.rs`:**
   - In `handle_chat_completions`: Remove the block that checks `model_name == WILDCARD_MODEL_NAME` and calls `resolve_wildcard_model`. The code should go directly to `get_available_server_for_model`.
   - In `handle_stream_chat_completions`: Same removal.
   - Remove `use super::update_last_used_best_effort;` import
   - Remove all calls to `update_last_used_best_effort`

4. **`proxy/handlers/forward.rs`:**
   - In `handle_forward_post`: Remove the wildcard check block
   - Remove all calls to `update_last_used_best_effort`

5. **`proxy/handlers/models.rs`:**
   - In `handle_list_models`: Remove Phase 5 (the wildcard prepend block). Remove the `has_available_llm` variable computation.
   - The wildcard entry `data.insert(0, serde_json::json!({ "id": WILDCARD_MODEL_NAME, ... }))` must be removed

6. **`proxy/handlers/mod.rs`:**
   - Remove the `update_last_used_best_effort` function entirely
   - Remove any re-exports of it

7. **`models/manager.rs`:**
   - Remove `get_last_used()` method
   - Remove `set_last_used()` method
   - Remove the `use` import of `LastUsedModelRecord`

8. **`db/queries/mod.rs`:**
   - Remove `pub mod last_used_model_queries;`
   - Remove `pub use last_used_model_queries::*;`

9. **Delete `db/queries/last_used_model_queries.rs`**

10. **`db/queries/types.rs`:**
    - Remove `LastUsedModelRecord` struct

11. **`proxy/mod.rs`:**
    - Remove `pub use types::WILDCARD_MODEL_NAME;` (check if it exists)

**Steps:**
- [ ] Remove `WILDCARD_MODEL_NAME` from `proxy/types.rs`
- [ ] Remove wildcard fields and methods from `proxy/state.rs`
- [ ] Remove wildcard checks from `proxy/handlers/chat.rs` (both handlers)
- [ ] Remove wildcard check from `proxy/handlers/forward.rs`
- [ ] Remove wildcard entry from `proxy/handlers/models.rs`
- [ ] Remove `update_last_used_best_effort` from `proxy/handlers/mod.rs`
- [ ] Remove `get_last_used`/`set_last_used` from `models/manager.rs`
- [ ] Remove `last_used_model_queries` module from `db/queries/mod.rs`
- [ ] Delete `db/queries/last_used_model_queries.rs`
- [ ] Remove `LastUsedModelRecord` from `db/queries/types.rs`
- [ ] Remove `WILDCARD_MODEL_NAME` re-export from `proxy/mod.rs` (if present)
- [ ] Run `cargo build --package tama-core`
  - Did it compile? Fix any remaining references to removed symbols.
- [ ] Run `cargo test --package tama-core`
  - Did all remaining tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "refactor: remove wildcard model routing and last_used_model tracking"

**Acceptance criteria:**
- [ ] `cargo build --workspace` succeeds with zero errors
- [ ] No references to `WILDCARD_MODEL_NAME`, `resolve_wildcard_model`, `wildcard_resolve_guard`, `update_last_used_best_effort`, `get_last_used`, `set_last_used`, or `LastUsedModelRecord` remain in the codebase (grep to verify)
- [ ] `cargo test --package tama-core` passes
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 4: Integrate alias resolution into request handlers

**Context:**
Now that the wildcard code is gone and alias resolution exists in ProxyState, this task wires `resolve_alias` into the 5 handlers that need it. The alias is resolved BEFORE the normal model lookup, so the rest of the pipeline works unchanged.

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/chat.rs`
- Modify: `crates/tama-core/src/proxy/handlers/forward.rs`
- Modify: `crates/tama-core/src/proxy/handlers/models.rs`

**What to implement:**

1. **`handle_chat_completions` in `chat.rs`:**
   After parsing `model_name` from the request body, add:
   ```rust
   let resolved_name = state.resolve_alias(model_name).await;
   ```
   Then use `resolved_name` everywhere `model_name` was used for routing (server lookup, load, forward). Keep the original `model_name` for logging.

2. **`handle_stream_chat_completions` in `chat.rs`:**
   Same change — resolve alias after parsing model_name, use resolved name for routing.

3. **`handle_forward_post` in `forward.rs`:**
   Same change — resolve alias after extracting model name.

4. **`handle_get_model` in `models.rs`:**
   Before the config lookup, check if `model_id` is an alias and track whether it was resolved:
   ```rust
   // Check if model_id is an alias first
   let aliases = state.aliases.read().await;
   let (lookup_id, is_alias) = if let Some(resolved) = aliases.get(&model_id) {
       (resolved.clone(), true)
   } else {
       (model_id.clone(), false)
   };
   drop(aliases);

   // Use lookup_id for the config lookup instead of model_id
   // ... existing config lookup logic with lookup_id ...

   // When building the response JSON, if is_alias is true,
   // set the response's "id" field to the original model_id (the alias name)
   // so the client sees the name it asked for.
   ```

5. **`handle_list_models` in `models.rs`:**
   After building the `data` array (after the "unloaded models" phase), append alias entries:
   ```rust
   let aliases = state.aliases.read().await;
   for (alias_name, resolved_name) in aliases.iter() {
       let is_ready = state.get_available_server_for_model(resolved_name).await.is_some();
       // Skip if the resolved name is already in the list (avoid duplicates)
       if !seen_ids.contains(resolved_name) {
           data.push(serde_json::json!({
               "id": alias_name,
               "object": "model",
               "created": 0,
               "owned_by": "tama-proxy",
               "ready": is_ready,
               "alias": true,
           }));
       }
   }
   ```

6. **Add integration tests in `crates/tama-core/src/proxy/handlers/tests.rs`:**
   Use `#[tokio::test]` and construct a mock `ProxyState` with aliases pre-populated in the cache.
   - `test_chat_completions_resolves_alias` — ProxyState has an alias, verify resolved name used for routing
   - `test_list_models_includes_aliases` — verify alias entries appear with `alias: true` flag
   - `test_get_model_resolves_alias` — verify alias resolves and response `id` is set to original alias name

**Steps:**
- [ ] Add `resolve_alias` call to `handle_chat_completions` in `chat.rs`
- [ ] Add `resolve_alias` call to `handle_stream_chat_completions` in `chat.rs`
- [ ] Add `resolve_alias` call to `handle_forward_post` in `forward.rs`
- [ ] Add `resolve_alias` call to `handle_get_model` in `models.rs`
- [ ] Add alias entries to `handle_list_models` in `models.rs`
- [ ] Write integration tests for alias resolution in handlers
- [ ] Run `cargo test --package tama-core proxy::handlers`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: integrate alias resolution into request handlers"

**Acceptance criteria:**
- [ ] All 5 handlers resolve aliases before model lookup
- [ ] `/v1/models` list includes alias entries with `alias: true` flag
- [ ] `/v1/models/:id` resolves alias and returns target model info
- [ ] Non-alias names pass through unchanged (existing behavior preserved)
- [ ] `cargo test --package tama-core` passes

---

### Task 5: Web API endpoints for alias CRUD

**Context:**
This task creates the REST API for managing aliases from the web UI. Standard CRUD with CSRF protection, following the exact same pattern as the model CRUD endpoints.

**Files:**
- Create: `crates/tama-web/src/api/aliases/mod.rs`
- Modify: `crates/tama-web/src/api.rs`
- Modify: `crates/tama-web/src/router.rs`

**What to implement:**

1. **Create `crates/tama-web/src/api/aliases/mod.rs`:**

```rust
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json, Path};
use std::sync::Arc;
use tama_core::proxy::ProxyState;

/// GET /tama/v1/aliases — list all aliases
pub async fn list_aliases(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Use spawn_blocking for DB access
    // Call tama_core::db::get_all_aliases
    // Return Json(vec of AliasResponse)
}

/// POST /tama/v1/aliases — create a new alias
#[derive(serde::Deserialize)]
pub struct CreateAliasBody {
    pub name: String,
    pub model_id: i64,
    #[serde(default)]
    pub description: Option<String>,
}
pub async fn create_alias(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<CreateAliasBody>,
) -> impl IntoResponse {
    // Validate name: non-empty, max 128 chars, regex ^[a-zA-Z0-9][a-zA-Z0-9_-]*$
    // Validate model_id exists (query model_configs)
    // Check uniqueness (try insert, catch UNIQUE violation → 409)
    // Insert via tama_core::db::insert_alias
    // Call state.reload_aliases() on success
    // Return 201 Created with { "ok": true, "id": N }
}

/// GET /tama/v1/aliases/:id — get single alias
pub async fn get_alias(
    State(state): State<Arc<ProxyState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Query by id, return 404 if not found
}

/// PUT /tama/v1/aliases/:id — update alias
#[derive(serde::Deserialize)]
pub struct UpdateAliasBody {
    pub name: Option<String>,
    pub model_id: Option<i64>,
    pub description: Option<Option<String>>,
    pub enabled: Option<bool>,
}
pub async fn update_alias(
    State(state): State<Arc<ProxyState>>,
    Path(id): Path<i64>,
    Json(body): Json<UpdateAliasBody>,
) -> impl IntoResponse {
    // Validate new name uniqueness if changed
    // Update via tama_core::db::update_alias
    // Call state.reload_aliases() on success
}

/// DELETE /tama/v1/aliases/:id — delete alias
pub async fn delete_alias(
    State(state): State<Arc<ProxyState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Delete via tama_core::db::delete_alias
    // Call state.reload_aliases() on success
    // Return { "ok": true }
}
```

All handlers must use `tokio::task::spawn_blocking` for DB access (same pattern as model CRUD). All state-changing handlers (POST, PUT, DELETE) must call `state.reload_aliases().await` after the DB mutation.

2. **Update `crates/tama-web/src/api.rs`:**
   Add `pub mod aliases;` alongside the existing `pub mod backends;`, `pub mod models;`, etc.

3. **Update `crates/tama-web/src/router.rs`:**
   In `build_web_routes()`, add to the `csrf_routes` sub-router:
   ```rust
   .route(
       "/tama/v1/aliases",
       get(api::aliases::list_aliases)
           .post(api::aliases::create_alias)
           .layer(json_body_limit),
   )
   .route(
       "/tama/v1/aliases/:id",
       get(api::aliases::get_alias)
           .put(api::aliases::update_alias)
           .delete(api::aliases::delete_alias),
   )
   ```

4. **Add tests:**
   - Test create alias happy path returns 201
   - Test duplicate name returns 409
   - Test invalid model_id returns 422
   - Test name format validation returns 422
   - Test update alias works
   - Test delete alias works
   - Test reload_aliases called after mutation (verify cache is updated)

**Steps:**
- [ ] Create `crates/tama-web/src/api/aliases/mod.rs` with all 5 handlers
- [ ] Add `pub mod aliases` to `crates/tama-web/src/api/mod.rs`
- [ ] Add routes to `crates/tama-web/src/router.rs` under `csrf_routes`
- [ ] Write tests for all CRUD operations
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix and re-run.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: add alias CRUD API endpoints"

**Acceptance criteria:**
- [ ] All 5 endpoints work correctly (GET list, POST create, GET single, PUT update, DELETE)
- [ ] Name validation: regex, uniqueness (409), model existence (422)
- [ ] `reload_aliases` called after every mutation
- [ ] Routes are CSRF-protected (under `csrf_routes`)
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes

---

### Task 6: Web UI — Aliases page

**Context:**
This task creates the Leptos/WASM Aliases page — a card-based list with create/edit modal. Placed in the sidebar between Benchmarks and Config with a 🏷️ icon.

**Files:**
- Create: `crates/tama-web/src/pages/aliases/mod.rs`
- Create: `crates/tama-web/src/pages/aliases/api.rs`
- Create: `crates/tama-web/src/pages/aliases/types.rs`
- Modify: `crates/tama-web/src/components/sidebar.rs`
- Modify: `crates/tama-web/src/lib.rs` (routing)
- Modify: `crates/tama-web/src/pages/mod.rs` (page module registration)

**What to implement:**

1. **Create `crates/tama-web/src/pages/aliases/types.rs`:**
```rust
#[derive(serde::Deserialize, Clone, Debug)]
pub struct Alias {
    pub id: i64,
    pub name: String,
    pub model_id: i64,
    pub model_name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct CreateAliasRequest {
    pub name: String,
    pub model_id: i64,
    pub description: Option<String>,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct UpdateAliasRequest {
    pub name: Option<String>,
    pub model_id: Option<i64>,
    pub description: Option<Option<String>>,
    pub enabled: Option<bool>,
}
```

2. **Create `crates/tama-web/src/pages/aliases/api.rs`:**
   Functions for fetching/creating/updating/deleting aliases via `crate::utils::{get_request, post_request, put_request, delete_request}`. Follow the pattern from `pages/model_editor/api.rs` — use `extract_and_store_csrf_token` on GET responses to capture the CSRF token for subsequent POST/PUT/DELETE requests.

   ```rust
   use crate::utils::{delete_request, extract_and_store_csrf_token, get_request, post_request, put_request};

   pub async fn fetch_aliases() -> Result<Vec<Alias>, String> {
       let resp = get_request("/tama/v1/aliases").send().await.map_err(|e| e.to_string())?;
       extract_and_store_csrf_token(&resp);
       resp.json().await.map_err(|e| e.to_string())
   }
   ```

3. **Create `crates/tama-web/src/pages/aliases/mod.rs`:**
   - **AliasesPage component:** Fetches aliases list on mount, renders card list
   - **AliasCard component:** Shows name, description, target model, enabled dot (●/○), Edit/Delete buttons
   - **AliasModal component:** Create/edit form with name input (validated), target model dropdown (from models list), description textarea, enabled checkbox
   - **Empty state:** "No aliases yet. Click + New to create one."

4. **Update sidebar in `crates/tama-web/src/components/sidebar.rs`:**
   Add between the Benchmarks link (line ~122) and the Config link (line ~130):
   ```rust
   <A href="/ui/aliases" attr:class="sidebar-item" attr:data-tooltip="Aliases" on:click=move |_| mobile_open.set(false)>
       <span class="sidebar-item__icon">"🏷️"</span>
       <span class="sidebar-item__label">"Aliases"</span>
   </A>
   ```

5. **Update routing in `crates/tama-web/src/lib.rs`:**
   Add between the benchmarks route (line ~302) and the logs route (line ~303):
   ```rust
   <Route path=path!("/ui/aliases") view=pages::aliases::AliasesPage />
   ```

**Steps:**
- [ ] Create `types.rs` with Alias, CreateAliasRequest, UpdateAliasRequest
- [ ] Create `api.rs` with fetch_aliases, create_alias, update_alias, delete_alias functions
- [ ] Create `mod.rs` with AliasesPage, AliasCard, and AliasModal components
- [ ] Add sidebar menu item in `sidebar.rs`
- [ ] Add route in app routing
- [ ] Run `cargo build --package tama-web`
  - Did it compile? If not, fix and re-run.
- [ ] Run `trunk build --release` (or the project's build command for the web UI)
  - Did it build? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add Aliases page to web UI"

**Acceptance criteria:**
- [ ] Aliases page renders card list with name, description, target model, enabled state
- [ ] Create modal works with name validation and model dropdown
- [ ] Edit modal pre-populates fields
- [ ] Delete shows confirmation
- [ ] Sidebar has 🏷️ Aliases item between Benchmarks and Config
- [ ] `cargo build --workspace` succeeds

---

### Task 7: Cleanup, integration tests, and final verification

**Context:**
Final cleanup pass — verify the full workspace builds, all tests pass, and the feature works end-to-end. Also remove any remaining references to wildcard code that might have been missed.

**Files:**
- Modify: Any files with remaining wildcard references
- Test: Workspace-wide test run

**What to implement:**

1. **Grep for remaining references:**
   ```bash
   grep -r "WILDCARD_MODEL_NAME\|whatevers-hot-n-fresh\|resolve_wildcard_model\|wildcard_resolve_guard\|update_last_used_best_effort\|get_last_used\|set_last_used\|LastUsedModelRecord\|last_used_model_queries" --include="*.rs" crates/
   ```
   Any hits that are NOT in migration v27 (which references `last_used_model` for the DROP TABLE) or NOT in comments explaining what was removed should be cleaned up.

2. **Run full workspace checks:**
   ```bash
   cargo fmt --all
   cargo clippy --workspace -- -D warnings
   cargo build --workspace
   cargo test --workspace
   ```

3. **Verify migration v27:**
   - Test on empty DB: migration applies cleanly
   - Test on v26 DB: migration applies, `last_used_model` dropped, `model_aliases` created
   - Test seed: when models exist, `whatevers-hot-n-fresh` alias is created

**Steps:**
- [ ] Run grep for remaining wildcard references
- [ ] Clean up any remaining references
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Verify migration v27 on empty and populated DBs
- [ ] Commit with message: "chore: cleanup remaining wildcard references, verify workspace"

**Acceptance criteria:**
- [ ] No remaining references to wildcard code (except migration v27 DROP and comments)
- [ ] `cargo fmt --all` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] Migration v27 applies cleanly on empty and v26 DBs

---

## Task Dependencies

```
Task 1 (DB + migration) ──┐
                          ├──> Task 2 (ProxyState cache)
Task 3 (remove wildcard) ─┘
                          ├──> Task 4 (handler integration)
Task 2 ───────────────────┘
                          ├──> Task 5 (Web API)
Task 2 + Task 4 ──────────┘
                          ───> Task 6 (Web UI)
                          ───> Task 7 (cleanup + verification)
```

Tasks 1 and 3 are independent and can be done in parallel.
Task 2 depends on Tasks 1 and 3.
Task 4 depends on Task 2.
Task 5 depends on Task 2 (needs reload_aliases).
Task 6 depends on Task 5 (needs API endpoints).
Task 7 depends on all.

---

## Verification Checklist

- [ ] `cargo build --release --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo fmt --all` passes
- [ ] Migration v27 creates `model_aliases` table with correct schema
- [ ] Migration v27 drops `last_used_model` table
- [ ] Default `whatevers-hot-n-fresh` alias seeded on migration
- [ ] Alias CRUD API works (create, read, update, delete)
- [ ] Alias resolution in chat/stream/forward handlers works
- [ ] `/v1/models` includes alias entries with `alias: true`
- [ ] `/v1/models/:alias` resolves and returns target model info
- [ ] Web UI Aliases page renders correctly
- [ ] No references to `WILDCARD_MODEL_NAME` remain (except migration)
