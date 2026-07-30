# Benchmark Suite Plan

**Goal:** A one-button "Suite" that auto-detects a model's capabilities (MTP head, MTP draft file, spec decoding) and runs the appropriate series of benchmarks sequentially as a single job, grouped in history.
**Architecture:** Capability detection in tama-core (GGUF `nextn_predict_count` + heuristic fallback) surfaced in the models API; a new `POST /tama/v1/benchmarks/suite` endpoint that submits ONE `JobKind::Benchmark` job sequencing the existing `run_benchmark_inner` / `run_mtp_benchmark_inner` / `run_spec_benchmark_inner` fns with a shared `Arc<Job>` (ADR-0004); sub-runs linked by a new nullable `suite_id` column; UI is a 4th benchmarks tab plus a per-model "Run Suite" deep-link.
**Tech Stack:** Rust, Axum, Leptos, rusqlite, gguf_parser.
**Depends on:** plan-180 (per-type history rendering, status derivation), plan-181 (batch/ubatch prefill), plan-182 (shared selectors/submit helper the Suite tab reuses).

---

### Task 1: GGUF nextn detection + capability computation

**Context:**
`GgufMetadata` (`crates/tama-core/src/models/gguf.rs:9-16`) reads only basic fields via the `gguf_parser` crate. Models with built-in MTP heads (DeepSeek/GLM/dflash-style) declare `{arch}.nextn_predict_count` in GGUF metadata. Fallback heuristic when the key is absent: a quant classified `QuantKind::Mtp` exists (`crates/tama-core/src/types/quant.rs:11-34`), OR `mtp_model` is set (`config/types/model.rs:52-56`), OR `spec_decoding.spec_types` contains `draft-mtp` (model.rs:277).

**Files:**
- Modify: `crates/tama-core/src/models/gguf.rs`
- Modify: wherever GgufMetadata is produced/consumed (rg `GgufMetadata` across tama-core)
- Modify: `crates/tama/src/api/models/info.rs` (model JSON, :134-182)

**What to implement:**
1. `gguf_parser::GgufFile` DOES expose arbitrary keys via `get_metadata(key) -> Option<&GgufValue>` with `.as_u64()` — no hedge needed. Add `pub nextn_predict_count: Option<u64>` to `GgufMetadata`; populate in `parse_gguf_metadata` from `format!("{arch}.nextn_predict_count")`.
2. **Data-source split (important):** at model-LIST time (`api/models/info.rs::model_entry_json`) the GGUF is not parsed and per-model file I/O there is too expensive — list-time `capabilities` uses the HEURISTIC only. The authoritative GGUF `nextn_predict_count` check happens in the suite endpoint (Task 3), which parses the selected model's GGUF header on demand (one ~100KB read, fine at run time). Do NOT add GGUF parsing to the list handler.
3. Add a computed helper, e.g. `fn model_capabilities(config: &ModelConfig, nextn: Option<u64>) -> ModelCapabilities` returning `struct ModelCapabilities { supports_mtp: bool, has_mtp_draft_file: bool, has_mmproj: bool }` (place in tama-core near models). `supports_mtp = nextn.unwrap_or(0) > 0 || has_mtp_draft_file || config.mtp_model.is_some() || spec_types contains "draft-mtp"`. List time calls it with `nextn: None`; the suite endpoint passes the parsed value.
4. Surface `capabilities` in the model JSON by **explicitly adding it to the `serde_json::json!({...})` block** in `model_entry_json` (the block is field-by-field — nothing flows automatically).

**Steps:**
- [ ] Write failing tests: GGUF parsing test with a metadata map containing e.g. `deepseek2.nextn_predict_count = 1` (check existing gguf.rs tests for the harness); heuristic unit tests for each branch of `model_capabilities` (with `nextn: None` and with `Some(1)`).
- [ ] Run `cargo nextest run --package tama-core -- gguf` and `-- capabilities` — confirm fail.
- [ ] Implement.
- [ ] Run `cargo nextest run --workspace`
- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: "feat: detect MTP capability from GGUF nextn metadata + heuristics"

**Acceptance criteria:**
- [ ] `nextn_predict_count` parsed when present; heuristic covers its absence
- [ ] Model list JSON includes `capabilities`

---

### Task 2: suite_id column + insert plumbing

**Context:**
Migration `_0042_add_benchmark_suite_id.rs` (after plan-181's `_0041` — bump `LATEST_VERSION` to 42 and register in `migrations.rs`) adds a nullable `suite_id TEXT`. There are TWO insert layers to thread: `Repository::insert_benchmark(&self, params: &BenchmarkParams)` (`crates/tama-core/src/db/repository.rs:193`, struct at :16) converts field-by-field into `queries::BenchmarkInsertParams` and calls `benchmark_queries::insert_benchmark` (:60); listing goes through `BenchmarkRow`/`list_benchmarks` (:100). The 3 production call sites (run.rs, mtp.rs, spec.rs) construct `tama_core::db::repository::BenchmarkParams { ... }` — all must keep compiling after this task, so THIS task (not Task 3) adds `suite_id: Option<String>` to ALL layers, with existing call sites passing `None` for now.

**Files:**
- Create: `crates/tama-core/src/db/migrations/_0042_add_benchmark_suite_id.rs`
- Modify: `crates/tama-core/src/db/migrations.rs` (mod decl, MIGRATIONS entry, `LATEST_VERSION = 42`)
- Modify: `crates/tama-core/src/db/queries/benchmark_queries.rs` (`BenchmarkInsertParams`, `BenchmarkRow`, insert SQL, list SELECT, plus the tests' hand-rolled `test_conn()` CREATE TABLE and `make_benchmark` literals)
- Modify: `crates/tama-core/src/db/repository.rs` (`BenchmarkParams` struct + `insert_benchmark` forwarding: `suite_id: params.suite_id.as_deref()`; **also the `test_benchmark_crud_round_trip` literal at ~:484** — add `suite_id: None`, it constructs `BenchmarkParams` as a full literal and will not compile otherwise)
- Modify: `crates/tama/src/api/benchmarks/run.rs`, `mtp.rs`, `spec.rs` (add `suite_id: None` to their `BenchmarkParams { ... }` literals — Task 3 wires real values)
- Modify: `crates/tama/src/api/benchmarks/mod.rs` (`BenchmarkHistoryEntry` + history mapping to expose `suite_id`)

**What to implement:**
- Migration: `ALTER TABLE benchmarks ADD COLUMN suite_id TEXT;` (nullable, no index needed at expected scale).
- Thread `suite_id: Option<String>` through ALL layers: `BenchmarkInsertParams` → `BenchmarkRow`/list SELECT → `Repository::BenchmarkParams` + `insert_benchmark` forwarding → `BenchmarkHistoryEntry`. Existing call sites pass `None`, INCLUDING the `test_benchmark_crud_round_trip` test literal (repository.rs:~484) (keeps this task independently compilable/committable).

**Steps:**
- [ ] Write failing test: insert with `suite_id: Some("suite-abc")`, list, assert round-trip; NULL for plain runs.
- [ ] Run `cargo nextest run --package tama-core -- benchmark_queries` — confirm fail.
- [ ] Implement.
- [ ] Run `cargo nextest run --workspace`
- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: "feat: suite_id column for grouping benchmark runs"

**Acceptance criteria:**
- [ ] suite_id round-trips; existing rows have NULL

---

### Task 3: Suite endpoint (sequential wrapper job)

**Context:**
Per ADR-0004: one job, sequential sub-runs, shared `Arc<Job>`, continue-on-error. Existing inner fns all have signature `(Arc<JobManager>, Arc<Job>, req, db_path, proxy_base_url, client, repo_handle) -> Result<()>` (`run.rs:19`, `mtp.rs:70`, spec.rs). `submit_benchmark_job` (`api/benchmarks/mod.rs:198-262`) spawns and marks the job finished — the suite needs its own submit wrapper so sub-runs don't finish the shared job. Each inner fn currently inserts its own history row — they need an optional `suite_id` (add a field to each request struct or a parameter; simplest: add `#[serde(skip)] pub suite_id: Option<String>` to each benchmark request struct, defaulting None).

**Files:**
- Create: `crates/tama/src/api/benchmarks/suite.rs`
- Modify: `crates/tama/src/api/benchmarks/mod.rs` (route registration, request DTOs, suite_id plumbing)
- Modify: `crates/tama/src/api/benchmarks/run.rs`, `mtp.rs`, `spec.rs` (accept + forward suite_id to insert)
- Modify: router registration (find with `rg "benchmarks/run" crates/tama/src`)

**What to implement:**
- `BenchmarkSuiteRequest { model_id, quant: Option<String>, backend_name: Option<String>, gpu_variant: Option<String>, types: Option<Vec<String>> }`.
- Auto-selection when `types` is None: always `["llama_bench", "spec"]`; add `"mtp"` and include `draft-mtp` in spec types when `capabilities.supports_mtp`. Defaults: llama_bench = baseline preset values (pp 2048, tg 128); mtp = server defaults (mtp.rs:28-41); spec = spec_scan preset (draft_max 16, ngram 12/48, gen_tokens 256, runs 3). Batch/ubatch prefill from the model's `n_batch`/`n_ubatch` (plan-181).
- Handler: generate `suite_id` (uuid or `suite-<job_id>`), submit one job via a suite-specific wrapper. **Do not copy-paste `submit_benchmark_job` (~60 lines)** — refactor it to accept the run-future/closure as a parameter (it already takes the inner fn; generalize so the suite passes a closure that sequences sub-runs and only then lets the job finish). For each selected type, build that type's request struct and call its `*_inner` with the shared job; on `Err`, log to the job and CONTINUE; track per-type outcomes. **Authoritative MTP check here:** parse the model's GGUF header on demand (Task 1's `parse_gguf_metadata`) and use `nextn_predict_count` for final type selection.
- **Required fields when building sub-requests:** `SpecBenchmarkRunRequest.spec_types: Vec<SpecType>` is REQUIRED (no serde default) — populate explicitly (all 4 ngram types; add `SpecType::DraftMtp` when `supports_mtp`). `BenchmarkRunRequest` has required `pp_sizes`/`tg_sizes`/`runs`/`warmup` — populate from baseline preset. Add `#[serde(skip)] pub suite_id: Option<String>` (default None) to each benchmark request struct so inner fns can forward it to their `BenchmarkParams` insert (replacing Task 2's `None`).
- After all sub-runs: suite-level job status = failed if all failed, partial-equivalent logged if some failed (job status enum may be binary — check `JobManager`; if binary, use failed only when all failed, and rely on per-row status from plan-180 Task 6 for partial visibility).
- Register route `POST /tama/v1/benchmarks/suite` with the same auth/scopes as the other benchmark routes.

**Steps:**
- [ ] Write failing tests: auto-selection logic (extract pure fn `select_suite_types(caps: &ModelCapabilities) -> Vec<SuiteType>`); request-building defaults test.
- [ ] Run `cargo nextest run --package tama -- suite` — confirm fail.
- [ ] Implement endpoint + plumbing.
- [ ] Run `cargo nextest run --workspace`
- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: "feat: POST /benchmarks/suite sequential benchmark suite job"

**Acceptance criteria:**
- [ ] One job runs all selected benchmarks sequentially with one log/SSE stream
- [ ] A failing sub-run doesn't abort the suite; each sub-run writes a history row with shared suite_id
- [ ] `types` omitted → capability-based auto-selection

---

### Task 4: Suite UI tab + models-page button + grouped history

**Context:**
Uses plan-182's shared `ModelQuantSelect`/`BackendSelect`/`submit_bench_job`. Suite tab is the 4th tab (`tab_buttons` component already in use). Models page (`crates/tama/src/pages/models.rs`) currently has no benchmark links — add a "Run Suite" action deep-linking to `/tama/benchmarks?tab=suite&model=<id>` (check how routing/query params are handled in the app; benchmarks page must read them to preselect).

**Files:**
- Create: `crates/tama/src/pages/benchmarks/suite_bench.rs`
- Modify: `crates/tama/src/pages/benchmarks/mod.rs` (4th tab, query-param preselect, grouped history rendering)
- Modify: `crates/tama/src/pages/models.rs` and `crates/tama/src/components/model_card.rs` (Run Suite button)
- Modify: `crates/tama/css/` (suite group header styling if needed)

**What to implement:**
- `SuiteBench` component: `ModelQuantSelect` + `BackendSelect`, capability-driven checkboxes (llama-bench always on; spec always on; MTP + draft-mtp ticked when `capabilities.supports_mtp`, disabled with tooltip when not), collapsed "Advanced" overrides (defaults documented in placeholder text), Run button via `submit_bench_job("/tama/v1/benchmarks/suite", body)`, JobLogPanel wiring mirroring other tabs.
- Badges on mtp/spec tabs: use `capabilities` to mark ineligible models in their dropdowns (e.g. suffix " (no MTP)") — light touch, don't block selection.
- History grouping: rows sharing a `suite_id` render under one collapsible group header (suite timestamp + model + per-type status chips); ungrouped rows render as before.
- Models page: "Run Suite" button per model → navigate to `/tama/benchmarks?tab=suite&model=<id>`. **The per-model action buttons live in `crates/tama/src/components/model_card.rs`** (rendered from models.rs via callbacks) — add the button there or via a new callback prop, not only in models.rs. Read the query params on the benchmarks page with `leptos_router::hooks::use_query_map` (existing pattern — see `crates/tama/src/pages/logs.rs`).

**Steps:**
- [ ] Implement component + tab + routing preselect; `cargo check --package tama` iteratively.
- [ ] Implement history grouping + models-page button.
- [ ] `cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings && cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: "feat: benchmark suite UI tab, models-page Run Suite, grouped history"

**Acceptance criteria:**
- [ ] Suite tab auto-ticks types from capabilities; submitting shows live job log
- [ ] History groups suite runs under one expandable header
- [ ] Models page deep-link preselects model and tab
