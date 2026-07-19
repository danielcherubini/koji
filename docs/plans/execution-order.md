# Execution Order — Post-Audit Backlog (2026-07-18)

Maps the 40 findings from [the codebase audit](../reviews/2026-07-18-codebase-improvement.md) to their numbered backlog plans, with dependency-first phased execution order. After plan-quick-fixes landed (`7412709b`), this is where we pick up next.

## Finding → Plan Mapping

### ✅ Already Fixed — Phase 0 (plan-quick-fixes `7412709b`)
| # | Severity | Summary | Status |
|---|----------|---------|--------|
| F3   | 🔴 High    | config↔proxy cycle (`count_active_keys` move to `db::queries`)        | ✅ Fixed     |
| F6\*  | 🔴 High    | backup restore false-success → **501 stopgap** (full impl = plan-163)  | ⚠️ Stopgap only; full impl in Phase 2.3 |
| F21   | 🟡 Medium | benchmark-history DELETE CSRF gap + middleware unit tests              | ✅ Fixed     |
| F25   | 🟡 Medium | `handle_logout` unrouted                                              | ✅ Fixed     |
| F35   | 🟡 Medium | sveltekit doc drift → leptos correction                               | ✅ Fixed     |

### 🔴 High Severity — Remaining
| # | Summary | Plan | Effort | Phase |
|---|---------|------|--------|-------|
| **F1** | Two competing DB access layers (`Repository` vs `BackendManager`/`ModelManager`); method overlap; handlers open both per request (two SQLite connections); DTO/Record duplication (~43 fields duplicated) | [plan-160](plan-160-db-access-consolidation.md) | 🟠 Large | 1.1 — gates everything downstream |
| **F2** | Raw rusqlite leaks across ~40 files; raw SQL in a handler (`delete.rs`); `Repository::open()` per-request re-running migrations | [plan-160](plan-160-db-access-consolidation.md) | 🟠 Large | 1.1 — tied to F1 |
| **F4** | Error contract drift: three wire shapes across management API; OpenAPI spec contradicts helper and docs (~54 flat sites in ~12 files, worst offender `updates.rs` ×23 never importing the helper) | [plan-161](plan-161-error-contract.md) | ⚪ Medium | 2.1 — must follow F1/F2 since many error sites live in handlers being rewritten by DB consolidation |
| **F5** | Stringly-typed model identity: derivation rule open-coded at ~14+ sites applied inconsistently (pull paths skip lowercase → live bug where mixed-case repo ids silently miss lookups); two same-named `resolve_model_id` with different semantics | [plan-162](plan-162-model-identity-configkey.md) | ⚪ Medium | 3.1 — ConfigKey newtype is additive; can run after DB consolidation settles identity resolution |
| **F8** | 🔴 High OAuth two login flow handlers have zero tests (~260 LOC security-critical: authorize URL state/CSRF param/code exchange/cookie issuance) — only `auth_middleware` covered | [plan-164](plan-164-oauth2-flow-tests.md) | ⚪ Medium | 3.2 — test-only additions |
| **F9** | `forward_request` core routing untested (circuit breaker dead-PID detection + cleanup → 502/503, cooldown behavior, metric increments) — helpers well-tested but orchestrator not | [plan-165](plan-165-forward-request-tests.md) | ⚪ Medium | 3.3 — direct fn tests, no router needed |
| **F10** | Pull pipeline HTTP handlers and download/verify orchestration untested (validation/enqueue rejections, SSE job stream, hash verification, corrupt-GGUF admission via wiremock HF) — queue machinery has ~25 tests but the HTTP surface does not | [plan-166](plan-166-pull-handler-tests.md) | 🟠 Large | 3.4 — test-only plus one production seam fix for `hf_api()` static cache |

### 🟡 Medium Severity
| # | Summary | Plan | Effort | Phase |
|---|---------|------|--------|-------|
| **F11** | God files: `pull_queue.rs` (1,756 — ~1,230 inline tests); `api/updates.rs` (~1k + 24 error sites); `auth.rs` (1,526 mixing API keys + OAuth two + session cookies) | [cleanup](cleanup-file-splits-wave2.md) | 🟠 Large | 5.1 — pure move, no behavior change |
| **F12** | Server-side SSE: job-event stream duplicated ~85 lines across two handlers; hand-rolled event→wire mapping already drifted (`downloads.rs` embeds `"event"` in JSON, `updates.rs` does not); inconsistent KeepAlive | [db-query-from-row](db-query-from-row-server-sse-consolidation.md) | ⚪ Medium | 7.2 — after file splits since PullEvent lives in split module dir |
| **F13** | Benchmark job submission triplicated (~90 LOC identical across three handlers) while shared helper `submit_benchmark_job` is dead code (never wired in, ~60 lines unused) | [server-sse-consolidation](server-sse-consolidation-api-handler-boilerplate.md) Task 2 | 🟢 Small | 4.1 — helper extraction |
| **F14** | Model-resolution chain repeated six times (~20–45 LOC each, ~180 total with verbatim pairs including `models_dir` resolution in files.rs) | [server-sse-consolidation](server-sse-consolidation-api-handler-boilerplate.md) Task 3 | ⚪ Medium | 4.2 — helper extraction |
| **F15** | Config-dir / db_dir resolution: four variants at ~17+ sites with divergent failure behavior (variant A silently falls back to CWD `./tama.db`; B returns 404; C uses base_dir) | [server-sse-consolidation](server-sse-consolidation-api-handler-boilerplate.md) Task 3 | ⚪ Medium | 4.2 — consolidation + no more CWD fallbacks |
| **F16** | gpu_variant primitive obsession: string-matched across ≥3 places (literal array in install.rs match arms, in env.rs) despite existing enum (`GpuType`) nothing upstream uses; compaction device lossy-parses invalid input instead of 422 | [plan-170](plan-170-newtypes.md) Task 1 | ⚪ Medium | 6.1 — additive traits on existing enum |
| **F17** | HuggingFace endpoint + URL construction open-coded at six sites with `env::var` + format!; search.rs ignores HF_ENDPOINT entirely (hardcoded const bypassing env var) — mirror support inconsistent by construction | [plan-170](plan-170-newtypes.md) Task 2 | ⚪ Medium | 6.2 — new helper fns |
| **F18** | "Valid repo_id" defined three different ways with divergent accept sets: crud's charset whitelist allows `.` so `../x` passes; tama_handlers blacklists `..`/`\`; hf.rs omits backslash check — path traversal risk | [plan-170](plan-170-newtypes.md) Task 3 | ⚪ Medium | 6.3 — one strictest union validator |
| **F24** | Bench execution + download engines untested; tama-mock wired into zero tests and can bit-rotate (chunked/resumable parallel downloads corruption/backoff failure modes not covered) | [router-consolidation](router-consolidation-test-coverage-wave2.md) Task 3 | ⚪ Medium | 8.4 — after lifecycle trait adoption |
| **F28** | Naming: ModelCard→ModelToml; ModelStatus misplaced in gpu module; Loading vs Starting divergence between parallel state enums for same machine (API-visible); [supervisor] config section → [lifecycle]; dead ProcessSupervisor | [plan-173](plan-173-naming-domain-terms.md) Tasks 5–6 | 🟠 Large | 8.2 — breaking renames with three SQLite migrations + compat aliases |
| **F29** | Config mirror types in three copies (~1,300 LOC total) requiring hand-sync when adding quant kinds to core silently desynchronizes WASM copy; pull wizard re-implements ~100-line quant table | [plan-176](plan-176-leptos-ui-consolidation.md) Tasks 4–5 | 🟠 Large | 9.2 — Leptos UI type mirrors consolidated |
| **F30** | DB row-mapping closures + SELECT column lists repeated per query file (~200–250 lines of `row.get(N)?` across model_config ×3, alias ×3, tts ×2, update_check ×2) | [test-coverage-wave2](test-coverage-wave2-db-query-from-row.md) | ⚪ Medium | 8.1 — after DB consolidation settles record types |
| **F31** | Leptos UI: form/helper duplication (~350 LOC); divergent data-fetching patterns (`LocalResource` vs spawn_local + loading/error); forms use three approaches (Action on:click, on:submit) | [plan-176](plan-176-leptos-ui-consolidation.md) Tasks 4–6 | ⚪ Medium | 9.2 — after config mirrors consolidated |
| **F32** | ProxyState accessor leaks fields as pub(crate) but every one has a pub accessor returning the lock itself; service locator methods (`model_mgr()`, `open_db()`); whole management API reaches through single type | [plan-177](plan-177-proxystate-substructs.md) | 🟠 Large | 9.3 — after DB consolidation settles Repository ownership |
| **F33** | Route tables split across two routers in two crates kept by comments (~90% overlap; shadow-route bug: /system/health excluded from unified router but never mounted in tama → falls to SPA wildcard returning index.html); bench→proxy wrong-direction imports | [api-handler-boilerplate](api-handler-boilerplate-router-consolidation.md) | ⚪ Medium | 4.3 — mechanical extraction + one live bug fix |
| **F34** | `println!`/`eprintln!` remain in production paths after plan-improved-logging tracing (~30+ sites: library stdout bypasses JSON log file and config) | [file-splits-wave2](file-splits-wave2-cleanup.md) Task 4 | 🟢 Small | 4.1 — pure replacement |

### 🟢 Low Severity
| # | Summary | Plan | Effort | Phase |
|---|---------|------|--------|-------|
| **F36** | Remaining route/module test gaps + five permanently ignored backend tests; small untested utils (rename.rs data-loss class, installer extract/prebuilt) | [router-consolidation](router-consolidation-test-coverage-wave2.md) Task 4 | 🟢 Small | 8.4 — after lifecycle trait adoption |
| **F37** | Small-scale duplication batch: jitter/backoff ×2 across single/parallel downloads; mean/stddev inline ×5; path-traversal guard ×12 | *No dedicated plan* — lands naturally with F16/F18 adoptions or as part of api-handler-boilerplate helper extraction | 🟢 Small | 7.3 — mechanical hoisting |
| **F38** | Dead code small batch: four dead ProxyState accessors; eight dead DB fns (`delete_model_records` etc.); dead types (`ModelAliasRecord`); unrouted handlers | [file-splits-wave2](file-splits-wave2-cleanup.md) Task 5 (overlaps with F26) | 🟢 Small | 4.1 — pure deletions |
| **F39** | Style drift: camelCase inside snake_case payload; `_` prefix convention effectively dead (~5 uses vs ~1,300+ without); `test_` prefix violations across ~87 files | [file-splits-wave2](file-splits-wave2-cleanup.md) Task 6 | 🟢 Small | 4.1 — mechanical renames |
| **F40** | Naming small batch: fetch_* forbidden term for pull ops; quantisation spellings mixed in; module name mismatch (gpu_types.rs mostly non-GPU mirrors); composite colon-keys leaking into SQL LIKE patterns | [plan-173](plan-173-naming-domain-terms.md) Tasks 6–7 (overlaps with F28) | 🟢 Small | 8.4 — mechanical renames |

---

## Phased Execution Order — Dependency-First Cascade

### **Phase 0: Quick Fixes** ✅ COMPLETED
| Plan | Findings | Status | Notes |
|------|----------|--------|-------|
| plan-quick-fixes | F3, F6(stopgap), F21, F25, F35 | ✅ `7412709b` — landed | Broke config↔proxy cycle; stopgap for backup restore false-success → 501; CSRF route + middleware tests; /logout route; SvelteKit doc drift → Leptos correction |

### **Phase 1: DB Foundation (Sequential)**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 1.1 | [plan-160](plan-160-db-access-consolidation.md) | F1, F2 | 🟠 Large | **Gates everything downstream.** Two competing access layers + raw rusqlite leaks across ~40 files with DTO/Record duplication (~43 fields duplicated field-for-field forcing dual constructors on `ModelConfig`) and handlers opening both Repository AND Manager in one request (two SQLite connections). Consolidates to single Repository via axum state so migrations run once at startup. |

### **Phase 2: Error Contract & Backup Restore Full**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 2.1 | [plan-161](plan-161-error-contract.md) | F4 | ⚪ Medium | ~54 flat error sites in twelve files. Must follow plan-160 because many of those handlers are being rewritten by DB consolidation (especially `updates.rs` ×23 — the worst offender). Shape-assertion tests lock future drift. |
| 2.2 | [plan-162](plan-162-model-identity-configkey.md) | F5 | ⚪ Medium | Stringly-typed model identity: derivation rule open-coded at ~14+ sites applied inconsistently (pull paths skip lowercase → live bug where mixed-case repo ids silently miss lookups); two same-named `resolve_model_id` with different semantics. ConfigKey newtype is additive — can run after DB consolidation settles identity resolution. |
| 2.3 | [plan-163](plan-163-backup-restore.md) | F7(full), F8(stopgap completed) | ⚪ Medium | Completes F7/F8 stopgap from Phase 0. Wires existing tested merge machinery (`tama_core::backup::{extract_backup, merge_*}` — ~16 tests covering the logic). Routes unrouted `create_backup` handler; fixes docs/api/backup.md to match reality. |

### **Phase 3: Test Coverage for Critical Paths (Parallel Tracks)**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 3.1 | [plan-164](plan-164-oauth2-flow-tests.md) | F8 | ⚪ Medium | ~16 tests covering five untested OAuth two login-flow functions (~260 LOC security-critical: authorize URL state/CSRF param/code exchange/cookie issuance). Wiremock for token/userinfo endpoints. |
| 3.2 | [plan-165](plan-165-forward-request-tests.md) | F9 | ⚪ Medium | Direct tests for `forward_request` core routing — dead-PID detection + cleanup → 502/503, circuit-breaker cooldown and trip behavior, metric increments. No router needed — direct function calls. |
| 3.3 | [plan-166](plan-166-pull-handler-tests.md) | F10 | 🟠 Large | First tests for pull HTTP surface — validation/enqueue rejections, SSE job stream, hash verification, corrupt-GGUF admission via wiremock HF. One production seam fix: `hf_api()` static cache hardcodes huggingface.co instead of respecting env var override. ~8h vs ~3–5h each above due to complexity. |

### **Phase 4: Cleanup & Mechanical Refactors (Parallel Tracks)**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 4.1 | [file-splits-wave2](file-splits-wave2-cleanup.md) | F34, F38, F39 | 🟢 Small | ~1,900 lines dead code removed (`config/migrate` 171 LOC + Leptos components ~850 LOC + orphan `jobs.rs` 606 LOC); six unused deps dropped; blanket allows eliminated. Fewest colliding diffs of any plan — run first in this phase. |
| 4.2 | [server-sse-consolidation](server-sse-consolidation-api-handler-boilerplate.md) | F13, F14, F15 | ⚪ Medium | Benchmark job submission triplication → shared helper extraction (F13); model-resolution chain ×6 extracted to `resolve_model_record` helper (F14); config-dir four variants consolidated with no more CWD fallbacks + uniform 404 on misconfiguration (F15). |
| 4.3 | [api-handler-boilerplate](api-handler-boilerplate-router-consolidation.md) | F33 | ⚪ Medium | Single-source route table eliminating ~90% overlap across two routers in two crates; fixes shadow-route bug `/system/health` → SPA wildcard fallback returning index.html; process helpers moved from proxy to crate-level with cross-crate ownership test. Mechanical extraction with one live bug fix — independent except naming renames if domain terms breaking this release cycle. |

### **Phase 5: Newtypes & DB Query Patterns**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 5.1 | [plan-170](plan-170-newtypes.md) | F16, F17, F18 | ⚪ Medium | `GpuType` gains FromStr/Display + string-form serde for gpu_variant adoption across BackendConfig/ModelConfig/env.rs/install request DTOs (F16); HF endpoint helpers consolidate URL construction at six sites with auth headers and strictest union of three validators with traversal test (F17, F18). |
| 5.2 | [test-coverage-wave2](test-coverage-wave2-db-query-from-row.md) | F30 | ⚪ Medium | Per-record `from_row` + COLUMNS const for ~6 record types eliminating repeated row-mapping closures (~200–250 lines of `row.get(N)?`) plus column-drift guard test. One task per record type independently commitable after DB consolidation settles Record/DTO duplication. |

### **Phase 6: Larger Structural Refactors**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 6.1 | [cleanup](cleanup-file-splits-wave2.md) | F11 | 🟠 Large | Split three god files: `pull_queue.rs` → service/recovery/events/tests (~5 tasks each independently commitable); pure move with no behavior change. After cleanup removes dead code references within those same files reducing split scope slightly. |
| 6.2 | [plan-173](plan-173-naming-domain-terms.md) | F2, F28, F40 | 🟠 Large | All breaking renames in one release: GpuType→GpuVariant, server→backend (vars/logs/config-resolve); download→pull (public surface + 3 SQLite migrations compat aliases); API-visible enums Loading→Starting, [supervisor]→[lifecycle]. **Must land AFTER plan-170** which adds FromStr/serde to enum under current name. |
| 6.3 | [plan-174](plan-174-typed-api-responses.md) | F9 (partial) | ⚪ Medium | Type `/status` response opencode ModelEntry, and shared success bodies with golden shape tests. Drift guards lock future wire changes at compile time. Depends on naming if typing CRUD responses since state enum renames affect those types — but /status endpoint is self-contained without that dependency so can run in parallel with file splits. |

### **Phase 7: Leptos UI & ProxyState Composition**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 7.1 | [db-query-from-row](db-query-from-row-server-sse-consolidation.md) | F12 | ⚪ Medium | Serde-tagged `PullEvent`/`UpdateEvent` + shared job-event stream for two handlers; uniform KeepAlive. Removes payload drift between downloads and updates SSE endpoints — independent of other plans but after file splits since PullEvent lives in split module dir. |
| 7.2 | [plan-176](plan-176-leptos-ui-consolidation.md) | F29, F31 | 🟠 Large | Single-source config mirrors via #[path] inclusion — WASM frontend uses same source file as core eliminating three copies of ~1,300 LOC total plus quant inference drift and DOM helper duplication. Import paths change so any plans touching those modules adapt during merge. |
| 7.3 | [plan-177](plan-177-proxystate-substructs.md) | F32 | 🟠 Large | RegistryState/MetricsState/PullState composition with domain methods over lock-guard accessors; service locator narrowed. Shim migration — five tasks. After DB consolidation settles Repository ownership from phase 1 work plus naming-domain-terms settled state enums in phase 6 since those renames affect serialized enum values that ProxyState tracks. |

### **Phase 8: Remaining Test Coverage**
| # | Plan | Findings | Effort | Notes |
|---|------|----------|--------|-------|
| 8.1 | [router-consolidation](router-consolidation-test-coverage-wave2.md) | F24, F36 | 🟠 Large | Compaction/TTS through lifecycle traits + failure-mode tests (spawn-error cleanup for stuck Starting entries); update-check orchestration via wiremock HF; bench execution env stubs + download engine parallel/resumable tests; tama-mock integration smoke test to un-ignore five backend API tests. Best effort after phase 7 lifecycle trait adoption since compaction/TTS need those traits stable. |

---

## Dependency Graph — Text Summary

```
Phase 0: plan-quick-fixes ✅ done (Quick Fixes)
│
├── Phase 1.1: plan-160 (DB Access Consolidation) ──────────────┐ gates everything downstream
│   │                                                          │ eliminates two competing access layers + raw rusqlite leaks across ~40 files with DTO/Record duplication (~43 fields duplicated field-for-field forcing dual constructors on ModelConfig) and handlers opening both Repository AND Manager in one request (two SQLite connections). Consolidates to single Repository via axum state so migrations run once at startup.
│   ▼                                                          │
├── Phase 2.1: plan-161 (Error Contract Unification) ────────────┤ ~54 flat error sites in twelve files must follow plan-160 since many of those handlers are being rewritten by that work (especially updates.rs ×23 — the worst offender). Shape assertion tests lock future drift.
│   │                                                          )
├── Phase 2.2: plan-162 (Model Identity ConfigKey Newtype) ──────┤ Stringly-typed model identity derivation rule open-coded at ~14+ sites applied inconsistently. ConfigKey newtype is additive — can run after DB consolidation settles identity resolution.
│   │                                                          )
├── Phase 2.3: plan-163 (Backup Restore Full) ──────────────────┤ Completes F7/F8 stopgap from Phase 0. Wires existing tested merge machinery tama_core::backup::{extract_backup, merge_*} — ~16 tests covering the logic. Routes unrouted create_backup handler; fixes docs/api/backup.md to match reality.
│   │                                                          )
├── Phase 3: Test Coverage for Critical Paths ──────────────────┤ All independent of each other and phase four plus refactors. No production changes needed except plan-166's one seam fix in hf_api() static cache (hardcodes huggingface.co instead of respecting env var override).
│   ├── plan-164: OAuth Flow Tests (~16 covering five untested login-flow functions ~260 LOC security-critical authorize URL state/CSRF param/code exchange/cookie issuance via wiremock token/userinfo endpoints. Test-only additions, no production changes needed.)
│   ├── plan-165: Forward Request Core Routing (direct tests for forward_request core routing dead-PID detection + cleanup → 502/503 circuit-breaker cooldown and trip behavior metric increments. No router needed — direct function calls.)
│   └── plan-166: Pull Handler Orchestration (~8h vs ~3–5h each above due to complexity. Validation enqueue rejections, SSE job stream, hash verification, corrupt-GGUF admission via wiremock HF plus env redirection models_dir/configs_dir + production seam fix in hf_api() static cache hardcoding huggingface.co instead of respecting env var override.)
│   │                                                          )
├── Phase 4 (Parallel Tracks): Cleanup & Mechanical Refactors ──┤ All independent after phase three structural work settles.
│   ├── plan-167 (Cleanup): Dead Code Removal (~1,900 lines: config/migrate module Leptos components orphan jobs.rs); six unused deps dropped; blanket allows eliminated. Fewest colliding diffs of any plan so run first in this batch.)
│   ├── plan-168 (API Handler Boilerplate): API Handler Boilerplate Extraction (benchmark job submission triplication → shared helper extraction ~3–5h — helpers exist but never wired into success paths.)
│   └── plan-169 (Router Consolidation): Router Consolidation (single-source route table eliminating ~90% overlap across two routers plus fixes shadow-route bug /system/health → SPA wildcard fallback returning index.html. Mechanical extraction with one live bug fix independent except naming renames if domain terms breaking this release cycle.)
│   │                                                          )
├── Phase 5: Newtypes & DB Query Patterns ──────────────────────┤ Independent after phase four cleanup removes dead code references.
│   ├── plan-170: GpuType gains FromStr/Display + string-form serde for gpu_variant adoption BackendConfig ModelConfig env.rs install DTOs; HF endpoint helpers consolidate URL construction six sites auth headers strictest union three validators traversal test. Must land BEFORE naming-domain-terms which renames same enum — plan explicitly says "land AFTER Task one of this plan" to avoid rebase conflict.)
│   │   └── plan-171 (DB Query from_row): Per-record from_row + COLUMNS const for ~6 record types eliminating repeated row-mapping closures (~200–250 lines of `row.get(N)?`) plus column-drift guard test. One task per record type independently commitable after DB consolidation settles Record/DTO duplication.)
│   │                                                          )
├── Phase 6: Larger Structural Refactors ───────────────────────┤ Sequential recommended within phase due to file-level overlaps and naming dependencies but can parallelize if branching strategy allows separate PRs.
│   │   ├── plan-172 (File Splits Wave 2) (pull_queue.rs updates.rs auth.rs into focused modules three god files each with multiple tasks independently commitable pure move no behavior change after cleanup removes dead code references within those same files reducing split scope slightly.)
│   ├── plan-173: Naming Domain Terms (all breaking renames in one release GpuType→GpuVariant server→backend vars/logs/config-resolve download→pull public surface plus 3 SQLite migrations compat aliases API-visible enums like Loading→Starting and [supervisor]→[lifecycle]. Must land AFTER plan-170 which adds FromStr/serde to enum under current name.)
│   └── plan-174: Typed Responses (/status response opencode ModelEntry, shared success bodies golden shape tests lock wire changes compile time. Depends on naming if typing CRUD since state enum renames affect types but self-contained without that dependency so can run in parallel with file splits.)
│   │                                                          )
├── Phase 7: Leptos UI & ProxyState Composition ────────────────┤ These are the largest remaining structural changes after DB consolidation settles Repository ownership from phase one work plus naming-domain-terms settled state enums in phase six.
│   │   ├── plan-175 (Server SSE Consolidation): Server SSE Consolidation (Serde-tagged PullEvent/UpdateEvent + shared job-event stream for two handlers with uniform KeepAlive. Removes payload drift between downloads and updates SSE endpoints — independent of other plans but after file splits since PullEvent lives in split module dir.)
│   ├── plan-176: Leptos UI Type Mirrors (single-source config via #[path] inclusion WASM frontend same source core eliminating three copies ~1,300 LOC total quant inference drift DOM helper duplication. Import paths change so any plans touching those modules adapt during merge — coordination flag noted below.)
│   └── plan-177: ProxyState Sub-Structs (Registry/Metrics/Pull composition domain methods over lock-guard accessors service locator narrowed five tasks compiler-driven updates after DB consolidation settles Repository ownership from phase one work plus naming-domain-terms settled state enums in phase six since those renames affect serialized enum values that ProxyState tracks.)
│   │                                                          )
└── Phase 8: Remaining Test Coverage ───────────────────────────┤ Best effort after phase seven lifecycle trait adoption since compaction/TTS need those traits stable plus F36 covers remaining route/module gaps ~102 LOC untested utils rename.rs data-loss class installer extract/prebuilt.
        └── plan-178 (Test Coverage Wave 2): Compaction/TTS through lifecycle traits + failure-mode tests including spawn-error cleanup for stuck Starting entries; update-check orchestration via wiremock HF endpoints; bench execution env stubs plus download engine parallel/resumable chunked coverage with corruption/backoff failure modes tested Tama-mock integration smoke test to un-ignore five backend API tests.)
```

## Coordination Flags Between Plans

| Flag | From → To | What to Watch | Resolution |
|------|-----------|---------------|------------|
| C1 | [plan-170](plan-170-newtypes.md) Task 3 (F16/F17) vs [plan-173](plan-173-naming-domain-terms.md) | GpuType renamed before newtype adoption (`FromStr`/serde added under current name `GpuVariant`) | Land naming-domain-terms first then apply FromStr traits — minor rename adjustment in impl only. Plan-170 explicitly says "land plan-173 AFTER Task 1 of this plan" to avoid rebase conflict. |
| C2 | [plan-177](plan-177-proxystate-substructs.md) → any | ProxyState sub-structs need to know who owns Repository (API layer vs proxy lifecycle) | Land DB consolidation first — it decides that. After F1/F2: Repository is API-layer single entry point; managers stay for tama-core internal use only. |
| C3 | [file-splits-wave2](file-splits-wave2-cleanup.md) → any | Dead code deletion affects references other plans make (`Repository::conn`, `delete_model_records` in F38; ProcessSupervisor; ModelState::from_str_fallback) | Whichever lands first: the other skips those items. Each marked "if already gone" so no conflict. |
| C4 | [plan-176](plan-176-leptos-ui-consolidation.md) → any | WASM frontend type mirrors collapsed via #[path] inclusion changing import paths for config types/quant inference/DOM helpers etc — affects imports in plans touching those modules | Run after plans referencing old import paths have landed or adapt during merge. Small rename adjustment. |

## Quick Reference: What's Next?

After plan-quick-fixes (Phase 0 ✅), the next step is **[plan-160](plan-160-db-access-consolidation.md)** → DB Access Consolidation. It gates every handler touching the database and makes all downstream diffs smaller by eliminating two competing access layers, raw rusqlite leaks across ~40 files with DTO/Record duplication (~43 fields duplicated field-for-field forcing dual constructors on ModelConfig), and handlers opening both Repository AND Manager in one request (two SQLite connections).

**Recommended immediate sequence:** plan-160 → DB Access Consolidation) → [plan-161](plan-161-error-contract.md) Error Contract Unification + shape tests lock future drift) → {[plan-162](plan-162-model-identity-configkey.md)} Model Identity ConfigKey newtype additive with derivation-site consolidation fixing live bug mixed-case repo ids silently miss lookups} plus {[plan-163](plan-163-backup-restore.md)} Backup Restore full implementation completing stopgap from Phase 0 wiring existing tested merge machinery tama_core::backup::{extract_backup, merge_*}}.
