# Benchmarks Frontend Refactor Plan

**Goal:** Eliminate ~500 lines of duplication in the benchmarks frontend, make the three tabs symmetric, and fix tab-switch state loss.
**Architecture:** Extract shared Leptos components (`ModelQuantSelect`, `BackendSelect`, submit helper) in `crates/tama/src/pages/benchmarks/`, extract `LlamaBenchForm` from `mod.rs`, hoist shared form state into the parent `Benchmarks` component, and adopt existing shared utilities (`crate::utils::target_value`, `components/section_card.rs`, `components/alert_banner.rs`).
**Tech Stack:** Rust, Leptos (WASM).
**Depends on:** plan-180 (bug fixes — the refactor moves the same code; do plan-180 first to avoid conflicts).

---

### Task 1: Extract LlamaBenchForm component

**Context:**
`Benchmarks()` in `crates/tama/src/pages/benchmarks/mod.rs` is ~800 lines (mod.rs:148-955) with the llama-bench form + results nested ~6 levels deep inside a tab-conditional closure. `MtpBench` and `SpecBench` are already separate components; extracting `LlamaBenchForm` makes the three tabs symmetric siblings.

**Files:**
- Create: `crates/tama/src/pages/benchmarks/llama_bench.rs`
- Modify: `crates/tama/src/pages/benchmarks/mod.rs`

**What to implement:**
- Move the llama-bench tab's form state, submit logic, results view, and presets usage into a `#[component] pub fn LlamaBench(...)` in the new file, mirroring the structure of `mtp_bench.rs` (props: shared state signals + `history_refresh`).
- `mod.rs` keeps: tab switching, history table, shared state. The tab closure becomes `<LlamaBench ... />` like the other two tabs.
- Move llama-bench-only helpers (`render_summaries_table` if only used by llama-bench results — it's also used by history detail, so keep shared renderers in mod.rs or move to utils.rs as appropriate). **Note:** `parse_threads` is an inline **closure** at mod.rs:210 (not a fn at 221-231) — extract it to `pub fn parse_threads(s: &str) -> Option<Vec<u32>>` in utils.rs as part of Task 4, not here.
- Purely mechanical move — no behavior changes.

**Steps:**
- [ ] Move code; run `cargo check --package tama` until clean.
- [ ] Run `cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings && cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: "refactor: extract LlamaBenchForm from benchmarks mod.rs"

**Acceptance criteria:**
- [ ] mod.rs shrinks to roughly tab-shell + history (~400 lines)
- [ ] No behavior change (compile + clippy clean)

---

### Task 2: Shared ModelQuantSelect and BackendSelect components

**Context:**
Model+Quant selectors are copy-pasted 3× (`mod.rs:393-481`, `mtp_bench.rs:142-222`, `spec_bench.rs:310-388`) and Backend selectors 3× (`mod.rs:484-510`, `mtp_bench.rs:225-250`, `spec_bench.rs:391-416`) — ~360 lines total, character-for-character similar including the BTreeMap dedup-by-display-name and `"id:quant"` flattening logic.

**Files:**
- Create: `crates/tama/src/pages/benchmarks/selectors.rs` (or add to `utils.rs` if small — prefer a new file)
- Modify: the three tab components

**What to implement:**
- `#[component] pub fn ModelQuantSelect(models: ..., selected: RwSignal<String>, /* any per-tab extras */) -> impl IntoView` — contains the model `<select>` and quant `<select>` exactly as currently rendered (same classes/markup).
- `#[component] pub fn BackendSelect(backends: ..., selected: RwSignal<String>) -> impl IntoView` — the variant-aware dropdown (after plan-180 all three pages use `fetch_installed_backend_variants` data).
- Replace the three inline copies with the components. Keep each tab's specific signals/props passed in — don't over-abstract (YAGNI: only extract what's actually duplicated).

**Steps:**
- [ ] Implement components; swap call sites one tab at a time, `cargo check --package tama` after each.
- [ ] `cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit: "refactor: shared ModelQuantSelect/BackendSelect components"

**Acceptance criteria:**
- [ ] ~250-300 lines removed; all three tabs render identical selectors via shared components

---

### Task 3: Shared submit helper + hoist form state to parent

**Context:**
(a) `submit_benchmark` prologue/epilogue, the `post_request → status>=400 → text() → job_id` block, `on_result_cb`/`on_status_cb`, error display, JobLogPanel block, and run button are duplicated between mtp/spec (and partially mod.rs) — ~90 lines each. (b) Tab switches re-create MtpBench/SpecBench, re-fetching `/models` + `/backends` and losing form state; the parent also fetches unconditionally. Decision: hoist shared state (models, backends, selections) into `Benchmarks`; children receive via props. One fetch total; state survives tab switches.

**Files:**
- Modify: `crates/tama/src/pages/benchmarks/utils.rs` (add `submit_bench_job`)
- Modify: `crates/tama/src/pages/benchmarks/mod.rs` (hoist state)
- Modify: the three tab components

**What to implement:**
- `pub fn submit_bench_job(url: &str, body: serde_json::Value) -> impl Future<Output = Result<String, String>>` (or async fn) in utils.rs: POST, on `status >= 400` return `Err(response text)`, else parse `job_id`. Use in all three tabs.
- Extract the shared per-tab scaffolding where clean: the `on_result_cb`/`on_status_cb` wiring can become a small helper taking (`is_running`, `error_msg`, `history_refresh`) signals.
- Hoist `use_benchmark_form_state()` + backend variants fetch into `Benchmarks`; pass `BenchmarkFormState` and the backends signal down as props to all three tabs. Remove per-tab fetches.
- Replace hand-rolled error `<div class="alert alert-danger">` blocks with the existing `components/alert_banner.rs` component if its API fits; otherwise keep markup but share via a tiny local component.

**Steps:**
- [ ] Implement helper + swap the three submit paths.
- [ ] Hoist state; update component signatures and call sites.
- [ ] `cargo check --package tama`, `cargo fmt --all`, `cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit: "refactor: shared bench submit helper and hoisted form state"

**Acceptance criteria:**
- [ ] Switching tabs no longer re-fetches models/backends and preserves selections
- [ ] All three tabs use `submit_bench_job`

---

### Task 4: Consistency pass + dead code removal + utils tests

**Context:**
Final sweep: dedup types, adopt existing utilities, unify formatting, delete dead code, and add unit tests for the pure helpers.

**Files:**
- Modify: `crates/tama/src/pages/benchmarks/types.rs`, `utils.rs`, `mod.rs`, `mtp_bench.rs`, `spec_bench.rs`, `llama_bench.rs`
- Modify: `crates/tama/src/api/benchmarks/mod.rs` (only if moving `BenchmarkHistoryEntry` — see below)

**What to implement:**
1. **Type dedup:** `HistoryEntry` (types.rs:63-83) duplicates `BenchmarkHistoryEntry` (`api/benchmarks/mod.rs:131-149`) field-for-field in the same crate. Keep ONE — the api module's type — and import it in the frontend. **First add `Deserialize` to `BenchmarkHistoryEntry`** (it currently derives only `Debug, Serialize`; the frontend deserializes history JSON into it).
2. **Dead code:** remove `BenchmarkFormState::model_refresh` (utils.rs:61-62, `#[expect(dead_code)]`) and its Effect dependency; remove the redundant `&str` draft_max field from `SPEC_BENCH_PRESETS` (types.rs:154-158) and consolidate with the `SpecPreset` system in spec_bench.rs:37-94 (keep one preset mechanism); remove `"mtp_sweep"` from `BENCHMARK_TYPES` (types.rs:6-15) if nothing renders it; fix duplicated doc comment lines (types.rs:17-20).
3. **Consistency:** replace all ~33 hand-rolled `e.target().unwrap().dyn_into::<web_sys::Html*Element>().unwrap().value()` chains with `crate::utils::target_value(&ev)` (exists at `crates/tama/src/utils/mod.rs:162`); route all t/s formatting through `format_mean_stddev` (fix mtp_bench.rs:478's raw `format!("{:.1}", ..)`); replace `view! { <div></div> }.into_any()` empties with `().into_view()`.
4. **Extract + test pure helpers:** extract the `parse_threads` closure (mod.rs:210) into `pub fn parse_threads(s: &str) -> Option<Vec<u32>>` in utils.rs (it's already pure — no leptos/web_sys deps), then add `#[cfg(test)]` unit tests in utils.rs for `parse_sizes`, `split_id_quant`, `split_name_variant`, and `parse_threads`. Pin CURRENT behavior: `parse_threads` maps unparseable entries via `unwrap_or(0)` then drops them with `.filter(|v| *v > 0)`; `"auto"`/empty → `None`. Tests run fine natively (`cargo nextest run --package tama -- utils`, default ssr feature — the crate has existing `#[test]` modules), but do NOT invoke `format_timestamp`/`format_relative` in tests — they use `js_sys` and panic off-WASM.

**Steps:**
- [ ] Write utils tests first (after extracting `parse_threads`); run `cargo nextest run --package tama -- utils` — they should PASS (pinning existing behavior).
- [ ] Apply items 1-3 mechanically; `cargo check --package tama` after each item.
- [ ] Run full gate: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo clippy --package tama --features ssr --all-targets -- -D warnings && cargo nextest run --workspace`
- [ ] Commit: "refactor: benchmarks frontend consistency pass and dead code removal"

**Acceptance criteria:**
- [ ] One HistoryEntry type; zero `#[expect(dead_code)]` in benchmarks module
- [ ] No hand-rolled dyn_into event extraction remains in benchmarks pages
- [ ] utils parsing helpers have unit tests
