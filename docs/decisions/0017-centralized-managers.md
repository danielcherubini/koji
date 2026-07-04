# Centralized managers for data access

All database access is funneled through two centralized managers: `BackendManager` (backend CRUD, discovery, installation, resolution) and `ModelManager` (model config CRUD, file tracking, pull tracking, download queue, update checks). These replaced 29+ scattered `db::open()` calls and direct `db::queries::*` usage across web handlers, CLI commands, and proxy lifecycle code.

Each manager wraps a `rusqlite::Connection` and exposes typed methods for all operations in its domain. Callers open a fresh instance per operation (SQLite `Connection` is `Send` but not `Sync`). The managers provide convenience methods like `save_model_config()` that handle the `ModelConfig` → `ModelConfigRecord` conversion, avoiding callers duplicating 10+ lines of record construction.

The `ModelManager::transaction()` helper wraps `conn.transaction()` for atomic multi-step operations (e.g. delete model + cascade delete files + queue entry). Raw `conn()` access is available for async functions that must not hold `&Connection` across `.await`.

This pattern was chosen over an ORM (e.g. SQLx, Diesel) because Tama's schema is simple and stable — the overhead of compile-time query checking or runtime reflection was not justified. It was chosen over a generic repository pattern because the two managers map cleanly to the two core domains (backends and models) without abstraction leakage.

**Status:** accepted

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
