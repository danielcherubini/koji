# Use SQLite for all persistent state

## Context and Problem Statement

Tama stores configuration (global settings, backend configs, model configs, sampling templates, TTS configs, download queue, metrics history, aliases, benchmarks) across multiple TOML files and a SQLite database. TOML config scattered state across files — `config.toml`, per-model configs in `configs/`, profiles in `profiles/` — making backups, migrations, and edits error-prone. Editing TOML by hand is fragile, and the web UI had to read/write multiple files with no transactional safety.

## Decision Drivers

* Single source of truth for all app state
* Transactional safety — edits either fully succeed or fully roll back
* Easy backup/restore (single file)
* Web UI can edit state without file I/O race conditions
* Queryable — filtering, sorting, and searching without loading everything into memory

## Considered Options

* SQLite (single file, embedded, no separate server)
* TOML files (status quo)
* JSON files
* Separate database server (PostgreSQL, etc.)

## Decision Outcome

Chosen option: "SQLite", because it provides ACID transactions, is embedded in the binary (no external dependency), backs up as a single file, and supports queries for filtering and sorting. The migration from TOML is idempotent — on first run, existing `config.toml` is read and all data is inserted into the database, then the file is renamed to `config.toml.migrated`.

### Consequences

* Good, because all state is in one file — backup/restore is a single `cp`
* Good, because the web UI can edit config without file locking concerns
* Good, because migrations can add columns/tables without breaking existing data
* Bad, because SQLite is a single-writer database — concurrent writes are serialized (not a problem for Tama's workload)
* Bad, because `config.toml` is no longer human-editable — users must use the web UI or binary APIs

### Confirmation

The migration runs once at startup (`migrate_toml_to_db()`). The `config.toml` endpoint returns 410 Gone after migration. All config reads/writes go through DB query functions. The workspace test suite verifies migration idempotency and round-trip correctness.

## Pros and Cons of the Options

### SQLite

Embedded, zero-config, ACID transactions, single-file backup.

* Good, because no external service to manage
* Good, because Rust's `rusqlite` is mature and well-tested
* Good, because migrations are versioned and idempotent
* Bad, because concurrent writes are serialized (acceptable for Tama)
* Bad, because TOML is no longer the source of truth

### TOML files (status quo)

Human-readable, git-friendly, but scattered across multiple files with no transactions.

* Good, because human-editable and version-controlled
* Bad, because editing requires file locking to avoid corruption
* Bad, because backup means copying a directory tree
* Bad, because no query capability — must load and parse everything

### JSON files

Similar to TOML but no type safety without schemas.

* Good, because widely supported
* Bad, because no human-friendly formatting conventions
* Bad, because same scattering problems as TOML

### Separate database server

Full-featured but overkill for a single-user local app.

* Good, because supports concurrent writes and complex queries
* Bad, because requires installing and managing a separate service
* Bad, because adds network dependency and attack surface

## More Information

* PR #128: [migrate global config from TOML to SQLite](https://github.com/danielcherubini/tama/pull/128)
* Implementation plan: `docs/plans/2026-06-29-config-to-db.md`
