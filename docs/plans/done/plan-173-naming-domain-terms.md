# Naming & Domain Terms Plan

**Goal:** Finish the domain-terminology migration mandated by CONTEXT.md — `gpu_variant` (not gpu_type), `backend` (not server), `pull` (not download), `Starting` (not Loading) — batching every breaking rename (API routes, DTOs, config keys, DB columns, serialized enum values) so they ship in ONE release with one changelog note (audit F27, F28, F40).

**Architecture:** Mechanical renames plus three SQLite migrations (new `_0038`–`_0040` following the `_0034_rename_download_queue_to_pull_queue.rs` precedent) and serde aliases for the two API-visible compat seams (`ModelState` reads `"loading"` as `Starting`; config reads `"supervisor"` as `lifecycle`). CONTEXT.md is the authority. Do not touch historical migration files (`_0001`–`_0037`), historical ADRs, or `docs/plans/done/` — they are the record of what was true when written. **Sequencing:** land plan-170 (newtypes) AFTER Task 1 of this plan — plan-170 extends the GPU enum with `FromStr`/serde under its current name `GpuType`; renaming first avoids a rebase conflict. If plan-172 has already landed, `ModelState::try_from_str`/`from_str_fallback` and `ProcessSupervisor::with_log_dir` no longer exist — skip the steps that mention them (each is marked).

**Tech Stack:** Rust, Axum, SQLite (rusqlite), tokio, Leptos (WASM), serde

---

### Task 1: Rename `GpuType`→`GpuVariant`, `GpuTypeDto`→`GpuVariantDto`, bench `gpu_type` fields (F27a)

**Context:**
CONTEXT.md reserves `gpu_variant` for the GPU compilation target (`cuda`/`rocm`/`vulkan`/`metal`/`cpu`). The central enum is still `GpuType` (`crates/tama-core/src/gpu/detect.rs:6`, re-exported at `gpu/mod.rs:16`), the WASM mirror is `GpuTypeDto` (`crates/tama/src/components/backend_card.rs:22`), and bench reports carry `ModelInfo.gpu_type: String` (`crates/tama-core/src/bench/mod.rs:149`) which is serialized to the frontend (the WASM side reads the raw key `"gpu_type"` at `crates/tama/src/pages/benchmarks/mod.rs:807`). Note the casing difference between the two enums: core has `RocM`, the DTO has `Rocm` — preserve both casings exactly during the rename (unifying them is a separate decision, not this plan's). The `BenchReport` JSON is only streamed to the frontend (the DB `results_json` column stores `Vec<BenchSummary>`, not `ModelInfo`), so the only wire consumer of the renamed key is `pages/benchmarks/mod.rs:807`. Do NOT rename inside `db/migrations/_0003_create_backend_installations.rs` or `_0032_remove_gpu_type_column.rs` (historical).

**Files:**
- Modify: `crates/tama-core/src/gpu/detect.rs`
- Modify: `crates/tama-core/src/gpu/mod.rs`
- Modify: `crates/tama-core/src/db/backfill/mod.rs`
- Modify: `crates/tama-core/src/bench/mod.rs`
- Modify: `crates/tama-core/src/bench/runner.rs`
- Modify: `crates/tama-core/src/bench/llama_bench/discovery.rs`
- Modify: `crates/tama-core/src/bench/llama_bench/mod.rs`
- Modify: `crates/tama-core/src/bench/display.rs` (only if `print_bench_report` still exists — plan-172 Task 3 deletes it)
- Modify: `crates/tama/src/components/backend_card.rs`
- Modify: `crates/tama/src/pages/benchmarks/mod.rs`

**What to implement:**

1. **`gpu/detect.rs`:** rename `pub enum GpuType` → `pub enum GpuVariant` and `impl GpuType` → `impl GpuVariant`; update the 8 match arms in `variant_folder()` and the test module references (lines ~461–478). Keep variants `Cuda { version: String }, Vulkan, Metal, RocM { version: String }, CpuOnly, Custom` byte-identical. Update the doc comment on the enum if it says "GPU type".
2. **`gpu/mod.rs`:** line 16 re-export — `GpuType` → `GpuVariant` in the `pub use` list (keep `DEFAULT_CUDA_VERSION`).
3. **`db/backfill/mod.rs:31`:** field `gpu_type: Option<crate::gpu::GpuType>` → `gpu_variant: Option<crate::gpu::GpuVariant>`; update any constructor use of the field within that file.
4. **`bench/mod.rs:149`:** `pub gpu_type: String` → `pub gpu_variant: String` on `ModelInfo`; update its doc comment (`/// GPU type (e.g., "CUDA", "Vulkan", "CPU")` → `/// GPU variant label (e.g., "CUDA", "Vulkan", "CPU")`).
5. **`bench/runner.rs`:** rename `fn _detect_gpu_type(backend_path: &str, has_nvidia: bool) -> String` (:29) → `fn detect_gpu_variant_label` (drop the dead `_` prefix while renaming — plan-172 Task 5 removes the convention from AGENTS.md); update the construction `gpu_type: _detect_gpu_type(...)` (:275) → `gpu_variant: detect_gpu_variant_label(...)`; rename the 3 tests (:362,369,376) accordingly (`test_detect_gpu_variant_label_*`).
6. **`bench/llama_bench/discovery.rs`:** `pub(super) fn detect_gpu_type` (:64) → `pub(super) fn detect_gpu_variant_label`; update its doc comment and the 6 tests in the same file. Update the caller in `bench/llama_bench/mod.rs:186` (`gpu_type: discovery::detect_gpu_type(&backend_path)` → `gpu_variant: discovery::detect_gpu_variant_label(&backend_path)`).
7. **`bench/display.rs`:** only if `print_bench_report` survived plan-172 — update `report.model_info.gpu_type` (:36) → `gpu_variant`.
8. **`components/backend_card.rs`:** `pub enum GpuTypeDto` (:22) → `pub enum GpuVariantDto`; `impl GpuTypeDto` (:31) → `impl GpuVariantDto` (keep `Rocm` casing); update all construction/match sites in the file and the test at :556-565 (`test_gpu_type_label` → `test_gpu_variant_label`). Find external users: `rg "GpuTypeDto" crates/tama/src` — update each import/site.
9. **`pages/benchmarks/mod.rs:807`:** `model_info.get("gpu_type")` → `model_info.get("gpu_variant")`. Also check `api/benchmarks/history.rs` for any `gpu_type` key references (`rg "gpu_type" crates/tama/src`).

**Steps:**
- [ ] Apply the renames per items 1–9 (`rg "GpuType|gpu_type" crates/ -g "*.rs"` before and after; expected remaining hits: historical migrations only)
- [ ] Run `cargo nextest run --package tama-core -- gpu::` and `-- bench` — pass
- [ ] Run `cargo nextest run --package tama` — pass (catches DTO/mirror fallout)
- [ ] Run `rg "GpuType\b|gpu_type" crates/ -g "*.rs"` — only `db/migrations/_0003*` and `_0032*` remain
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor!: rename GpuType/gpu_type to GpuVariant/gpu_variant per CONTEXT.md"

**Acceptance criteria:**
- [ ] `rg "GpuType\b" crates/ -g "*.rs"` — zero hits outside historical migrations
- [ ] Bench report JSON emits `"gpu_variant"`; the WASM reader uses the new key
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 2: Scrub `server`-means-backend leftovers (F27b)

**Context:**
The plan-144/150 type-level renames landed, but variable names, log strings, and comments still say "server" where CONTEXT.md mandates "backend". Worst offender: `Config::resolve_backend` returns `(&ModelConfig, &BackendConfig)` and every caller binds the ModelConfig as `server_config` — factually wrong, not just stale. Scope is deliberately limited to the files below (verified during planning); do NOT roam the tree renaming unrelated "server" occurrences (e.g. `llama-server` binary names, "compaction server" Python process, `handle_all_logs` strings, CSS) — the audit scoped this to the listed sites. All edits are private (no wire/config change), so no compat step.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/idle_timeout.rs`
- Modify: `crates/tama-core/src/proxy/handlers/forward.rs`
- Modify: `crates/tama-core/src/proxy/handlers/tts.rs`
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/bench/runner.rs`
- Modify: `crates/tama-core/src/bench/llama_bench/mod.rs`
- Modify: `crates/tama/src/api/benchmarks/spec.rs`
- Modify: `crates/tama/src/api/benchmarks/mtp.rs`

**What to implement:**

1. **`lifecycle/mod.rs`:**
   - :76 and :452 — `let servers = config.resolve_backends_for_model(...)` → `let backends = ...`; update `.first()` uses on the next lines.
   - `server_config` → `model_config` throughout the file (word-boundary; ~30 uses — `sed -i 's/\bserver_config\b/model_config/g'` is safe here, then eyeball the diff).
   - Log/comment strings: :24 `falling back to another server` → `another backend`; :92 `"Server '{}' already loaded/starting for model '{}'"` → `"Backend '{}' already loaded/starting for model '{}'"`; :322 `"Backend process {} for server '{}' exited with {}"` → `for backend '{}'`; :328 same change; :433 `info!("Server '{}' loaded successfully", ...)` → `"Backend '{}' loaded successfully"`; :603 `.with_context(|| format!("Server '{}' not loaded", ...))` → `"Backend '{}' not loaded"`; :610 `"Server '{}' is not ready (state: {:?})"` → `"Backend '{}' is not ready ..."`; :484-485 comment `Collect all Ready server names AND non-inference server names` → `... backend names ...`; :488 `let ready_servers: Vec<String>` → `ready_backends` (+ uses at :509, :535).
   - Do NOT change `BackendState` variants, `load_model` behavior, or any `backend_name` bindings.
2. **`lifecycle/idle_timeout.rs`** (same file family, same class): :11 doc `Check if any server has been idle` → `any backend`; :25,53 `stuck_starting_servers` → `stuck_starting_backends` (+ uses at :162,167); :157 `"Removed failed server '{}' from model map"` → `failed backend`; :203 `"Killing orphaned process group {} for stuck server '{}'"` → `stuck backend`; :216 comment `Handle dead Ready servers` → `dead Ready backends`. Sweep the rest of the file with `rg -n "server" crates/tama-core/src/proxy/lifecycle/idle_timeout.rs` and apply the same substitution to remaining domain-term hits (vars/comments/logs), leaving `llama-server`-style binary names alone.
3. **`handlers/forward.rs`:** :80-81 comment `// No model field — forward to first available server or return error` → `first available backend`; :90 `"message": "No backend server available"` → `"No backend available"` (this IS a wire-visible error message — acceptable, batched breaking release; note in commit message); :120,124 doc comments (`/// Forward a request to the first available backend server.` / `simply pick the first available server.`) → drop the second `server`.
4. **`handlers/tts.rs`:** :58 doc `/// Ensure a TTS backend is loaded and return its server URL.` → `backend URL`; :79 comment `// After loading, get the server URL from models map` → `backend URL`.
5. **`proxy/types.rs`:** :173 doc `/// Check if the server has failed and the cooldown has elapsed.` → `backend`; test keys at :601,615,627,630,642,645,661,665 — `"server-a"`/`"server-b"` → `"backend-a"`/`"backend-b"` (including the comment at :627,642).
6. **`config/resolve/mod.rs`:** :75-88 — loop binding `for (config_name, server) in models` → `for (config_name, model_config) in models`; update `!server.enabled` (:76), `self.backends.get(&server.backend)` (:83), `server.backend` in the debug log (:88).
7. **`server_config` bindings outside lifecycle (same `resolve_backend` mis-binding):** `bench/runner.rs:98,269`, `bench/llama_bench/mod.rs:97`, `api/benchmarks/spec.rs:153`, `api/benchmarks/mtp.rs:156` — per file, rename `server_config` → `model_config` (word-boundary sed per file), then `cargo check --package <crate>`.

**Steps:**
- [ ] Apply item 1–7 edits file by file; after each file run `rg -n "\bserver\b|\bservers\b|server_config" <file>` and clear remaining domain-term hits (leave binary names / CSS / `TtsServer`-style proper nouns)
- [ ] Run `cargo nextest run --package tama-core -- proxy::lifecycle` — pass
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: scrub server-means-backend leftovers in lifecycle/handlers/resolve"

**Acceptance criteria:**
- [ ] `rg "\bserver_config\b" crates/ -g "*.rs"` — zero hits
- [ ] `rg "resolve_backends_for_model" -A 1 crates/tama-core/src` shows no `let servers` bindings
- [ ] Listed log strings say "backend"; no behavior or wire-shape change except the one noted error message
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 3: `download`→`pull` on the public surface — routes, DTOs, config key, DB columns (F27c, BREAKING)

**Context:**
The core rename landed (plan-150: `pull_queue` table, `PullQueueService`, `bytes_pulled`), but the public surface still says "download": HTTP routes `/tama/v1/downloads/*` (handlers in the same file are already named `get_active_pulls`/`get_pull_history`/`cancel_pull`/`pull_events_sse`), DTOs `Downloads*Response`, config key `download_queue_poll_interval_secs` (a column in the `app_config` DB table — needs a migration, not just a rename), and the `model_files.downloaded_at` column. Frontend callers of the routes live in `crates/tama/src/lib.rs` (:88, :100, :233, :262), `components/pull_quant_wizard.rs:533`, and the doc example in `utils/sse_stream.rs:16`; the WASM config mirrors carry the config key at `types/config/proxy.rs:51,133`, `types/config/mod.rs:206,239`, `pages/config_editor/types.rs:131,250`, `pages/config_editor/forms/proxy/advanced.rs:86,89`, and `types/config/patch.rs:47`. The `downloaded_at` JSON key emitted by `api/models/files.rs:23` has NO frontend readers (verified) — safe to rename with the column. Two internal module files are also renamed: `models/pull/download.rs`→`transfer.rs` (it holds `pull_gguf_with_progress`/`PullResult`) and `tama_handlers/pull/download.rs`→`start.rs` (it holds `start_pull_from_queue`). Everything ships together as ONE breaking change; the changelog note is part of the commit message. Do NOT rename historical docs (`docs/plans/done/`, `docs/decisions/0009-*`).

**Files:**
- Create: `crates/tama-core/src/db/migrations/_0038_rename_app_config_pull_poll_interval.rs`
- Create: `crates/tama-core/src/db/migrations/_0039_rename_model_files_pulled_at.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: `crates/tama-core/src/db/migrations/migrations_tests.rs`
- Rename: `crates/tama/src/api/downloads.rs` → `crates/tama/src/api/pulls.rs`
- Rename: `crates/tama/tests/downloads_api.rs` → `crates/tama/tests/pulls_api.rs`
- Rename: `crates/tama-core/src/models/pull/download.rs` → `crates/tama-core/src/models/pull/transfer.rs`
- Rename: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs` → `crates/tama-core/src/proxy/tama_handlers/pull/start.rs`
- Modify: `crates/tama-core/src/models/pull/mod.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/mod.rs`
- Modify: `crates/tama/src/router.rs`, `crates/tama/src/api.rs`, `crates/tama/src/lib.rs`
- Modify: `crates/tama/src/components/pull_quant_wizard.rs`, `crates/tama/src/utils/sse_stream.rs`
- Modify: `crates/tama-core/src/config/types/proxy.rs`, `crates/tama-core/src/config/types/mod.rs`, `crates/tama-core/src/config/types/config_tests.rs`
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs`
- Modify: `crates/tama/src/types/config/proxy.rs`, `crates/tama/src/types/config/mod.rs`, `crates/tama/src/types/config/patch.rs`, `crates/tama/src/pages/config_editor/types.rs`, `crates/tama/src/pages/config_editor/forms/proxy/advanced.rs`
- Modify: `crates/tama-core/src/db/queries/model_queries.rs`, `crates/tama-core/src/db/queries/types.rs`, `crates/tama-core/src/db/repository.rs`, `crates/tama-core/src/models/update.rs`, `crates/tama-core/src/backup/merge.rs`, `crates/tama-core/src/backup/archive.rs`, `crates/tama/src/api/models/files.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/types.rs` (+ its users: `tama_handlers/mod.rs`, `tama_handlers/tests.rs`, `pull/verify.rs`, `pull/start.rs`, `proxy/pull_queue.rs`)
- Modify: `crates/tama-core/src/models/manager.rs`, `crates/tama-core/src/models/pull/single.rs`, `crates/tama-core/src/models/pull/parallel.rs`, `crates/tama-core/src/proxy/pull_jobs.rs` (residual comments/messages)
- Rename: `docs/api/downloads.md` → `docs/api/pulls.md`
- Modify: `docs/api/README.md`, `AGENTS.md`

**What to implement:**

1. **Routes.** In `crates/tama/src/router.rs`: `/tama/v1/downloads/:job_id/cancel` (:261), `/tama/v1/downloads/active` (:311), `/tama/v1/downloads/history` (:315), `/tama/v1/downloads/events` (:319) → `/tama/v1/pulls/:job_id/cancel`, `/tama/v1/pulls/active`, `/tama/v1/pulls/history`, `/tama/v1/pulls/events`. Update the handler paths from `api::downloads::` to `api::pulls::` (see item 3). Frontend callers: `crates/tama/src/lib.rs` (:88, :100 `EventSource` URLs; :233, :262 fetch URLs), `components/pull_quant_wizard.rs:533` (`EventSource`), `utils/sse_stream.rs:16` (doc example). No compat aliases — one clean break (release note in commit message).

2. **Migrations.** Two new files following the `_0034` shape (`pub const MIGRATION: (i32, bool, &str)`):
   - `_0038_rename_app_config_pull_poll_interval.rs`: `(38, false, r#"ALTER TABLE app_config RENAME COLUMN download_queue_poll_interval_secs TO pull_queue_poll_interval_secs;"#)`.
   - `_0039_rename_model_files_pulled_at.rs`: `(39, false, r#"ALTER TABLE model_files RENAME COLUMN downloaded_at TO pulled_at;"#)`.
   Register both in `crates/tama-core/src/db/migrations.rs` (two `mod` lines after `_0037_add_langfuse;` and two entries at the end of the `MIGRATIONS` const). Update `migrations_tests.rs`: line ~1081 references `"download_queue_poll_interval_secs"` in an expected-column list for `app_config` — change to the new name; add assertions mirroring the existing rename-migration test pattern (if a `_0034` test exists, copy its shape: run `run_up_to(conn, 37)`, insert a row with the old column, run to 39, assert the new column exists with the data intact).

3. **DTOs + API module rename.** `git mv crates/tama/src/api/downloads.rs crates/tama/src/api/pulls.rs`; in `api.rs:12` change `pub mod downloads;` → `pub mod pulls;`. In the file: `DownloadsActiveResponse` → `PullsActiveResponse`, `DownloadsHistoryResponse` → `PullsHistoryResponse`, `DownloadCancelResponse` → `PullCancelResponse` (struct names + constructors at :118, :154, :168, :176, :180 + derive lines; the JSON field names inside (`items`, `total`, `cancelled`, …) stay). Update `router.rs` handler paths (`api::pulls::get_active_pulls` etc.). `git mv crates/tama/tests/downloads_api.rs crates/tama/tests/pulls_api.rs` and update its `use tama_web::api::pulls::{PullsActiveResponse, PullsHistoryResponse, PullCancelResponse};` plus the 3 hardcoded route strings in `build_pull_router` (:33-41). Check `crates/tama/Cargo.toml` `[[test]]` entries — if `downloads_api` is explicitly registered, update the entry (planning found explicit entries only for `server_test` and `config_structured_test`; re-verify).

4. **Config key `pull_queue_poll_interval_secs`.** Core: `config/types/proxy.rs:96` (field) and :136 (default fn — also rename `default_download_queue_poll_interval` → `default_pull_queue_poll_interval`); `config/types/mod.rs:120,286`; `config/types/config_tests.rs:160,212,284`; `db/queries/app_config_queries.rs` (:36 field, :157 param, :181, :195, :222, :268 SQL column lists). WASM mirror: `types/config/proxy.rs:51,133`, `types/config/mod.rs:206,239`, `pages/config_editor/types.rs:131` + its test JSON literal at :250 (`"download_queue_poll_interval_secs": 3` → new key), `pages/config_editor/forms/proxy/advanced.rs:86,89`, `types/config/patch.rs:47`. SSR PATCH handler: `crates/tama/src/api.rs:322-324` and the test fixture at :498. The serde key on the wire changes with the field name — same breaking batch.

5. **`model_files.pulled_at`.** `db/queries/model_queries.rs`: SQL at :68, :75, :132, :159 (column lists) and row-mapping field names at :146, :173. `db/queries/types.rs:81`: `ModelFileRecord.downloaded_at` → `pulled_at`. `db/repository.rs`: `ModelFileDto.downloaded_at` (:69) → `pulled_at` and the mapping at :481. `models/update.rs:381` (test literal). `backup/merge.rs:162,164` (SQL — CAREFUL: this reads from an ATTACHed backup DB which may have the OLD column name; the merge SQL references `downloaded_at` on the SOURCE schema. Decision: leave `backup/merge.rs` unchanged and add a comment `// source backups may predate the pulled_at rename; column names here refer to the backup schema` — plan-163 owns backup restore). `backup/archive.rs:498,506,658` — test-only schema fixtures: update to the new column name only if the production archive code writes the live schema (read `archive.rs` first; if its `CREATE TABLE model_files` is a test fixture of an OLD backup, leave it). `crates/tama/src/api/models/files.rs:23`: JSON key `"downloaded_at"` → `"pulled_at"` (no frontend readers — verified).

6. **`QuantDownloadSpec` → `QuantPullSpec`.** `proxy/tama_handlers/types.rs:29` (struct) and the `quants: Vec<QuantDownloadSpec>` field at :51 (field name `quants` stays — wire-compatible); users: `tama_handlers/mod.rs:31` re-export, `tama_handlers/tests.rs`, `pull/verify.rs:18,209,495`, `pull/start.rs:5,18` (after item 7's rename), `proxy/pull_queue.rs:349` doc comment. The type is `Deserialize`-only on the request path — no wire change.

7. **Internal module renames.** `git mv crates/tama-core/src/models/pull/download.rs crates/tama-core/src/models/pull/transfer.rs`; in `models/pull/mod.rs:10` change `pub mod download;` → `pub mod transfer;`; fix references (`rg "pull::download|pull::transfer|download::pull_gguf" crates/tama-core/src` — expect hits in `pull/mod.rs` re-exports and `tama_handlers/pull/start.rs`). `git mv crates/tama-core/src/proxy/tama_handlers/pull/download.rs crates/tama-core/src/proxy/tama_handlers/pull/start.rs`; in `tama_handlers/pull/mod.rs` change `pub mod download;` → `pub mod start;` and `pub use download::start_pull_from_queue;` → `pub use start::start_pull_from_queue;`.

8. **Residual comments/messages.** `models/manager.rs:237` comment `// ── Download queue ──` → `// ── Pull queue ──`; `pull_jobs.rs:10,44` doc comments (`Download finished` → `Pull finished`, `Download duration` → `Pull duration`); `models/pull/single.rs` + `parallel.rs` doc comments and error strings: change user-facing `Download failed` / `Download complete` / `Downloading` wording to `Pull failed` / `Pull complete` / `Pulling` (error messages are log/UI-visible, not wire-typed — safe).

9. **Docs.** `git mv docs/api/downloads.md docs/api/pulls.md`; inside it, retitle and rewrite route paths to `/tama/v1/pulls/*` (keep the content otherwise). `docs/api/README.md:14` row → `| [Pulls](pulls.md) | pulls.md | Monitor file pull progress |` and :28 SSE row wording (`downloads, updates, jobs` → `pulls, updates, jobs`). `AGENTS.md`: line 253 (`querying and modifying models, backends, downloads, benchmarks` → `… backends, pulls, benchmarks …`) and line 271 (`docs/api/downloads.md — Download progress monitoring` → `docs/api/pulls.md — Pull progress monitoring`). Do NOT touch `docs/plans/README.md` (historical titles) or `docs/decisions/0009-*`.

**Steps:**
- [ ] Create the two migrations + register them; update `migrations_tests.rs`; run `cargo nextest run --package tama-core -- db::migrations` — pass
- [ ] Apply item 3 (routes/DTOs/module) and item 1 (router + frontend callers); run `cargo nextest run --package tama --test pulls_api` — pass
- [ ] Apply item 4 (config key, core + WASM + UI); run `cargo nextest run --package tama-core -- config` and `cargo nextest run --package tama` — pass
- [ ] Apply item 5 (`pulled_at`); run `cargo nextest run --package tama-core -- db::` — pass
- [ ] Apply items 6–8; run `cargo nextest run --workspace` — pass
- [ ] Apply item 9 (docs)
- [ ] Run `rg "downloads|Downloads" crates/ -g "*.rs"` — remaining hits only in historical migrations, `docs/plans/done`, ADRs, or explicitly-exempt spots (e.g. `backup/merge.rs` comment); `rg "/tama/v1/downloads" crates/` — zero hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor!: rename download->pull on routes, DTOs, config key, and DB columns (breaking: /tama/v1/downloads/* -> /tama/v1/pulls/*, download_queue_poll_interval_secs -> pull_queue_poll_interval_secs, model_files.downloaded_at -> pulled_at)"

**Acceptance criteria:**
- [ ] `/tama/v1/pulls/*` routes serve and `/tama/v1/downloads/*` is gone (proven by `pulls_api.rs` tests)
- [ ] Migrations 38/39 rename the columns preserving data (migration test)
- [ ] `QuantPullSpec`, `PullsActiveResponse`/`PullsHistoryResponse`/`PullCancelResponse` are the only type names (`rg "QuantDownloadSpec|Downloads.*Response|DownloadCancelResponse" crates/` — zero)
- [ ] `docs/api/pulls.md` exists; AGENTS.md and docs/api/README.md reference it
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 4: `ModelCard`→`ModelToml`, `ModelStatus`→`ModelStateSnapshot` (+ move out of gpu), `ModelState::Loading`→`Starting` (F28)

**Context:**
CONTEXT.md forbids "model card" (the type is a TOML file format) and canonizes `Starting` (the `BackendState` enum already uses `Starting`; the parallel `ModelState` enum diverges with `Loading`). Three linked renames: (a) `ModelCard` (`models/card.rs:10`) → `ModelToml` — file `card.rs` keeps its name; (b) `ModelStatus` (`gpu/types.rs:184`) → `ModelStateSnapshot` AND moved out of the gpu module (it is a model type embedded in `MetricSample.models` and `MetricsSnapshot.models`) into a new `crates/tama-core/src/models/types.rs` — chosen over `proxy/types.rs` because `gpu/types.rs` would then depend on `proxy::types`, while `proxy/types.rs` already depends on `crate::gpu` (a new cycle); `models/` imports nothing from `gpu`, so `gpu/types.rs` importing `crate::models::ModelStateSnapshot` keeps the graph acyclic; (c) `ModelState::Loading` → `Starting` with a serde alias for compat: the enum is `#[serde(rename_all = "lowercase")]`, so the wire value changes `"loading"`→`"starting"`; `#[serde(alias = "loading")]` keeps READING old payloads working (SSE snapshots, stored JSON); the WASM mirror (`crates/tama/src/gpu_types.rs`) makes the identical change. Display strings on the dashboard become "Starting"; CSS class hooks that happen to be the literal `"loading"` (e.g. `components/model_card.rs:205`) are styling, not vocabulary — leave them. If plan-172 has landed, `ModelState::try_from_str`/`from_str_fallback` are already deleted — skip step 3c.

**Files:**
- Create: `crates/tama-core/src/models/types.rs`
- Modify: `crates/tama-core/src/models/mod.rs`
- Modify: `crates/tama-core/src/models/card.rs`
- Modify: `crates/tama-core/src/gpu/types.rs`
- Modify: `crates/tama-core/src/gpu/mod.rs`
- Modify: `crates/tama-core/src/proxy/status.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/proxy/server/tests.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/handlers.rs`
- Modify: `crates/tama-core/src/models/registry.rs`, `crates/tama-core/src/models/pull/metadata.rs`, `crates/tama-core/src/config/defaults.rs`, `crates/tama-core/src/proxy/tama_handlers/tests.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs`, `crates/tama-core/src/proxy/lifecycle/mod.rs`
- Modify: `crates/tama-core/src/lib.rs` (crate docs)
- Modify: `crates/tama/src/gpu_types.rs`
- Modify: `crates/tama/src/pages/dashboard/metrics.rs`
- Modify: `crates/tama/src/components/model_card.rs`, `crates/tama/src/components/gpu_device_card.rs`, `crates/tama/src/pages/dashboard/tests.rs`

**What to implement:**

1. **`ModelCard` → `ModelToml`.** `models/card.rs:10`: rename the struct + its `impl` blocks + doc comment (`A model card describing…` → `A model TOML document describing a model and its available quants. Lives at ~/.config/tama/configs/<company>-<model>.toml.`); also fix the two "quantisations" spellings at :7,16 → "quants" (F40 piggyback). Update every user: `models/registry.rs`, `models/pull/metadata.rs`, `models/mod.rs` (re-export), `config/defaults.rs`, `proxy/state.rs` (:197 signature `Option<crate::models::card::ModelCard>` → `ModelToml`), `proxy/lifecycle/mod.rs` (two `model_card: Option<&crate::models::card::ModelCard>` params in `load_model` and `resolve_model_gpu_device` — rename the PARAM to `model_toml` too), `proxy/tama_handlers/models/handlers.rs` (local `model_card` bindings), `proxy/tama_handlers/pull/verify.rs`, `proxy/tama_handlers/tests.rs`. Rename `ProxyState::get_model_card` (`proxy/state.rs:197`) → `get_model_toml` and update all callers (`rg "get_model_card" crates/` — includes `lifecycle/mod.rs:39`, `tama_handlers/models/handlers.rs`, `tama_handlers/models/utils.rs:71`). `crates/tama-core/src/lib.rs:3,6-8` crate docs: "model card management" → "model TOML management", "## Model Card Configuration" → "## Model TOML Configuration", and the sentence about model cards. Local variables named `model_card` → `model_toml` in the touched files. Do NOT rename the Leptos component `crates/tama/src/components/model_card.rs` (it is a UI card, a different concept).

2. **`ModelStatus` → `ModelStateSnapshot` + move.** Create `crates/tama-core/src/models/types.rs`:
   ```rust
   //! Shared model-related types that don't belong to a single submodule.

   use serde::{Deserialize, Serialize};

   /// Per-model loaded/idle status snapshot, embedded in `MetricSample.models`
   /// and `MetricsSnapshot.models` and streamed to the dashboard over SSE.
   ```
   moving the struct verbatim from `gpu/types.rs:184` (all fields + serde attrs + doc comments) renamed to `ModelStateSnapshot`; its `state` field type stays `crate::gpu::ModelState`. Register `pub mod types;` in `models/mod.rs` and `pub use types::ModelStateSnapshot;`. In `gpu/types.rs`: delete the struct; change `pub models: Vec<ModelStatus>` (:158, :294) → `Vec<crate::models::ModelStateSnapshot>`; update the stale doc at :36 (`used in [ModelStatus]`). `gpu/mod.rs:22`: remove `ModelStatus` from the re-export. Update users: `proxy/status.rs:12,24,62` (`collect_model_statuses` return type and constructor — also consider renaming it `collect_model_state_snapshots`; YES, rename it, and update its 3 callers/tests: `rg "collect_model_statuses" crates/`), `proxy/server/tests.rs:460,463` (comments + type refs), `crates/tama/src/pages/dashboard/metrics.rs:113` mirror struct → rename to `ModelStateSnapshot` + update the mirror comment to `tama_core::models::ModelStateSnapshot`, and its users (`pages/dashboard/metrics.rs:58,169,187`, `components/gpu_device_card.rs:9,36,88,102,104,124,181,365,374,436`, `pages/dashboard/tests.rs`).

3. **`ModelState::Loading` → `Starting`.**
   a. `gpu/types.rs:39` enum: rename the variant; add compat alias:
      ```rust
      /// The backend is currently starting up.
      #[serde(alias = "loading")]
      Starting,
      ```
      (with `rename_all = "lowercase"` this serializes as `"starting"` and accepts `"loading"` on read).
   b. `as_str()` (:53): `Self::Starting => "starting"`.
   c. ONLY IF plan-172 has not deleted them: `try_from_str` (:27) and `from_str_fallback` (:68) — map both `"starting"` and `"loading"` → `Starting`; update their tests.
   d. Core users: `proxy/status.rs:49` (`Some(BackendState::Starting { .. }) => (crate::gpu::ModelState::Starting, None)`), `proxy/status.rs:504` (test assert), `tama_handlers/models/handlers.rs:83` (`ModelState::Starting` in the Starting arm — the response DTO `ListedModelResponse.state` then serializes `"starting"`).
   e. WASM mirror `crates/tama/src/gpu_types.rs`: rename the variant identically, add the same `#[serde(alias = "loading")]`, update `as_str`.
   f. WASM users: `components/model_card.rs` matches at :72,83,97,108,205,213,214,497 — `ModelState::Loading` → `ModelState::Starting`; user-facing display text `"Loading"` (:83) / `"Loading…"` (:108) → `"Starting"` / `"Starting…"`; the string-signal at :205 (CSS hook) stays `"loading"` with a comment `// CSS class hook, not domain vocabulary`. `components/gpu_device_card.rs:368` and `pages/dashboard/tests.rs:449` test helpers (`"loading" => ModelState::Loading` → accept both `"loading"` and `"starting"`); `pages/dashboard/tests.rs:68,213` doc comments updated to "starting".

**Steps:**
- [ ] Apply item 1 (`ModelToml`); run `cargo nextest run --package tama-core -- models::` — pass
- [ ] Apply item 2 (move + rename); run `cargo nextest run --package tama-core -- proxy::status` and `cargo nextest run --package tama` — pass
- [ ] Apply item 3 (`Starting`); run `cargo nextest run --workspace` — pass
- [ ] Run `rg "ModelCard|ModelStatus|ModelState::Loading|get_model_card" crates/ -g "*.rs"` — zero hits (except the untouched Leptos `components/model_card.rs` filename, which contains no `ModelCard` type reference to tama-core)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor!: ModelCard->ModelToml, ModelStatus->ModelStateSnapshot, ModelState::Loading->Starting (wire value loading->starting, serde alias keeps reads compatible)"

**Acceptance criteria:**
- [ ] `ModelStateSnapshot` lives in `crates/tama-core/src/models/types.rs`; `gpu/types.rs` has no model-domain structs
- [ ] Serialized `ModelState` emits `"starting"` and still deserializes `"loading"` (unit test both directions on the core enum)
- [ ] `rg "crate::gpu::ModelStatus|gpu::ModelStatus" crates/` — zero hits
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 5: `[supervisor]`→`[lifecycle]` config section + delete dead `ProcessSupervisor` (F28)

**Context:**
The `[supervisor]` config section holds backend-lifecycle knobs (restart policy, max restarts, health-check timings) — CONTEXT.md's term is "backend lifecycle". The config is DB-backed (table `app_supervisor`, queried via `get_supervisor`/`upsert_supervisor`/`SupervisorRecord` in `db/queries/app_config_queries.rs`) and mirrored twice in WASM (`types/config/supervisor.rs`, `pages/config_editor/types.rs:71`) plus a config-editor section (`Section::Supervisor`, tab label "Supervisor", anchor `cfg-supervisor`, `SupervisorForm`, `SupervisorPatch`). Rename strategy: struct `Supervisor`→`Lifecycle` everywhere, `Config.supervisor` field→`lifecycle` with `#[serde(alias = "supervisor")]` for backward-compatible READING of old config JSON/TOML payloads (writes emit `"lifecycle"`), DB table renamed by migration `_0040`, query fns/record renamed, config-editor section renamed (label "Lifecycle", keep the 👀 icon, anchor `cfg-lifecycle`). Separately, `ProcessSupervisor` (`crates/tama-core/src/process.rs:88`) has zero production constructors (only its own tests construct it) — delete the struct, its impl (`new`, `with_log_dir`, `with_gpu_env`, `run`), and its tests; verify `HealthCheck`/`ProcessEvent` users before deleting anything shared.

**Files:**
- Create: `crates/tama-core/src/db/migrations/_0040_rename_app_supervisor_to_app_lifecycle.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`, `crates/tama-core/src/db/migrations/migrations_tests.rs`
- Rename: `crates/tama-core/src/config/types/supervisor.rs` → `crates/tama-core/src/config/types/lifecycle.rs`
- Modify: `crates/tama-core/src/config/types/mod.rs`
- Modify: `crates/tama-core/src/db/queries/app_config_queries.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/idle_timeout.rs`, `crates/tama-core/src/proxy/lifecycle/tests.rs`
- Modify: `crates/tama-core/src/db/backfill/migrate_toml_to_db.rs`
- Rename: `crates/tama/src/types/config/supervisor.rs` → `crates/tama/src/types/config/lifecycle.rs`
- Modify: `crates/tama/src/types/config/mod.rs`, `crates/tama/src/types/config/patch.rs`
- Modify: `crates/tama/src/pages/config_editor/types.rs`, `crates/tama/src/pages/config_editor/mod.rs`
- Rename: `crates/tama/src/pages/config_editor/forms/supervisor.rs` → `crates/tama/src/pages/config_editor/forms/lifecycle.rs`
- Modify: `crates/tama/src/pages/config_editor/forms/mod.rs`
- Modify: `crates/tama-core/src/process.rs`

**What to implement:**

1. **Migration `_0040`:** `(40, false, r#"ALTER TABLE app_supervisor RENAME TO app_lifecycle;"#)`; register in `migrations.rs`; extend `migrations_tests.rs` (copy the assertion style used for `_0034`: seed pre-40, run to 40, assert `app_lifecycle` exists with the row intact).
2. **Core config types:** `git mv` the file; in it rename `pub struct Supervisor` → `Lifecycle` (+ doc comment `Supervisor configuration` → `Backend lifecycle configuration`); the 6 `default_*` fns stay named as-is (private). In `config/types/mod.rs`: `mod supervisor;` → `mod lifecycle;`, `pub use supervisor::*;` → `pub use lifecycle::*;`, field :40 `pub supervisor: Supervisor` → `#[serde(alias = "supervisor")] pub lifecycle: Lifecycle`; update `from_db` (:142-160: `get_supervisor` → `get_lifecycle`, `supervisor_row` → `lifecycle_row`, error message `"app_supervisor row not found…"` → `"app_lifecycle row not found…"`, local `supervisor` → `lifecycle`, constructor field at :235) and `to_db` (:303-311: `upsert_supervisor` → `upsert_lifecycle`, `self.supervisor.*` → `self.lifecycle.*`). Check `Config::default()`/seed paths in the same file.
3. **DB queries (`app_config_queries.rs`):** `SupervisorRecord` → `LifecycleRecord` (+ doc `A row from the app_supervisor table` → `app_lifecycle`); `get_supervisor` → `get_lifecycle` (:347, SQL `FROM app_supervisor` → `app_lifecycle`); `upsert_supervisor` → `upsert_lifecycle` (:320, SQL); the seed at :566 (`INSERT OR IGNORE INTO app_supervisor` → `app_lifecycle`); module doc at :4. Update the file's tests.
4. **Core users:** `proxy/lifecycle/idle_timeout.rs:33-34` (`cfg.supervisor.max_restarts` → `cfg.lifecycle.*`), `proxy/lifecycle/tests.rs:761,818` (`config.supervisor.max_restarts = 0` → `config.lifecycle.*`), `db/backfill/migrate_toml_to_db.rs:250-255` (`config.supervisor.*` → `config.lifecycle.*` — note this reads the RUST struct post-deserialization; with the serde alias in item 2, old TOML `[supervisor]` sections still parse). Sweep: `rg "\.supervisor|Supervisor" crates/tama-core/src` after the edits.
5. **WASM mirrors + editor:** `git mv crates/tama/src/types/config/supervisor.rs crates/tama/src/types/config/lifecycle.rs` (struct `Supervisor`→`Lifecycle`, mirror comment); `types/config/mod.rs`: `pub use patch::SupervisorPatch` → `LifecyclePatch`, field :54 `pub supervisor: Supervisor` → `#[serde(alias = "supervisor")] pub lifecycle: Lifecycle`, mod decl; `types/config/patch.rs:23,26,111`: `SupervisorPatch` → `LifecyclePatch` (+ doc); `pages/config_editor/types.rs:25` field + :71 struct → `Lifecycle` (+ alias), test JSON literal at :231 (`"supervisor": {` → `"lifecycle": {`); `pages/config_editor/mod.rs`: `Section::Supervisor` → `Section::Lifecycle` (:12, :24 label `"Supervisor"` → `"Lifecycle"`, :35 icon stays 👀, :56 import, :154 list, :159/:206 anchor `cfg-supervisor` → `cfg-lifecycle`); `git mv forms/supervisor.rs forms/lifecycle.rs` (component `SupervisorForm` → `LifecycleForm`); `forms/mod.rs:15` re-export. Check `crates/tama/src/api.rs` for `"supervisor"` JSON handling in config GET/PATCH (the serde alias covers reads; PATCH paths go through `LifecyclePatch` — update any string keys).
6. **Delete `ProcessSupervisor` (`process.rs`).** Re-verify zero production constructors: `rg "ProcessSupervisor::new|ProcessSupervisor" crates/ --type rust` — expect only `process.rs` (def + tests at :311,323,342). Delete the struct (:88-98), its full `impl` block (:100-…, including `new`, `with_log_dir`, `with_gpu_env`, `run`), and the test fns that construct it. Then check what it leaves orphaned: `rg "HealthCheck|ProcessEvent" crates/tama-core/src` — if `HealthCheck`/`ProcessEvent` have no remaining users outside `process.rs`, delete them too (they were the supervisor's config/events); if used elsewhere, keep. Keep `configure_backend_command`, `check_health`, and everything else in the file.

**Steps:**
- [ ] Create + register migration `_0040`; extend `migrations_tests.rs`; run `cargo nextest run --package tama-core -- db::migrations` — pass
- [ ] Apply items 2–4 (core); run `cargo nextest run --package tama-core -- config` and `-- proxy::lifecycle` — pass
- [ ] Apply item 5 (WASM + editor); run `cargo nextest run --package tama` — pass
- [ ] Delete `ProcessSupervisor` per item 6; run `cargo nextest run --package tama-core` — pass
- [ ] Run `rg "supervisor|Supervisor" crates/ -g "*.rs"` — zero hits outside historical migrations/`docs/`
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor!: rename [supervisor] config section to [lifecycle]; delete dead ProcessSupervisor (config JSON key supervisor->lifecycle, serde alias keeps reads compatible)"

**Acceptance criteria:**
- [ ] `app_lifecycle` table exists post-migration with data preserved (migration test)
- [ ] Old config payloads with `"supervisor"` still deserialize (serde alias unit test on `Config`)
- [ ] Config editor shows a "Lifecycle" section; `rg "Section::Supervisor|SupervisorForm|SupervisorPatch" crates/tama/src` — zero hits
- [ ] `ProcessSupervisor` fully deleted; `cargo nextest run --workspace` passes; clippy clean

---

### Task 6: Naming small batch — `lookup_*`, `core_mirrors`, `FieldUpdate`, `AliasUpdate`, `delete_update_checks_for_backend` (F40)

**Context:**
Loose-end renames and micro-abstractions from the audit's low-severity naming sweep: (a) CONTEXT.md forbids "fetch" for pull-domain work — the 4 metadata fns in `models/pull/` become `lookup_*` (they are lookups, not pulls); (b) `crates/tama/src/gpu_types.rs` mostly mirrors NON-GPU config enums — rename the module to `core_mirrors` (9 import sites); (c) the `"name:variant"` composite-key LIKE pattern is hand-built (with manual `\`/`_`/`%` escaping) at two call sites — one query helper ends that; (d) two PATCH DTOs use `Option<Option<T>>` tri-states — introduce a self-documenting `FieldUpdate<T>`; `update_alias`'s 5 positional params become a params struct; (e) `check_single` tells you nothing (it triggers an update check for one item — becomes `check_item_for_update`); (f) the `handle_tama_get_model as handle_tama_get_model_fn` import alias is gratuitous. Each is independent; they share one commit only for release-note hygiene.

**Files:**
- Modify: `crates/tama-core/src/models/pull/api.rs`, `crates/tama-core/src/models/pull/metadata.rs` (rename + doc)
- Modify all callers of the 4 fns: `crates/tama-core/src/updates/checker/model.rs`, `crates/tama-core/src/models/update.rs`, `crates/tama/src/api/hf.rs`, `crates/tama-core/src/db/backfill/initial_backfill.rs`, `crates/tama-core/src/db/backfill/hf_metadata.rs`, `crates/tama/src/api/models/files.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs`, `crates/tama-core/src/proxy/tama_handlers/system.rs`, `crates/tama-core/src/proxy/tama_handlers/pull/start.rs`
- Rename: `crates/tama/src/gpu_types.rs` → `crates/tama/src/core_mirrors.rs`
- Modify: `crates/tama/src/lib.rs` + the 9 import sites (`pages/models.rs:8`, `pages/config_editor/types.rs:4`, `pages/config_editor/forms/supervisor.rs:4` or its renamed `lifecycle.rs`, `forms/general.rs:4`, `forms/compaction.rs:5`, `pages/dashboard/tests.rs:6`, `components/model_card.rs:11`, `pages/dashboard/metrics.rs:3`, plus any `crate::gpu_types` in `pages/config_editor/types.rs:13` comment)
- Modify: `crates/tama-core/src/db/queries/update_check_queries.rs`, `crates/tama-core/src/db/repository.rs`, `crates/tama/src/api/backends/install.rs`, `crates/tama/src/api/backends/manage/remove.rs`
- Create: `crates/tama/src/api/field_update.rs`
- Modify: `crates/tama/src/api.rs` (register `pub mod field_update;`)
- Modify: `crates/tama/src/api/aliases/mod.rs`, `crates/tama-core/src/db/queries/alias_queries.rs`, `crates/tama-core/src/db/repository.rs`
- Modify: `crates/tama/src/api/backends/compaction.rs`
- Modify: `crates/tama/src/api/updates.rs`, `crates/tama/src/router.rs`
- Modify: `crates/tama-core/src/proxy/server/router.rs`
- Comment-only: `crates/tama-core/src/proxy/tama_handlers/types.rs:16,27`, `crates/tama-core/src/models/pull/quant.rs:1`, `crates/tama-core/src/bench/llama_bench/mod.rs:56`, `crates/tama/src/components/pull_wizard/mod.rs:129,139`, `crates/tama/src/components/pull_wizard/components/selection_step.rs:20`, `crates/tama/src/components/pull_wizard/components/repo_input.rs:14`

**What to implement:**

1. **`fetch_*` → `lookup_*`** in `models/pull/api.rs`: `fetch_blob_metadata` (:80) → `lookup_blob_metadata`, `fetch_hf_metadata` (:113) → `lookup_hf_metadata`, `fetch_model_pipeline_tag` (:208) → `lookup_model_pipeline_tag`; in `models/pull/metadata.rs`: `fetch_community_card` (:336) → `lookup_community_card`. Update the doc comments' first lines (`Fetch per-file blob metadata…` → `Look up per-file blob metadata…`). Update all callers (file list above; `rg "fetch_blob_metadata|fetch_hf_metadata|fetch_model_pipeline_tag|fetch_community_card" crates/` must end at zero). Leave unrelated `fetch_*` names alone (e.g. `fetch_capabilities_from_backend`, `fetch_models_from_backend` — backend-side, not pull-domain… rename these too if the sweep is trivial: NO — out of scope; CONTEXT.md's "fetch" ban is for model pulls).

2. **"quantisation" → "quantization"/"quant"** in the 8 comment-only sites listed under Files (user-facing strings in the wizard: `selection_step.rs:20` "quantisation files" → "quant files"; `repo_input.rs:14` "quantisations" → "quants"). No identifier changes.

3. **`gpu_types.rs` → `core_mirrors.rs`:** `git mv`; `crates/tama/src/lib.rs:48` `mod gpu_types;` → `mod core_mirrors;`; the 9 `use crate::gpu_types::…` import sites → `use crate::core_mirrors::…`; the doc comment at `pages/config_editor/types.rs:13` (`enums from gpu_types …`) → `core_mirrors`; update the file's own header comment (`//! Mirror types from tama-core …` stays accurate — keep).

4. **`delete_update_checks_for_backend`.** In `db/queries/update_check_queries.rs` add:
   ```rust
   /// Delete all update check records for a backend name, covering every
   /// gpu_variant (`name:%`) plus the legacy variant-less row (`name`).
   /// Handles the SQL LIKE escaping of `name` internally so callers never
   /// hand-write patterns.
   pub fn delete_update_checks_for_backend(conn: &Connection, name: &str) -> Result<()> {
       let escaped = name.replace('\\', "\\\\").replace('_', "\\_").replace('%', "\\%");
       delete_update_checks_by_pattern(conn, "backend", &format!("{}:%", escaped))?;
       delete_update_check(conn, "backend", name)?;
       Ok(())
   }
   ```
   Add a pass-through on `Repository` (next to `delete_update_checks_by_pattern` at `repository.rs:420`) with the same name. Replace the two hand-rolled escape+pattern+double-delete blocks at `api/backends/install.rs:640-648` and `api/backends/manage/remove.rs:180-187` with a single `let _ = repo.delete_update_checks_for_backend(&name);` each. Add a query test in `db/queries/tests.rs` (rows `llama_cpp:cpu`, `llama_cpp:vulkan`, `llama_cpp`, `other:cpu` → call with `"llama_cpp"` → first three gone, `other:cpu` intact; plus an escaping case with a name containing `_`).

5. **`FieldUpdate<T>` + PATCH tri-states.** Create `crates/tama/src/api/field_update.rs`:
   ```rust
   //! Tri-state for PATCH bodies: field absent (leave unchanged), explicit null
   //! (clear), or a value (set). Replaces `Option<Option<T>>` at API boundaries.

   use serde::{Deserialize, Deserializer};

   /// PATCH tri-state. `#[serde(default)]` on the field gives `Unchanged` when absent.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub enum FieldUpdate<T> {
       Unchanged,
       Clear,
       Set(T),
   }

   impl<T> Default for FieldUpdate<T> {
       fn default() -> Self { Self::Unchanged }
   }

   impl<'de, T> Deserialize<'de> for FieldUpdate<T>
   where
       T: Deserialize<'de>,
   {
       fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
       where
           D: Deserializer<'de>,
       {
           Ok(match Option::<T>::deserialize(deserializer)? {
               Some(v) => Self::Set(v),
               None => Self::Clear,
           })
       }
   }
   ```
   Register `pub mod field_update;` in `crates/tama/src/api.rs`. Apply to `UpdateAliasRequest.description` (`api/aliases/mod.rs:277`): `#[serde(default)] pub description: FieldUpdate<String>` — update the handler's match on it (`Some(Some(_))` → `FieldUpdate::Set(_)`, `Some(None)` → `FieldUpdate::Clear`, `None` → `FieldUpdate::Unchanged`) and its conversion into the query param (map `Set(v)` → `Some(Some(v))`, `Clear` → `Some(None)`, `Unchanged` → `None`). Apply to `CompactionToggleRequest.port` (`api/backends/compaction.rs:14`): `#[serde(default)] pub port: FieldUpdate<u16>` with the same mapping. Add unit tests in `field_update.rs`: missing key → `Unchanged`; explicit `null` → `Clear`; value → `Set` (test against a small local struct using `serde_json::from_str`).

6. **`AliasUpdate` params struct.** In `db/queries/alias_queries.rs`:
   ```rust
   /// Fields to update on an alias. `None` leaves the column unchanged;
   /// `description: Some(None)` clears it.
   #[derive(Debug, Default)]
   pub struct AliasUpdate<'a> {
       pub name: Option<&'a str>,
       pub model_id: Option<i64>,
       pub description: Option<Option<&'a str>>,
       pub enabled: Option<bool>,
   }
   ```
   Change `update_alias(conn, id, name, model_id, description, enabled)` (:119) → `update_alias(conn: &Connection, id: i64, update: AliasUpdate) -> Result<()>` (body swaps the 5 params for `update.name` etc.). Update callers: `db/repository.rs:344` (its `Repository::update_alias` keeps its own signature but delegates with the struct — better: change it to `pub fn update_alias(&self, id: i64, update: queries::AliasUpdate)` and fix `api/aliases/mod.rs:204` to build the struct), `repository.rs:650` (test), `alias_queries.rs:235,269,276` (tests). Do NOT touch `crates/tama/src/pages/aliases/mod.rs` — its `update_alias` is a different, WASM-side API client fn.

7. **`check_single` → `check_item_for_update`.** `api/updates.rs:227` rename (handler doc already says "Check single item" — expand to `/// POST /tama/v1/updates/check/:item_type/:item_id — trigger an update check for one backend variant or model`); `router.rs:170` update `post(api::updates::check_item_for_update)`.

8. **Router alias.** `proxy/server/router.rs:31`: `handle_tama_get_model as handle_tama_get_model_fn,` → `handle_tama_get_model,`; :60 use `get(handle_tama_get_model)`.

**Steps:**
- [ ] Apply items 1–3; run `cargo nextest run --package tama-core` and `--package tama` — pass
- [ ] Apply item 4; run `cargo nextest run --package tama-core -- db::queries` — new test passes
- [ ] Apply items 5–6; run `cargo nextest run --package tama -- api::aliases` and `-- api::backends` — pass
- [ ] Apply items 7–8; run `cargo nextest run --workspace` — pass
- [ ] Run `rg "fetch_blob_metadata|fetch_hf_metadata|fetch_model_pipeline_tag|fetch_community_card|gpu_types|check_single|handle_tama_get_model_fn|QuantDownloadSpec" crates/ -g "*.rs"` — zero hits; `rg -i "quantisation" crates/ -g "*.rs"` — zero hits
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: naming small batch — lookup_*, core_mirrors, FieldUpdate tri-state, AliasUpdate params, delete_update_checks_for_backend"

**Acceptance criteria:**
- [ ] All 4 metadata fns renamed with zero leftover callers; module `core_mirrors` in place with all 9 imports updated
- [ ] `FieldUpdate<T>` used by both PATCH DTOs; tri-state unit tests (absent/null/value) pass
- [ ] `update_alias` takes `AliasUpdate`; both LIKE-pattern call sites use `delete_update_checks_for_backend`
- [ ] `cargo nextest run --workspace` passes; clippy clean
