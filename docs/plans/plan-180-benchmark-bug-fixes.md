# Benchmark Bug Fixes Plan

**Goal:** Fix six benchmark bugs: spec/MTP results missing from history, indistinguishable backend dropdown entries, stale history after MTP/spec runs, hardcoded draft_ngl, always-"success" status, and silent llama-bench submit failures.
**Architecture:** Fixes span the history API (`crates/tama/src/api/benchmarks/history.rs`), the run endpoint (`crates/tama/src/api/benchmarks/run.rs`), tama-core's `run_llama_bench`, and the Leptos frontend (`crates/tama/src/pages/benchmarks/`). Decisions are recorded in `docs/adr/0005-gpu-variant-as-dto-field.md`.
**Tech Stack:** Rust, Axum, Leptos (WASM), rusqlite, serde_json.

---

### Task 1: Fix spec results conversion arm in history API

**Context:**
`SpecBenchResult` (`crates/tama-core/src/bench/llama_cli_spec/mod.rs:163-170`) serializes as `{baseline_tg_ts, baseline_tg_stddev, entries: [...]}` — it has NO `summaries` key. In `crates/tama/src/api/benchmarks/history.rs` (~line 138-144) the conversion `match` scrutinizes `raw.get("summaries")` in BOTH arms, so the spec conversion block (which maps `tg_ts_mean → tg_mean` etc.) is dead code and spec rows fall through to an empty array. The frontend then shows "PP — · TG —" and an empty detail table. The fix is to restructure the match so the spec arm matches on `raw.get("entries")` when `baseline_tg_ts` is present.

**Files:**
- Modify: `crates/tama/src/api/benchmarks/history.rs`
- Test: add tests in `crates/tama/src/api/benchmarks/history.rs` (or a `tests` module in `crates/tama/src/api/benchmarks/`) — check where existing api tests live first with `rg -l "#\[cfg\(test\)\]" crates/tama/src/api/benchmarks/`

**What to implement:**
- Restructure the `summaries` computation as an if/else chain (or match on a tuple) with this precedence:
  1. `raw.get("summaries")` is an array → use as-is (llama-bench BenchReport).
  2. `raw.get("entries")` is an array AND `raw.get("baseline_tg_ts").is_some()` → run the EXISTING spec conversion block unchanged (it already maps `tg_ts_mean→tg_mean`, `tg_ts_stddev→tg_stddev`, `spec_type`, `draft_max`, `ngram_n`, `ngram_m`, `delta_pct`, `status`).
  3. `raw.is_array()` → legacy rows, use as-is.
  4. Otherwise → empty array.
- Do NOT change the conversion block's field mapping logic — only the scrutinee.

**Steps:**
- [ ] Write a failing unit test that feeds a serialized `SpecBenchResult` JSON (`{"baseline_tg_ts": 50.0, "baseline_tg_stddev": 1.0, "entries": [{"tg_ts_mean": 80.0, "tg_ts_stddev": 2.0, "spec_type": "ngram-simple", "draft_max": 16, "delta_pct": 60.0, "status": "success"}]}` — note real `SpecEntry.status` values are `"success"`/`"failed"`/`"skipped_oom"`) through the conversion and asserts the output summaries array has 1 entry with `tg_mean == 80.0` and `spec_type == "ngram-simple"`. Extract the conversion into a testable pure fn `fn summaries_from_results_json(raw: &serde_json::Value, tg_sizes: &[u32]) -> serde_json::Value` in history.rs (the conversion is pure — no async/DB; note `tg_sizes` is `Vec<u32>` in `list_benchmark_history`, NOT u64).
- [ ] Run `cargo nextest run --package tama -- summaries_from_results_json` — confirm it fails.
- [ ] Implement the scrutinee fix.
- [ ] Run `cargo nextest run --package tama` — all pass.
- [ ] Run `cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit: "fix: spec benchmark results missing from history (dead conversion arm)"

**Acceptance criteria:**
- [ ] Spec conversion arm executes for SpecBenchResult-shaped JSON
- [ ] llama-bench and legacy-array rows still pass through unchanged (test both)

---

### Task 2: Add MTP results conversion arm in history API

**Context:**
`MtpBenchResult` (`crates/tama-core/src/bench/llama_cli_mtp/mod.rs:112-119`) serializes as `{entries: [...], aggregate: {...}}` — no `summaries`, no `baseline_tg_ts`. There is no match arm for this shape, so MTP rows return empty results. Entries have `draft_max`, `name`, `predicted_per_second`, `accept_rate` (verify exact field names by reading `crates/tama-core/src/bench/llama_cli_mtp/mod.rs` `MtpPromptResult` struct before implementing).

**Files:**
- Modify: `crates/tama/src/api/benchmarks/history.rs`

**What to implement:**
- Add a third arm (between spec and legacy): `raw.get("entries")` is an array AND `raw.get("aggregate").is_some()` → convert each entry to summary format:
  - `prompt_tokens: 0`, `gen_tokens: tg_sizes.first()` (same as spec arm)
  - `tg_mean` ← entry's `predicted_per_second`, `tg_stddev` ← 0.0 (or entry's stddev field if one exists — check the struct)
  - carry through `draft_max`, `accept_rate`, and `name` as extra display fields
  - `status`: **MtpPromptResult has NO status field** — it has `error: Option<String>`. Derive: `"failed"` when `error` is non-null, else `"success"` (use these exact values to match the spec convention). This matters for Task 6's failure counting.
- Share the per-entry summary-building code with the spec arm via a small helper if clean; don't force it.

**Steps:**
- [ ] Write failing test with an MtpBenchResult-shaped JSON asserting `tg_mean` and `accept_rate` appear in output.
- [ ] Run `cargo nextest run --package tama -- summaries_from_results_json` — confirm fail.
- [ ] Implement the arm.
- [ ] Run `cargo nextest run --package tama` — all pass.
- [ ] `cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit: "fix: mtp benchmark results missing from history (no conversion arm)"

**Acceptance criteria:**
- [ ] MTP rows return non-empty `results` with `accept_rate` and `draft_max` preserved
- [ ] Spec and llama-bench rows unaffected

---

### Task 3: Per-type detail tables in history UI

**Context:**
The shared detail renderer `render_summaries_table` (`crates/tama/src/pages/benchmarks/mod.rs:32-~140`) only has TEST/PHASE/T·S columns (plus conditional Batch/µ-batch). After Tasks 1-2, spec rows carry `spec_type`/`draft_max`/`delta_pct` and MTP rows carry `draft_max`/`accept_rate` — these deserve their own columns. Decision: per-type detail tables, not one generic table.

**Files:**
- Modify: `crates/tama/src/pages/benchmarks/mod.rs`

**What to implement:**
- Split rendering by benchmark engine (the history entry has an `engine`/`backend` field — check `BenchmarkHistoryEntry` in `crates/tama/src/api/benchmarks/mod.rs:131-149` for the exact discriminator field, e.g. `engine`):
  - `llama_bench` → existing `render_summaries_table` unchanged.
  - `llama_cli_spec` → new `render_spec_table`: columns SPEC TYPE | DRAFT MAX | T/S (±STDDEV) | Δ% (use `delta_pct_display` when present).
  - `llama_cli_mtp` → new `render_mtp_table`: columns TEST (entry `name`) | DRAFT MAX | T/S | ACCEPT %.
- Update the expanded-row dispatch (~mod.rs:942) to pick the table by engine.
- In the "Best t/s" cell (~mod.rs:872-885), spec/MTP rows should use max `tg_mean` for TG and "—" for PP (they have no PP phase) — verify this already falls out of the existing `best("pp_mean")` returning None → "—"; if not, special-case it.

**Steps:**
- [ ] Implement the two render fns + dispatch (frontend WASM code has no unit test harness for views; verify by `cargo check --package tama --target wasm32-unknown-unknown` if that's the frontend target, otherwise `cargo check --package tama`).
- [ ] Run `cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Manually verify with `make run` if a populated DB is available; otherwise rely on API-level tests from Tasks 1-2.
- [ ] Commit: "feat: per-type detail tables for spec/mtp benchmark history"

**Acceptance criteria:**
- [ ] Spec rows show spec_type/draft_max/Δ% columns; MTP rows show draft_max/accept%
- [ ] llama-bench rows render identically to before

---

### Task 4: gpu_variant-aware backend dropdown on llama-bench page

**Context:**
The main bench page dropdown (`crates/tama/src/pages/benchmarks/mod.rs:470-494`) uses `fetch_bench_backends` (`utils.rs:136-159`) which reads only `type`/`display_name`, so llama.cpp CPU and CUDA both appear as "llama.cpp" with identical values. The backends API already returns `gpu_variant` per card (`crates/tama/src/api/backends/types.rs:33-69`). The mtp/spec pages already solved this via `fetch_installed_backend_variants` (`utils.rs:164-213`, value `"name:variant"`, label `"llama.cpp (cuda)"`) and `split_name_variant` (`utils.rs:40-48`). But `POST /tama/v1/benchmarks/run` only accepts `backend_name` — per ADR-0005, add a separate `gpu_variant` field (mirroring `api/benchmarks/mtp.rs:184-192` and `spec.rs:191-199`).

**Files:**
- Modify: `crates/tama/src/pages/benchmarks/mod.rs` (dropdown + submit body)
- Modify: `crates/tama/src/api/benchmarks/mod.rs` (`BenchmarkRunRequest` DTO — find exact location with `rg "struct BenchmarkRunRequest" crates/tama/src`)
- Modify: `crates/tama/src/api/benchmarks/run.rs` (pass variant through)
- Modify: `crates/tama-core/src/bench/llama_bench/mod.rs` (`run_llama_bench` signature, ~line 72-128)

**What to implement:**
1. Frontend mod.rs: replace `fetch_bench_backends(available_backends)` with `fetch_installed_backend_variants(...)` (same call the mtp page makes at `mtp_bench.rs:33`); the dropdown rendering code can stay as-is since the signal shape is the same `Vec<(String, String)>`.
2. Frontend submit (~mod.rs:271-279): `let (backend_name, gpu_variant) = split_name_variant(&selected_backend.get());` and include both keys in the JSON body (empty/Auto → `None` for both). Mirror `mtp_bench.rs:51-53`.
3. DTO: add `pub gpu_variant: Option<String>` to `BenchmarkRunRequest` (serde default).
4. run.rs: pass `req.gpu_variant` into `run_llama_bench`.
5. tama-core `run_llama_bench`: **types matter here** — `model_config.gpu_variant` is `Option<GpuVariant>` (an enum in `crates/tama-core/src/gpu/detect.rs`: `Cuda{version}`, `Vulkan`, `RocM`, `CpuOnly`, `Custom`), NOT a string, and `Config::resolve_backend_path(&self, name: &str, model_variant: Option<&GpuVariant>, manager)` takes `Option<&GpuVariant>`. Mirror mtp.rs/spec.rs's parsing pattern: in run.rs parse `req.gpu_variant: Option<String>` → `Option<GpuVariant>` via `<GpuVariant as FromStr>::from_str`; add a `gpu_variant: Option<GpuVariant>` parameter to `run_llama_bench` (and `run_llama_bench_with_dir` if that's where the resolve call lives); at the `resolve_backend_path` call (~line 128) compute `let variant = gpu_variant.as_ref().or(model_config.gpu_variant.as_ref());` and pass `variant`. **Note:** mtp/spec pass the request variant only; llama-bench needs the `.or()` fallback to preserve its existing Auto behavior (using the model's own gpu_variant when no override is supplied) — do not switch to request-only. Update all callers (rg for `run_llama_bench`/`run_llama_bench_with_dir` — one production caller run.rs:90 plus two tests calling `run_llama_bench_with_dir` directly; pass `None` there).

**Steps:**
- [ ] Write/adjust a tama-core test for `run_llama_bench`'s variant resolution if one exists nearby (`rg -l "run_llama_bench" crates/tama-core/src`); otherwise add a unit test for the `variant = gpu_variant.or(model...)` precedence logic extracted as a small pure helper.
- [ ] Run `cargo nextest run --package tama-core -- llama_bench` — confirm behavior.
- [ ] Implement frontend + DTO + passthrough.
- [ ] Run `cargo nextest run --workspace`
- [ ] Run `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: "feat: gpu_variant-aware backend selection for llama-bench runs"

**Acceptance criteria:**
- [ ] Dropdown shows "llama.cpp (cuda)" / "llama.cpp" (cpu) distinctly and only installed variants
- [ ] Selected variant reaches `resolve_backend_path` (verify via test or tracing log)
- [ ] Empty selection keeps Auto behavior (model's own backend/variant)

---

### Task 5: Refresh history after MTP/spec runs + fix hardcoded draft_ngl + surface llama-bench submit errors

**Context:**
Three small frontend fixes bundled: (a) `history_refresh` is local to `Benchmarks` (mod.rs:204) and only llama-bench callbacks bump it (mod.rs:324-335); `MtpBench`/`SpecBench` can't reach it, so their results don't appear until reload. (b) `mtp_bench.rs:75` hardcodes `"draft_ngl": Some(99u32)` while the "GPU layers" input feeds `"ngl"` — verify against `crates/tama/src/api/benchmarks/mtp.rs` request struct which field is which, then wire the input correctly (check whether the input label "GPU layers for the draft model" means it should feed `draft_ngl`). (c) llama-bench submit (mod.rs:311-321) fails silently; mtp/spec set an `error_msg` signal — add the same to mod.rs.

**Files:**
- Modify: `crates/tama/src/pages/benchmarks/mod.rs`
- Modify: `crates/tama/src/pages/benchmarks/mtp_bench.rs`
- Modify: `crates/tama/src/pages/benchmarks/spec_bench.rs`

**What to implement:**
- (a) Add a `history_refresh: RwSignal<u32>` (match actual type at mod.rs:204) parameter to `MtpBench` and `SpecBench` components; bump it in their `on_status_cb` (the completion callback — fires after the job finishes and the row is inserted) AND `on_result_cb`, mirroring llama-bench's mod.rs:324-335 which bumps in both. Do NOT bump only in `on_result_cb`: for MTP/spec, `progress.result()` fires inside the runner BEFORE `run_*_benchmark_inner` inserts the DB row, so a result-only bump can refetch before the row exists. Pass the signal at the call sites in mod.rs:382-388.
- (b) Read `crates/tama/src/api/benchmarks/mtp.rs` request DTO and defaults (mtp.rs:28-41). **Decided wiring:** the single "GPU layers for the draft model" input (mtp_bench.rs:257) should feed `"draft_ngl"` (matching its label), and `"ngl"` should be sent as `None` (server default 99) — remove the hardcoded `"draft_ngl": Some(99u32)` at mtp_bench.rs:75. Send `None` when the input is empty so server defaults apply.
- (c) Add `error_msg: RwSignal<Option<String>>` to the llama-bench form; on submit failure set it with the response text (copy the pattern from mtp_bench.rs:73-96); render the same alert block mtp/spec use (mtp_bench.rs:301-313).

**Steps:**
- [ ] Implement all three.
- [ ] Run `cargo check --package tama` then `cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit: "fix: history refresh after mtp/spec runs, draft_ngl input wiring, llama-bench submit errors"

**Acceptance criteria:**
- [ ] History table refreshes after an MTP or spec run completes
- [ ] draft_ngl/ngl inputs map to the correct server fields; empty input → server default
- [ ] llama-bench submit failure shows an error banner

---

### Task 6: Derive run status (success / partial / failed)

**Context:**
`status` is written as `"success"` unconditionally at insert time in run.rs, mtp.rs, spec.rs even when individual test entries fail. Decision: derive `success` (all entries ok), `partial` (some failed), `failed` (run errored before producing results).

**Files:**
- Modify: `crates/tama/src/api/benchmarks/run.rs`, `crates/tama/src/api/benchmarks/mtp.rs`, `crates/tama/src/api/benchmarks/spec.rs`
- Modify: `crates/tama/src/pages/benchmarks/mod.rs` (badge rendering for `partial`)
- Modify: `crates/tama/css/` (badge style for partial, if no generic warning-badge class exists — check `rg "badge" crates/tama/css/`)

**What to implement:**
- Add a helper (in `crates/tama/src/api/benchmarks/mod.rs`) `fn derive_status(entries_ok: usize, entries_failed: usize, run_errored: bool) -> &'static str`: errored → "failed"; failed>0 → "partial"; else "success".
- Each insert site computes counts from its result struct before serializing: **spec** entries have a real `status` field (`"success"`/`"failed"`/`"skipped_oom"` — count `"failed"`); **MTP** entries have `error: Option<String>` (count non-null as failed). **llama-bench has NO per-test failure concept**: `BenchSummary` (`tama-core/src/bench/mod.rs:88`) has no status field, `parse_bench_json` only emits successful tests, and failed runs `bail!` before insert — so llama-bench always derives `"success"` (only spec/MTP can be `partial`). On the error path of each `*_inner`, keep current behavior (no insert).
- Frontend: badge class mapping — add `partial` → warning/yellow style next to the existing success green (~mod.rs history row rendering).

**Steps:**
- [ ] Write failing unit tests for `derive_status` (all three branches).
- [ ] Run `cargo nextest run --package tama -- derive_status` — confirm fail.
- [ ] Implement helper + wire the three insert sites.
- [ ] Run `cargo nextest run --workspace`
- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: "feat: derive benchmark run status (success/partial/failed)"

**Acceptance criteria:**
- [ ] A spec/MTP run with some failed entries stores `partial`; a run that errors stores no row (unchanged); llama-bench runs always store `success`
- [ ] History UI renders a distinct badge for `partial`
