# Provider Abstraction + tamad Daemon Split Plan

**Goal:** Replace the monolithic "backend" concept with a provider abstraction (local vs remote) and split local process management into a separate tamad daemon.

**Architecture:** tama (proxy) handles request routing, model resolution, and remote API forwarding. tamad (separate daemon) manages local backend processes (spawn, health-check, install). tama communicates with tamad over gRPC or HTTP. Remote providers (OpenAI-compatible, Anthropic) are forwarded directly from tama with no tamad involved.

**Tech Stack:** Rust, gRPC (tonic 0.12 + prost 0.13), SQLite migration, existing axum proxy.

**Out of scope:** Routing local models through tamad (tamad process management implementation). This plan delivers the provider type system, DB schema, API endpoints, remote forwarding, and tamad skeleton. The actual tamad process management (spawn, health-check, install) and wiring local providers through tamad is a follow-up plan.

---

## Phase 1: Core Types & Database

### Task 1: Provider type system

**Context:**
The current `BackendType` enum (`LlamaCpp`, `IkLlama`, `TtsKokoro`, `Compaction`, `Custom`, `Docker`) conflates engine type with deployment model. We need to separate "what runs" (engine) from "how it's deployed" (local managed by tamad vs remote HTTP endpoint). This is the foundation everything else builds on.

`Engine` covers ALL current `BackendType` variants plus remote engines. `BackendType` is kept as-is (renamed to `InstallationType` in Task 9).

**Files:**
- Create: `crates/tama-core/src/providers/mod.rs`
- Create: `crates/tama-core/src/providers/types.rs`
- Modify: `crates/tama-core/src/lib.rs` (add `pub mod providers;`)

**What to implement:**

In `crates/tama-core/src/providers/types.rs`:

```rust
/// How the provider is deployed
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Display, EnumString, EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    #[default]
    Local,   // Managed by tamad
    Remote,  // Direct HTTP endpoint
}

/// The underlying inference engine
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display, EnumIs, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    // Local engines (tamad-managed)
    LlamaCpp,
    IkLlama,
    TtsKokoro,
    Compaction,
    Docker,
    Custom,
    // Remote engines (direct HTTP)
    #[strum(serialize = "openai")]
    #[strum(to_string = "openai")]
    #[serde(rename = "openai")]
    OpenAI,     // OpenAI-compatible (includes vLLM, llama.cpp API, etc.)
    Anthropic,
}

/// A registered provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: i64,
    pub name: String,
    pub provider_type: ProviderType,
    pub engine: Engine,
    /// For local: which tamad manages this provider
    pub tamad_id: Option<String>,
    /// For remote: base URL of the API
    pub base_url: Option<String>,
    /// For remote: API key (stored encrypted in DB)
    pub api_key: Option<String>,
    pub created_at: i64,
}

/// A registered tamad connection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TamadConnection {
    pub id: String,          // stable identifier (UUID)
    pub name: String,        // display name
    pub url: String,         // "grpc://..." or "http://..."
    pub protocol: Protocol,  // "grpc" | "http"
    pub token: Option<String>,
    pub status: TamadStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display, EnumString, EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Grpc,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display, EnumString, EnumIs)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TamadStatus {
    Online,
    Offline,
    Unknown,
}
```

Note: `OpenAI` variant uses explicit `strum(serialize = "openai")` and `serde(rename = "openai")` so the wire format is `"openai"` (not `"open_ai"` which is what `snake_case` derives automatically from the identifier `OpenAI`).

In `crates/tama-core/src/providers/mod.rs`:
```rust
pub mod types;

pub use types::{Engine, Provider, ProviderType, Protocol, TamadConnection, TamadStatus};
```

**Steps:**
- [ ] Create `providers/mod.rs` and `providers/types.rs` with empty type stubs
- [ ] Add `pub mod providers;` to `crates/tama-core/src/lib.rs`
- [ ] Write unit tests for Engine display/parse roundtrip in `providers/types.rs` `#[cfg(test)]` module:
  - Test `Engine::OpenAI.to_string() == "openai"` (explicit serialize)
  - Test `Engine::from_str("openai") == Ok(Engine::OpenAI)`
  - Test all other variants round-trip via snake_case
  - Test `is_*` methods from EnumIs derive
- [ ] Run `cargo nextest run --package tama-core -- providers`
  - Did it fail? Good — types are empty stubs.
- [ ] Implement types as specified above
- [ ] Run `cargo nextest run --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add provider type system (Provider, Engine, TamadConnection)"

**Acceptance criteria:**
- [ ] `Provider`, `Engine`, `ProviderType`, `TamadConnection` types compile
- [ ] All enums derive Display, EnumString (FromStr), EnumIs, Serialize, Deserialize with `rename_all = "snake_case"`
- [ ] `Engine::OpenAI` serializes to and parses from `"openai"` (not `"open_ai"`)
- [ ] Display/FromStr roundtrip works for all variants
- [ ] `is_*` methods work for all enum variants
- [ ] All tests pass, clippy clean

---

### Task 2: Database schema — new tables + Repository methods

**Context:**
We need new DB tables for the provider registry and tamad connections. The existing `backend_installations` and `backend_configs` tables will be migrated later — for now we just add the new tables alongside them.

Migrations follow the pattern: create `crates/tama-core/src/db/migrations/_0046_create_provider_registry.rs` exporting `pub const MIGRATION: (i32, bool, &str)`, add `mod _0046_create_provider_registry;` to `crates/tama-core/src/db/migrations.rs`, append to `MIGRATIONS` array, and bump `LATEST_VERSION` from 45 to 46. See `migrations_tests.rs` — it asserts last entry == LATEST_VERSION.

Repository methods follow the pattern in `crates/tama-core/src/db/repository.rs`: `Repository` wraps `conn: Connection` (private field) and exposes domain-level methods that call query functions. API handlers use `Repository` from `WebState`, NOT `ProxyState::open_db`.

**Files:**
- Create: `crates/tama-core/src/db/queries/provider_queries.rs`
- Create: `crates/tama-core/src/db/queries/tamad_queries.rs`
- Create: `crates/tama-core/src/db/migrations/_0046_create_provider_registry.rs`
- Modify: `crates/tama-core/src/db/queries/mod.rs` (add `mod provider_queries;` and `mod tamad_queries;` + re-exports)
- Modify: `crates/tama-core/src/db/migrations.rs` (add `mod _0046_create_provider_registry;`, append to `MIGRATIONS`, bump `LATEST_VERSION` to 46)
- Modify: `crates/tama-core/src/db/repository.rs` (add provider/tamad Repository methods)

**What to implement:**

In `crates/tama-core/src/db/queries/provider_queries.rs`:

SQL for `provider_registry`:
```sql
CREATE TABLE IF NOT EXISTS provider_registry (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    provider_type TEXT NOT NULL CHECK(provider_type IN ('local', 'remote')),
    engine TEXT NOT NULL,
    tamad_id TEXT,
    base_url TEXT,
    api_key TEXT,
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
```

Functions:
- `insert_provider(conn: &Connection, name: &str, provider_type: &str, engine: &str, tamad_id: Option<&str>, base_url: Option<&str>, api_key: Option<&str>) -> Result<i64>` — insert new provider, returns row id
- `get_provider(conn: &Connection, name: &str) -> Result<Option<Provider>>` — maps TEXT columns to ProviderType/Engine via `FromStr`
- `list_providers(conn: &Connection) -> Result<Vec<Provider>>`
- `update_provider(conn: &Connection, name: &str, base_url: Option<&str>, api_key: Option<&str>) -> Result<()>`
- `delete_provider(conn: &Connection, name: &str) -> Result<bool>` — returns true if row existed

In `crates/tama-core/src/db/queries/tamad_queries.rs`:

SQL for `tamad_registry`:
```sql
CREATE TABLE IF NOT EXISTS tamad_registry (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    url TEXT NOT NULL,
    protocol TEXT NOT NULL CHECK(protocol IN ('grpc', 'http')),
    token TEXT,
    status TEXT NOT NULL DEFAULT 'unknown' CHECK(status IN ('online', 'offline', 'unknown'))
);
```

Functions:
- `insert_tamad(conn: &Connection, id: &str, name: &str, url: &str, protocol: &str, token: Option<&str>) -> Result<()>`
- `get_tamad(conn: &Connection, id: &str) -> Result<Option<TamadConnection>>`
- `list_tamads(conn: &Connection) -> Result<Vec<TamadConnection>>`
- `update_tamad(conn: &Connection, id: &str, url: &str, token: Option<&str>) -> Result<()>`
- `delete_tamad(conn: &Connection, id: &str) -> Result<bool>`
- `update_tamad_status(conn: &Connection, id: &str, status: &str) -> Result<()>`

In `_0046_create_provider_registry.rs`:
```rust
pub const MIGRATION: (i32, bool, &str) = (
    46,
    false, // fk_off
    "CREATE TABLE IF NOT EXISTS provider_registry (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL UNIQUE,
        provider_type TEXT NOT NULL CHECK(provider_type IN ('local', 'remote')),
        engine TEXT NOT NULL,
        tamad_id TEXT,
        base_url TEXT,
        api_key TEXT,
        created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
    );
    CREATE TABLE IF NOT EXISTS tamad_registry (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        url TEXT NOT NULL,
        protocol TEXT NOT NULL CHECK(protocol IN ('grpc', 'http')),
        token TEXT,
        status TEXT NOT NULL DEFAULT 'unknown' CHECK(status IN ('online', 'offline', 'unknown'))
    );",
);
```

In `repository.rs`, add methods following the existing pattern (e.g., `get_all_aliases`):
```rust
pub fn insert_provider(&self, name: &str, provider_type: &str, engine: &str, tamad_id: Option<&str>, base_url: Option<&str>, api_key: Option<&str>) -> anyhow::Result<i64> {
    queries::insert_provider(&self.conn, name, provider_type, engine, tamad_id, base_url, api_key)
        .with_context(|| format!("Failed to insert provider '{}'", name))
}

pub fn get_provider(&self, name: &str) -> anyhow::Result<Option<crate::providers::Provider>> {
    queries::get_provider(&self.conn, name)
        .with_context(|| format!("Failed to get provider '{}'", name))
}

pub fn list_providers(&self) -> anyhow::Result<Vec<crate::providers::Provider>> {
    queries::list_providers(&self.conn)
        .with_context(|| "Failed to list providers")
}

pub fn update_provider(&self, name: &str, base_url: Option<&str>, api_key: Option<&str>) -> anyhow::Result<()> {
    queries::update_provider(&self.conn, name, base_url, api_key)
        .with_context(|| format!("Failed to update provider '{}'", name))
}

pub fn delete_provider(&self, name: &str) -> anyhow::Result<bool> {
    queries::delete_provider(&self.conn, name)
        .with_context(|| format!("Failed to delete provider '{}'", name))
}

// Tamad methods:
pub fn insert_tamad(&self, id: &str, name: &str, url: &str, protocol: &str, token: Option<&str>) -> anyhow::Result<()> { ... }
pub fn get_tamad(&self, id: &str) -> anyhow::Result<Option<crate::providers::TamadConnection>> { ... }
pub fn list_tamads(&self) -> anyhow::Result<Vec<crate::providers::TamadConnection>> { ... }
pub fn update_tamad(&self, id: &str, url: &str, token: Option<&str>) -> anyhow::Result<()> { ... }
pub fn delete_tamad(&self, id: &str) -> anyhow::Result<bool> { ... }
pub fn update_tamad_status(&self, id: &str, status: &str) -> anyhow::Result<()> { ... }
```

**Steps:**
- [ ] Create query files with empty function stubs
- [ ] Write tests for provider CRUD operations (in-memory DB) in `provider_queries.rs` `#[cfg(test)]` module
- [ ] Write tests for tamad CRUD operations in `tamad_queries.rs` `#[cfg(test)]` module
- [ ] Run `cargo nextest run --package tama-core -- provider_queries`
  - Did it fail? Good — functions are empty stubs.
- [ ] Implement SQL schema + query functions (map TEXT → enum via `FromStr`)
- [ ] Create migration file `_0046_create_provider_registry.rs`
- [ ] Add `mod _0046_create_provider_registry;` to `migrations.rs`
- [ ] Append to `MIGRATIONS` array
- [ ] Bump `LATEST_VERSION` from 45 to 46
- [ ] Add Repository methods in `repository.rs`
- [ ] Run `cargo nextest run --package tama-core -- provider_queries`
- [ ] Run `cargo nextest run --package tama-core -- tamad_queries`
- [ ] Run `cargo nextest run --package tama-core -- migrations_tests`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add provider_registry and tamad_registry DB tables"

**Acceptance criteria:**
- [ ] `provider_registry` table created on fresh DB (migration 46)
- [ ] `tamad_registry` table created on fresh DB (migration 46)
- [ ] All CRUD operations work with in-memory DB
- [ ] Provider name is UNIQUE constraint
- [ ] Tamad id is PRIMARY KEY
- [ ] Repository methods delegate to query functions with `.with_context()`
- [ ] `migrations_tests` passes (last entry == LATEST_VERSION)
- [ ] All tests pass, clippy clean

---

## Phase 2: API Endpoints

### Task 3: Provider CRUD API handlers

**Context:**
Add `/tama/v1/providers/*` management endpoints. These are CRUD routes that live in the `tama` crate (NOT tama-core), following the architectural boundary enforced by `router_ownership_test.rs`. All management handlers go in `crates/tama/src/api/` and are wired in `crates/tama/src/router.rs`.

Follow the pattern of `crates/tama/src/api/backends/` (list.rs, register.rs, manage/) for the file structure, and the aliases router pattern (`get(list_aliases).post(create_alias).layer(json_body_limit)` in csrf_routes, router.rs:287) for route registration.

Handlers use the `Repository` from `WebState`. Do NOT call `ProxyState::open_db` directly — that's for proxy-owned routes only and is `pub(crate)`.

All routes (GET, POST, PATCH, DELETE) go in `csrf_routes` with `.layer(json_body_limit)` on POST/PATCH, matching the aliases pattern (`get(list_aliases).post(create_alias).layer(json_body_limit)`).

**Files:**
- Create: `crates/tama/src/api/providers/mod.rs`
- Create: `crates/tama/src/api/providers/list.rs`
- Create: `crates/tama/src/api/providers/register.rs`
- Create: `crates/tama/src/api/providers/manage.rs`
- Modify: `crates/tama/src/api.rs` (add `pub mod providers;`)
- Modify: `crates/tama/src/router.rs` (register routes in `build_web_routes` → `csrf_routes`)
- Modify: `crates/tama/tests/router_ownership_test.rs` (add new paths to `TAMA_MANAGED_PATHS`, update `EXPECTED_TAMA_PATH_COUNT`)
- Modify: `crates/tama/src/api/openapi.rs` (add provider routes to OpenAPI spec)
- Create: `docs/api/providers.md` (API documentation)

**What to implement:**

Request DTOs (NOT the Provider struct directly — it has server-generated fields):
```rust
// In register.rs
pub struct CreateProviderRequest {
    pub name: String,
    pub provider_type: String, // "local" or "remote"
    pub engine: String,
    pub tamad_id: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

// In manage.rs
pub struct UpdateProviderRequest {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}
```

Routes (all wired in `csrf_routes` in `router.rs` `build_web_routes`, matching aliases pattern):
- `GET    /tama/v1/providers` → `list.rs` handler — list all providers
- `POST   /tama/v1/providers` → `register.rs` handler — create provider (with json_body_limit)
- `GET    /tama/v1/providers/:name` → `list.rs` handler — get provider by name
- `PATCH  /tama/v1/providers/:name` → `manage.rs` handler — update provider (with json_body_limit)
- `DELETE /tama/v1/providers/:name` → `manage.rs` handler — delete provider

For POST validation: local needs `tamad_id`, remote needs `base_url`.

In `router_ownership_test.rs`, add the 5 new paths to `TAMA_MANAGED_PATHS` and increment `EXPECTED_TAMA_PATH_COUNT` by 5 (from 54 to 59).

In `openapi.rs`, add the 5 routes following the existing pattern.

**Steps:**
- [ ] Create `api/providers/mod.rs` with handler re-exports
- [ ] Implement `list.rs` (GET all, GET by name) using `WebState.repository`
- [ ] Implement `register.rs` (POST create with validation: local needs tamad_id, remote needs base_url)
- [ ] Implement `manage.rs` (PATCH update, DELETE remove)
- [ ] Add `pub mod providers;` to `api.rs`
- [ ] Register routes in `router.rs` `build_web_routes` inside `csrf_routes` (with `.layer(json_body_limit)` on POST/PATCH)
- [ ] Update `router_ownership_test.rs` (add 5 paths, increment count from 54 to 59)
- [ ] Update `openapi.rs` with new routes
- [ ] Create `docs/api/providers.md` following existing API doc format
- [ ] Run `cargo nextest run --package tama --features ssr -- providers`
- [ ] Run `cargo nextest run --package tama --features ssr -- router_ownership`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: add /tama/v1/providers CRUD API endpoints"

**Acceptance criteria:**
- [ ] All 5 endpoints respond correctly
- [ ] POST validates provider_type matches fields (local has tamad_id, remote has base_url)
- [ ] DELETE returns 404 if provider not found
- [ ] Routes have CSRF protection (in csrf_routes)
- [ ] `router_ownership_test` passes with updated count (59)
- [ ] OpenAPI spec includes new routes
- [ ] All tests pass, clippy clean

---

### Task 4: Tamad CRUD API handlers

**Context:**
Register and manage tamad connections via API. Same architectural pattern as Task 3 — management routes in `crates/tama/src/api/`, wired in `router.rs`, using Repository from WebState.

The health check endpoint is a **stub** at this stage — it returns `{"status": "unknown"}` with a note that the tamad client isn't wired yet. The actual health check is implemented in Task 8.

**Files:**
- Create: `crates/tama/src/api/tamads/mod.rs`
- Create: `crates/tama/src/api/tamads/list.rs`
- Create: `crates/tama/src/api/tamads/register.rs`
- Create: `crates/tama/src/api/tamads/manage.rs`
- Modify: `crates/tama/src/api.rs` (add `pub mod tamads;`)
- Modify: `crates/tama/src/router.rs` (register routes in `build_web_routes` → `csrf_routes`)
- Modify: `crates/tama/tests/router_ownership_test.rs` (add new paths, update count)
- Modify: `crates/tama/src/api/openapi.rs` (add tamad routes to OpenAPI spec)
- Create: `docs/api/tamads.md` (API documentation)

**What to implement:**

Request DTOs:
```rust
pub struct CreateTamadRequest {
    pub name: String,
    pub url: String,
    pub protocol: String, // "grpc" or "http"
    pub token: Option<String>,
}

pub struct UpdateTamadRequest {
    pub url: Option<String>,
    pub token: Option<String>,
}
```

Routes (all in `csrf_routes`):
- `GET    /tama/v1/tamads` → `list.rs` — list all tamads
- `POST   /tama/v1/tamads` → `register.rs` — register tamad (with json_body_limit)
- `GET    /tama/v1/tamads/:id` → `list.rs` — get tamad
- `PATCH  /tama/v1/tamads/:id` → `manage.rs` — update tamad (with json_body_limit)
- `DELETE /tama/v1/tamads/:id` → `manage.rs` — unregister tamad
- `POST   /tama/v1/tamads/:id/health` → `manage.rs` — trigger health check (returns stub `{"status": "unknown"}`)

POST auto-generates UUID for tamad id using `uuid::Uuid::new_v4().to_string()`.

In `router_ownership_test.rs`, add the 6 new paths to `TAMA_MANAGED_PATHS` and increment `EXPECTED_TAMA_PATH_COUNT` by 6 (from 59 to 65).

**Steps:**
- [ ] Create `api/tamads/mod.rs` with handler re-exports
- [ ] Implement `list.rs` (GET all, GET by id)
- [ ] Implement `register.rs` (POST create — auto-generate UUID for tamad id)
- [ ] Implement `manage.rs` (PATCH update, DELETE remove, POST health stub)
- [ ] Add `pub mod tamads;` to `api.rs`
- [ ] Register routes in `router.rs` `build_web_routes` inside `csrf_routes`
- [ ] Update `router_ownership_test.rs` (add 6 paths, increment count from 59 to 65)
- [ ] Update `openapi.rs` with new routes
- [ ] Create `docs/api/tamads.md`
- [ ] Run `cargo nextest run --package tama --features ssr -- tamads`
- [ ] Run `cargo nextest run --package tama --features ssr -- router_ownership`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: add /tama/v1/tamads CRUD API endpoints"

**Acceptance criteria:**
- [ ] All 6 endpoints respond correctly
- [ ] POST auto-generates UUID for tamad id
- [ ] Health check endpoint returns stub `{"status": "unknown"}`
- [ ] Routes have CSRF protection
- [ ] `router_ownership_test` passes with updated count (65)
- [ ] All tests pass, clippy clean

---

## Phase 3: Remote Provider Forwarding

### Task 5: OpenAI-compatible forwarding + model→provider linkage

**Context:**
Remote providers with `engine = "openai"` need the request forwarded to their `base_url` with the `api_key` as a Bearer token. The request format is already OpenAI-compatible, so no transformation needed.

**Model → Provider linkage:** A new column `provider_name TEXT` is added to `model_configs`. When set, it overrides the `backend` field for routing. If `provider_name` resolves to a remote provider, use `RemoteForwarder`. If unset (legacy), fall through to existing `backend` routing.

The `provider_name` must be available at runtime in the in-memory config. The forward path reads from `state.registry.model_configs` (populated from DB via `db::load_model_configs`). The in-memory `ModelConfig` struct in `config/types/model.rs` needs a `provider_name` field, and both `from_db_record` and `to_db_record` must map it.

**Integration point:** The remote check must happen BEFORE `ensure_model_loaded` is called, because `ensure_model_loaded` → `ProxyState::load_model` spawns a local backend process. Remote models are never "loaded" — they're always available (or aren't). The check is added inside `ensure_model_loaded` itself, after alias resolution.

**Setting provider_name:** This task adds the plumbing (column, in-memory field, forwarding). Setting the value on a model is out of scope — it can be done via direct DB update or a future API endpoint. The column defaults to NULL so existing models are unaffected.

**Adding fields to ModelConfigRecord / ModelConfig:** Both are full struct literals used in many files. `ModelConfig` derives `Default`, so test files can use `..Default::default()`. Production code that constructs full literals must add the field explicitly.

**Files:**
- Create: `crates/tama-core/src/proxy/remote/mod.rs`
- Create: `crates/tama-core/src/proxy/remote/openai.rs`
- Create: `crates/tama-core/src/db/migrations/_0047_add_model_provider_name.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (add `pub mod remote;`)
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (add remote provider check in `ensure_model_loaded`, add `get_provider` method to ProxyState)
- Modify: `crates/tama-core/src/db/migrations.rs` (add migration 47, bump LATEST_VERSION to 47)
- Modify: `crates/tama-core/src/db/queries/types.rs` (add `provider_name` to `ModelConfigRecord`, `COLUMNS`, `INSERT_COLUMNS`, `from_row`)
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs` (update queries for `provider_name`, add to upsert SET clause, update `test_model_config_columns_match_insert_columns` count)
- Modify: `crates/tama-core/src/config/types/model.rs` (add `provider_name` field to in-memory `ModelConfig`, map in `from_db_record` AND `to_db_record`)
- Modify: `crates/tama-core/src/proxy/types.rs` (add `remote_forwarder: RemoteForwarder` field to `ProxyState`, update `Clone` impl)
- Modify: `crates/tama-core/src/proxy/state/mod.rs` (initialize `remote_forwarder` in `ProxyState::new`)
- Modify: `crates/tama-core/src/db/queries/tests.rs` (add `provider_name: None` to all `ModelConfigRecord` literals)
- Modify: `crates/tama-core/src/models/manager_tests.rs` (add `provider_name: None`)
- Modify: `crates/tama-core/src/config/types/model_tests.rs` (add `provider_name: None` to all `ModelConfigRecord` literals, add `..Default::default()` to all `ModelConfig` literals)
- Modify: `crates/tama-core/src/db/backfill/hf_metadata.rs` (add `provider_name: None`)
- Modify: `crates/tama-core/src/updates/checker/orchestration_tests.rs` (add `provider_name: None`)
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs` (add `..Default::default()` to `ModelConfig` literals)
- Modify: `crates/tama/src/api/aliases/mod.rs` (add `provider_name: None`)
- Modify: `crates/tama/src/api/models/info.rs` (add `provider_name: None`)
- Modify: `crates/tama/src/api/models/crud/mod.rs` (add `provider_name` to `apply_patch_body` preserving from existing, add to `apply_model_body` base)
- Modify: `crates/tama/src/api/models/crud/tests.rs` (add `provider_name: None` / `..Default::default()`)

**What to implement:**

In `crates/tama-core/src/proxy/remote/openai.rs`:

```rust
#[derive(Clone)]
pub struct RemoteForwarder {
    client: reqwest::Client,
}

impl RemoteForwarder {
    pub fn new() -> Self { Self { client: reqwest::Client::new() } }

    pub async fn forward(
        &self,
        provider: &crate::providers::Provider,
        parts: &http::request::Parts,
        body: bytes::Bytes,
    ) -> Result<http::Response<axum::body::Body>> {
        // Build target URL: provider.base_url + parts.uri.path()
        // Clone headers, add Authorization: Bearer <api_key>
        // Forward request body
        // Return response (streamed for SSE)
    }
}
```

Add `get_provider` method to `ProxyState` in `proxy/types.rs`:
```rust
impl ProxyState {
    pub(crate) async fn get_provider(&self, name: &str) -> Option<crate::providers::Provider> {
        self.open_db()
            .and_then(|conn| crate::db::queries::get_provider(&conn, name).ok().flatten())
    }
}
```

In `ensure_model_loaded` (`proxy/lifecycle/mod.rs`), after alias resolution (line ~38), add:
```rust
// Check if model has a provider_name that resolves to a remote provider
let model_configs = state.registry.model_configs.read().await;
if let Some(config) = model_configs.get(&resolved_model) {
    if let Some(ref provider_name) = config.provider_name {
        drop(model_configs);
        if let Some(provider) = state.get_provider(provider_name).await {
            if provider.provider_type.is_remote() {
                // Return sentinel: "remote:<provider_id>"
                // Caller checks for "remote:" prefix and uses RemoteForwarder
                return Ok(format!("remote:{}", provider.id));
            }
        }
    } else {
        drop(model_configs);
    }
} else {
    drop(model_configs);
}
// ... existing local backend routing ...
```

The caller (`handle_forward_post` / `resolve_and_load_server`) checks if the returned backend_name starts with `"remote:"` and if so, extracts the provider id, looks up the `Provider` from DB, and uses `RemoteForwarder` instead of the normal forward path.

In `db/queries/types.rs`, add `pub provider_name: Option<String>` to `ModelConfigRecord`, update `COLUMNS` (38 → 39 columns), `INSERT_COLUMNS` (37 → 38), and positional `from_row`.

In `model_config_queries.rs`, add `provider_name` to upsert SET clause as `provider_name = excluded.provider_name`, update `params!` list, and update `test_model_config_columns_match_insert_columns` counts (38 → 39, 37 → 38).

In `config/types/model.rs`, add `pub provider_name: Option<String>` to `ModelConfig` (it derives Default, so `None` is the default), map in both `from_db_record` and `to_db_record`.

In `proxy/types.rs`, add `pub(crate) remote_forwarder: RemoteForwarder` and update the manual `Clone` impl.

In `proxy/state/mod.rs`, initialize `remote_forwarder: RemoteForwarder::new()` in `ProxyState::new`.

Migration `_0047`:
```rust
pub const MIGRATION: (i32, bool, &str) = (47, false, "ALTER TABLE model_configs ADD COLUMN provider_name TEXT;");
```

**Steps:**
- [ ] Write tests for URL construction (path joining, prefix stripping) and header injection
- [ ] Create migration `_0047_add_model_provider_name.rs`
- [ ] Add migration to `migrations.rs`, bump `LATEST_VERSION` to 47
- [ ] Add `provider_name` field to `ModelConfigRecord` in `db/queries/types.rs`
- [ ] Update `COLUMNS` (39), `INSERT_COLUMNS` (38), `from_row` for new column
- [ ] Update queries in `model_config_queries.rs` (upsert SET + params + test count 39/38)
- [ ] Add `provider_name` to in-memory `ModelConfig` in `config/types/model.rs`
- [ ] Map `provider_name` in both `from_db_record` AND `to_db_record`
- [ ] Update all `ModelConfigRecord` struct literals in listed files (add `provider_name: None`)
- [ ] Update all `ModelConfig` struct literals in listed files (add `..Default::default()` or explicit field)
- [ ] Update `apply_patch_body` and `apply_model_body` in `tama/src/api/models/crud/mod.rs` to preserve `provider_name`
- [ ] Implement `RemoteForwarder` in `remote/openai.rs` (derive Clone)
- [ ] Add `pub mod remote;` to `proxy/mod.rs`
- [ ] Add `remote_forwarder` field to `ProxyState` in `proxy/types.rs`
- [ ] Update `ProxyState`'s manual `Clone` impl
- [ ] Add `get_provider` method to `ProxyState`
- [ ] Initialize `remote_forwarder` in `ProxyState::new`
- [ ] Add remote provider check in `ensure_model_loaded` (`proxy/lifecycle/mod.rs`)
- [ ] Update callers to check for `"remote:"` prefix and use `RemoteForwarder`
- [ ] Run `cargo nextest run --package tama-core -- remote`
- [ ] Run `cargo nextest run --package tama-core -- migrations_tests`
- [ ] Run `cargo nextest run --package tama --features ssr -- crud`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit with message: "feat: add OpenAI-compatible remote provider forwarding"

**Acceptance criteria:**
- [ ] Request to remote OpenAI provider is forwarded with correct base_url
- [ ] API key injected as Authorization: Bearer header
- [ ] Response streamed back to client
- [ ] Migration 47 adds `provider_name` column to `model_configs`
- [ ] Legacy models (no `provider_name`) still route to local backends
- [ ] `ProxyState` Clone impl compiles with new field
- [ ] `to_db_record` preserves `provider_name` (no data loss on save)
- [ ] `apply_patch_body` preserves `provider_name` from existing model
- [ ] All tests pass, clippy clean (both crates)

---

### Task 6: Anthropic API forwarding with translation

**Context:**
Anthropic uses a different API format than OpenAI. tama needs to translate between them so clients can use the OpenAI format against Anthropic through tama.

**Files:**
- Create: `crates/tama-core/src/proxy/remote/anthropic.rs`

**What to implement:**

Translation layer:
- OpenAI `chat/completions` → Anthropic `messages`
- Map `model`, `messages` (roles map directly), `temperature`, `max_tokens`, `stream`
- Anthropic response → OpenAI response format
- Streaming: Anthropic SSE events → OpenAI SSE events

Key differences to handle:
- Anthropic requires `anthropic-version` header (use "2023-06-01")
- Anthropic uses `x-api-key` or Bearer auth
- System messages: Anthropic has `system` field at top level, OpenAI has `role: "system"` in messages array
- Anthropic streaming events differ from OpenAI (delta format)

Wire into `RemoteForwarder::forward` — when `provider.engine.is_anthropic()`, route through the Anthropic translator instead of direct forwarding.

**Steps:**
- [ ] Write tests for request translation (OpenAI → Anthropic) with system messages
- [ ] Write tests for response translation (Anthropic → OpenAI)
- [ ] Write tests for streaming translation (SSE events)
- [ ] Implement request translator
- [ ] Implement response translator (including streaming)
- [ ] Wire into RemoteForwarder::forward (check `provider.engine.is_anthropic()`)
- [ ] Run `cargo nextest run --package tama-core -- anthropic`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add Anthropic API forwarding with OpenAI translation"

**Acceptance criteria:**
- [ ] OpenAI chat/completions request translated to Anthropic messages
- [ ] System messages extracted from messages array → Anthropic `system` field
- [ ] Anthropic response translated back to OpenAI format
- [ ] Streaming works (SSE events translated)
- [ ] All tests pass, clippy clean

---

## Phase 4: tamad Daemon

### Task 7: tamad service skeleton

**Context:**
tamad is a separate binary that manages local backend processes. It exposes both gRPC (tonic) and HTTP (axum) endpoints on configurable addresses.

gRPC requires: `.proto` file, `tonic-build` in `build.rs`, `tonic`/`prost` as deps. The proto definition and `build.rs` live in `tama-core` (NOT tamad) so the generated client code is available to both the tamad server and the tama client without circular dependencies.

**Version pins:** Use `tonic = "0.12"`, `tonic-build = "0.12"`, `prost = "0.13"`, `protoc-bin-vendored = "3"`. Version 0.14 of tonic removed prost codegen from tonic-build, so we pin 0.12.

**protoc availability:** Use `protoc-bin-vendored` in `build.rs` to hermetically bundle protoc — zero environment changes needed.

**Files:**
- Create: `crates/tamad/Cargo.toml`
- Create: `crates/tamad/src/main.rs`
- Create: `crates/tamad/src/server.rs`
- Create: `crates/tama-core/proto/tamad.proto`
- Create: `crates/tama-core/build.rs`
- Create: `crates/tama-core/src/tamad/mod.rs`
- Create: `crates/tama-core/src/tamad/protocol.rs` (shared protocol definitions)
- Modify: `Cargo.toml` (add tamad to workspace members, add `tonic = "0.12"`/`prost = "0.13"`/`tonic-build = "0.12"`/`protoc-bin-vendored = "3"` to `[workspace.dependencies]`)
- Modify: `crates/tama-core/Cargo.toml` (add `tonic`/`prost` as deps, `tonic-build`/`protoc-bin-vendored` as build-deps, add `build = "build.rs"`)
- Modify: `crates/tama-core/src/lib.rs` (add `pub mod tamad;`)

**What to implement:**

`crates/tamad/Cargo.toml`:
```toml
[package]
name = "tamad"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
tama-core = { path = "../tama-core" }
tonic = { workspace = true }
prost = { workspace = true }
tokio = { workspace = true }
axum = { workspace = true }
tracing = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
```

`crates/tama-core/build.rs`:
```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_binary()?);
    tonic_build::configure()
        .compile_protos(&["proto/tamad.proto"], &["proto"])?;
    Ok(())
}
```

`crates/tama-core/proto/tamad.proto`:
```protobuf
syntax = "proto3";

package tamad;

service TamadService {
  rpc ListProviders(Empty) returns (ListProvidersResponse);
  rpc InstallProvider(InstallProviderRequest) returns (InstallProviderResponse);
  rpc LoadModel(LoadModelRequest) returns (LoadModelResponse);
  rpc UnloadModel(UnloadModelRequest) returns (Empty);
  rpc UpdateProvider(UpdateProviderRequest) returns (UpdateProviderResponse);
  rpc RemoveProvider(RemoveProviderRequest) returns (Empty);
  rpc Logs(LogsRequest) returns (stream LogEntry);
  rpc HealthCheck(Empty) returns (HealthResponse);
}

message Empty {}

message LoadModelRequest {
  string provider_name = 1;
  string model_path = 2;
  string gpu_variant = 3;
  map<string, string> params = 4;
}

message LoadModelResponse {
  string endpoint_url = 1;
  int32 pid = 2;
  string status = 3;
}

message UnloadModelRequest {
  string provider_name = 1;
  string model_name = 2;
}

message ListProvidersResponse {
  repeated ProviderInfo providers = 1;
}

message ProviderInfo {
  string name = 1;
  string engine = 2;
  string version = 3;
  string status = 4;
  string gpu_variant = 5;
}

message InstallProviderRequest {
  string name = 1;
  string engine = 2;
  string version = 3;
  string gpu_variant = 4;
}

message InstallProviderResponse {
  string status = 1;
}

message UpdateProviderRequest {
  string name = 1;
  string version = 2;
}

message UpdateProviderResponse {
  string status = 1;
}

message RemoveProviderRequest {
  string name = 1;
}

message LogsRequest {
  string provider_name = 1;
}

message LogEntry {
  string timestamp = 1;
  string level = 2;
  string message = 3;
}

message HealthResponse {
  string status = 1;
  string version = 2;
}
```

In `crates/tama-core/src/tamad/mod.rs`, include the generated module:
```rust
pub mod protocol;

// Include the tonic-generated code
pub mod tamad_service {
    include!(concat!(env!("OUT_DIR"), "/tamad.rs"));
}

// Re-export for convenience
pub use tamad_service::tamad_service_client::TamadServiceClient;
pub use tamad_service::tamad_service_server::{TamadService, TamadServiceServer};
```

`crates/tama-core/src/tamad/protocol.rs` — shared types (with serde derives for HTTP path):
```rust
use serde::{Serialize, Deserialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadModelRequest {
    pub provider_name: String,
    pub model_path: String,
    pub gpu_variant: String,
    pub params: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadModelResponse {
    pub endpoint_url: String,
    pub pid: i32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnloadModelRequest {
    pub provider_name: String,
    pub model_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub engine: String,
    pub version: String,
    pub status: String,
    pub gpu_variant: String,
}
```

`crates/tamad/src/main.rs` — CLI with `--addr` (default `0.0.0.0:50051`) and `--protocol` (default `grpc`, accepts `grpc`, `http`, or `both`) flags. Use `std::env::args` parsing (no clap dependency needed).

`crates/tamad/src/server.rs` — starts gRPC server (tonic) and/or HTTP server (axum) based on `--protocol`. Wire up stub handlers using the re-exported `TamadService` trait. All methods return "not implemented" except `HealthCheck` (returns `HealthResponse { status: "ok", version: env!("CARGO_PKG_VERSION") }`).

**Steps:**
- [ ] Add workspace deps: `tonic = "0.12"`, `prost = "0.13"`, `tonic-build = "0.12"`, `protoc-bin-vendored = "3"` in `Cargo.toml`
- [ ] Create `crates/tamad/Cargo.toml` with dependencies (include serde_json, anyhow)
- [ ] Add `tamad` to workspace members
- [ ] Add `tonic` and `prost` to `crates/tama-core/Cargo.toml` dependencies
- [ ] Add `tonic-build` and `protoc-bin-vendored` to `crates/tama-core/Cargo.toml` `[build-dependencies]`
- [ ] Add `build = "build.rs"` to `crates/tama-core/Cargo.toml` `[package]`
- [ ] Create `crates/tama-core/proto/tamad.proto` with service definition
- [ ] Create `crates/tama-core/build.rs` with `std::env::set_var("PROTOC", ...)` + `tonic_build::configure().compile_protos(...)`
- [ ] Define shared protocol types in `crates/tama-core/src/tamad/protocol.rs`
- [ ] Create `crates/tama-core/src/tamad/mod.rs` with generated module include + re-exports
- [ ] Add `pub mod tamad;` to `crates/tama-core/src/lib.rs`
- [ ] Implement `main.rs` with CLI args (std::env::args)
- [ ] Implement `server.rs` with stub handlers using `tama_core::tamad::TamadService` trait
- [ ] Run `cargo build --package tamad`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tamad -- -D warnings`
- [ ] Commit with message: "feat: add tamad daemon skeleton with gRPC/HTTP server"

**Acceptance criteria:**
- [ ] tama-core compiles with new build.rs (protoc-bin-vendored, tonic-build 0.12)
- [ ] Generated tonic code available via `tama_core::tamad::TamadServiceClient` and `TamadService`/`TamadServiceServer`
- [ ] tamad binary compiles
- [ ] tamad starts and listens on configured address
- [ ] Health check endpoint responds with `{"status": "ok"}`
- [ ] Other endpoints return "not implemented" (UNIMPLEMENTED gRPC status / 501 HTTP)
- [ ] All tests pass, clippy clean

---

### Task 8: tamad → tama client integration

**Context:**
tama needs to communicate with tamad instances. Add a client that speaks the tamad protocol (gRPC or HTTP based on the tamad's configured protocol). Uses the generated tonic client from `tama_core::tamad::TamadServiceClient`.

This task also wires up the health check in Task 4 — replace the stub with actual health check via the client.

**Client storage:** `ProxyState` is constantly cloned across tasks, so mutable collections use `Arc<RwLock<...>>`. The tamad client pool: `Arc<tokio::sync::RwLock<HashMap<String, TamadClient>>>`.

**Initialization:** `ProxyState::new` is synchronous — it cannot `await TamadClient::connect()`. Clients are connected lazily on first use.

**Lazy creation in tamad_health_check:** The method must look up the `TamadConnection` from `tamad_registry` (via `self.open_db()` + `queries::get_tamad`), construct `TamadClient::new(&conn_record)`, insert into the map, then perform the health check. If the tamad doesn't exist in the DB, return an error.

**ProxyState fields:** All fields are `pub(crate)`. The health-check handler in the `tama` crate needs a `pub` accessor method.

**gRPC endpoint construction:** `grpc://` scheme must be translated to `http://` for tonic (tonic uses HTTP/2). Use `tonic::transport::Endpoint::new(http::Uri::from_maybe_shared(...)?).connect().await`.

**Files:**
- Create: `crates/tama-core/src/tamad/client.rs`
- Modify: `crates/tama-core/src/proxy/types.rs` (add `tamad_clients: Arc<RwLock<HashMap<String, TamadClient>>>` field to `ProxyState`, update `Clone` impl, add `pub async fn tamad_health_check(&self, tamad_id: &str) -> Result<bool>` accessor)
- Modify: `crates/tama-core/src/proxy/state/mod.rs` (initialize `tamad_clients` with empty `Arc<RwLock<HashMap>>` in `ProxyState::new`)
- Modify: `crates/tama-core/src/tamad/mod.rs` (add `pub mod client;`)
- Modify: `crates/tama/src/api/tamads/manage.rs` (replace health check stub with `state.tamad_health_check(tamad_id)`)

**What to implement:**

```rust
pub struct TamadClient {
    connection: crate::providers::TamadConnection,
    channel: Option<tonic::transport::Channel>,  // gRPC channel (lazy)
    http_client: reqwest::Client,
}

impl TamadClient {
    pub fn new(connection: &crate::providers::TamadConnection) -> Self {
        Self {
            connection: connection.clone(),
            channel: None,
            http_client: reqwest::Client::new(),
        }
    }

    async fn ensure_channel(&mut self) -> Result<&tonic::transport::Channel> {
        if self.channel.is_none() && self.connection.protocol.is_grpc() {
            // Translate grpc:// to http:// for tonic
            let url = self.connection.url.replace("grpc://", "http://");
            let uri: http::Uri = url.parse().context("Invalid tamad URI")?;
            let endpoint = tonic::transport::Endpoint::new(uri);
            self.channel = Some(endpoint.connect().await?);
        }
        self.channel.as_ref().context("No channel available")
    }

    pub async fn load_model(&mut self, req: &LoadModelRequest) -> Result<LoadModelResponse> {
        match self.connection.protocol {
            crate::providers::Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.load_model(tonic::Request::new(/* convert req */)).await?;
                Ok(/* convert response */)
            }
            crate::providers::Protocol::Http => {
                // POST to {url}/load-model with JSON body
            }
        }
    }

    pub async fn unload_model(&mut self, req: &UnloadModelRequest) -> Result<()> {
        // Similar pattern
    }

    pub async fn health_check(&mut self) -> Result<bool> {
        match self.connection.protocol {
            crate::providers::Protocol::Grpc => {
                let channel = self.ensure_channel().await?.clone();
                let mut client = crate::tamad::TamadServiceClient::new(channel);
                let response = client.health_check(tonic::Request::new(
                    crate::tamad::tamad_service::Empty {}
                )).await?;
                Ok(response.get_ref().status == "ok")
            }
            crate::providers::Protocol::Http => {
                let resp = self.http_client
                    .get(&format!("{}/health", self.connection.url))
                    .send().await?;
                Ok(resp.status().is_success())
            }
        }
    }
}
```

Add to `ProxyState` in `proxy/types.rs`:
```rust
pub(crate) tamad_clients: Arc<tokio::sync::RwLock<HashMap<String, TamadClient>>>,
```
Update the manual `Clone` impl (just `Arc::clone()`).

Add `pub` accessor with lazy creation:
```rust
impl ProxyState {
    pub async fn tamad_health_check(&self, tamad_id: &str) -> Result<bool> {
        let mut clients = self.tamad_clients.write().await;

        // Lazy creation: if client not in map, load from DB and create
        let client = clients.entry(tamad_id.to_string()).or_insert_with(|| {
            // Load TamadConnection from tamad_registry
            let conn = self.open_db()
                .expect("db should be open")
                .query_row(
                    "SELECT id, name, url, protocol, token, status FROM tamad_registry WHERE id = ?1",
                    [tamad_id],
                    |r| {
                        Ok(crate::providers::TamadConnection {
                            id: r.get(0).unwrap(),
                            name: r.get(1).unwrap(),
                            url: r.get(2).unwrap(),
                            protocol: r.get(3).unwrap(),
                            token: r.get(4).unwrap(),
                            status: r.get(5).unwrap(),
                        })
                    }
                ).expect("tamad must exist in registry");
            TamadClient::new(&conn)
        });

        client.health_check().await
    }
}
```

Wait — the above has a problem: `or_insert_with` takes `&str` key but we need the full entry API. Use the proper pattern:

```rust
pub async fn tamad_health_check(&self, tamad_id: &str) -> Result<bool> {
    let mut clients = self.tamad_clients.write().await;

    // Check if client exists
    if let Some(client) = clients.get_mut(tamad_id) {
        return client.health_check().await;
    }

    // Lazy creation: load from tamad_registry
    let conn = self.open_db()
        .with_context(|| "DB not available")?;

    let tamad_record = crate::db::queries::get_tamad(&conn, tamad_id)
        .with_context(|| "Failed to look up tamad")?
        .ok_or_else(|| anyhow::anyhow!("tamad '{}' not found in registry", tamad_id))?

    let client = TamadClient::new(&tamad_record);
    clients.insert(tamad_id.to_string(), client);

    // Re-borrow the inserted client
    clients.get_mut(tamad_id)
        .ok_or_else(|| anyhow::anyhow!("Failed to get newly inserted client"))?
        .health_check()
        .await
}
```

Initialize in `ProxyState::new` (`proxy/state/mod.rs`):
```rust
tamad_clients: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
```

Replace the health check stub in `api/tamads/manage.rs` with `state.tamad_health_check(tamad_id).await`.

**Steps:**
- [ ] Write tests for client construction (use a mock axum server for HTTP path)
- [ ] Implement `TamadClient` with both gRPC and HTTP paths
- [ ] Add `pub mod client;` to `crates/tama-core/src/tamad/mod.rs`
- [ ] Add `tamad_clients` field to `ProxyState` in `proxy/types.rs`
- [ ] Update `ProxyState`'s manual `Clone` impl
- [ ] Add `tamad_health_check` pub accessor to `ProxyState` (with lazy DB lookup + client creation)
- [ ] Initialize `tamad_clients` with empty HashMap in `ProxyState::new`
- [ ] Replace health check stub in `api/tamads/manage.rs`
- [ ] Run `cargo nextest run --package tama-core -- tamad::client`
- [ ] Run `cargo nextest run --package tama --features ssr -- tamads`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: add tamad client with gRPC/HTTP support"

**Acceptance criteria:**
- [ ] TamadClient connects via gRPC (tonic::transport::Channel) or HTTP based on protocol
- [ ] gRPC scheme `grpc://` translated to `http://` for tonic
- [ ] load_model/unload_model/health_check methods work
- [ ] Client pool in ProxyState uses `Arc<RwLock<HashMap>>` pattern
- [ ] Lazy client creation: loads TamadConnection from DB on first use
- [ ] Health check API endpoint returns actual status from tamad
- [ ] All tests pass, clippy clean

---

## Phase 5: Migration & Rename

### Task 9: Backend → Installation migration

**Context:**
Rename all references from "backend" to "installation" across code, DB, API, and UI. This is the biggest task — it touches many files. The migration must be backward compatible during the transition period.

**Naming resolution:**
- `backend_installations` table → `provider_installations`
- `backend_configs` table → `provider_configs`
- `backend_queries.rs` → `installation_queries.rs` (avoids collision with `provider_queries.rs`)
- `api/backends/` → `api/installations/`
- API paths: `/tama/v1/backends/*` → `/tama/v1/installations/*` (old paths aliased with deprecation)
- `BackendType` → `InstallationType`
- `BackendInfo` → `InstallationInfo`
- `BackendManager` → `InstallationManager`
- `BackendSource` → `InstallationSource`
- `BackendConfig` → `InstallationConfig` (in DB queries)
- `BackendInstallationRecord` → `InstallationRecord`
- `BackendConfigRecord` → `InstallationConfigRecord`
- Query functions: `insert_backend_installation` → `insert_installation`, etc.

**Scope boundary:** The config section `backends` in `Config.backends: HashMap<String, BackendConfig>` (`config/types/mod.rs`) keeps its name during this migration (it's a separate concern — the structured config layer). Only DB tables, core module, API routes, and UI pages are renamed.

**SQLite RENAME TABLE behavior:** SQLite `ALTER TABLE ... RENAME TO` preserves all indexes attached to the table (they keep their old names but remain functional). We drop and recreate them with new names for consistency.

**Existing indexes** (from migrations):
- `idx_backend_installations_name` → `idx_provider_installations_name`
- `idx_backend_installations_name_variant` → `idx_provider_installations_name_variant`
- `idx_backend_configs_name_variant` → `idx_provider_configs_name_variant`
- `idx_backend_configs_logical_variant` → `idx_provider_configs_logical_variant`

The UNIQUE constraint on `(name, gpu_variant, version)` is a table-level constraint (auto-index), which survives the rename automatically — no need to recreate.

**Files to modify** (complete list from `grep -rln "backend_installations\|backend_configs"` + API/UI files):

tama-core:
- `crates/tama-core/src/backends/` → rename module to `installations/`
- `crates/tama-core/src/backends/docker/mod.rs`
- `crates/tama-core/src/backends/manager.rs`
- `crates/tama-core/src/backends/migration.rs`
- `crates/tama-core/src/backup/archive.rs`
- `crates/tama-core/src/backup/merge.rs`
- `crates/tama-core/src/config/resolve/mod.rs`
- `crates/tama-core/src/config/resolve/tests/server_resolution.rs`
- `crates/tama-core/src/config/types/mod.rs`
- `crates/tama-core/src/db/backfill/initial_backfill.rs`
- `crates/tama-core/src/db/backfill/migrate_toml_to_db.rs`
- `crates/tama-core/src/db/migrations.rs` (update migration references)
- `crates/tama-core/src/db/migrations/migrations_tests.rs` (update v20/v23 test queries)
- `crates/tama-core/src/db/mod.rs` (update `test_migration_v3_creates_backend_installations`)
- `crates/tama-core/src/db/queries/backend_queries.rs` → `installation_queries.rs`
- `crates/tama-core/src/db/queries/mod.rs` (update module name)
- `crates/tama-core/src/proxy/lifecycle/mod.rs`
- `crates/tama-core/src/lib.rs` (update module re-export)

tama:
- `crates/tama/src/api/backends/` → `crates/tama/src/api/installations/`
- `crates/tama/src/api/backends/list.rs`
- `crates/tama/src/api/backends/manage/config.rs`
- `crates/tama/src/api/backends/manage/types.rs`
- `crates/tama/src/api/backup.rs`
- `crates/tama/src/api.rs` (update module name)
- `crates/tama/src/router.rs` (update routes + add deprecated aliases)
- `crates/tama/src/lib.rs` (update Leptos Route path from `/tama/backends` to `/tama/installations`)
- `crates/tama/src/components/sidebar.rs` (update sidebar link)
- `crates/tama/src/components/job_log_panel.rs` (if references backend paths)
- `crates/tama/src/components/docker_register_modal.rs` (if references backend paths)
- `crates/tama/src/components/backend_card.rs` → rename or update
- `crates/tama/src/pages/` (update backend-related pages)
- `crates/tama/tests/router_ownership_test.rs` (update paths + count for aliases)
- `crates/tama/tests/backends_api.rs` → `installations_api.rs`
- `crates/tama/tests/server_test.rs`
- `crates/tama/src/api/openapi.rs` (update route paths)

Docs:
- `docs/api/backends.md` → `docs/api/installations.md`
- `docs/api/README.md` (update reference)
- `AGENTS.md` (update API docs reference)

Create:
- `crates/tama-core/src/db/migrations/_0048_rename_backend_to_provider.rs`

**What to implement:**

DB migration `_0048`:
```sql
-- Rename tables
ALTER TABLE backend_installations RENAME TO provider_installations;
ALTER TABLE backend_configs RENAME TO provider_configs;

-- SQLite RENAME TABLE preserves indexes (keeps old names).
-- Drop old indexes and recreate with new names for consistency.
DROP INDEX IF EXISTS idx_backend_installations_name;
CREATE INDEX idx_provider_installations_name ON provider_installations(name);

DROP INDEX IF EXISTS idx_backend_installations_name_variant;
CREATE INDEX idx_provider_installations_name_variant ON provider_installations(name, gpu_variant);

DROP INDEX IF EXISTS idx_backend_configs_name_variant;
CREATE INDEX idx_provider_configs_name_variant ON provider_configs(name, gpu_variant);

DROP INDEX IF EXISTS idx_backend_configs_logical_variant;
CREATE INDEX idx_provider_configs_logical_variant ON provider_configs(logical_id, gpu_variant);
```

Migration tuple: `(48, false, "...")` — `fk_off` is `false` (no inbound FKs to these tables).

Code changes — rename types:
- `BackendType` → `InstallationType` (update enum, Display, FromStr, all methods)
- `BackendInfo` → `InstallationInfo`
- `BackendSource` → `InstallationSource`
- `BackendManager` → `InstallationManager`
- `BackendInstallationRecord` → `InstallationRecord`
- `BackendConfigRecord` → `InstallationConfigRecord`
- Query functions: `insert_backend_installation` → `insert_installation`, etc.

API routes: `/tama/v1/backends/*` → `/tama/v1/installations/*`
Old paths aliased with `Deprecation: true` response header.

In `router_ownership_test.rs`, update all backend paths to installation paths and add deprecated alias paths. Update `EXPECTED_TAMA_PATH_COUNT` from 65 to 93 (14 backend paths renamed + 14 deprecated aliases added = +28).

Backup/restore compatibility: Pre-v48 backups have `backend_installations`/`backend_configs` tables. The backup merge code (`backup/merge.rs`) must handle both old and new table names. Add a check: if `provider_installations` doesn't exist but `backend_installations` does, use the old name.

**Steps:**
- [ ] Create DB migration `_0048_rename_backend_to_provider.rs` with correct SQL
- [ ] Add migration to `migrations.rs`, bump `LATEST_VERSION` to 48
- [ ] Rename `backends/` module to `installations/` in tama-core
- [ ] Update all type names (BackendType → InstallationType, BackendInfo → InstallationInfo, etc.)
- [ ] Rename `backend_queries.rs` to `installation_queries.rs`
- [ ] Rename query functions (insert_backend_installation → insert_installation, etc.)
- [ ] Update all SQL strings in renamed files (table names, column references)
- [ ] Update all files that reference backend tables (backup, config, backfill, lifecycle)
- [ ] Update `migrations_tests.rs` v20/v23 tests to use new table/index names
- [ ] Update `db/mod.rs::test_migration_v3_creates_backend_installations` to use new table/index names
- [ ] Rename `api/backends/` to `api/installations/` in tama crate
- [ ] Update router routes + add deprecated aliases (old paths → new paths with `Deprecation: true` header)
- [ ] Update `router_ownership_test.rs` (update backend paths, add alias paths, update count)
- [ ] Rename `tests/backends_api.rs` → `tests/installations_api.rs`
- [ ] Update `tests/server_test.rs` references
- [ ] Update `api/backup.rs` for old/new table name compatibility
- [ ] Update `api/openapi.rs` with renamed routes
- [ ] Update `lib.rs` Leptos Route path
- [ ] Update UI (sidebar link, components, pages)
- [ ] Rename `docs/api/backends.md` → `docs/api/installations.md`
- [ ] Update `docs/api/README.md` reference
- [ ] Update `AGENTS.md` API docs reference
- [ ] Run `cargo nextest run --package tama-core -- migrations`
  - Did migration tests pass? If not, fix index/table references.
- [ ] Run `cargo nextest run --package tama-core -- db::tests`
  - Did v3 test pass? If not, fix table/index assertions.
- [ ] Run `cargo nextest run --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit with message: "refactor: rename backend → installation everywhere"

**Acceptance criteria:**
- [ ] All code references updated (BackendType → InstallationType, BackendInfo → InstallationInfo, etc.)
- [ ] DB migration runs cleanly on existing data (renames tables + recreates indexes with correct names)
- [ ] Old API paths still work with `Deprecation: true` response header
- [ ] New API paths at `/tama/v1/installations/*`
- [ ] Backup/restore handles both old and new table names
- [ ] `router_ownership_test` passes with updated count (including aliases)
- [ ] `migrations_tests` passes (v20/v23 tests use new table/index names)
- [ ] `db/mod.rs` v3 test passes with new table/index names
- [ ] Full test suite passes
- [ ] Clippy clean across workspace

---

## Verification

After all tasks:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --package tama --features ssr --all-targets -- -D warnings
cargo clippy --package tamad --all-targets -- -D warnings
cargo nextest run --workspace
```
