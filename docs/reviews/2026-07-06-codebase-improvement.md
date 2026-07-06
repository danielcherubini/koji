# Codebase Improvement Report — 2026-07-06

## Summary

28 findings across 8 categories. 14 high, 10 medium, 4 low.

The codebase is well-structured overall with strong test coverage in config resolution and backend management. The most impactful opportunities are: (1) splitting 6 god files over 1000 lines, (2) eliminating 200+ occurrences of duplicated error-response boilerplate, (3) closing the DB layer leak from API handlers, and (4) aligning naming with CONTEXT.md domain terms.

## Context

- **CONTEXT.md:** loaded (domain glossary + engineering rules)
- **ADRs reviewed:** none (no `docs/adr/` directory)
- **Plans reviewed:** 8 (model-editor-redesign, dashboard-bar-charts, model-sort-group, remove-cli-promote-web, mtp-draft-model-fixes, model-editor-gpu-isolation-polish, gpu-env-var-isolation, mtp-draft-model-fixes)

---

## Findings

### 🔴 High Severity

#### 1. `config/types.rs` is a god file (1,407 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama-core/src/config/types.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** Contains ALL config struct definitions (Config, General, ProxyConfig, ModelConfig, BackendConfig, Supervisor, CompactionConfig, SpecDecodingConfig, ModelModalities, QuantKind, QuantEntry, HealthCheck), DB serialization (Config::from_db 150+ lines, Config::to_db 150+ lines), 20+ default functions, and ~250 lines of tests. Five unrelated responsibilities in one file.
- **Proposal:** Split into `config/types/general.rs`, `proxy.rs`, `model.rs`, `backend.rs`, `supervisor.rs`, `compaction.rs`, with a thin `mod.rs` re-exporting everything.

#### 2. `proxy/tama_handlers/models.rs` mixes handlers, utils, and tests (1,303 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama-core/src/proxy/tama_handlers/models.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** Three distinct concerns: 5 CRUD handlers, 4 utility functions (resolve_model_id, build_model_entry, generate_display_name), 3 capability detection functions, and ~260 lines of tests.
- **Proposal:** Split into `models/handlers.rs`, `models/opencode.rs`, `models/utils.rs`, `models/tests.rs`.

#### 3. `updates/checker.rs` is monolithic (1,079 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama-core/src/updates/checker.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** Contains GgufListingCache, UpdateChecker with 5 methods, check_model (~200 lines with 3 nested phases), check_backend (~120 lines), and standalone helpers. Single file does caching, backend checking, model checking, and status determination.
- **Proposal:** Split into `checker/cache.rs`, `checker/backend.rs`, `checker/model.rs`, `checker/mod.rs`, `checker/helpers.rs`.

#### 4. `proxy/forward.rs` mixes headers, SSE, stats, and forwarding (1,119 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama-core/src/proxy/forward.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** Header filtering, JSON rewriting, SSE processing, inference stats extraction, and the main forward_request (~350 lines with circuit breaker, dead process detection, streaming branches) all in one file.
- **Proposal:** Split into `forward/headers.rs`, `forward/json.rs`, `forward/sse.rs`, `forward/stats.rs`, `forward/request.rs`.

#### 5. `gpu/system.rs` mixes types, NVIDIA, AMD, and metrics (1,121 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama-core/src/gpu/system.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** Type definitions (~200 lines), NVIDIA detection (~80 lines), AMD detection (~350 lines of verbose sysfs reads), and metrics collection (~80 lines). Four unrelated responsibilities.
- **Proposal:** Split into `gpu/types.rs`, `gpu/nvidia.rs`, `gpu/amd.rs`, `gpu/system.rs` (metrics + public API).

#### 6. Benchmark job submission copy-pasted across 3 files (~100 lines duplicated)

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/api/benchmarks/run.rs`, `spec.rs`, `mtp.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** The exact same ~60 lines of job submission boilerplate (jobs.submit, tokio::spawn, jobs.finish) and ~45 lines of ProgressSink boilerplate are copy-pasted identically across 3 files.
- **Proposal:** Extract `submit_benchmark_job()` helper in `benchmarks/mod.rs` that takes a closure for the inner function. Create a generic `ProgressSink<Name>` struct.

#### 7. Error response pattern repeated 200+ times across API

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/api/` (updates.rs, aliases/mod.rs, backends/manage.rs, backends/install.rs, models/files.rs, and more)
- **Severity:** High
- **Confidence:** High
- **Problem:** `(Json(serde_json::json!({"error": e.to_string()}))).into_response()` repeated hundreds of times with minor variations.
- **Proposal:** Create `fn error_response(status: StatusCode, msg: impl Into<String>) -> impl IntoResponse` in `api/mod.rs`.

#### 8. DB layer leak — API handlers query SQLite directly (35 direct calls)

- **Lens:** Coupling Issues
- **Files:** `crates/tama/src/api/` (updates.rs, models/info.rs, models/files.rs, benchmarks/*.rs, backends/*.rs, aliases/mod.rs)
- **Severity:** High
- **Confidence:** High
- **Problem:** API handlers import `tama_core::db::queries::*` directly, open DB connections themselves, and use DB record types (ModelFileRecord, ModelConfigRecord, DownloadQueueItem) in function signatures. Zero abstraction between HTTP and SQLite.
- **Proposal:** Create a `Repository` trait or `AppContext` struct in `tama-core` that exposes domain-level operations. API handlers call repository methods instead of raw queries.

#### 9. `ProxyState` has 25+ public fields exposing internal state

- **Lens:** Coupling Issues
- **Files:** `crates/tama-core/src/proxy/types.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** All fields are `pub`, exposing RwLock, Semaphore, watch::Sender, and internal HashMaps. External code can directly mutate internal state. Web UI fields (web_jobs, web_update_checker) are public on the core type.
- **Proposal:** Make fields `pub(crate)`, provide public API through `impl ProxyState { pub fn ... }`. Extract web UI fields into a separate `WebState` struct.

#### 10. GPU Vendor & Model State stored as `String` instead of enums

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/gpu/system.rs`, `crates/tama/src/pages/dashboard/metrics.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** `vendor: String` ("nvidia"/"amd") and `state: String` ("idle"/"loading"/"ready"/"unloading"/"failed") allow arbitrary values. Code compares `state == "failed"` as string literals.
- **Proposal:** `enum GpuVendor { Nvidia, Amd }` and `enum ModelState { Idle, Loading, Ready, Unloading, Failed }` with serde support.

#### 11. DB queries return raw tuples instead of typed records

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/db/queries/app_config_queries.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** `get_supervisor()` returns `Option<(String, u32, u64, u64, u64, u32)>` (6-tuple), `get_proxy()` returns a 12-tuple. Config types destructure as `proxy_row.0`, `proxy_row.1`, etc.
- **Proposal:** Define typed structs (`SupervisorRecord`, `ProxyRecord`) with named fields. Use `#[derive(Debug)]` and implement `From<Row>` patterns.

#### 12. `gpu_type` used where domain term is `gpu_variant` (15+ locations)

- **Lens:** Naming
- **Files:** `crates/tama-core/src/backends/types.rs`, `db/backfill/mod.rs`, `backends/installer/mod.rs`, DB migrations
- **Severity:** High
- **Confidence:** High
- **Problem:** CONTEXT.md defines **gpu_variant** as the domain term and forbids "GPU type". Yet `gpu_type` is a struct field, DB column, and appears in 10+ files. Additionally, `BackendInfo` has BOTH `gpu_type: Option<GpuType>` and `gpu_variant: String` — structural confusion.
- **Proposal:** Rename all `gpu_type` to `gpu_variant`. Clarify or remove the overlapping field in `BackendInfo`. DB migration needed.

#### 13. `server` used where domain term is `backend` (20+ locations in proxy/)

- **Lens:** Naming
- **Files:** `crates/tama-core/src/proxy/state.rs`, `proxy/status.rs`, `proxy/types.rs`, `config/resolve/mod.rs`
- **Severity:** High
- **Confidence:** High
- **Problem:** CONTEXT.md defines **Backend** = inference engine binary and forbids "server". Yet `server_name` parameter, `resolve_server()`, `get_available_server_for_model()`, and `server_ready` all use forbidden term. Comments say "per-server inference stats" and "metrics for the proxy server".
- **Proposal:** Rename `server_name` → `backend_name`, `resolve_server` → `resolve_backend`, `get_available_server_for_model` → `get_available_backend_for_model`, `server_ready` → `backend_ready`. Update all comments.

#### 14. Backend lifecycle code has no test coverage

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/proxy/lifecycle/compaction.rs`, `idle_timeout.rs`, `tts.rs`, `mod.rs` (load_model, unload_model)
- **Severity:** High
- **Confidence:** High
- **Problem:** ~900 lines of critical process management code (spawn, health poll, idle timeout, dead PID detection, auto-restart, graceful shutdown) has zero test coverage. No trait abstractions for subprocess execution, port allocation, or health checking — making it impossible to test without real processes.
- **Proposal:** Add trait abstractions (`HealthChecker`, `ProcessSpawner`, `PortAllocator`) with default impls calling real functions and test impls returning mock data. Test the 3-phase idle timeout logic and load_model pipeline.

---

### 🟡 Medium Severity

#### 15. `tama/src/types/config.rs` duplicates `tama-core/src/config/types.rs` (986 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama/src/types/config.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Near-duplicate of core config types for WASM compatibility (BTreeMap vs HashMap). Intentional design choice but creates maintenance burden — every change to core types must be mirrored.
- **Proposal:** Keep duplication (valid WASM design) but split into modules matching the core types split. Consider `#[derive(serde::Serialize, serde::Deserialize)]` with `#[serde(with = "hash_map_as_btree_map")]` adapters to share types.

#### 16. `api/backends/manage.rs` packs 5 handlers + types + tests (1,013 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama/src/api/backends/manage.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Five handlers (update_backend, remove_backend_version, activate_backend_version, update_backend_default_args, update_backend_source) plus request types and ~150 lines of tests. All follow the same pattern.
- **Proposal:** Split into `manage/update.rs`, `manage/remove.rs`, `manage/activate.rs`, `manage/config.rs`, `manage/types.rs`, `manage/tests.rs`.

#### 17. `pages/config_editor.rs` packs mirror types + 5 forms + page (937 lines)

- **Lens:** File Length + Structure
- **Files:** `crates/tama/src/pages/config_editor.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Mirror types (~200 lines), main ConfigEditor component (~80 lines), and 5 form components (~600 lines) all in one file.
- **Proposal:** Split into `config_editor/types.rs`, `config_editor/forms/general.rs`, `forms/proxy.rs`, `forms/supervisor.rs`, `forms/sampling.rs`, `forms/compaction.rs`, `config_editor/mod.rs`.

#### 18. `BackendManager::open` boilerplate repeated 16 times

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/api/backends/manage.rs` (6), `list.rs` (4), `install.rs` (3), `updates.rs` (3)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Identical pattern of `config_dir.clone()`, `tokio::task::spawn_blocking(move || BackendManager::open(...))`, and error mapping repeated across 4 files.
- **Proposal:** Extract `async fn open_backend_manager(state: &ProxyState) -> Result<BackendManager, (StatusCode, Json<Value>)>` helper.

#### 19. Model CRUD spawn_blocking boilerplate repeated across 4 files (~120 lines)

- **Lens:** DRY Violations
- **Files:** `crates/tama/src/api/models/crud/create.rs`, `update.rs`, `rename.rs`, `delete.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Identical `spawn_blocking` → `match Ok(Ok(val))` → `trigger_proxy_reload` → `Json(val).into_response()` pattern in every CRUD handler.
- **Proposal:** Extract `async fn spawn_and_json<F, T>(f: F) -> impl IntoResponse` generic wrapper.

#### 20. `web_types` module in `tama-core` couples core to web UI

- **Lens:** Coupling Issues
- **Files:** `crates/tama-core/src/web_types.rs`, `crates/tama-core/src/proxy/types.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `tama-core` has a `web_types` module (JobManager, CapabilitiesCache, UploadEntry) gated behind `#[cfg(feature = "web-ui")]`. ProxyState has 8 web UI fields. Core library shouldn't know about web concepts.
- **Proposal:** Move `web_types` to the `tama` crate. Extract web fields from `ProxyState` into a `WebState` struct.

#### 21. `download` used where domain term is `pull` (15+ locations)

- **Lens:** Naming
- **Files:** `crates/tama-core/src/models/download/`, `models/manager.rs`, `models/pull/download.rs`, DB migrations
- **Severity:** Medium
- **Confidence:** High
- **Problem:** CONTEXT.md defines **Pull** = "downloading a model" and forbids "download". Yet `models/download/` directory, `download_single()`, `DownloadResult`, `DownloadQueueItem`, `log_download()`, `bytes_downloaded`, and DB table `download_queue` all use forbidden term.
- **Proposal:** Rename `download` → `pull` in model subsystem (directory names, function names, types, DB tables). DB migration needed. Note: `download` in TTS/network context is acceptable.

#### 22. Restart Policy, Log Level, Compaction Device as `String` instead of enums

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/config/types.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `Supervisor.restart_policy` (magic values "always"/"on-failure"), `General.log_level`, and `CompactionConfig.device` ("cpu"/"cuda"/"cuda:0"/"mps") stored as raw strings.
- **Proposal:** `enum RestartPolicy { Always, OnFailure }`, `enum LogLevel { Debug, Info, Warn, Error }` (matching tracing::Level), `enum CompactionDevice { Cpu, Cuda(Option<u32>), Mps }`.

#### 23. Error response format inconsistent (flat vs structured)

- **Lens:** Inconsistent Patterns
- **Files:** `crates/tama-core/src/proxy/tama_handlers/system.rs` (flat), `models.rs` (structured), `pull/handlers.rs` (structured)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `system.rs` uses flat `"error": "string"` while `models.rs` and `pull/handlers.rs` use structured `{"error": {"message": "...", "type": "..."}}`. Error type names are not centralized.
- **Proposal:** Standardize on structured format. Define `enum AppErrorType { NotFound, ValidationError, LoadModelError, ... }` and a shared `error_response(status, message, error_type)` helper.

#### 24. `unwrap()` in production code (4 occurrences)

- **Lens:** Inconsistent Patterns
- **Files:** `crates/tama-core/src/proxy/tama_handlers/system.rs:108`, `backend_logs.rs:155,165`, `web_types.rs:129`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** AGENTS.md says "Return `Result<T, E>` instead of `unwrap()` or `expect()` in public APIs". SSE event construction and Response building use `unwrap()` — can panic on serialization failure.
- **Proposal:** Replace with `expect("descriptive message")` at minimum, or propagate errors through SSE stream using `Result<Event, axum::Error>`.

#### 25. Update checking and download queue processor untested

- **Lens:** Missing Tests / Testability
- **Files:** `crates/tama-core/src/updates/checker.rs`, `crates/tama-core/src/proxy/download_queue.rs`
- **Severity:** Medium
- **Confidence:** Medium
- **Problem:** `check_backend()`, `check_model()`, `run_check()`, and `GgufListingCache` are untested (async, network-dependent). `queue_processor_loop()` (~150 lines of CAS-based claiming, dead task detection, stale recovery) is completely untested.
- **Proposal:** Use `wiremock` for HF API calls in update checker tests. Test queue processor with in-memory DB + mocked DownloadQueueService.

#### 26. Config test fixtures duplicated across 9 test files (~2,700 lines)

- **Lens:** DRY Violations
- **Files:** `crates/tama-core/src/config/resolve/tests/` (basic.rs, gpu_device.rs, context_np.rs, kv_cache_types.rs, unified_slots.rs, path_resolution.rs, aliases.rs, spec_decoding/*.rs)
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Every test file repeats ~40 lines of fixture setup (temp dir, model files, BTreeMap, Config, ModelConfig with 20+ fields, BackendConfig).
- **Proposal:** Create `test_helpers` module with `temp_model_dir()`, `sample_config()`, `sample_server(overrides)`, `sample_backend()`.

#### 27. `network.rs` module entirely dead (220 lines)

- **Lens:** Dead Code
- **Files:** `crates/tama-core/src/network.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** Fully-featured module with `NetworkStats`, `get_primary_interface()`, `collect_network_stats()` — all 4 public items never imported by any other file. Module declared in `lib.rs` but zero consumers.
- **Proposal:** Remove entirely. If network stats are needed for dashboard, wire it up or remove.

#### 28. `logging.rs` functions never called (3 public functions dead)

- **Lens:** Dead Code
- **Files:** `crates/tama-core/src/logging.rs`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** `init()`, `init_with_file()`, and `log_path()` are `pub` but never called anywhere in the codebase.
- **Proposal:** Either wire up logging initialization in `main.rs` or remove the module.

---

### 🟢 Low Severity

#### 29. Handler return types inconsistent (`Response` vs `Json<T>` vs `Json<Value>`)

- **Lens:** Inconsistent Patterns
- **Files:** `crates/tama-core/src/proxy/tama_handlers/` (system.rs, models.rs, status.rs)
- **Severity:** Low
- **Confidence:** High
- **Problem:** Some handlers return typed `Json<T>`, most return generic `Response`, and `handle_tama_list_models` returns `Json<serde_json::Value>` (untyped).
- **Proposal:** Define typed response structs for all endpoints. Replace `Json<serde_json::Value>` with proper structs.

#### 30. Deprecated struct fields from 1.45.0 migration still present

- **Lens:** Dead Code
- **Files:** `crates/tama-core/src/gpu/system.rs:233`, `crates/tama/src/pages/dashboard/metrics.rs:121`
- **Severity:** Low
- **Confidence:** High
- **Problem:** `#[deprecated(since = "1.45.0", note = "use state field instead")]` fields still in the codebase.
- **Proposal:** Remove deprecated fields after verifying no external consumers.

#### 31. `rename_legacy` module marked for removal

- **Lens:** Dead Code
- **Files:** `crates/tama-core/src/config/rename_legacy.rs`, `config/loader.rs`
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** Comments indicate the module should be removed once all users have migrated. No timeline specified.
- **Proposal:** Set a deprecation deadline (e.g., 2 more versions) and remove after.

#### 32. `ModelConfig` has 30+ fields spanning 5+ domains (god object)

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/config/types.rs`
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** ModelConfig contains backend config, GPU config, sampling params, spec decoding, vision config, health check, quants, and more. Any change to one domain risks breaking others.
- **Proposal:** Consider composing ModelConfig from smaller structs (BackendConfig, GpuConfig, SamplingConfig, SpecDecodingConfig) rather than flat 30+ fields. Low priority given recent plans touching this file.

---

## Decisions

All 28 findings **approved** for the implementation backlog. Zero dismissed, zero deferred.

## Top Recommendation

**Tackle Finding #7 (error response helper) first** — it's the quickest win: one helper function eliminates 200+ occurrences of duplicated boilerplate across the entire API layer. Takes ~30 minutes and immediately improves consistency (also addresses Finding #23).

Second: **Finding #8 (DB layer leak)** — creating a repository/service layer is the highest-impact architectural improvement. It decouples HTTP from SQLite, enables testing, and removes DB record types from API signatures.
