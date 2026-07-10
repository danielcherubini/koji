# AGENTS.md - Tama Development Guide

This file documents build commands, code style, and conventions for the Tama project.

## Build & Testing

### Prerequisites

The project uses build performance optimizations that require these tools installed:

- **mold** — Fast linker (configured in `.cargo/config.toml`). Install: `sudo dnf install mold` (Fedora) or `sudo pacman -S mold` (Arch)
- **clang** — Used as linker driver for mold. Usually pre-installed.
- **cargo-nextest** — Faster parallel test runner. Install: `cargo install --locked cargo-nextest`

### Workspace Commands

```bash
# Build all crates
cargo build --workspace

# Release build
cargo build --release --workspace

# Run all tests (use cargo-nextest — ~40% faster than cargo test)
cargo nextest run --workspace

# Run tests for a specific crate
cargo nextest run --package tama-core

# Run a single test
cargo nextest run --package tama-core test_function_name

# Run tests with filtering
cargo nextest run --package tama-core -- backends::registry::tests::test_add

# Check formatting, clippy, and tests (full gate — run before commit/PR)
cargo check --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo nextest run --workspace
```

### Targeted Testing (during development)

**Never run the full workspace unless you're about to commit.** Use targeted commands that only compile + test the affected crate:

```bash
# Just the crate you're working on (~2-3s warm vs 9s workspace)
cargo nextest run --package tama-core

# Just the module (~1-2s warm)
cargo nextest run --package tama-core -- proxy::lifecycle

# Just one test (fastest)
cargo nextest run --package tama-core -- test_load_model_pipeline

# Quick compile check for the crate (no test runtime)
cargo check --package tama-core
```

**Workflow:**
1. **While coding:** `cargo nextest run --package <crate> -- <module>` after each change
2. **Before commit:** `cargo nextest run --workspace` (full gate)
3. **CI/PR:** full workspace + clippy + fmt

### Build Performance Config

The project is configured for fast incremental builds (`.cargo/config.toml` + `Cargo.toml` dev profile):

- **mold linker** — 2-10x faster linking vs GNU ld
- **`debug = "line-tables-only"`** — Enough for backtraces, no full DWARF for deps
- **`debug = false` for dependencies** — Skips debug info for 1000+ dep crates
- **`--profile debugging`** — Use when you need full debug info for a debugger

### Makefile

```bash
make build        # Release build
make install      # Install binary
make test         # Run all tests
make check        # fmt + clippy + test
make clippy       # Lint with -D warnings
make fmt          # Format all code
make run          # Run in dev mode (proxy + web UI)
make dev          # Leptos frontend dev server with hot reload
```

## Code Style

### Imports

- Group standard library imports first (`std::...`)
- Then external crates (`anyhow::...`, `tokio::...`)
- Then local module imports (`crate::...`)
- Use `use` for single imports, `use crate::...::*` for re-exports
- No unused imports
- Prefer `use anyhow::{anyhow, Context, Result}` over `use anyhow::Result` when using multiple items

### Formatting

- `cargo fmt --all` for formatting
- 4-space indentation
- No trailing whitespace
- Blank line between logical blocks
- Max line length: 100 chars (wrap naturally)

### Types

- Prefer `Result<T, E>` over `Option<T>` for fallible operations
- Use `anyhow::Result` (alias: `Result`) for error handling
- Use `anyhow::Context` for adding context to errors
- Structs derive `Debug`, `Clone`, `Serialize`, `Deserialize` when appropriate
- Use `#[derive(Default)]` for structs with sensible defaults

### Naming Conventions

- `snake_case` for functions, variables, modules
- `PascalCase` for types, structs, enums
- `UPPER_SNAKE_CASE` for constants
- Prefix test functions with `test_`
- Prefix private functions with `_` (e.g., `_hf_api()`)

### Error Handling

- Return `Result<T, E>` instead of `unwrap()` or `expect()` in public APIs
- Use `.with_context()` to add context to errors
- Use `anyhow::bail!` for early returns with errors
- Avoid `unreachable!()` - return errors for edge cases instead
- Chain errors with `?` operator where appropriate

### Documentation

- Add doc comments to public functions and structs
- Use `///` for single-line docs, consecutive `///` lines for multi-line docs or `/** ... */` for block docs
- Include `///` before `#[test]` for test documentation
- Document parameters and return values

## Testing

### Test Organization

- Tests in `#[cfg(test)]` modules at bottom of source files
- Group related tests with `mod tests { ... }`
- Use `#[tokio::test]` for async tests

### Test Patterns

```rust
#[test]
fn test_function_name() {
    // Arrange
    let input = "test input";
    
    // Act
    let result = my_function(input);
    
    // Assert
    assert_eq!(result, expected);
}

#[tokio::test]
async fn test_async_function() {
    let result = my_async_function().await;
    assert!(result.is_ok());
}

#[test]
#[serial]
fn test_concurrent_access() {
    // Use serial attribute for tests with shared state
}
```

### Test Helpers

- Create helper functions in `tests/` module
- Use `tempfile::tempdir()` for temporary directories
- Use `assert_matches!` for pattern matching on Results
- Use `assert!(condition, "custom message")` for custom error messages

## Project Structure

```text
tama/
├── crates/
│   ├── tama-core/      # Core library (types, models, logic)
│   ├── tama/           # Main binary with web control plane (WASM + SSR)
│   │   └── css/        # CSS source files (edit these, NOT dist/)
│   └── tama-mock/      # Mock utilities for testing
├── config/              # Configuration templates
├── docs/                # Documentation
├── installer/           # Windows installer scripts
└── target/              # Build artifacts (ignored)
```

**CSS convention:** Always edit files in `crates/tama/css/`. The `crates/tama/dist/` directory is Trunk build output (untracked in git) — never edit files there directly. Trunk regenerates `dist/` from `css/` during build.

## Patterns

### Metrics: `watch::Sender<HashMap>` for SP/MP

Use `tokio::sync::watch::Sender<HashMap<K, V>>` for single-producer multi-consumer metrics distribution. The producer writes the full snapshot on each update; consumers receive the latest map without holding locks during iteration.

Example: `ProxyState.inference_stats` — one writer (metrics loop) pushes per-server tok/s, multiple readers (SSE handlers, dashboard) consume without blocking the producer.

### Version Management: `INSERT OR REPLACE` with unique index

When managing versions of an entity (backends, models, etc.), use `INSERT OR REPLACE` with a unique index on the identifying columns and deactivate old versions on insert.

Example: Backends use unique index on `(name, gpu_variant, version)` and `UPDATE ... SET active = 0 WHERE name = ? AND version != ?` to ensure only one active version per backend.

## Conventions

### TDD Approach

1. Write failing test
2. Verify it fails
3. Implement minimal code
4. Verify test passes
5. Refactor if needed
6. Commit frequently

### Code Review

- Follow DRY principle
- No premature optimization (YAGNI)
- Prefer composition over inheritance
- Keep functions small and focused
- Add tests for edge cases

### Git Workflow

- Feature branches from `main`
- Descriptive commit messages
- `feat:`, `fix:`, `chore:`, `docs:` prefixes
- Push to remote before merging

### Version Bumping

When bumping the version, update **all** of these files:

| File | Field |
|------|-------|
| `Cargo.toml` | `[workspace.package] version` |
| `crates/tama-core/Cargo.toml` | `[package] version` |
| `crates/tama/Cargo.toml` | `[package] version` |
| `crates/tama-mock/Cargo.toml` | `[package] version` |

After bumping, run `cargo fmt --all` before committing — CI will fail on formatting errors.

## TAMA Management API

The TAMA proxy exposes a management REST API for querying and modifying models, backends, downloads, benchmarks, and more.

### Environment Variables

Always use the environment variables for API access — never hardcode URLs or tokens:

| Variable | Purpose |
|----------|---------|
| `$TAMA_URL` | Base URL of the TAMA proxy (e.g. `http://127.0.0.1:18910`) |
| `$TAMA_TOKEN` | Bearer token for authentication |

### API Docs

Full API reference lives in `docs/api/`. Read the relevant file before making API calls:

- `docs/api/models.md` — Model CRUD, refresh, verify
- `docs/api/backends.md` — Backend install, update, activate, remove
- `docs/api/aliases.md` — Alias management
- `docs/api/downloads.md` — Download progress monitoring
- `docs/api/huggingface.md` — HF metadata and quant listing
- `docs/api/config.md` — Global config read/save
- `docs/api/benchmarks.md` — Run and manage benchmarks
- `docs/api/updates.md` — Check and apply updates
- `docs/api/backup.md` — Backup and restore
- `docs/api/self-update.md` — Binary self-update
- `docs/api/system.md` — System capabilities
- `docs/api/logs.md` — Log retrieval
- `docs/api/sse.md` — SSE event streams
- `docs/api/jobs.md` — Async job tracking
- `docs/api/errors.md` — Error response format

### Usage Pattern

```bash
# List models
curl -s -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/tama/v1/models" | jq .

# Get a single model by ID
curl -s -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/tama/v1/models/306" | jq .

# Create a model
curl -s -X POST -H "Authorization: Bearer $TAMA_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"repo_id": "owner/repo", "backend": "llama_cpp"}' \
  "$TAMA_URL/tama/v1/models" | jq .
```

All API paths are prefixed with `/tama/v1/`. Always prepend `$TAMA_URL` and include the `Authorization: Bearer $TAMA_TOKEN` header.

## No External Rules

This project does not use Cursor rules (.cursor/) or Copilot instructions (.github/copilot-instructions.md).