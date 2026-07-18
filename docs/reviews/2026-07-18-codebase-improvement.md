# Codebase Improvement Report — 2026-07-18

## Summary

40 findings across 8 lenses. 10 high, 25 medium, 5 low.

The 2026-07-06 audit's 28 findings were substantially addressed (verified: god files split, ProxyState encapsulated, enums added, lifecycle traits + 38 tests, error helper adopted, web_types moved, test fixtures shared, `gpu_type`/`server`/`download` renames mostly landed). The dominant themes now:

1. **The DB layer is mid-migration** — `Repository` (plan-146) now competes with `BackendManager`/`ModelManager` (ADR-0017), handlers open both, DTO/Record types duplicate, and raw `rusqlite` leaks across 19 files. Two partial abstractions coexisting is worse than either alone.
2. **Recent features (plans 151–158) shipped without tests on critical paths** — OAuth2 login flow, `forward_request` circuit breaker, pull-pipeline HTTP handlers.
3. **Dead/dishonest code accumulated** — backup restore returns success for a no-op, a whole `platform/` module is dead, a 606-line orphan file, dead Leptos components, unused deps.
4. **Error contract drifted again** — 3 error shapes on the wire; OpenAPI spec contradicts the helper and the docs.

## Context

- CONTEXT.md: loaded (domain glossary; no Engineering Rules section — defaults applied)
- ADRs reviewed: 20 (3 in `docs/adr/` + 17 in `docs/decisions/`)
- Plans reviewed: 15 (`docs/plans/done/` plan-144 through plan-158)
- Prior review: `docs/reviews/2026-07-06-codebase-improvement.md` — fixes verified per finding below

## Verified fixed from 2026-07-06 audit

God files split ✔ · error helper exists ✔ (adoption incomplete → F4) · ProxyState encapsulated ✔ (accessor leaks remain → F32) · GpuVendor/ModelState/RestartPolicy/LogLevel/CompactionDevice enums ✔ · typed DB records ✔ · lifecycle traits + tests ✔ · update checker + pull queue tests ✔ · config test fixtures shared ✔ · web_types moved to tama ✔ · network.rs alive ✔ · logging.rs alive ✔ · benchmark submission helpers ✔ (partial → F13) · `BackendManager::open`/`spawn_model_crud` helpers ✔ · `gpu_type`→`gpu_variant` DB layer ✔ (types remain → F27) · `server`→`backend` types ✔ (strings/vars remain → F27) · `download`→`pull` core ✔ (API/config surface remains → F27) · ModelConfig composition note ✔

## Findings

### 🔴 High Severity

#### 1. Two competing DB access layers: `Repository` vs `BackendManager`/`ModelManager`

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/db/repository.rs` (908 lines), `crates/tama-core/src/models/manager.rs`, `crates/tama-core/src/backends/manager.rs`, `crates/tama/src/api/models/crud/update.rs:40,78`, `crud/delete.rs:31,39`, `api/updates.rs:128,252,442,550,670`
- **Severity:** High
- **Confidence:** High
- **Problem:** ADR-0017 explicitly chose centralized managers *over* a generic repository. `db/repository.rs` (added by plan-146) now duplicates manager domains (model configs, files, pulls, aliases, benchmarks, queue, update checks) with method-level overlap: `Repository::get_model_config` ↔ `ModelManager::get_config`, `Repository::get_pull_queue_item` ↔ `ModelManager::queue_get_by_job_id`, etc. Handlers open **both** layers in one request (two SQLite connections) because reads are DTO-land and writes are manager-land. `ModelConfigDto` duplicates `ModelConfigRecord` field-for-field (~43 fields) forcing two `from_db_record*` constructors on `ModelConfig`. `ModelConfigRecord` is `#[deprecated]` yet remains `ModelManager`'s entire public API return type. repository.rs's doc claim "record types are `pub(crate)`" is false — they're all `pub`.
- **Proposal:** Pick one layer per domain and finish the migration: either (a) honor ADR-0017 — fold Repository's DTO-returning reads into the managers, delete `db/repository.rs`; or (b) amend ADR-0017 and make Repository the single API-layer entry point, demoting managers to proxy-internal use. Either way: no handler opens both, collapse Record/DTO to one struct per table, honor the `pub(crate)` claim.

#### 2. DB funnel leaks: raw `rusqlite` across 19 files, raw SQL in a handler, 28 per-request `Repository::open`

- **Lens:** Coupling
- **Files:** `crates/tama-core/src/db/mod.rs:15` (`pub use rusqlite::Connection`), `models/manager.rs:40` (`pub fn conn()`), `proxy/{state.rs,api_keys.rs,auth.rs,scope_middleware.rs,forward/request.rs}`, `config/resolve/mod.rs:551`, `bench/llama_bench/mod.rs:233`, `backup/{archive.rs,merge.rs}`, `crates/tama/src/api/models/crud/delete.rs:194-200`, `crates/tama/src/api/**` (28 `Repository::open` sites)
- **Severity:** High
- **Confidence:** High
- **Problem:** The ADR-0017 funnel is porous: 19 files outside `db/` use `rusqlite::Connection` (12 direct `Connection::open`), `db/mod.rs` blesses it with a re-export, and public APIs take raw connections (`proxy::api_keys::*` 8 fns, `ModelManager::conn()`, `ProxyState::open_db()`). Worst case: `delete.rs` opens a Repository AND a ModelManager, then runs raw SQL (`DELETE FROM model_configs WHERE id = ?1`) through the manager escape hatch — while `ModelManager::delete_config(id)` exists; `rusqlite` is a dependency of the `tama` crate solely for this. Separately, every one of the 28 API handlers constructs its own `Repository::open(&config_dir)`, and each `open()` re-runs the full migration suite.
- **Proposal:** Make the `Connection` re-export and manager escape hatches `pub(crate)`; convert `proxy::api_keys::*` free fns into a small store struct; replace delete.rs raw SQL with `delete_config` and drop `rusqlite` from `crates/tama/Cargo.toml`; construct `Repository` once at startup and share via axum state (migrations run once).

#### 3. Module-level circular dependency: `config` ↔ `proxy`

- **Lens:** Coupling
- **Files:** `crates/tama-core/src/config/types/mod.rs:108`, `crates/tama-core/src/proxy/types.rs:258`, 22 proxy files importing `crate::config`
- **Severity:** High
- **Confidence:** High (verified directly)
- **Problem:** `Config::from_db()` calls `crate::proxy::api_keys::count_active_keys(&conn)` to derive `api_keys_enabled`, while `ProxyState` stores `Arc<RwLock<Config>>` — a true module-level import cycle (config → proxy::api_keys → config). `config/types/mod.rs` also reaches into `db::queries::get_proxy` directly, doubling as its own persistence layer.
- **Proposal:** `count_active_keys` is a pure SQL count — move it to `db::queries` (new `api_key_queries.rs`); `proxy::api_keys` re-uses it. Zero architectural change; breaks the only true cycle.

#### 4. Error contract drift: 3 wire shapes; OpenAPI spec contradicts helper and docs

- **Lens:** Inconsistent Patterns
- **Files:** `crates/tama/src/api/error.rs` (nested `{"error":{"message","type"}}`, canonical per `docs/api/errors.md`), flat `{"error":"..."}` at ~55 sites in 14 files (`api/updates.rs` ×24 — never imports the helper, `backends/install.rs` ×6, `manage/remove.rs` ×5, `manage/activate.rs` ×5, `self_update.rs` ×4, `backends/list.rs` ×4, `hf.rs` ×3, …), third shape `{"error":code,"message":msg}` in `proxy/tama_handlers/api_keys.rs:66`; `crates/tama/src/api/openapi.rs:644`
- **Severity:** High
- **Confidence:** High (verified directly: openapi.rs codifies flat `{"error":{"type":"string"}}`, contradicting error.rs and errors.md)
- **Problem:** Three divergent error wire formats across the management API; the spec, the helper, and the docs all disagree. Clients cannot parse errors uniformly. tama-core handlers (`tts.rs` ×13, `pull/handlers.rs` ×10) also hand-roll structured JSON.
- **Proposal:** Migrate all flat sites to `error_response`/`error_body`; add a structured helper in `tama-core::proxy::handlers`; fix the `ErrorResponse` schema in openapi.rs; add a shape-assertion test per module.

#### 5. Stringly-typed model identity: `config_key` rule open-coded 12+ times, applied inconsistently, two same-named `resolve_model_id`

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/db/mod.rs:35`, `crates/tama/src/api/models/crud/{update.rs:88,171, rename.rs:106, delete.rs:89,254}`, `crates/tama-core/src/models/verify.rs:300`, `proxy/tama_handlers/pull/{verify.rs:213, handlers.rs:133,256}`, `components/pull_quant_wizard.rs:451`; resolvers: `proxy/tama_handlers/models/utils.rs:50` vs `crates/tama/src/api/models/info.rs:29`
- **Severity:** High
- **Confidence:** High
- **Problem:** A model has ≥5 identifiers (DB id, config_key, repo_id, api_name, alias) with no type distinguishing them. The canonical rule `config_key = repo_id.to_lowercase().replace('/', "--")` is inlined at 12+ sites and applied *inconsistently*: pull paths skip the lowercase, the wizard reverses the call order — so the same repo maps to different keys depending on entry point. Identity resolution is duplicated under the same name with different semantics (config-key resolution vs DB-id resolution).
- **Proposal:** `ConfigKey` newtype in tama-core with `ConfigKey::from_repo_id()` as the only derivation site; make managers/Repository/handlers take it; rename one resolver (`resolve_config_key` vs `resolve_db_id`).

#### 6. Backup restore is a dishonest no-op; backup feature half-dead

- **Lens:** Dead Code
- **Files:** `crates/tama/src/api/backup.rs:283`, `crates/tama/src/router.rs:22,158-161`, `crates/tama-core/src/backup/{archive.rs,merge.rs}`, `docs/api/backup.md`
- **Severity:** High
- **Confidence:** High (verified directly)
- **Problem:** `POST /tama/v1/restore` spawns a task whose body is `// TODO: Implement actual restore logic` + `let _ = (config_dir, temp_dir, job);` — it **silently discards the uploaded backup, deletes the file, and reports success**. `create_backup` handler exists but is never routed; `docs/api/backup.md` documents a `GET /tama/v1/backup` endpoint that doesn't exist; consequently `tama_core::backup::{create_backup, extract_backup, merge::*}` (~700 lines) have no live callers.
- **Proposal:** Either implement restore (wire `extract_backup` + `merge_*`, route `create_backup`) or remove the endpoints and fix the docs. At minimum, `start_restore` must return 501/not-implemented instead of false success.

#### 7. Entire `platform/` module is dead code (226 lines); Makefile `update` target runs nonexistent commands

- **Lens:** Dead Code
- **Files:** `crates/tama-core/src/platform/{mod.rs,linux.rs}` (11 pub fns), `crates/tama-core/src/lib.rs:23`, `Makefile:31-36`
- **Severity:** High
- **Confidence:** High (verified directly: zero callers outside the module)
- **Problem:** All 11 systemd-service functions (`install_proxy_service`, `start_service`, …) have zero callers — leftover from the removed CLI (ADR-0005). `make update` still runs `tama service stop/start`, which cannot work (no CLI exists).
- **Proposal:** Delete `crates/tama-core/src/platform/` + the `pub mod platform;`; fix or remove the Makefile `update` target's `tama service …` lines.

#### 8. OAuth2 login flow handlers have zero tests

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/proxy/auth.rs` — `handle_login` (:509), `handle_login_callback` (:568), `handle_logout` (:667), `fetch_userinfo` (:413), `build_oauth2_client_from_config` (:396)
- **Severity:** High
- **Confidence:** High
- **Problem:** ~260 lines of security-critical code (authorize-URL generation, state/CSRF param, code exchange, cookie issuance, logout) with no test invoking them. The existing 17 tests cover `auth_middleware`/cookie validation only. A broken state check = CSRF on login; a broken callback locks all users out.
- **Proposal:** `authenticator_url` is already a config seam (proven by an existing test with a local axum mock). Add tests: login redirect params; callback with valid state + wiremock token/userinfo → 303 + signed cookie; state mismatch → error redirect; token 500 → error redirect; logout clears cookie.

#### 9. `forward_request` core routing logic untested (circuit breaker, dead-process cleanup)

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/proxy/forward/request.rs` (625 lines)
- **Severity:** High
- **Confidence:** High
- **Problem:** The hot path for every inference request has no direct test: dead-PID detection + cleanup (502 `BackendCrashedError`), circuit-breaker threshold → 503 during cooldown, cooldown pass-through, metric increments. Helpers are well-tested (33 tests); the orchestrator is not.
- **Proposal:** Build `ProxyState` via the existing `models/tests/helpers.rs::create_state_with_model` pattern; insert state with `consecutive_failures` ≥ threshold → assert 503; dead PID → assert 502 + cleanup; wiremock backend URL for pass-through. No refactor needed.

#### 10. Pull pipeline HTTP handlers and download/verify orchestration untested

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/proxy/tama_handlers/pull/{handlers.rs (517), download.rs (565), verify.rs (542)}`
- **Severity:** High
- **Confidence:** High
- **Problem:** No `#[cfg(test)]` in any of the three. The queue *machinery* is well-tested (25 tests), but the HTTP surface — validation, enqueue rejections, SSE job stream, progress wiring, hash verification (corrupt-GGUF admission) — is not.
- **Proposal:** Route-level tests using the existing `crates/tama/tests/downloads_api.rs` pattern (tempdir `PullQueueService` + axum `oneshot`); `HF_ENDPOINT` override + wiremock for download/verify; assert hash-mismatch → failed job, invalid quant → 400, SSE emits job events.

### 🟡 Medium Severity

#### 11. God files: `pull_queue.rs` (1,756), `api/updates.rs` (1,022), `auth.rs` (1,526)

- **Lens:** File Length + Structure
- **Files:** `crates/tama-core/src/proxy/pull_queue.rs` (queue mgmt + DB + event broadcast + async task lifecycle; ~1,230 of the lines are inline tests), `crates/tama/src/api/updates.rs` (SSE + version checking + update queuing; 24 hand-rolled error sites), `crates/tama-core/src/proxy/auth.rs` (~700 prod lines mixing API keys + OAuth2 + session cookies + middleware)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Three files each carry 3+ distinct responsibilities. pull_queue's inline test module (~1,230 lines) could move to a `tests/` submodule; updates.rs is also the worst error-shape offender (F4); auth.rs mixes three auth mechanisms.
- **Proposal:** Split `pull_queue.rs` → `pull_queue/{service,recovery,events,tests}.rs`; `updates.rs` → `updates/{check,apply,events}.rs`; `auth.rs` → `auth/{middleware,api_key,session,oauth2}.rs`.

#### 12. Server-side SSE: job-event stream duplicated ~85 lines + hand-rolled event→wire mapping (already drifted)

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/api/backends/jobs.rs:64-167` vs `api/benchmarks/history.rs:74-180` (near-verbatim snapshot→replay→live loop); `api/downloads.rs:202-282` vs `api/updates.rs:356-402` (~60-line match each, payload drift: downloads embeds `"event"` in JSON, updates doesn't)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Two copies of the SSE replay machinery; two copies of enum→SSE mapping with divergent payloads; `UpdateEvent` derives `Serialize` yet is re-flattened field-by-field. `keep_alive` also applied inconsistently (`jobs.rs:166`, `history.rs:184`, `backend_logs.rs:195` lack it).
- **Proposal:** Derive `#[serde(tag="event")]` on `PullEvent`/`UpdateEvent` + `to_sse_event()`; extract one `job_event_stream(job)` and one `broadcast_to_sse(rx)` helper; add KeepAlive uniformly.

#### 13. Benchmark job submission triplicated; shared helper `submit_benchmark_job` is dead code

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/api/benchmarks/{run.rs:9-50, mtp.rs:52-102, spec.rs:57-96}`; dead helper `benchmarks/mod.rs:208-260`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Identical ~30-line submit→db_path→spawn→finish block in all 3 handlers (~90 lines); the intended shared helper was written but never wired in (60 lines dead).
- **Proposal:** Route all three through `submit_benchmark_job` (adjust inner-fn signatures) or delete the helper and extract `benchmark_job_ctx(&state)`.

#### 14. Model-resolution chain repeated 6× across model API handlers

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/api/models/crud/update.rs:39-72,122-155`, `crud/rename.rs:36-56`, `crud/delete.rs:160-184`, `models/files.rs:47-89,199-241`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `Repository::open` + `resolve_model_id` + `get_model_config` + NotFound/ValidationError mapping (~20-45 lines each, ~150-180 total); the files.rs pair is verbatim including `models_dir` resolution.
- **Proposal:** `resolve_model_record(config_dir, id_str) -> Result<(i64, ModelConfigRecord), (StatusCode, Value)>` in `api/models/info.rs`.

#### 15. Config-dir/`db_dir` resolution: 4 variants at 17+ sites with divergent failure behavior

- **Lens:** DRY Violations
- **Files:** `api/aliases/mod.rs` ×5, `api/backup.rs:70`, `api/backends/list.rs:38`, `api/helpers.rs:18`, `api/updates.rs:83,189,233,424,543,654`, `api/api.rs:176-207`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Variant A silently falls back to `./tama.db` (CWD); variant B returns 404; variant C uses `base_dir()`; variant D is the only factored helper. Same misconfiguration → different behavior depending on endpoint.
- **Proposal:** Single `resolve_config_dir(&ProxyState) -> Result<PathBuf, Response>` (+ `open_repository(state)`) in `api/helpers.rs`; delete A–C.

#### 16. `gpu_variant` primitive obsession: string-matched across 4 domains despite an existing enum

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/gpu/env.rs:28,45-48,76-79`, `crates/tama/src/api/backends/install.rs:65,90,255,413`, `crates/tama-core/src/config/types/{backend.rs:14, model.rs:74}`, `proxy/lifecycle/mod.rs:188`, `components/install_modal.rs:93`; bypassed enum: `gpu/detect.rs:6-13`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The variant taxonomy lives in ≥3 places (literal array in install.rs, match arms in env.rs, the `GpuType` enum nothing upstream uses). Ad-hoc case-normalizing string compares; `CompactionToggleRequest.device` lossy-parses invalid input into silently keeping the old value instead of 422.
- **Proposal:** `GpuVariant` newtype/enum in config with `FromStr`/`Display`/serde; use in `BackendConfig`, `ModelConfig`, `resolve_gpu_env`, install request (serde validation for free). Deserialize compaction `device` as `CompactionDevice` directly.

#### 17. HuggingFace endpoint resolution + URL construction open-coded at 6 sites; `search.rs` ignores `HF_ENDPOINT`

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/models/pull/{download.rs:28,92,110, api.rs:83,116,211}`, `proxy/tama_handlers/pull/download.rs:174-179`, `models/search.rs:4`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `env::var("HF_ENDPOINT").unwrap_or_else(...)` + `format!` URL building repeated at 6 sites; `HF_API_BASE` const bypasses the env var entirely — mirror-endpoint support is inconsistent by construction. Auth headers hand-rolled twice.
- **Proposal:** One `HfEndpoints` struct (`resolve_url`, `api_model_url`, `api_blobs_url`) + `hf_auth_headers()`; point search.rs at it.

#### 18. "Valid repo_id" defined three different ways (security-relevant)

- **Lens:** Weak Abstractions
- **Files:** `crates/tama/src/api/models/crud/mod.rs:302-313`, `crates/tama-core/src/proxy/tama_handlers/types.rs:129-131`, `crates/tama/src/api/hf.rs:27-29`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Three validators with different accept sets: crud's charset whitelist allows `.` (so `../x` passes); tama_handlers blacklists `..`/`\`; hf.rs omits the backslash check. A repo id rejected by the pull path can be accepted by the HF-proxy path.
- **Proposal:** One `is_valid_repo_id`/`RepoId::parse` in `tama_core::models`; use everywhere; unit-test traversal cases once.

#### 19. Untyped response payloads (`serde_json::Value` everywhere) + hand-maintained 1,123-line OpenAPI spec

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/proxy/status.rs:94`, `proxy/tama_handlers/models/utils.rs:63`, `proxy/handlers/models.rs:21-23`, 145 `serde_json::json!` sites, `crates/tama/src/api/openapi.rs`
- **Severity:** Medium
- **Confidence:** High (evidence) / Medium (fix scope)
- **Problem:** Core wire formats have no Rust types; responses pattern-matched by string keys; the OpenAPI spec is a second hand-written description guaranteed to drift (already has — F4). No proposal (fix-scope confidence < High) — start by typing the two highest-traffic responses (`StatusResponse`, `ModelEntry`).

#### 20. `spawn_blocking` applied unevenly for blocking DB calls

- **Lens:** Inconsistent Patterns
- **Files:** `api/aliases/mod.rs` ×5, `api/updates.rs:128,550,752`, `api/backends/list.rs:65`, `api/backends/install.rs:507`, `api/models/info.rs:20,148,211`, `api/benchmarks/{run,spec,mtp}.rs` (mixed within one function)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Canonical pattern exists (`api/helpers.rs::spawn_model_crud`/`open_backend_manager`; files.rs comment "keep rusqlite::Connection off .await points") but ~10 sites run blocking SQLite on the async executor; worst: `updates.rs:550` opens `BackendManager` inside `tokio::spawn(async …)`.
- **Proposal:** Route listed sites through the helpers/`spawn_blocking`; fix updates.rs:550 first.

#### 21. Benchmark-history DELETE route sits outside CSRF middleware; CSRF middleware itself untested

- **Lens:** Inconsistent Patterns
- **Files:** `crates/tama/src/router.rs:308` (verified directly), `crates/tama/src/api/middleware.rs` (154 lines, no `#[cfg(test)]`)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `DELETE /tama/v1/benchmarks/history/:id` is mounted in the unprotected root router under a "Benchmark GET routes (no CSRF needed)" comment — every other DELETE is behind `enforce_same_origin`. The middleware (the entire CSRF defense) has no unit tests for its negative paths.
- **Proposal:** Move the route into `csrf_routes`; add table-driven middleware tests (`tower::oneshot` pattern from scope_middleware's 25 tests).

#### 22. Compaction & TTS spawn paths bypass the plan-148 lifecycle traits (and are untested)

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/proxy/lifecycle/compaction.rs:94` (`Command::new("uv")`), `lifecycle/tts.rs:90` (`Command::new(&python_bin)`), `proxy/server/mod.rs:161` (shells out to `kill`)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `ProcessSpawner` exists but is only used for the main LLM path; compaction/TTS spawn directly with zero tests — the same failure modes (spawn failure, port allocation, health timeout) the traits were built to test.
- **Proposal:** Route both spawn sites through `ProcessSpawner`/`PortAllocator`; add tests mirroring `test_load_model_pipeline_with_mock_health_checker`.

#### 23. Update-check orchestration (`check_model`/`check_backend`/`run_check`) untested

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/updates/checker/{model.rs (402), backend.rs (142), mod.rs:101}`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The decision table and cache are well-tested (~30 tests), but the fetch→compare→persist pipeline that uses them has no test — wiring bugs (wrong repo, wrong quant filter, results not persisted) wouldn't be caught.
- **Proposal:** wiremock HF metadata + in-memory DB; drive `run_check`; assert persisted rows + emitted events.

#### 24. Bench execution + benchmark API routes + download engines untested; `tama-mock` wired into zero tests

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/bench/{llama_bench/mod.rs, llama_cli_spec/server.rs}`, `crates/tama/src/api/benchmarks/*` (~1,450 lines, zero route tests), `crates/tama-core/src/models/pull/{parallel.rs (295), single.rs (160)}` (range/resume/retry untested), `crates/tama-mock/`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Process-spawning bench orchestration and the chunked/resumable download engines (corruption/backoff failure modes) lack tests; the purpose-built mock backend is referenced by no test and can bit-rot.
- **Proposal:** Env seams exist (`LLAMA_SERVER_PATH`, `LLAMA_BENCH_PATH`) — point at stubs; wiremock ranged responses for pull_parallel; one integration test spawning `tama-mock` (`CARGO_BIN_EXE_tama-mock`) smoke-tests it and provides the crash/hang seam for F9.

#### 25. `handle_logout` defined but never routed — no way to log out with OAuth2

- **Lens:** Dead Code
- **Files:** `crates/tama-core/src/proxy/auth.rs:667`, routers register only `/login`, `/login/callback`, `/login/error` (verified directly)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Doc says "Handle GET /logout" but no `/logout` route exists anywhere.
- **Proposal:** Route it (`/logout`) — likely a missing route rather than surplus code.

#### 26. Dead code batch: `config/migrate` (171 lines), dead Leptos components (~870), orphan `jobs.rs` (606), unused deps, blanket `allow(dead_code)`, `rename_legacy` overdue

- **Lens:** Dead Code
- **Files:** `crates/tama-core/src/config/migrate/`; `crates/tama/src/components/{sampling_templates_section.rs, supervisor_section.rs, general_section.rs, sparkline.rs, backup_section.rs}`; `crates/tama/src/jobs.rs` (verified: no `mod jobs` in lib.rs); `Cargo.toml` (`inquire`, `clap`, `utoipa`, `utoipa_gen`, `axum_server`, `http_body_util` — zero references); `crates/tama/src/lib.rs:1-2` (`#![allow(dead_code)]` + `#![allow(deprecated)]`); `crates/tama-core/src/config/rename_legacy.rs` (TODO(v1.60) — workspace is at 2.0.0, deadline passed)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** ~1,900 lines of dead code + 6 unused deps; the crate-level blanket allows are exactly how this accumulates invisibly; stale item-level allows mask used items.
- **Proposal:** Delete all of the above; remove both blanket allows; run `cargo machete` in CI; execute the overdue rename_legacy removal.

#### 27. Naming: domain-term leftovers — `GpuType`/`GpuTypeDto`/bench `gpu_type`; `server`-means-backend in lifecycle/logs/config-resolve; `download` on the pull API surface

- **Lens:** Naming
- **Files:** `crates/tama-core/src/gpu/detect.rs:6` (`GpuType` + `variant_folder()`), `crates/tama/src/components/backend_card.rs:22` (`GpuTypeDto`), `bench/mod.rs:149`, `bench/llama_bench/discovery.rs:64`, `db/backfill/mod.rs:31`; `proxy/lifecycle/mod.rs:76,452` (`let servers = config.resolve_backends_for_model(...)`), log strings :92,322,433,603,610, `handlers/forward.rs:80,90,120,124`, `handlers/tts.rs:58,79`, `config/resolve/mod.rs:75-88` (loop var `server` bound to ModelConfig); `models/pull/download.rs` module, `tama_handlers/pull/download.rs`, `QuantDownloadSpec`, `download_queue_poll_interval_secs` (config/API/UI), `/tama/v1/downloads/*` routes + `Downloads*Response` DTOs (while handlers in the same file are `*_pulls`), `model_files.downloaded_at` column
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The plan-144/150 renames stopped at the DB layer; the central enum, benchmark reports, lifecycle logs/vars, and the public API/config surface still use forbidden terms. CONTEXT.md is explicit (gpu_variant, backend, pull).
- **Proposal:** Rename `GpuType`→`GpuVariant` (+Dto, bench fields); scrub `servers`/`server` vars and log strings; rename the two `download.rs` modules and `QuantDownloadSpec`→`QuantPullSpec`; decide on public-surface renames (`/tama/v1/pulls/*`, `pull_queue_poll_interval_secs`, `pulled_at`) with a compat note — these are breaking.

#### 28. Naming: `ModelCard`, `ModelStatus` (misplaced in gpu module), `Loading` vs `Starting` divergence, `Supervisor` config, `ProcessSupervisor` (dead)

- **Lens:** Naming
- **Files:** `crates/tama-core/src/models/card.rs:10` + `lib.rs:3` crate doc; `gpu/types.rs:184` (`ModelStatus` wraps `state: ModelState`); `gpu/types.rs:38` (`ModelState::Loading`) vs `proxy/types.rs` (`BackendState::Starting`) vs CONTEXT.md (canon: Starting); `crates/tama/src/types/config/supervisor.rs:9` (`[supervisor]` = backend-lifecycle knobs); `crates/tama-core/src/process.rs:88` (`ProcessSupervisor` — zero constructors outside its file)
- **Severity:** Medium
- **Confidence:** High on violations
- **Problem:** Five domain-term contradictions, two of them user-facing (config schema, serialized state enum) and one ("Loading"/"Starting") an actual vocabulary divergence between two parallel state enums for the same machine.
- **Proposal:** `ModelCard`→`ModelToml`; `ModelStatus`→`ModelStateSnapshot` + move out of gpu; align `Loading`→`Starting` (API-visible — version it); `[supervisor]`→`[lifecycle]` (breaking, batch with F27's config renames); delete `ProcessSupervisor` (dead).

#### 29. Config mirror types in 3 copies (acknowledged hand-sync) + quant inference duplicated across csr/ssr

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/types/config/` (1,321 lines), `crates/tama/src/pages/config_editor/types.rs` (326 lines, in-code NOTE admits hand-sync), `crates/tama/src/gpu_types.rs:49-165` (enum mirrors incl. from_str drift: core returns Option, mirror returns default), `components/pull_wizard/mod.rs:126-200` (csr re-implements the ~100-line quant-pattern table)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The config struct tree (67 fields) exists in 3 places kept in sync by hand; adding a quant kind to core silently desynchronizes the WASM copy.
- **Proposal:** Collapse `config_editor/types.rs` onto `crate::types::config`; move pure dependency-free types/functions (quant inference, `QuantKind`, enums) into a wasm32-compilable `tama_core::types` leaf module both sides use (ADR-0010-safe).

#### 30. DB row-mapping closures + SELECT column lists repeated per query file

- **Lens:** DRY Violations
- **Files:** `crates/tama-core/src/db/queries/model_config_queries.rs:110-152,160-205,216-280` (35-field mapper ×3 + column list ×3), `alias_queries.rs` ×3, `tts_config_queries.rs` ×2, `update_check_queries.rs` ×2, `model_queries.rs` ×2, `metrics_queries.rs` ×2
- **Severity:** Medium
- **Confidence:** High
- **Problem:** ~200-250 lines of repeated `row.get(N)?` mapping and column lists.
- **Proposal:** Per-record `fn from_row(&Row)` + `const COLUMNS: &str` in `db/queries/types.rs`.

#### 31. Leptos UI: form/helper duplication (~350 lines) + divergent data-fetching/form patterns

- **Lens:** DRY Violations
- **Files:** `pages/model_editor/{settings,files,sampling,hardware,advanced}_form.rs` (`set_input_value`/`set_checked` verbatim ×5), `pages/benchmarks/{mod,spec_bench,mtp_bench}.rs` (fetch-models/fetch-backends/`parse_sizes`/auto-select ×3, ~200 lines), fetching: `LocalResource` (2 pages) vs manual `spawn_local` + hand-rolled loading/error (18 files); forms: `Action` vs `on:click` vs `on:submit`; `pages/keys/api.rs:43` hand-rolls PATCH (no `patch_request` helper), `logs.rs:72`/`self_update_section.rs:38` raw gloo requests
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Duplicated DOM helpers and fetch boilerplate; three fetching and three form patterns coexist; the shared request helpers have gaps (PATCH).
- **Proposal:** Move DOM helpers to `utils`; extract `use_benchmark_form_state()`; add `patch_request`; standardize mutations on one pattern.

#### 32. `ProxyState` accessor leaks: 17 public accessors hand out `Arc<RwLock<…>>`/`Sender`; service-locator methods

- **Lens:** Coupling
- **Files:** `crates/tama-core/src/proxy/types.rs:257-299,346-453`, `proxy/state.rs` (482 lines), 20+ `crates/tama/src/api/**` handlers on `State<Arc<ProxyState>>`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Fields are `pub(crate)` but every one has a pub accessor returning the lock itself — encapsulation is nominal; `model_mgr()` and `open_db()` make it a service locator; the whole management API reaches through one type.
- **Proposal:** Group into focused sub-structs (`RegistryState`, `MetricsState`, `PullState`) with domain methods; keep ProxyState as thin composition (extend the state.rs direction).

#### 33. `/tama/v1` route tables split across two routers in two crates, kept in sync by comments; wrong-direction edges (`bench→proxy`, `config→proxy` utilities)

- **Lens:** Coupling
- **Files:** `crates/tama-core/src/proxy/server/router.rs` (`build_router` + `build_unified_router` — same paths in both, overlap dropped by convention), `crates/tama/src/router.rs:110-319`; `bench/runner.rs:18` (imports `proxy::process`), process helpers split between `crate::process` and `crate::proxy::process`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Three route tables must be kept consistent by comment ("Routes that overlap … are intentionally excluded") — a shadow-route bug waiting to happen. Leaf modules reach upward into proxy for generic process utilities.
- **Proposal:** Core router exposes only proxy-exclusive routes (inference + lifecycle); management CRUD lives solely in tama's table. Move generic process helpers (`check_health`, `is_process_alive`, `kill_process`, `override_arg`) into `crate::process`.

#### 34. `println!`/`eprintln!` remain in production paths after plan-158 (tracing)

- **Lens:** Inconsistent Patterns
- **Files:** `bench/runner.rs:262,286,318` (mixes with `tracing::` in same file), `bench/display.rs`, `db/backfill/initial_backfill.rs` (6 println + 10 tracing), `models/pull/{single.rs ×4, parallel.rs ×3}` (progress bars in library code), `backends/installer/{mod,prebuilt,source/build}.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** 31 non-test hits; library stdout writes bypass the JSON log file and `log_level` config.
- **Proposal:** Replace with `tracing::info!/warn!`; route progress-bar output through the existing callback or indicatif-only.

#### 35. Docs claim SvelteKit; Leptos is the live UI

- **Lens:** Inconsistent Patterns
- **Files:** `CONTEXT.md` ("a SvelteKit web control plane"), `docs/decisions/0003-use-sveltekit-over-leptos-wasm.md` (claims migration "removed Leptos, Trunk, and all WASM build infrastructure"), `crates/tama/dist/index.html` (loads `tama-*_bg.wasm`), `crates/tama/src/lib.rs:300-312` (11 Leptos pages routed), `crates/tama/ui/` (empty `node_modules` only), AGENTS.md (`make dev` = Leptos)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The SvelteKit migration was decided but never implemented; the docs actively misdescribe the live system (this audit had to reconcile it). Every future audit/onboarding hits the same confusion.
- **Proposal:** Amend CONTEXT.md + decision 0003 (status: proposed, not implemented) or delete the SvelteKit references; remove empty `crates/tama/ui/`.

### 🟢 Low Severity

#### 36. Remaining route/module test gaps + 5 permanently ignored tests

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama/src/api/{aliases/mod.rs, models/files.rs, backends/install.rs, backends/list.rs, backends/jobs.rs, hf.rs, logs.rs}` (no route tests), `crates/tama-core/src/proxy/tama_handlers/system.rs`, `tama_handlers/models/handlers.rs` (load/unload/list/get), `crates/tama/tests/backends_api.rs` (all 5 tests `#[ignore]`); small untested utils: `proxy/rename.rs` (95, data-loss class), `backends/installer/{source/install.rs, extract.rs, prebuilt.rs}`, `config/loader.rs`, `utils/sse_stream.rs`
- **Severity:** Low
- **Confidence:** High
- **Proposal:** Reuse the `manage/tests.rs::test_web_state` registry fixture to un-ignore the 5 backend tests; extend `models/tests/helpers.rs` harness for load/unload/list/get; CRUD round-trip tests for aliases per `crud/tests.rs` pattern.

#### 37. Small-scale duplication batch

- **Lens:** DRY Violations
- **Files:** `models/pull/{single.rs:14-24, parallel.rs:17-27}` (jitter/backoff ×2); mean/stddev ×5 (`bench/mod.rs:223-258` ×4 inline + `llama_cli_spec/mod.rs:286`); `default_*` serde fns duplicated across api/benchmarks + bench core + pages; `pages/model_editor/api.rs:65-118` (7 identical parse-insert blocks); path-traversal guard ×12 across 7 backend api files; `validate_alias_name` duplicated client/server with divergent error strings; job-conflict + safe-remove blocks duplicated `manage/remove.rs:130-178` ≈ `install.rs:589-640`; `model_editor/api.rs` fetch tail ×5
- **Severity:** Low
- **Confidence:** High
- **Proposal:** Hoist each to its module root or a shared helper (`reject_traversal`, `run_db`, shared backoff, shared mean/stddev).

#### 38. Dead code small batch

- **Lens:** Dead Code
- **Files:** `proxy/state.rs` (`is_model_loaded`, `get_backend_pid`, `get_circuit_breaker_failures`, `get_model_state_with_access`), `proxy/types.rs:415` (`set_pull_queue`); 8 dead DB fns (`delete_model_records`, `get_all_running_items`, `get_running_item`, `mark_stale_running_as_queued`, `get_all_tts_configs`, `Repository::conn`, `Repository::get_pull_queue_item`); `forward/json.rs:16` (`build_forward_uri` — only its tests call it); dead types `ModelAliasRecord`, `RestartResponse`; `installer/download.rs` (`download_file`) + triplicated `emit` helper; unused lifecycle trait scaffolding (`SpawnedProcess`, `ProcessSpawner`, `PortAllocator`, `MockProcessSpawner`, `MockPortAllocator` — "reserved for future tests", zero usage); ~15 test-only production fns; `api/api.rs:37` `get_logs` (unrouted), `api/logs.rs` `get_all_logs` (unrouted)
- **Severity:** Low
- **Confidence:** High
- **Proposal:** Delete each (or gate behind `#[cfg(test)]`); lands naturally with F26.

#### 39. Style drift batch

- **Lens:** Inconsistent Patterns
- **Files:** `api/backends/types.rs:14` (`CompactionCardDto` camelCase inside snake_case payload; mirror `pages/backends.rs:16`; also `request_timeout_ms` type drift u64 vs Option<u64>); `fn get_` prefix ×102 vs bare accessors; `_`-prefix-for-private convention (5 uses vs ~1,320 without — AGENTS.md rule effectively dead); `test_` prefix 87 violations (css_test.rs, dashboard/tests.rs, utils/mod.rs); test module layout split (127 inline vs 23 files with `tests.rs`/`*_tests.rs` mix); SSE keep_alive inconsistencies (folded into F12)
- **Severity:** Low
- **Confidence:** High
- **Proposal:** Flip `CompactionCardDto` to snake_case; amend AGENTS.md (drop `_`-prefix rule, code reality wins); bulk-rename the 87 tests.

#### 40. Naming small batch

- **Lens:** Naming
- **Files:** `fetch_*` metadata fns in `models/pull/` (CONTEXT.md forbids "fetch" for pull); "quantisation" spellings (card.rs, tama_handlers/types.rs); `crates/tama/src/gpu_types.rs` module name (mostly non-GPU mirrors → `core_mirrors`); composite `"name:variant"` colon-keys leaking into SQL LIKE patterns (`update_check_queries.rs:105-114`); `Option<Option<T>>` tri-states + 5-positional-arg `update_alias`; `system: bool` across 9 platform fns (moot if F7 lands); `check_single` vague name; gratuitous `handle_tama_get_model_fn` alias
- **Severity:** Low
- **Confidence:** High on each violation
- **Proposal:** Rename along with F27/F28 batches; `FieldUpdate<T>` enum for PATCH tri-states; params struct for `update_alias`.

## Top Recommendation

**Start with the DB layer workstream (F1 + F2 + F3).** They are one interlocked problem — two competing access layers, a porous funnel, and the codebase's only module-level cycle — and they gate the value of everything else (every new handler writes into the mess). Highest-leverage sequence: (a) F3's one-function move breaks the cycle in an hour; (b) decide Repository vs managers (amend ADR-0017 or fold) — this is a decision, not just work; (c) enforce the funnel (`pub(crate)`, shared Repository in state, drop raw SQL).

**But first, two cheap correctness fixes this week:** F6 (restore endpoint returning false success — dishonest API) and F21 (CSRF route placement + F25 logout route) — small diffs, real user-facing risk.
