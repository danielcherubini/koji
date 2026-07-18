# Cleanup Plan

**Goal:** Remove the accumulated dead code, unused dependencies, blanket lint allows, stray `println!`s, and style drift identified by the 2026-07-18 audit (F26, F34, F38, F39) — roughly 2,000 lines of dead code and 6 dependency entries — with each item verified against `rg` before deletion.

**Architecture:** Pure deletions and mechanical replacements; no behavior change to any live path. Tasks are ordered: file/module deletions first (Task 1), then dependency + lint-allow cleanup (Task 2, easier once dead files are gone), then the small dead-code batch (Task 3), then `println!`→`tracing` (Task 4, which only converts call sites that survive Task 3), then style (Task 5). Every deletion item was verified to have zero callers during planning; the executing agent must re-verify each with the given `rg` command immediately before deleting and DROP any item that has gained a caller.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio, Leptos (WASM)

---

### Task 1: Delete dead modules and files (F26)

**Context:**
Four verified-dead chunks: (a) `crates/tama-core/src/config/migrate/mod.rs` (171 lines) — `cleanup_stale_mmproj_args` is re-exported but never called and `rename_legacy_directories` has zero callers; (b) `crates/tama-core/src/config/rename_legacy.rs` — the kronk→tama directory migration carries `TODO(v1.60)` and the workspace is at 2.0.0, so the one-time migration is overdue for removal; (c) five Leptos components with zero external references (verified: `rg` finds only their `mod` declarations and their own files) — `backup_section.rs` is already commented out in `components/mod.rs:10`; (d) `crates/tama/src/jobs.rs` (606 lines) — an orphan file with no `mod jobs` declaration anywhere, so it is not even compiled. All deletions were verified with `rg` during planning; re-verify before each `rm`.

**Files:**
- Delete: `crates/tama-core/src/config/migrate/` (whole directory)
- Delete: `crates/tama-core/src/config/rename_legacy.rs`
- Delete: `crates/tama/src/components/sampling_templates_section.rs`
- Delete: `crates/tama/src/components/supervisor_section.rs`
- Delete: `crates/tama/src/components/general_section.rs`
- Delete: `crates/tama/src/components/sparkline.rs`
- Delete: `crates/tama/src/components/backup_section.rs`
- Delete: `crates/tama/src/jobs.rs`
- Modify: `crates/tama-core/src/config/mod.rs`
- Modify: `crates/tama-core/src/config/loader.rs`
- Modify: `crates/tama/src/components/mod.rs`

**What to implement:**

1. **config/migrate:** Re-verify: `rg "cleanup_stale_mmproj_args|rename_legacy_directories|config::migrate" crates/ --type rust` must show only `crates/tama-core/src/config/mod.rs` lines 4 and 12 and `config/migrate/mod.rs` itself. Then `rm -rf crates/tama-core/src/config/migrate`. In `crates/tama-core/src/config/mod.rs` delete line 4 (`pub mod migrate;`) and line 12 (`pub use migrate::cleanup_stale_mmproj_args;`).

2. **rename_legacy:** Re-verify: `rg "rename_legacy|migrate_legacy_data_dir|Migration" crates/ --type rust` — expect hits only in `config/mod.rs` (lines 5, 13), `config/loader.rs` (lines ~25–28), and `rename_legacy.rs` itself (check `Migration` hits carefully — the name may collide with unrelated types; only act on `rename_legacy` ones). Then `rm crates/tama-core/src/config/rename_legacy.rs`. In `config/mod.rs` delete line 5 (`mod rename_legacy;`) and line 13 (`pub use rename_legacy::{migrate_legacy_data_dir, Migration};`). In `crates/tama-core/src/config/loader.rs` delete the migration call block (lines ~21–28): the `// One-time auto-migration …` comment, the `TODO(v1.60)` comment, and the `if let Err(e) = super::rename_legacy::migrate_legacy_data_dir(&base) { … }` statement, leaving `Ok(base)` as the function tail.

3. **Leptos components:** Re-verify each name has zero references outside its own file and `components/mod.rs`: `rg "SamplingTemplatesSection|sampling_templates_section" crates/tama/src`, `rg "SupervisorSection|supervisor_section" crates/tama/src`, `rg "GeneralSection|general_section" crates/tama/src`, `rg "SparklineChart|components::sparkline|mod sparkline" crates/tama/src`, `rg "BackupSection|backup_section" crates/tama/src`. Then delete the 5 files. In `crates/tama/src/components/mod.rs` delete line 10 (`// pub mod backup_section; // TODO: Fix compilation`), line 13 (`pub mod general_section;`), line 20 (`pub mod sampling_templates_section;`), line 25 (`pub mod sparkline;`), line 26 (`pub mod supervisor_section;`). Note: `crates/tama/src/utils/chart_utils.rs:1` has a doc comment mentioning "sparkline" — leave it, it documents the shared chart utils, not the deleted component.

4. **jobs.rs orphan:** Re-verify: `rg "mod jobs" crates/tama/src` and `git ls-files --error-unmatch crates/tama/src/jobs.rs` (it IS tracked — delete via `git rm crates/tama/src/jobs.rs` so the deletion is staged).

5. After all deletions: `cargo check --workspace` must pass. If the compiler surfaces NEW dead-code warnings in files that referenced the deleted ones, fix them in this commit (expected: none — all references were severed at `mod.rs` level).

**Steps:**
- [ ] Run the `rg` re-verification for each of the 8 deletion targets; drop any that gained a caller and note it in the commit message
- [ ] Delete `config/migrate/`, `rename_legacy.rs`, the 5 components, `jobs.rs`; apply the three `mod.rs`/`loader.rs` edits
- [ ] Run `cargo check --workspace` — compiles
- [ ] Run `cargo nextest run --workspace` — all pass (deletions only)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "chore: delete dead config/migrate, rename_legacy, Leptos components, orphan jobs.rs"

**Acceptance criteria:**
- [ ] All 8 deletion targets are gone from the tree and from `git status`
- [ ] `rg "migrate::|rename_legacy|supervisor_section|sampling_templates_section|general_section|SparklineChart|backup_section|mod jobs" crates/ --type rust` — zero hits
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 2: Remove unused dependencies and blanket lint allows (F26)

**Context:**
Dependency audit (verified during planning): `inquire` (workspace `Cargo.toml:35` + `crates/tama-core/Cargo.toml:21`), `clap` (workspace `Cargo.toml:16` only — no crate references it), `utoipa` + `utoipa-gen` (`crates/tama/Cargo.toml:30-31`, optional, referenced only by the `ssr` feature list at line 63), and `http-body-util` (workspace `Cargo.toml:43` + `crates/tama-core/Cargo.toml:32`) have zero code references. **`axum-server` is NOT unused in tama-core** (`crates/tama-core/src/proxy/server/listener.rs:2,27,85` uses it) — but the `axum-server` optional dep in `crates/tama/Cargo.toml` (line 34, in the `ssr` feature list) IS unused (no `axum_server` reference in `crates/tama/src`), so only that one is removed. The crate-level `#![allow(dead_code)]` + `#![allow(deprecated)]` at `crates/tama/src/lib.rs:1-2` are how the dead code in Task 1 accumulated invisibly; removing them exposes ~10 stale item-level allows on LIVE items (`InstallModal`, `BackendCard`, `get_job`, `job_events_sse`, …) plus a few genuinely dead items the compiler will now flag. Decision: remove both blanket allows; fix fallout by (a) removing stale `#[allow(dead_code)]` from live items, (b) deleting newly-flagged dead items, (c) adding TARGETED `#[allow(deprecated)]` at the two files that reference deprecated tama-core Record types (migrating off them is plan-160's job, not this plan's).

**Files:**
- Modify: `Cargo.toml` (workspace)
- Modify: `crates/tama-core/Cargo.toml`
- Modify: `crates/tama/Cargo.toml`
- Modify: `crates/tama/src/lib.rs`
- Modify: whichever files the compiler flags (known candidates below)

**What to implement:**

1. **Dependency removal:**
   - Workspace `Cargo.toml`: delete line 16 (`clap = { version = "4", features = ["derive"] }`), line 35 (`inquire = "0.7"`), line 43 (`http-body-util = "0.1"`).
   - `crates/tama-core/Cargo.toml`: delete `inquire.workspace = true` (line 21) and `http-body-util.workspace = true` (line 32). KEEP `axum-server.workspace = true` (line 28 — used by `proxy/server/listener.rs`).
   - `crates/tama/Cargo.toml`: delete `utoipa = { version = "5", features = ["axum_extras"], optional = true }` (line 30), `utoipa-gen = { version = "5", optional = true }` (line 31), `axum-server = { workspace = true, optional = true }` (line 34); in the `ssr` feature list (lines 59-66) remove `"dep:axum-server"`, `"dep:utoipa"`, `"dep:utoipa-gen"`. Keep everything else.
   - Re-verify before each removal: `rg "inquire" crates/ -g "*.rs"` (0 hits), `rg "use clap|clap::" crates/ -g "*.rs"` (0), `rg "utoipa" crates/tama/src -g "*.rs"` (0), `rg "http_body_util" crates/ -g "*.rs"` (0), `rg "axum_server" crates/tama/src -g "*.rs"` (0 — tama-core stays).
   - Regenerate the lockfile: `cargo check --workspace` (updates `Cargo.lock`; commit it).

2. **Blanket allows:** In `crates/tama/src/lib.rs` delete lines 1–2 (`#![allow(dead_code)]` and `#![allow(deprecated)]`).

3. **Fallout cleanup** (after `cargo check --workspace 2>&1 | grep -c warning` triage):
   - Remove the stale item-level `#[allow(dead_code)]` from LIVE items: `crates/tama/src/components/install_modal.rs:56`, `crates/tama/src/components/backend_card.rs:124`, `crates/tama/src/api/backends/jobs.rs:16` (on `get_job`) and `:63` (on `job_events_sse`), `crates/tama/src/api/backends/types.rs:301`, `crates/tama/src/components/pull_wizard/mod.rs:124` and `:274`, `crates/tama/src/pages/benchmarks/types.rs:176`. For each: confirm the item is referenced (`rg` its name) — if referenced, delete the allow; if NOT referenced, delete the ITEM.
   - `crates/tama/src/components/form_validation.rs` (7 allows): `rg "form_validation::" crates/tama/src` shows zero usages during planning. If the compiler flags all its items as dead, delete the file and its `pub mod form_validation;` in `components/mod.rs`; if anything is used, keep the file and remove only the stale allows.
   - Deprecated-usage fallout in `crates/tama/src/pages/model_editor/types.rs` and `crates/tama/src/api/models/files.rs` (they reference `#[deprecated]` `ModelConfigRecord`/`ModelFileRecord` from tama-core): add a targeted `#[allow(deprecated)] // TODO(plan-160): migrate off Record types` on the specific `use` or fn sites the compiler points at. Do NOT migrate the types here.

**Steps:**
- [ ] Run the five `rg` re-verifications for the dependencies
- [ ] Apply the Cargo.toml edits; run `cargo check --workspace` — compiles and lockfile updates
- [ ] Remove the two blanket allows from `crates/tama/src/lib.rs`
- [ ] Run `cargo check --workspace 2>&1 | grep warning` — triage every warning per item 3
- [ ] Run `cargo nextest run --workspace` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean (this is the real gate: with blanket allows gone, clippy must still be warning-free)
- [ ] Commit with message: "chore: drop unused deps and blanket dead_code/deprecated allows"

**Acceptance criteria:**
- [ ] `rg "inquire|clap|utoipa|http.body.util" Cargo.toml crates/*/Cargo.toml` — zero hits (except no false positives like `clap` inside another word)
- [ ] `axum-server` remains in `crates/tama-core/Cargo.toml` only
- [ ] `crates/tama/src/lib.rs` starts without the two `#![allow(...)]` lines; no NEW blanket allows added anywhere
- [ ] `cargo clippy --workspace -- -D warnings` clean; `cargo nextest run --workspace` passes

---

### Task 3: Dead code small batch (F38)

**Context:**
The audit's second dead-code sweep. Every item below was verified to have zero callers (production AND test) during planning — the exact `rg` evidence is inline; re-verify before deleting. Three deliberate EXCLUSIONS: (a) `ProxyState::set_pull_queue` (`proxy/types.rs:415`) — the audit called it dead but it is called by `crates/tama/tests/downloads_api.rs:25`, a cross-crate integration test, so it can neither be deleted nor `#[cfg(test)]`-gated; KEEP it; (b) `lifecycle/traits.rs` scaffolding — plan-171 Task 1 routes compaction/TTS through `ProcessSpawner`/`PortAllocator` and uses `MockProcessSpawner`/`MockPortAllocator`/`SpawnedProcess`, so after plan-171 lands NOTHING in `traits.rs` is dead; if plan-171 has NOT landed when this task runs, delete ONLY `MockProcessSpawner` and `MockPortAllocator` (the traits and `SpawnedProcess` stay — `SpawnedProcess` is `ProcessSpawner::spawn`'s return type); (c) `ProcessSupervisor::with_log_dir` (`process.rs:119`) — plan-173 deletes the entire `ProcessSupervisor` struct; do not touch it here. `updates/checker/helpers.rs` (`determine_update_status`, `should_check_since`) is called only from `checker/tests.rs`, so it is `#[cfg(test)]`-gated rather than deleted.

**Files:**
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/db/queries/model_queries.rs`
- Modify: `crates/tama-core/src/db/queries/pull_queue_queries.rs`
- Modify: `crates/tama-core/src/db/queries/tts_config_queries.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/repository.rs`
- Modify: `crates/tama-core/src/proxy/forward/json.rs`
- Modify: `crates/tama-core/src/proxy/forward/tests/json.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/types.rs`
- Modify: `crates/tama-core/src/backends/installer/mod.rs`
- Modify: `crates/tama-core/src/backends/installer/prebuilt.rs`
- Modify: `crates/tama-core/src/backends/installer/download.rs`
- Modify: `crates/tama-core/src/backends/installer/source/build.rs`
- Modify: `crates/tama-core/src/backends/installer/source/install.rs`
- Modify: `crates/tama-core/src/gpu/vram.rs`
- Modify: `crates/tama-core/src/gpu/types.rs`
- Modify: `crates/tama-core/src/bench/display.rs`
- Modify: `crates/tama-core/src/models/pull/mod.rs`
- Modify: `crates/tama-core/src/models/card.rs`
- Modify: `crates/tama-core/src/backends/tts_kokoro/mod.rs`
- Modify: `crates/tama-core/src/config/types/proxy.rs`
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/config/loader.rs`
- Modify: `crates/tama-core/src/updates/checker/mod.rs`
- Modify: `crates/tama/src/api.rs`
- Modify: `crates/tama/src/api/logs.rs`

**What to implement:**

1. **ProxyState accessors** (`proxy/state.rs`): delete `is_model_loaded` (:99), `get_model_state_with_access` (:112), `get_backend_pid` (:123), `get_circuit_breaker_failures` (:135). Re-verify: `rg "is_model_loaded|get_model_state_with_access|get_backend_pid|get_circuit_breaker_failures" crates/` — only `state.rs`.

2. **Dead DB functions**: delete `delete_model_records` (`db/queries/model_queries.rs:185`), `get_all_running_items` (`pull_queue_queries.rs:276`), `get_running_item` (:259), `mark_stale_running_as_queued` (:303), `get_all_tts_configs` (`tts_config_queries.rs:70`), `Repository::conn` (`db/repository.rs:206` — NOTE: `ModelManager::conn` at `models/manager.rs:40` is a DIFFERENT, live function; do not touch it), `Repository::get_pull_queue_item` (`db/repository.rs:397`). Delete any unit tests that only exercise the deleted functions (check each file's test module after deleting — the compiler will flag orphaned references).

3. **Dead types**: delete `ModelAliasRecord` (`db/queries/types.rs:144`) and `RestartResponse` (`proxy/tama_handlers/types.rs:124`). Re-verify with `rg "ModelAliasRecord|RestartResponse" crates/`.

4. **`build_forward_uri`** (`proxy/forward/json.rs:16`): delete the function and its 3 tests in `proxy/forward/tests/json.rs` (`test_build_forward_uri_simple_path`, `test_build_forward_uri_with_query_string`, `test_build_forward_uri_no_query_returns_path_only`, lines ~69–88). Leave the rest of both files untouched.

5. **Installer cleanup**:
   - `backends/installer/download.rs`: delete `download_file` (:30) and its three test call sites (:301, :315, :332 — the tests only exercise this dead function; delete the tests too). If the file becomes empty of production code, check what else lives in it and whether `mod download;` in `installer/mod.rs` can go as well.
   - `emit` dedup: hoist ONE `pub(crate) fn emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>)` into `backends/installer/mod.rs` (the existing one at :36 — remove its `#[allow(dead_code)]` and widen to `pub(crate)`), and ALSO move `emit_error` there as `pub(crate)`. Delete the copies in `prebuilt.rs` (:15, :25) and `source/build.rs` (:8). Update call sites: `prebuilt.rs` gains `use super::{emit, emit_error};`; `source/install.rs:10` changes `use super::build::emit;` to `use crate::backends::installer::emit;` (verify the module path compiles — `installer` is `mod installer` under `backends`). Keep `test_emit_routes_to_sink` in `installer/mod.rs` — it now tests the shared helper.

6. **Zero-caller functions** (verified during planning — production AND tests): delete `VramInfo::available_bytes` (`gpu/vram.rs:19`), `ModelState::try_from_str` and `ModelState::from_str_fallback` (`gpu/types.rs:27,68`), `print_bench_report` (`bench/display.rs:13` — KEEP `format_stat`, it is used by `bench/mod.rs` tests), `pull_chunked` (`models/pull/mod.rs:80` — the `_with_progress` variant is the live one), `ModelCard::populate_sampling_from` (`models/card.rs:93`), `verify_tts_kokoro` (`backends/tts_kokoro/mod.rs:58`), `ProxyState::get_tts_server` (`proxy/lifecycle/tts.rs:289`), `is_auth_configured` (`config/types/proxy.rs:158`), `Config::open_db_from` and `Config::resolve_health_check` (`config/resolve/mod.rs:571,185`), `Config::with_models_dir` (`config/loader.rs:91`). Re-verify each with `rg "<name>\(" crates/ | rg -v "fn <name>"` immediately before deletion; DROP from the commit anything that gained a caller.

7. **`updates/checker/helpers.rs` gate** (`determine_update_status`, `should_check_since` — called only from `checker/tests.rs`): in `updates/checker/mod.rs` change `mod helpers;` → `#[cfg(test)] mod helpers;` and `pub use helpers::*;` → `#[cfg(test)] pub use helpers::*;`. Do NOT delete the file — plan-171 Task 2's orchestration tests live in the same module tree and future check code may legitimately use these.

8. **Unrouted handlers**: delete `get_logs` (`crates/tama/src/api.rs:37`) together with its `LogsQuery` struct (:28) and `default_lines` helper (used only by it — verify with `rg "LogsQuery" crates/tama/src`), and `get_all_logs` (`crates/tama/src/api/logs.rs:96`). Re-verify neither appears in `crates/tama/src/router.rs` or `crates/tama-core/src/proxy/server/router.rs` (they don't — the routed logs endpoints are `handle_all_logs` in tama-core and `get_backend_logs` in tama).

**Steps:**
- [ ] Re-verify every item with the inline `rg` commands; drop anything that gained a caller
- [ ] Apply deletions per items 1–8 (do the `traits.rs` check first: `rg "ProcessSpawner" crates/tama-core/src/proxy/lifecycle/compaction.rs` — if it hits, plan-171 has landed and `traits.rs` stays untouched)
- [ ] Run `cargo check --workspace` after each file group to catch orphaned test references early
- [ ] Run `cargo nextest run --workspace` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "chore: delete verified dead functions, types, and unrouted handlers (F38)"

**Acceptance criteria:**
- [ ] Every named item above is deleted (or explicitly dropped with the reason recorded in the commit message)
- [ ] `set_pull_queue`, `SpawnedProcess`, `ProcessSpawner`, `PortAllocator`, `ModelManager::conn`, `format_stat`, `pull_chunked_with_progress`, `updates/checker/helpers.rs` all still exist
- [ ] Exactly one `emit` and one `emit_error` exist under `crates/tama-core/src/backends/installer/`
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 4: Replace `println!`/`eprintln!` with tracing in production code (F34)

**Context:**
31 stray stdout/stderr writes remain in library code after plan-158 introduced tracing; they bypass the JSON log file and the `log_level` config. EXEMPT (do not touch): everything in `crates/tama-mock` (its prints are its purpose — a fake backend's stdout), everything under `#[cfg(test)]` (e.g. the `eprintln!` in `proxy/scope_middleware.rs:55` is test-gated), `bench/display.rs`'s prints (its only printing fn, `print_bench_report`, was deleted in Task 3 — verify), and the indicatif `ProgressBar` instances in `models/pull/` (progress bars are a deliberate UX surface; only the `pb.suspend(|| println!(...))` retry messages move to tracing). Mapping decision: retry/progress messages → `tracing::warn!` when they signal a failure being retried, `tracing::info!` for lifecycle milestones; the installer `emit`/`emit_error` `None`-sink fallbacks → `tracing::info!`/`tracing::error!` respectively.

**Files:**
- Modify: `crates/tama-core/src/bench/runner.rs`
- Modify: `crates/tama-core/src/db/backfill/initial_backfill.rs`
- Modify: `crates/tama-core/src/models/pull/single.rs`
- Modify: `crates/tama-core/src/models/pull/parallel.rs`
- Modify: `crates/tama-core/src/backends/installer/mod.rs`
- Modify: `crates/tama-core/src/backends/installer/prebuilt.rs`

**What to implement:**

1. **`bench/runner.rs`** (3 sites): line 262 `println!("Starting benchmark for '{}'...", backend_name);` → `tracing::info!("Starting benchmark for '{}'...", backend_name);`; line 286 `println!("Backend loaded in {:.0} ms", ...)` → `tracing::info!`; the multi-line `println!` at :318 (`"Running {} (warmup: {}, runs: {})..."`) → `tracing::info!`. Check the file's existing `use` block — it already uses `tracing::` elsewhere, so no new import should be needed.

2. **`db/backfill/initial_backfill.rs`** (6 sites at lines 24, 29, 33, 40, 77, 98): the "No installed models found" / "Backfilling…" / progress `[i/total]` / completion lines → `tracing::info!`; the two failure lines ("Failed to fetch metadata — skipping", "Failed to fetch blob metadata — continuing") → `tracing::warn!` (include the underlying error variable if one is in scope). Add `use tracing::{info, warn};` (check what the file already imports — it mixes `tracing::` already).

3. **`models/pull/single.rs`** (4 sites) and **`models/pull/parallel.rs`** (3 sites): every `pb.suspend(|| { println!(...) })` block → replace the whole `pb.suspend(...)` call with a plain `tracing::warn!(...)` carrying the same message + variables (`attempt`, `MAX_RETRIES`, chunk index, byte counts, the error `_e`/`e`). The `pb.dec(...)`/`pb.set_position(...)` calls around them stay. Rationale: these fire on retryable failures; the progress callback already reports byte progress, and the bar redraws itself after log lines via indicatif's stderr handling.

4. **Installer fallbacks** (after Task 3's dedup, exactly one `emit` in `installer/mod.rs` and one `emit_error`): change `None => println!("{line}")` to `None => tracing::info!("{line}")` in `emit`, and `None => eprintln!("{line}")` to `None => tracing::error!("{line}")` in `emit_error`. If `prebuilt.rs` still has its own copies (Task 3 not landed), convert them identically and note the duplication in the commit message rather than re-deduplicating here.

**Steps:**
- [ ] `rg "println!|eprintln!" crates/tama-core/src --type rust | rg -v "tests/|#\[cfg(test)\]"` — capture the before-list (expected: only the sites above plus test-gated/exempt ones)
- [ ] Apply conversions per items 1–4
- [ ] Run `cargo nextest run --package tama-core` — all pass (some tests may assert on log output — fix expectations if so)
- [ ] Re-run the `rg` sweep — only `crates/tama-mock`, `#[cfg(test)]`, and test-file hits remain
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: route remaining library println!/eprintln! through tracing"

**Acceptance criteria:**
- [ ] `rg "println!|eprintln!" crates/tama-core/src | rg -v "tests/|cfg(test)"` — zero hits in production code paths
- [ ] `crates/tama-mock` and all test code untouched
- [ ] Retry messages surface via `tracing::warn!` with the same content as before
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 5: Style drift batch — `CompactionCardDto` snake_case, AGENTS.md convention, `test_` prefixes (F39)

**Context:**
Three mechanical style fixes. (a) `CompactionCardDto` is the only camelCase DTO in an otherwise snake_case API (`crates/tama/src/api/backends/types.rs:14`, `#[serde(rename_all = "camelCase")]`) with a hand-synced WASM mirror (`crates/tama/src/pages/backends.rs:16`); the two have also drifted on `request_timeout_ms` (`u64` SSR vs `Option<u64>` WASM). Both sides flip to snake_case and the mirror aligns to `u64` — the only WASM reader is the compaction card in `pages/backends.rs` (~line 437) which reads `running`/`enabled`/`server_url`/`port`/`device` (snake_case already in Rust field names, so only the serde attribute changes) and never reads `request_timeout_ms`. (b) AGENTS.md's "Prefix private functions with `_`" convention has 7 uses versus ~1,320 without — code reality wins; the rule is deleted, existing `_`-prefixed fns are NOT renamed (not worth the churn). (c) 83 test functions lack the `test_` prefix (audit said 87; the exact verified distribution is below) — bulk-rename by prepending `test_`.

**Files:**
- Modify: `crates/tama/src/api/backends/types.rs`
- Modify: `crates/tama/src/pages/backends.rs`
- Modify: `AGENTS.md`
- Modify: `crates/tama-core/src/config/args_helpers.rs` (44 renames)
- Modify: `crates/tama/src/pages/dashboard/tests.rs` (15)
- Modify: `crates/tama/tests/css_test.rs` (8)
- Modify: `crates/tama-core/src/proxy/auth.rs` (7)
- Modify: `crates/tama/src/api/models/crud/tests.rs` (4)
- Modify: `crates/tama/src/utils/mod.rs` (2)
- Modify: `crates/tama/src/pages/config_editor/types.rs` (2)
- Modify: `crates/tama-core/src/backends/installer/mod.rs` (1)

**What to implement:**

1. **`CompactionCardDto` snake_case flip.** In `crates/tama/src/api/backends/types.rs:14` change `#[serde(rename_all = "camelCase")]` → `#[serde(rename_all = "snake_case")]` on `CompactionCardDto` (leave every other struct in the file alone). In `crates/tama/src/pages/backends.rs:16` make the same attribute change on the mirror AND change `request_timeout_ms: Option<u64>` → `request_timeout_ms: u64` (dropping the now-needless `#[serde(default, skip_serializing_if = "Option::is_none")]` on that field — keep the others). The SSR construction site (`crates/tama/src/api/backends/list.rs:279-285`) needs no change (field names are unchanged Rust idents). Search for any other reader of the camelCase wire keys: `rg '"requestTimeoutMs"|"serverUrl"' crates/` — fix if found.

2. **AGENTS.md.** In the Naming Conventions section, delete the bullet `- Prefix private functions with `_` (e.g., `_hf_api()`)`. Do not renumber or touch other bullets; do not rename any `_`-prefixed function.

3. **`test_` prefix bulk rename.** For each file, prepend `test_` to every `#[test]`/`#[tokio::test]` fn name not already starting with `test`:
   - `crates/tama-core/src/config/args_helpers.rs` (44 fns)
   - `crates/tama/src/pages/dashboard/tests.rs` (15 fns: `metric_current_deserializes_without_models_field`, …)
   - `crates/tama/tests/css_test.rs` (8 fns: `style_css_defines_*`, `rule_body_finds_top_level_rules_and_ignores_comments`)
   - `crates/tama-core/src/proxy/auth.rs` (7 fns: `no_auth_url_passes_through`, `skip_path_passes_through`, `no_auth_returns_401`, `caddy_forward_auth_header_passes`, `valid_bearer_token_passes`, `invalid_bearer_token_returns_401`, `authentik_unreachable_fails_open`)
   - `crates/tama/src/api/models/crud/tests.rs` (4 fns: `apply_model_body_*`)
   - `crates/tama/src/utils/mod.rs` (2 fns: `rw_signal_to_signal_returns_read_half`, `rw_signal_to_signal_returns_signal_that_tracks_writes`)
   - `crates/tama/src/pages/config_editor/types.rs` (2 fns: `api_keys_enabled_round_trips_through_form_config`, `full_config_round_trip_preserves_every_field`)
   - `crates/tama-core/src/backends/installer/mod.rs` (1 fn: `_assert_install_options_debug` → rename to `test_install_options_debug_assertion`, NOT `test__assert_…`)
   Mechanical approach: per file, `rg -n "#\[(tokio::)?test" -A 3 <file>` to list the fns, rename with an editor or `sed -i 's/fn NAME(/fn test_NAME(/'` per fn, then `cargo nextest run --package <crate> -- <file-stem>` before moving on. Test fns have no external callers, so no other edits are needed — but watch for tests that call EACH OTHER (none found during planning; verify per file).

**Steps:**
- [ ] Flip the two `CompactionCardDto` serde attributes + the `request_timeout_ms` alignment
- [ ] Run `cargo nextest run --package tama` — pass (the backends page tests exercise the DTO)
- [ ] Edit AGENTS.md (delete the `_`-prefix bullet)
- [ ] Rename the 83 test fns file-by-file, running `cargo nextest run --package <crate>` after each file
- [ ] Run `python3 -c` scan or `rg` to confirm zero remaining `#[test]` fns without the `test_` prefix (reuse the audit's counting method; expect 0)
- [ ] Run `cargo nextest run --workspace` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "style: snake_case CompactionCardDto, drop dead _-prefix rule, test_ prefixes"

**Acceptance criteria:**
- [ ] `CompactionCardDto` serializes snake_case on both SSR and WASM sides; `request_timeout_ms` is `u64` in both
- [ ] AGENTS.md contains no `_`-prefix-for-private convention
- [ ] Zero `#[test]`/`#[tokio::test]` fns without the `test_` prefix workspace-wide
- [ ] `cargo nextest run --workspace` passes; clippy clean
