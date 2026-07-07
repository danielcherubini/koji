# Remove CLI, Promote Web UI to Binary

**Goal:** Remove `tama-cli` and promote `tama-web` to be the `tama` binary — zero arguments, starts proxy + web UI server.
**Architecture:** `tama-web` renamed to `tama`, gains a `[[bin]]` target (`main.rs`) that wires `tama-core` proxy + axum web routes together. The library (`[lib]`) stays for WASM frontend builds.
**Tech Stack:** Rust, axum, tokio, leptos (WASM), cargo workspace

---

### Task 1: Rename crate directory and update Cargo.toml

**Context:**
The `tama-web` crate becomes the main `tama` crate. The directory is renamed and the Cargo.toml is updated to include a binary target alongside the existing library target. The library name stays `tama_web` internally to avoid collision with the binary name.

**Files:**
- Rename: `crates/tama-web/` → `crates/tama/`
- Modify: `crates/tama/Cargo.toml` (was `tama-web/Cargo.toml`)

**What to implement:**
1. Rename the directory: `git mv crates/tama-web crates/tama`
2. In `crates/tama/Cargo.toml`:
   - Change `[package] name = "tama-web"` to `name = "tama"`
   - Add `[[bin]]` section: `name = "tama"`, `path = "src/main.rs"`
   - Change `[lib]` to explicitly set `name = "tama_web"`, `crate-type = ["cdylib", "rlib"]` (add `rlib` if not present)
   - Change `default` features to `["ssr"]` (was `["web-ui"]` or similar — check current default)
   - In `[features]`, rename `ssr` feature's `tama-core/web-ui` dep if needed (keep as-is if it already exists)
   - Update any internal `tama-web` self-references
   - **Move packaging metadata** from the old `crates/tama-cli/Cargo.toml`:
     - Copy `[package.metadata.deb]` section (maintainer, assets, etc.)
     - Copy `[package.metadata.generate-rpm]` section
     These define the .deb and .rpm package contents. The binary path (`target/release/tama`) stays the same.
3. Do NOT modify any `.rs` files in this task — only the directory rename and Cargo.toml

**Steps:**
- [ ] Run `git mv crates/tama-web crates/tama`
- [ ] Edit `crates/tama/Cargo.toml`:
  - Set `name = "tama"` in `[package]`
  - Add `[[bin]] name = "tama" path = "src/main.rs"`
  - Add `[lib] name = "tama_web" path = "src/lib.rs" crate-type = ["cdylib", "rlib"]`
  - Set `default = ["ssr"]` in `[features]`
- [ ] Run `cargo check --package tama --features ssr`
  - Expect errors about missing `main.rs` — that's fine, it's created in Task 2
  - The library (`cargo check --package tama --features csr`) should compile without errors
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: rename tama-web to tama, add binary target"

**Acceptance criteria:**
- [ ] Directory is `crates/tama/` (not `crates/tama-web/`)
- [ ] Package name is `tama`
- [ ] `[[bin]]` target exists pointing to `src/main.rs`
- [ ] Library name is `tama_web` (internal Rust name)
- [ ] `default` features include `ssr`

---

### Task 2: Create main.rs — server startup

**Context:**
The new binary needs a `main.rs` that starts the server. It loads config, creates the ProxyState, builds the web routes, and listens. This replaces the old `tama-cli`'s `serve` command as the default behavior. No CLI arguments — everything comes from the config file.

**Files:**
- Create: `crates/tama/src/main.rs`
- Reference (read for patterns): `crates/tama-cli/src/handlers/serve.rs` (proxy startup), `crates/tama/src/router.rs` (build_web_routes)

**What to implement:**
Create `crates/tama/src/main.rs`. **Do NOT write this from scratch** — the entire server startup logic already exists in `crates/tama-cli/src/handlers/serve.rs` (the `start_proxy_server` function). Your job is to extract that logic into a zero-arg `main()` that reads host/port/auto_unload/idle_timeout from the config file instead of CLI args.

**Read `crates/tama-cli/src/handlers/serve.rs` FIRST** — it is the authoritative reference. The main.rs must replicate its behavior exactly, just without CLI argument parsing.

**Key details from serve.rs that MUST be included (do not skip any):**

1. **`ProxyState::new` takes TWO arguments:** `config` AND `db_dir` (an `Option<PathBuf>`). Get `db_dir` from `tama_core::config::Config::config_dir().ok()`.

2. **DB backfill and migrations** — Before creating `ProxyState`, open the DB and run:
   - `tama_core::db::backfill::run_initial_backfill` (if `needs_backfill`)
   - `tama_core::db::backfill::migrate_backend_registry_toml`
   - `tama_core::db::backfill::migrate_toml_to_db`
   These are in `serve.rs` lines ~50–80. Copy this logic exactly.

3. **Web UI feature gate** — The serve logic is wrapped in `#[cfg(feature = "web-ui")]` / `#[cfg(not(feature = "web-ui"))]`. Since the binary always has `ssr` (which enables `tama-core/web-ui`), the main.rs should use the web-ui path. Include the `#[cfg(feature = "ssr")]` gate.

4. **Binary version** — After creating state, set: `state_inner.web_binary_version = env!("CARGO_PKG_VERSION").to_string();`

5. **Logs directory** — Create logs dir: `std::fs::create_dir_all(updated_config.logs_dir()?)`

6. **Unified router** — Use `ProxyServer`, NOT raw `axum::serve`:
   ```rust
   let web_routes = tama_web::router::build_web_routes();
   let server = ProxyServer::new(state.clone()).await;
   let app = server.into_unified_router(web_routes).await;
   ```

7. **Shutdown cleanup** — The `on_shutdown` closure must:
   - Kill children of any active backend job (`jobs.kill_children`)
   - Unload TTS backends (`cleanup_state.unload_tts_backend`)
   See serve.rs lines ~100–120 for the exact code.

8. **Listener run** — Use `tama_core::proxy::server::listener::run(app, addr, Some(on_shutdown), None).await` — this handles OS signals (SIGTERM/SIGINT) and graceful shutdown. Do NOT use `axum::serve` directly.

**Suggested structure (follow serve.rs closely):**

```rust
use std::net::SocketAddr;
use std::sync::Arc;
use anyhow::Result;
use tama_core::config::Config;
use tama_core::proxy::ProxyServer;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load config (this runs initial migrations internally)
    let config = Config::load()?;

    // Set up HF_TOKEN from config
    if let Some(token) = &config.general.hf_token {
        if !token.is_empty() {
            std::env::set_var("HF_TOKEN", token);
            tracing::info!("HF_TOKEN configured from config file");
        }
    }

    // Parse host:port from config
    let host = config.proxy.host.clone();
    let port = config.proxy.port;
    let (host_addr, warning) = match host.parse::<std::net::IpAddr>() {
        Ok(addr) => (addr, false),
        Err(_) => (std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)), true),
    };
    let addr = SocketAddr::new(host_addr, port);
    if warning {
        tracing::warn!("Invalid host '{}' - using 127.0.0.1", host);
    }

    tracing::info!("Starting tama on {}", addr);
    tracing::info!("Auto-unload: {} (idle timeout: {}s)", config.proxy.auto_unload, config.proxy.idle_timeout_secs);

    // DB directory and migrations (copy from serve.rs exactly)
    let db_dir = Config::config_dir().ok();
    if let Some(ref dir) = db_dir {
        // ... run_backfill, migrate_backend_registry_toml, migrate_toml_to_db
    }

    // Create ProxyState with TWO arguments
    let state = Arc::new(tama_core::proxy::ProxyState::new(config.clone(), db_dir));

    #[cfg(feature = "ssr")]
    {
        // Ensure logs directory exists
        if let Some(ref dir) = config.logs_dir().ok() {
            let _ = std::fs::create_dir_all(dir);
        }

        // Set binary version
        let mut state_inner = (*state).clone();
        state_inner.web_binary_version = env!("CARGO_PKG_VERSION").to_string();
        let state = Arc::new(state_inner);

        // Build unified router
        let web_routes = tama_web::router::build_web_routes();
        let server = ProxyServer::new(state.clone()).await;
        let app = server.into_unified_router(web_routes).await;

        // Shutdown cleanup (copy from serve.rs exactly)
        let cleanup_state = Arc::clone(&state);
        let on_shutdown = async move {
            // Kill children of active job
            // Unload TTS backends
        };

        // Run with signal handling
        tama_core::proxy::server::listener::run(app, addr, Some(on_shutdown), None).await
    }

    #[cfg(not(feature = "ssr"))]
    {
        let server = ProxyServer::new(state.clone()).await;
        server.run(addr, None).await
    }
}
```

**Do NOT:**
- Add any CLI argument parsing (no clap, no structopt)
- Add any subcommands
- Use `axum::serve` directly (use `listener::run`)
- Skip DB migrations or shutdown cleanup
- Change any existing API routes or proxy behavior

**Do NOT:**
- Add any CLI argument parsing (no clap, no structopt)
- Add any subcommands
- Change any existing API routes or proxy behavior

**Steps:**
- [ ] Read `crates/tama-cli/src/handlers/serve.rs` to understand proxy startup tasks
- [ ] Read `crates/tama/src/router.rs` to understand `build_web_routes()` signature
- [ ] Create `crates/tama/src/main.rs` with the server startup code above
- [ ] Adapt the proxy background tasks from `serve.rs` (auto-load, idle timeout, etc.)
  - First, list every `tokio::spawn` or background task found in `serve.rs` to ensure none are missed
  - Replicate each task in main.rs with the same logic
- [ ] Run `cargo check --package tama --features ssr`
  - Did it compile? If not, fix import paths or missing deps
- [ ] Run `cargo build --package tama --features ssr`
  - Did it build? If not, fix issues
- [ ] Quick smoke test: start the binary briefly (`timeout 3 target/debug/tama || true`) and verify it doesn't panic on startup (config file error is expected if none exists, but no panics)
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add main.rs server startup (zero-arg binary)"

**Acceptance criteria:**
- [ ] `cargo build --package tama --features ssr` succeeds
- [ ] `main.rs` has no CLI argument parsing
- [ ] Server loads config from default path (host, port, auto_unload, idle_timeout all from config)
- [ ] DB migrations run (backfill, migrate_backend_registry_toml, migrate_toml_to_db)
- [ ] `ProxyState::new` called with both `config` and `db_dir` arguments
- [ ] `web_binary_version` set from `CARGO_PKG_VERSION`
- [ ] Unified router built via `ProxyServer::new(...).into_unified_router(...)`
- [ ] Shutdown cleanup kills job children and unloads TTS backends
- [ ] Uses `tama_core::proxy::server::listener::run` (not `axum::serve`)
- [ ] Graceful shutdown on SIGINT/SIGTERM

---

### Task 3: Update workspace Cargo.toml and all dependency references

**Context:**
The workspace and all crates that reference `tama-cli` or `tama-web` need their paths and names updated. This ensures `cargo build --workspace` works with the new structure.

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/tama-core/Cargo.toml` (if it references tama-web)
- Modify: `crates/tama-mock/Cargo.toml` (if it references tama-web or tama-cli)
- Modify: Any other `.toml` files that reference the old crate names

**What to implement:**
1. In root `Cargo.toml`:
   - Remove `"crates/tama-cli"` from `members`
   - Change `"crates/tama-web"` to `"crates/tama"` in `members`
2. In `crates/tama-core/Cargo.toml`:
   - If there's a `tama-web` dependency, change path to `../tama` and name to `tama`
   - The `web-ui` feature — check if it references `tama-web` and update
3. In `crates/tama-mock/Cargo.toml`:
   - Update any `tama-web` or `tama-cli` references
4. Search all `.toml` files for `tama-web` and `tama-cli` references and update paths

**Steps:**
- [ ] Run `grep -r "tama-web\|tama-cli" --include="*.toml" .` to find all references
- [ ] Update root `Cargo.toml`: remove `tama-cli` from members, rename `tama-web` → `tama`
- [ ] Update `crates/tama-core/Cargo.toml`: fix any `tama-web` path references
- [ ] Update `crates/tama-mock/Cargo.toml`: fix any references
- [ ] Update any other `.toml` files found by grep
- [ ] Run `cargo check --workspace`
  - Did it compile? If not, fix remaining references
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: update workspace and dependency references for tama rename"

**Acceptance criteria:**
- [ ] `cargo check --workspace` succeeds
- [ ] No `.toml` files reference `tama-web` or `tama-cli` paths
- [ ] Workspace members list is correct: `tama-core`, `tama`, `tama-mock`

---

### Task 4: Update CI and release workflows

**Context:**
The GitHub Actions workflows reference `crates/tama-web` for the WASM build and `tama-cli` for package building. These paths need updating.

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`

**What to implement:**
1. In `.github/workflows/ci.yml`:
   - Change `working-directory: crates/tama-web` to `working-directory: crates/tama` (for trunk build step)
   - Remove any `--package tama-cli` references (if any exist)
   - The `cargo clippy --package tama-web` step (if exists) → `--package tama`
2. In `.github/workflows/release.yml`:
   - Change `working-directory: crates/tama-web` to `working-directory: crates/tama` (for trunk build)
   - Change `cargo deb -p tama` — this should already be correct (the binary is `tama`)
   - Change `cargo generate-rpm -p crates/tama-cli` → `cargo generate-rpm -p crates/tama`
   - Verify artifact paths still produce `tama` binary (they should, no change needed)
   - Update release body text: remove CLI-specific install instructions, keep web UI URL

**Steps:**
- [ ] Update `.github/workflows/ci.yml`: change `crates/tama-web` → `crates/tama` in trunk build step
- [ ] Update `.github/workflows/release.yml`:
  - Change `crates/tama-web` → `crates/tama` in trunk build step
  - Change `cargo generate-rpm -p crates/tama-cli` → `-p crates/tama`
  - Update release body to remove CLI commands, keep web UI URL
- [ ] Run `actionlint .github/workflows/ci.yml .github/workflows/release.yml` if available, or visually verify YAML syntax
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "ci: update workflow paths for tama crate rename"

**Acceptance criteria:**
- [ ] CI workflow builds WASM from `crates/tama/`
- [ ] Release workflow builds packages from `crates/tama/`
- [ ] No references to `tama-cli` or `tama-web` in workflows
- [ ] YAML syntax is valid

---

### Task 5: Delete tama-cli and update docs

**Context:**
With the new binary in place and all references updated, the old `tama-cli` crate can be deleted. Docs (README, AGENTS.md) need updating to reflect the new structure.

**Files:**
- Delete: `crates/tama-cli/` (entire directory)
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/MIGRATION.md` (if it references CLI commands)

**What to implement:**
1. Delete `crates/tama-cli/`: `rm -rf crates/tama-cli/` (git will track the deletion)
2. Update `README.md`:
   - Change any `tama serve` → `tama`
   - Remove CLI-specific usage sections
   - Update install instructions to reflect web UI as primary interface
3. Update `AGENTS.md`:
   - Update project structure section (remove `tama-cli`, update `tama-web` → `tama`)
   - Update any code examples that reference CLI commands
4. Check `docs/` for any CLI command references and update

**Steps:**
- [ ] Run `git rm -r crates/tama-cli/`
- [ ] Update `README.md`: replace `tama serve` with `tama`, remove CLI usage sections
- [ ] Update `AGENTS.md`: update project structure, remove `tama-cli` references
- [ ] Check and update `docs/` files for CLI references
- [ ] Run `cargo check --workspace`
  - Did it compile? If not, there are remaining references to fix
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it pass? If not, fix warnings
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: remove tama-cli, update docs for web-only interface"

**Acceptance criteria:**
- [ ] `crates/tama-cli/` is deleted
- [ ] `cargo check --workspace` succeeds
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] README and AGENTS.md reflect new structure
- [ ] No remaining references to `tama-cli` in the codebase

---

### Task 6: Final verification — full build and test

**Context:**
A final end-to-end verification that everything builds, tests pass, and the binary works.

**Files:**
- No files to modify — verification only

**What to implement:**
Run the full check pipeline and verify the binary produces the expected output.

**Steps:**
- [ ] Run `cargo fmt --all --check`
  - Did it pass? If not, run `cargo fmt --all` and re-check
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
  - Did it pass? If not, fix warnings
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
  - Did it pass?
- [ ] Run `cargo test --workspace -- --nocapture`
  - Did all tests pass?
- [ ] Run `cargo build --release --package tama`
  - Did it build? Verify `target/release/tama` exists
- [ ] Run `target/release/tama --help` or just `target/release/tama` briefly (it should start the server, then Ctrl+C to stop)
  - Does it start without errors? (It will fail if no config exists, which is expected)
- [ ] Run `cd crates/tama && trunk build` (verify WASM frontend still builds)
  - Did it succeed?
- [ ] If all checks pass, the refactor is complete

**Acceptance criteria:**
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo build --release` produces `target/release/tama`
- [ ] WASM frontend builds with trunk
- [ ] Binary starts without panic (config file error is expected if none exists)
