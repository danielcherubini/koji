# Structured vLLM Config Plan

**Goal:** Persist vLLM/safetensors settings as a first-class typed field on `ModelConfig` (single `vllm_config` JSON column, mirroring the `spec_decoding` precedent) instead of smuggling them inside the free-form `args` string — so the Extra Args textarea only ever holds flags with no dedicated field, and the form fields stop duplicating args.

**Architecture:** Plan-081 introduced vLLM editor fields that sync bidirectionally with `ModelConfig.args` (parse/strip helpers in the frontend). That made the args string both storage and UI, so managed flags appear as "dupes" in Extra Args. This plan replaces that mechanism: (1) one nullable `vllm_config` TEXT column holding a JSON `VllmConfig` (exactly how `spec_decoding` / `sampling` / `modalities` already work), (2) `build_full_args` emits vLLM CLI flags from the typed config for transformers-format models, (3) a startup backfill (triggered from `proxy/server/mod.rs`, like `backfill_hf_metadata`) parses existing vLLM flags out of `args` into the column and strips them from `args` (self-heals model 1350 and any other safetensors models), (4) the editor binds form fields directly to the typed config and filters managed flags out of the Extra Args textarea.

**Tech Stack:** Rust (tama-core, tama SSR/CSR frontend), rusqlite migrations, serde JSON column, Leptos, cargo-nextest.

**Key precedent to mirror:** `spec_decoding` — a typed `SpecDecodingConfig` struct in `crates/tama-core/src/config/types/model.rs`, stored as one JSON TEXT column, surfaced as `Option<String>` raw JSON on `ModelConfigRecord`, emitted via `serde_json::to_value` in `model_entry_json`, merged with preserve-on-null PATCH semantics, and mirrored as a typed `SpecDecodingForm` in the editor. Every step of this plan has a `spec_decoding` analogue to copy — EXCEPT where this plan explicitly says otherwise (backfill, `build_full_args` emission, and the Extra Args filter have no spec_decoding analogue).

**Column/type decisions (already made):**
- ONE column `vllm_config TEXT DEFAULT NULL` — not 8 separate columns. Codebase convention for structured sub-configs is a single JSON column (`spec_decoding`, `modalities`, `sampling`, `health_check` all do this).
- Core struct name: `VllmConfig` in `crates/tama-core/src/config/types/model.rs` (next to `SpecDecodingConfig` ~line 388). Frontend mirror: the existing `VllmSettings` struct in `crates/tama/src/pages/model_editor/vllm_form.rs` (move to `types.rs` in Task 4). The frontend WASM build CANNOT use tama-core types (tama-core is an optional dep gated on the `ssr` feature) — mirroring is mandatory, see the `BackendOption` cfg pattern in `types.rs`.
- Field names (serde keys, identical core + frontend, snake_case — do NOT copy SpecDecodingConfig's `rename_all = "camelCase"`; the existing frontend `VllmSettings` uses snake_case and the two must match): `quantization`, `kv_cache_dtype`, `tensor_parallel_size`, `gpu_memory_utilization`, `max_model_len`, `max_num_batched_tokens` (all `Option<_>`), `enable_prefix_caching`, `trust_remote_code` (`bool`, default false).

---

### Task 1: `vllm_config` column + `VllmConfig` core type + DB plumbing

**Context:** All model settings live on `ModelConfig` (tama-core) and round-trip through `ModelConfigRecord` (raw DB row with positional column lists). Adding a typed sub-config requires touching the migration registry (including `LATEST_VERSION`), the record struct + its `COLUMNS`/`INSERT_COLUMNS` consts + positional `from_row`, the upsert column lists, and `to_db_record`/`from_db_record`. `spec_decoding` did exactly this; follow it field-for-field. Adding fields to `ModelConfigRecord` and `ModelConfig` breaks every exhaustive struct literal in the workspace — run `cargo check --workspace --all-targets` to enumerate them (verified sites: `db/repository.rs` test helper, `db/backfill/hf_metadata.rs` ×2, `db/backfill/initial_backfill.rs` ×2, `db/queries/tests.rs` ×4, `models/manager_tests.rs`, `api/models/info.rs`, `crates/tama/src/types/config/core_conv.rs` ~line 465 — the lossy `From` conversion, add `vllm: Default::default()` with a doc comment like spec_decoding's — `proxy/tama_handlers/pull/verify.rs` ×2 at lines 419 and 516, `crates/tama/src/api/models/crud/tests.rs` ×33 literals, `crates/tama/src/api/aliases/mod.rs` ×2 at lines 373/498, `crates/tama-core/src/updates/checker/orchestration_tests.rs` ×2 at lines 29/254, and `crates/tama-core/src/bench/llama_bench/mod.rs` ~line 469). NOTE: `config/types/model_tests.rs` will NOT break — its `ModelConfigRecord` literals use `..Default::default()` (the record derives `Default`); do not touch them.

**Files:**
- Create: `crates/tama-core/src/db/migrations/_0044_add_vllm_config.rs`
- Modify: `crates/tama-core/src/db/migrations.rs` (register: `mod _0044_add_vllm_config;` + `_0044_add_vllm_config::MIGRATION` in the `MIGRATIONS` const — copy the `_0043` lines; **bump `pub const LATEST_VERSION: i32 = 43;` → `44`** at ~line 84 — `migrations_tests.rs:27` and `db/mod.rs:231` assert it)
- Modify: `crates/tama-core/src/config/types/model.rs` (`VllmConfig` struct + `ModelConfig.vllm` field + `to_db_record`/`from_db_record`)
- Modify: `crates/tama-core/src/config/mod.rs` (add `VllmConfig` to the explicit `pub use types::{...}` re-export list — it is NOT glob-exported)
- Modify: `crates/tama-core/src/db/queries/types.rs` (`ModelConfigRecord.vllm_config: Option<String>` — append at END of struct; update the "All 37 columns" doc table to 38, index 37; the `pub(crate) const COLUMNS` (~line 87), `INSERT_COLUMNS` (~line 98), and positional `from_row` (~line 109) all live HERE on `impl ModelConfigRecord` — append `vllm_config` last in each)
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs` (upsert SQL/params only — the `spec_decoding = excluded.spec_decoding` line is ~51; add the vllm analogue to the INSERT column list + `?` placeholders + UPDATE assignment list + params vec. Also update `test_model_config_columns_match_insert_columns` which asserts hard-coded counts at ~lines 166–167: `select.len() == 37` / `insert.len() == 36` → 38/37)
- Modify: all broken exhaustive literals (see list above)

**What to implement:**
1. Migration `_0044_add_vllm_config.rs`, copying the `_0043` shape exactly:
   ```rust
   pub const MIGRATION: (i32, bool, &str) = (
       44,
       false, // does not require FKs off
       r#"
   ALTER TABLE model_configs ADD COLUMN vllm_config TEXT DEFAULT NULL;
   "#,
   );
   ```
   Plus a `#[cfg(test)]` test mirroring `_0043`'s: `run_up_to(&conn, 43)`, assert column absent, `run_up_to(&conn, 44)`, assert present.
2. `VllmConfig` in `config/types/model.rs` (place next to `SpecDecodingConfig`):
   ```rust
   /// vLLM-specific launch settings for transformers-format (safetensors) models.
   /// Persisted as JSON in the `vllm_config` column; mirrors `spec_decoding`.
   #[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
   pub struct VllmConfig {
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub quantization: Option<String>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub kv_cache_dtype: Option<String>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub tensor_parallel_size: Option<u32>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub gpu_memory_utilization: Option<f64>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub max_model_len: Option<u32>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub max_num_batched_tokens: Option<u32>,
       #[serde(default)]
       pub enable_prefix_caching: bool,
       #[serde(default)]
       pub trust_remote_code: bool,
   }
   ```
   Also add `pub fn is_empty(&self) -> bool` (true when all Options are None and both bools false) and `pub fn to_args(&self) -> Vec<String>` returning grouped entries in this fixed order: `--quantization v`, `--kv-cache-dtype v`, `--tensor-parallel-size v`, `--gpu-memory-utilization v`, `--max-model-len v`, `--max-num-batched-tokens v`, `--enable-prefix-caching`, `--trust-remote-code` (skip None/false). `to_args` is used by Task 2.
3. `ModelConfig`: add `#[serde(default)] pub vllm: VllmConfig` immediately after the `spec_decoding` field, copying that field's serde attributes exactly.
4. `to_db_record`: add `vllm_config: serde_json::to_string(&self.vllm).ok(),` (mirror the `spec_decoding` line ~285 — yes, this serializes even an empty config to `"{}"`; that is fine and matches spec_decoding — the Task-3 backfill does NOT gate on NULL). `from_db_record`: parse with fallback (mirror the `spec_decoding` block ~374): `record.vllm_config.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()`.
5. `ModelConfigRecord`: append `pub vllm_config: Option<String>, // raw JSON string` as the LAST field (index 37, after `n_ubatch`). Keep SELECT order == `from_row` indices == struct doc table == `COLUMNS` const order.
6. Do NOT change any GGUF/llama.cpp behavior, do NOT touch the frontend in this task (except `core_conv.rs`'s literal, which needs `vllm: Default::default()` to compile).

**Steps:**
- [ ] Write the migration test in `_0044_add_vllm_config.rs` first. Run `cargo nextest run --package tama-core -- db::migrations` — did it fail (migration unregistered/missing)?
- [ ] Register the migration + bump `LATEST_VERSION`; re-run — migration tests pass?
- [ ] Add a failing round-trip test (in `config/types/model.rs` tests or `db/repository.rs` tests): build a `ModelConfig` with `vllm.quantization = Some("fp8")`, `enable_prefix_caching = true` → `to_db_record` → upsert via `queries::upsert_model_config` on an in-memory DB → read back → `from_db_record` → assert the vllm fields survived.
- [ ] Run `cargo nextest run --package tama-core -- db` — did it fail (column missing)?
- [ ] Implement the record/queries/ModelConfig plumbing; fix all broken literals (`cargo check --workspace --all-targets` + `cargo check --package tama --features ssr --all-targets` to enumerate).
- [ ] Update `test_model_config_columns_match_insert_columns` counts (38/37).
- [ ] Run `cargo nextest run --package tama-core` — all pass?
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: add vllm_config column and VllmConfig type`

**Acceptance criteria:**
- [ ] Migration 44 applies cleanly on top of 43, `LATEST_VERSION` bumped, migration tests pass
- [ ] `ModelConfig.vllm` round-trips through the DB losslessly
- [ ] Full tama-core suite green, clippy clean

---

### Task 2: Emit vLLM flags from `vllm` config in `build_full_args`

**Context:** Plan-081 made `build_full_args` (`crates/tama-core/src/config/resolve/mod.rs`) emit the positional model path for transformers models and gate llama.cpp-only flags. Now the typed config (Task 1) must reach the command line. **Read the current function first and do not assume the layering:** `server.args` is merged FIRST (`let mut grouped = merge_args(default_args, &server.args)` ~line 227) — there is NO final merge with user args. Later injections follow two conventions: (a) per-flag `already_has_X` checks where user args win (`-m`, `-c`, `-ngl`), and (b) `merge_args(&grouped, &[flag])` where the typed field wins (`-b`, `-ub`, sampling ~line 563).

**Decision (made):** use convention (b) — `grouped = merge_args(&grouped, &server.vllm.to_args())` — typed config wins on collision, consistent with `-b`/`-ub`/sampling. After the Task-3 backfill, managed flags no longer live in `args`, so collisions should not occur in practice; on collision the column (what the editor fields edit) must win, otherwise the UI would appear broken.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs` (`build_full_args` transformers branch)
- Test: `crates/tama-core/src/config/resolve/tests/transformers_format.rs`

**What to implement:**
1. In `build_full_args`, inside the existing `is_transformers` region (the same branch that inserts the positional model path, added by plan-081), add `grouped = merge_args(&grouped, &server.vllm.to_args());` AFTER the positional-path block, guarded by `!server.vllm.is_empty()`. Place it in the same area as the other typed injections (near the sampling merge), NOT before the `server.args` merge at line 227.
2. Gate strictly on the transformers branch — a GGUF model with a non-empty `vllm` config must NOT get these flags.
3. Do NOT remove the positional path logic, the `already_has_positional` dedup, or any llama.cpp gating from plan-081. Do NOT reorder any existing statements.
4. Update the `build_full_args` doc comment (~lines 207–218) which documents the 4-layer merge order ("1. default_args … 4. sampling") — add the vllm layer to the list.

**Steps:**
- [ ] Write failing tests in `transformers_format.rs` (use the existing `test_helpers` there — `h::sample_config`/`sample_server`/`sample_backend`):
  - transformers + `vllm { quantization: Some("fp8"), max_model_len: Some(32768), enable_prefix_caching: true }` → args contain `--quantization fp8`, `--max-model-len 32768`, `--enable-prefix-caching`, and NOT `--trust-remote-code` / unset fields
  - transformers + empty `vllm` → no vLLM flags (existing positional-path tests keep passing)
  - GGUF + non-empty `vllm` → NO vLLM flags in output
  - user args containing `--quantization awq` + column `quantization: Some("fp8")` → only ONE `--quantization` in output and it is `fp8` (typed config wins)
  - Run `cargo nextest run --package tama-core -- config::resolve` — did they fail?
- [ ] Implement the emission; re-run — all pass?
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: emit vLLM flags from vllm config in build_full_args`

**Acceptance criteria:**
- [ ] Transformers models get vLLM flags from the typed config at spawn; typed config wins on collision
- [ ] GGUF models never get vLLM flags; all plan-081 behavior preserved
- [ ] Tests pass, clippy clean

---

### Task 3: Startup backfill — migrate vLLM flags out of `args` into `vllm_config`

**Context:** Existing safetensors models (e.g. DB id 1350, `Qwen/Qwen3.6-27B-FP8`) carry their vLLM flags inside `args` — possibly flattened one-token-per-line (`"--quantization"`, `"fp8"` as separate Vec entries) or grouped (`"--quantization fp8"`). After Tasks 1–2 those flags would be duplicated: once in `args`, once in the column. This backfill moves the 8 managed flags into `vllm_config` and strips them from `args` — leaving only true extras like `--attention-backend ROCM_AITER_UNIFIED_ATTN`.

**CRITICAL — trigger site:** `crates/tama-core/src/db/backfill/mod.rs` contains ONLY `mod` declarations and `pub use` re-exports — registering a function there RUNS NOTHING. The trigger for the closest analogue (`backfill_hf_metadata`) lives in `crates/tama-core/src/proxy/server/mod.rs` (~lines 66–86): a `SELECT COUNT(*) ...` gate + fire-and-forget `tokio::spawn`. Mirror THAT. Do NOT use `run_initial_backfill` in `crates/tama/src/main.rs` (~lines 68–90) — it is gated on `db_result.needs_backfill` (first-run-only flag) and would never fire on existing databases like the one hosting model 1350. This backfill is pure-DB (no network, unlike hf_metadata), so it MAY run synchronously at startup instead of spawned — state which you chose in a comment and why (synchronous is fine: it is a fast single pass over `model_configs`).

**Gate condition (decided):** gate purely on "args contains a managed flag" — NOT on `vllm_config IS NULL` (because `to_db_record` always serializes, so post-Task-1 upserts write `"{}"` and a NULL gate would permanently skip rows saved between deploy and backfill, and rows given managed flags later via raw API PUTs). When a row already has a non-empty `vllm_config`: existing column values WIN per-field (a human set them); extracted values only fill fields that are `None`/`false` in the existing config.

**Files:**
- Create: `crates/tama-core/src/config/vllm_args.rs` — shared parser: `pub fn extract_vllm_args(args: &[String]) -> (VllmConfig, Vec<String>)`
- Modify: `crates/tama-core/src/config/mod.rs` (`mod vllm_args;` + `pub use vllm_args::extract_vllm_args;` — existing submodules are private with selective re-exports; match that)
- Create: `crates/tama-core/src/db/backfill/vllm_config.rs`
- Modify: `crates/tama-core/src/db/backfill/mod.rs` (`mod vllm_config;` + `pub use`)
- Modify: `crates/tama-core/src/proxy/server/mod.rs` (trigger — mirror the `backfill_hf_metadata` block ~lines 66–86)

**What to implement:**
1. `extract_vllm_args(args: &[String]) -> (VllmConfig, Vec<String>)` in `config/vllm_args.rs`. Input is the stored `Vec<String>` where each entry is one "line" in either grouped or flattened form. Walk entries with an index and classify each (trimmed) entry:
   - `--flag=value` where flag is managed & non-boolean → parse value, drop entry.
   - Exact managed flag name → boolean flags set true and drop; value flags consume the NEXT entry as the value if it is non-empty and does not start with `--` (flattened form), then drop both. If no valid next entry, keep the bare flag entry (unparseable — preserve, do not silently drop user data).
   - Starts with `--flag ` (grouped, value in same entry) → parse value, drop entry.
   - Anything else → keep in the returned args Vec, order preserved. Comment lines (starting with `#`) are kept verbatim, matching the frontend stripper in Task 5.
   - Managed flag set: `--quantization`, `--kv-cache-dtype`, `--tensor-parallel-size`, `--gpu-memory-utilization`, `--max-model-len`, `--max-num-batched-tokens`, `--enable-prefix-caching`, `--trust-remote-code`. Value parsing: u32 for tensor-parallel-size/max-model-len/max-num-batched-tokens, f64 for gpu-memory-utilization, string for the rest (reject empty or whitespace-containing string values → keep entry instead). If a numeric value fails to parse → keep the entry (and its value entry) untouched.
2. Backfill `vllm_config.rs`: for every `model_configs` row whose `args` contains at least one managed flag: deserialize `args` (JSON Vec<String>), run `extract_vllm_args`; if the extracted config is non-empty: merge into any existing `vllm_config` (existing non-default fields win per-field), write the merged `vllm_config` JSON and the stripped `args` back. Rows with no managed flags are untouched. Log one `tracing::info!` line per migrated model.
3. Trigger in `proxy/server/mod.rs`: mirror the hf_metadata block — a cheap count query (e.g. rows whose `args LIKE '%--quantization%'` OR the other managed flags — or simply scan all rows in Rust, the table is small) gating the backfill call. Comment that this is intentionally NOT in `run_initial_backfill` because that is first-run-only.

**Steps:**
- [ ] Write failing unit tests for `extract_vllm_args`: grouped form, `--flag=value` form, flattened form (the exact model-1350 shape: `["--quantization fp8", "--kv-cache-dtype fp8", "--tensor-parallel-size 2", "--gpu-memory-utilization 0.92", "--attention-backend ROCM_AITER_UNIFIED_ATTN", "--max-num-batched-tokens 2560", "--enable-prefix-caching"]` → config populated, args reduced to `["--attention-backend ROCM_AITER_UNIFIED_ATTN"]`), unparseable numeric preserved, flag-followed-by-flag not eaten, unmanaged args order preserved.
  - Run `cargo nextest run --package tama-core -- config::vllm_args` — did they fail?
- [ ] Implement the parser; re-run — pass?
- [ ] Write a failing backfill test (in-memory DB): insert a row with grouped vLLM args, run the backfill fn, assert `vllm_config` JSON has the values and `args` is stripped; assert a row without managed flags is untouched; assert a row with existing `vllm_config` keeps its column values on conflict.
- [ ] Implement + register the backfill + add the trigger in `proxy/server/mod.rs`. Run `cargo nextest run --package tama-core -- db::backfill` — pass?
- [ ] Run full gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`
- [ ] Commit: `feat: backfill vllm_config from args at startup`

**Acceptance criteria:**
- [ ] The backfill actually RUNS at proxy startup (trigger in `proxy/server/mod.rs`, not just registration) — verify by the `tracing::info!` line on a DB with managed flags
- [ ] Model 1350's args self-heal to `["--attention-backend ROCM_AITER_UNIFIED_ATTN"]` on server start
- [ ] Rows without managed flags untouched; unparseable values preserved; existing column values win on conflict
- [ ] Workspace tests + clippy green

---

### Task 4: API + editor DTO plumbing for `vllm`

**Context:** The web API and the Leptos editor need the typed config end-to-end. Mirror `spec_decoding` at every hop — but note there are FIVE spec_decoding sites in `crud/mod.rs`, not two. After this task the API round-trips `vllm`; the editor UI is rewired in Task 5.

**Files:**
- Modify: `crates/tama/src/api/models/info.rs` (`model_entry_json` ~line 183 next to `spec_decoding`: `"vllm": serde_json::to_value(&m.vllm).unwrap_or_default(),`; also the test literal at ~line 407)
- Modify: `crates/tama/src/api/models/crud/mod.rs` — ALL of these, each mirroring the adjacent spec_decoding line EXACTLY (including its idiom — note the merge idiom is `.unwrap_or(existing)` / `.unwrap_or_else(...)`, NOT `.or(...)`, and the extracted existing value is a `SpecDecodingConfig` not an Option in some paths):
  - `ModelBody` struct (~line 70): `pub vllm: Option<tama_core::config::VllmConfig>,`
  - `ModelPatchBody` struct (~line 105): same field
  - `apply_model_patch`: extract existing (~line 116) + merge (~line 181)
  - `apply_model_body`: extract existing (~line 193) + base-literal default (~line 231) + final merge (~lines 306–308) — do NOT satisfy the compiler here with `Default::default()`; that silently breaks PUT preserve-semantics
- Modify: `crates/tama/src/pages/model_editor/types.rs` — move `VllmSettings` here from `vllm_form.rs` (keep the name; it is now the form mirror, like `SpecDecodingForm`); add `#[serde(default)] pub vllm: Option<serde_json::Value>` to `ModelDetail` (mirror its `spec_decoding` field ~line 102) and `pub vllm: VllmSettings` to `ModelForm` (mirror ~line 176); fix the literals at ~320 (the ~371 site is a `serde_json::json!` macro — add `"vllm": null` there for symmetry but it will not fail to compile)
- Modify: `crates/tama/src/pages/model_editor/api.rs` — `fetch_model` "new" branch literal (~lines 28–62, add `vllm: None`); save body `json!` (~line 151, next to `"spec_decoding": form.spec_decoding,`): `"vllm": form.vllm,`
- Modify: `crates/tama/src/pages/model_editor/mod.rs` — populate Effect: parse `d.vllm` into the form's typed field the same way `d.spec_decoding: Option<serde_json::Value>` becomes `SpecDecodingForm` (~lines 246–251 — copy that pattern); ALSO the exhaustive `form_data` literal in the save action (~lines 500–527): add `vllm: initial_form.vllm.clone(),`
- Modify: `crates/tama/src/api/openapi.rs` (~line 835 documents the model body schema with `"spec_decoding": {"type": ["object", "null"]}` — add the `vllm` analogue) and `docs/api/models.md` (spec_decoding documented at lines 53, 115, 151 — add `vllm` beside each)
- Test: `crates/tama/src/pages/model_editor/types.rs` `#[cfg(test)]` — extend the existing round-trip tests

**What to implement:**
1. Everywhere `spec_decoding` appears in the files above, add the `vllm` analogue directly beside it. Same serde attributes, same merge semantics, same parse-fallback behavior, same idioms as the adjacent line (do not invent new ones).
2. `VllmSettings` moves from `vllm_form.rs` to `types.rs` unchanged (fix imports in `vllm_form.rs`, `settings_form.rs`, `hardware_form.rs`, `advanced_form.rs`, `mod.rs`).
3. PATCH semantics: `vllm: null` in a PATCH body preserves the stored value (falls out of `Option<...>` + the merge idiom — add a test if a PATCH test module exists near the spec_decoding PATCH tests).

**Steps:**
- [ ] Add a failing round-trip test in `types.rs`: `ModelForm` with `vllm.tensor_parallel_size = Some(2)` serializes → deserializes losslessly; `ModelDetail` JSON without `vllm` parses to `None`.
  - Run `cargo nextest run --package tama -- pages::model_editor` — did it fail?
- [ ] Implement all plumbing; fix compile errors (`cargo check --package tama` and `--features ssr`).
- [ ] Run `cargo nextest run --package tama` — all pass?
- [ ] Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: `feat: plumb vllm config through API and editor DTOs`

**Acceptance criteria:**
- [ ] `GET /tama/v1/models/:id` emits `vllm`; PUT/PATCH accept and persist it; PUT/PATCH preserve-on-null semantics verified
- [ ] `ModelForm` carries typed `vllm`; missing field deserializes to default
- [ ] OpenAPI schema + `docs/api/models.md` document `vllm`
- [ ] Tests pass; both clippy gates clean

---

### Task 5: Editor binds to typed config; Extra Args hides managed flags

**Context:** The final rewiring. Today `mod.rs` owns a `vllm_settings: RwSignal<VllmSettings>` signal (~line 82) plus two guarded sync Effects (~lines 84–116) and an eager args-normalization block in the populate Effect (~lines 286–300) — all of which exists only because args was the storage. With the typed column, the form fields bind directly to `form.vllm`, the sync machinery is deleted, and the Extra Args textarea shows only unmanaged flags. The `is_transformers` tab gating (Sampling/Files hidden, format-conditional fields in Settings/Context/Advanced) stays exactly as-is.

**`strip_managed_flags` DOES NOT EXIST YET** — the stripping logic is currently inline as Step 1 of `vllm_form_to_args` (`vllm_form.rs` ~lines 140–160, driven by the private `classify_managed_line`). This task must EXTRACT it into `pub fn strip_managed_flags(existing: &str) -> String` (operating on the newline-joined form-args string, reusing `classify_managed_line`) and then delete the rest of `vllm_form_to_args`.

**Storage/display invariant (decided):** `save_model` serializes `form.args` (mod.rs ~line 493), NOT the textarea DOM. Therefore the populate Effect must strip `form.args` itself for transformers models (`f.args = strip_managed_flags(&f.args)`, in the same place the old eager-normalization block ran) — otherwise un-backfilled managed flags would ride along invisibly in `form.args` on every save while being hidden from the user. After populate-time stripping, display and storage cannot diverge.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/mod.rs` — delete the `vllm_settings` signal, both sync Effects, and the eager-normalization block; in the populate Effect set `vllm` from the detail (Task 4) AND strip `form.args` for transformers (above); stop passing the `vllm_settings` prop to the three forms (call sites ~lines 867, 876, 918); fix `vllm_form` imports (keep `strip_managed_flags`). KEEP the Sampling/Files active-tab reset effect.
- Modify: `crates/tama/src/pages/model_editor/settings_form.rs` — drop the `vllm_settings` prop; the three transformers fields read/write `form.update(|f| f.vllm.quantization = ...)` etc.; init effect reads `f.vllm.*`
- Modify: `crates/tama/src/pages/model_editor/hardware_form.rs` — same for max_model_len / kv_cache_dtype / max_num_batched_tokens
- Modify: `crates/tama/src/pages/model_editor/advanced_form.rs` — same for the two checkboxes (init ~line 73, textarea ~line 270); **Extra Args textarea**: for transformers models display `strip_managed_flags(&f.args)` (init effect) and on input store `strip_managed_flags(&raw)` — managed flags pasted into the textarea are dropped; add a `form-hint` for transformers: "vLLM flags like --quantization are set via the form fields"
- Modify: `crates/tama/src/pages/model_editor/vllm_form.rs` — extract `strip_managed_flags` (see above); delete `args_to_vllm_form`, `vllm_form_to_args`, `parse_managed_flag`, `can_parse_managed_value`, `build_vllm_flags`; keep `MANAGED_FLAGS`, `classify_managed_line`, `is_boolean_flag` (used by strip); delete tests of removed functions; keep/add `strip_managed_flags` tests (grouped, `=`-form, flattened pair removal, unparseable preserved, unmanaged order preserved)

**What to implement:**
1. Bind every vLLM form field directly to `form.vllm` (typed `VllmSettings` on `ModelForm` from Task 4). Number inputs keep the imperative `set_input_value` init pattern (keyed on model id) already in those forms — just source the values from `f.vllm.*`.
2. Delete ALL args-sync machinery from `mod.rs`. The populate Effect becomes: set form fields (including typed `vllm`), strip `form.args` for transformers, seed `last_saved_form`, done.
3. Extra Args filtering: display + store only unmanaged args for transformers. GGUF behavior untouched (textarea shows/stores `form.args` verbatim).
4. No DB, API, or arg-building changes in this task.

**Steps:**
- [ ] Update `vllm_form.rs` first (extract `strip_managed_flags`, remove dead helpers, fix tests). Run `cargo nextest run --package tama -- pages::model_editor` — remove/replace tests referencing deleted fns.
- [ ] Rewire `mod.rs` + the three forms. `cargo check --package tama` until clean.
- [ ] Add/adjust pure tests: `strip_managed_flags` cases above.
- [ ] Run `cargo nextest run --package tama` — all pass?
- [ ] Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: `feat: bind vLLM editor fields to typed config, filter Extra Args`

**Acceptance criteria:**
- [ ] No args⇄settings sync effects remain; no `args_to_vllm_form`/`vllm_form_to_args` anywhere; `strip_managed_flags` extracted and tested
- [ ] Form fields edit `form.vllm` and persist via the save body
- [ ] Populate strips managed flags from `form.args` for transformers (storage/display cannot diverge)
- [ ] Extra Args for transformers shows only unmanaged flags (e.g. just `--attention-backend ROCM_AITER_UNIFIED_ATTN` for model 1350); GGUF textarea unchanged
- [ ] Tab gating unchanged; tests + both clippy gates green

---

### Task 6: Backup/restore carries `vllm_config`

**Context:** `crates/tama-core/src/backup/merge.rs` (~lines 156–167) restores `model_configs` via `INSERT OR IGNORE ... (explicit ~32-column list) SELECT ... FROM backup_db.model_configs`. The list includes `spec_decoding` but not the new column — restores would silently drop `vllm_config` (exactly the data this plan creates). Naively appending the column breaks restoring PRE-44 backups (their `model_configs` lacks it). Note: `n_batch`/`n_ubatch` are already missing from this list — a pre-existing gap; do NOT fix that here (out of scope), but do not make it worse.

**Decision (made):** compute the restore column list as the intersection of the hard-coded list (with `vllm_config` added) and the backup DB's actual `model_configs` columns (query `pragma_table_info('model_configs')` on the attached `backup_db` before building the SQL). This adds `vllm_config` for new backups while keeping pre-44 backups restorable.

**Files:**
- Modify: `crates/tama-core/src/backup/merge.rs`
- Test: same file's `#[cfg(test)]` module (or wherever merge tests live — read the file)

**What to implement:**
1. Add `vllm_config` to the hard-coded `model_configs` column list.
2. Before building the INSERT…SELECT, query the attached backup DB's `pragma_table_info('model_configs')` and filter the column list to columns present in BOTH (the local DB is guaranteed to have all of them post-migration). Build the SQL from the filtered list.
3. Regression test: create a backup-shaped SQLite WITHOUT `vllm_config` (pre-44) and one WITH it (with a non-null value) → restore both → assert the first succeeds (column defaults NULL) and the second preserves the value.

**Steps:**
- [ ] Write the failing restore tests (both shapes). Run `cargo nextest run --package tama-core -- backup` — did they fail?
- [ ] Implement the intersection logic; re-run — pass?
- [ ] Run full gate: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`
- [ ] Commit: `feat: carry vllm_config through backup/restore`

**Acceptance criteria:**
- [ ] Restores of post-44 backups preserve `vllm_config`; restores of pre-44 backups still work
- [ ] Workspace tests + clippy green

---

## Cross-Cutting Notes

- **Single source of truth:** after this plan, `ModelConfig.vllm` owns the 8 vLLM settings; `args` holds only free-form extras; `build_full_args` combines them at spawn (typed config wins on collision).
- **Backward compatibility:** old rows have `vllm_config = NULL` → `VllmConfig::default()`; the backfill (Task 3) migrates flags out of `args` on first proxy startup after deploy. No manual steps.
- **Model 1350** (`Qwen/Qwen3.6-27B-FP8`) is the canonical test case: args currently `["--quantization fp8", "--kv-cache-dtype fp8", "--tensor-parallel-size 2", "--gpu-memory-utilization 0.92", "--attention-backend ROCM_AITER_UNIFIED_ATTN", "--max-num-batched-tokens 2560", "--enable-prefix-caching"]` → after backfill, args = `["--attention-backend ROCM_AITER_UNIFIED_ATTN"]` and the column carries the rest. Verify with `curl -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/tama/v1/models/1350"` after deploy.
- **GGUF safety:** every change is gated on `hf_format == "transformers"` or is additive-only; GGUF pull/editor/spawn paths must be untouched (regression tests exist in `config/resolve/tests/transformers_format.rs`).
- **Validation gate (every task):** `cargo fmt --all` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo clippy --package tama --features ssr --all-targets -- -D warnings` → targeted `cargo nextest run`; full `cargo nextest run --workspace` before the final commit of Tasks 3, 5, and 6.
- **After merge/deploy:** run `ssh root@tama update-tama`, then confirm model 1350's `args`/`vllm` via the API and open its editor page to visually confirm the fields are populated and Extra Args shows only `--attention-backend ROCM_AITER_UNIFIED_ATTN`.
