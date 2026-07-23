# Centralized managers for data access

All database access is funneled through two centralized managers: `BackendManager` (backend CRUD, discovery, installation, resolution) and `ModelManager` (model config CRUD, file tracking, pull tracking, download queue, update checks). These replaced 29+ scattered `db::open()` calls and direct `db::queries::*` usage across web handlers, CLI commands, and proxy lifecycle code.

Each manager wraps a `rusqlite::Connection` and exposes typed methods for all operations in its domain. Callers open a fresh instance per operation (SQLite `Connection` is `Send` but not `Sync`). The managers provide convenience methods like `save_model_config()` that handle the `ModelConfig` → `ModelConfigRecord` conversion, avoiding callers duplicating 10+ lines of record construction.

The `ModelManager::transaction()` helper wraps `conn.transaction()` for atomic multi-step operations (e.g. delete model + cascade delete files + queue entry). Raw `conn()` access is available for async functions that must not hold `&Connection` across `.await`.

This pattern was chosen over an ORM (e.g. SQLx, Diesel) because Tama's schema is simple and stable — the overhead of compile-time query checking or runtime reflection was not justified. It was chosen over a generic repository pattern because the two managers map cleanly to the two core domains (backends and models) without abstraction leakage.

**Status:** amended (2026-07-18, plan-160)

**Considered Options:**

- **Scattered DB calls** (status quo) — every caller opens its own connection and calls `db::queries::*` directly. Hard to track usage, inconsistent error handling, no encapsulation
- **Centralized managers** (chosen) — two domain-specific structs encapsulate all DB access. Easy to audit, consistent patterns, convenience methods reduce boilerplate
- **ORM** (SQLx, Diesel) — compile-time query checking and type safety, but overkill for Tama's simple schema and adds build complexity
- **Generic repository pattern** — abstracts over data sources, but Tama has a single SQLite database with no need for interchangeability

**Consequences:**

- Good, because all DB access is visible in two files — easy to audit, add indexes, or change queries
- Good, because convenience methods eliminate boilerplate (e.g. `save_model_config` handles conversion)
- Good, because `transaction()` helper ensures atomic multi-step operations
- Bad, because each caller opens a new `Connection` (SQLite open is cheap but not free)
- Bad, because raw `conn()` escape hatch is needed for async contexts (holds coupling to rusqlite)

## Amendment (2026-07-18, plan-160): Repository is the API-layer entry point

The "managers over repository" decision is amended for the `tama` API layer:

- `db::repository::Repository` is the single data-access entry point for ALL
  handlers in `crates/tama/src/api/**` — reads AND writes. The model-domain
  write methods (`save_model_config`, `get_files`, `upsert_file`, `delete_file`,
  `upsert_pull`, `get_pull`, `delete_config`, `update_verification`) were
  absorbed from `ModelManager`.
- `BackendManager` and `ModelManager` remain for tama-core-internal proxy
  lifecycle use (`ProxyState`, `PullQueueService`, lifecycle/update code).
  Their raw-connection escape hatches (`conn()`, `transaction()`) are
  `pub(crate)`; `tama_core::db` no longer publicly re-exports
  `rusqlite::Connection`.
- One struct per table: the `db::queries` record types
  (`ModelConfigRecord`, `ModelFileRecord`, `AliasResponse`, `BenchmarkRow`,
  `PullQueueItem`, `UpdateCheckRecord`, `ModelPullRecord`) are the canonical
  row representations returned by both `Repository` and the managers. The
  parallel DTO hierarchy in `db::repository` was deleted.
- One shared `Repository` is constructed at startup and stored in
  `WebState` (`Option<Arc<Mutex<Repository>>>`), so migrations run once —
  handlers no longer call `Repository::open` per request.
- API keys use `proxy::api_keys::ApiKeyStore`, a small struct borrowing a
  `&Connection`, instead of public free functions taking raw connections.

Rationale: two competing access layers (managers + Repository) with
method-level overlap forced handlers to open both (two SQLite connections
per request) and produced a field-for-field duplicated DTO hierarchy.
Centralizing on `Repository` for the API layer keeps ADR-0017's funnel
benefit (one auditable file per access pattern) where the API actually
lives, without disturbing the proxy's lifecycle internals.
