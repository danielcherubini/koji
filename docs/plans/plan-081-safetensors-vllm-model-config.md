# Safetensors / vLLM Model Config Support Plan

**Goal:** Let Tama natively model, configure, and load **safetensors (transformers) models** served by vLLM-style docker backends, distinct from the GGUF models it currently targets. Model weights are pulled to `models_dir/<org>/<repo>` via HF CLI (out of scope for in-Tama download); this plan covers format detection, editor UX, and correct arg building.

**Architecture:** Reuse the already-existent `ModelConfig.hf_format` field (`"gguf"` / `"transformers"`) as the single source of truth for format. (1) Correctly detect and set `hf_format` at pull time by inspecting the repo's `siblings` file listing. (2) Expose `hf_format` in the model-editor DTO and drive a **format-specific editor UI** — GGUF models keep the current quant/files forms; safetensors models get a vLLM-oriented form (quantization, kv-cache-dtype, tensor-parallel, gpu-memory-utilization, max-model-len, FP8 flags). (3) Make `build_full_args` emit the safetensors model **path** as a positional arg (not the GGUF-only `-m quant_file`), skip GGUF-only flag injection for transformers format, and gate the llama.cpp-only flags (`-c`/`-np`/`-b`/`-ub`/`-ngl`).

**Tech Stack:** Rust (tama-core, tama SSR/CSR frontend), Leptos, serde, SQLite (rusqlite), HuggingFace API.

---

### Task 1: Detect and persist `hf_format` at pull time

**Context:** `ModelConfig.hf_format` already exists and is stored in the DB (`model_configs.hf_format`), and the model editor / detail DTO already carries HF metadata fields. However, the pull flow **hardcodes** `hf_format = Some("gguf")` regardless of the actual repo (`crates/tama-core/src/models/pull/api.rs:164`), so a safetensors repo is mislabeled GGUF. This task fixes detection so downstream (editor, arg building) can trust `hf_format`.

**Files:**
- Modify: `crates/tama-core/src/models/pull/api.rs`
- Modify: `crates/tama-core/src/models/pull/mod.rs` (if `HfModelMetadata` / re-exports need a new helper)
- Test: tests inside `pull/api.rs` `#[cfg(test)]`

**What to implement:**
1. In `lookup_hf_metadata` (`crates/tama-core/src/models/pull/api.rs`, ~line 143-152), the HF `GET /api/models/{repo}` response is already fetched and available as the `response: serde_json::Value`. That JSON includes `siblings[].rfilename` for the entire repo. Extract all `rfilename` strings from `response["siblings"]` into a `Vec<String>`.
2. Set `meta.hf_format` by calling a new pure helper `detect_hf_format(&filenames: &[String]) -> &'static str` (returns `"gguf"` / `"transformers"` / `"gguf"` fallback). Replace the hardcoded `hf_format: Some("gguf".to_string())` at `pull/api.rs:164`.
3. `detect_hf_format` rules:
   - If any filename ends with `.safetensors` → `"transformers"`
   - Else if any filename ends with `.gguf` → `"gguf"`
   - Else → `"gguf"` (backward-compatible fallback)
4. **Do NOT call `list_gguf_files` or `parse_blob_siblings` for detection.** `list_gguf_files` auto-appends `-GGUF` to the repo name and returns `Err` for repos with no `.gguf` files (it will either bail on a safetensors-only repo or pull GGUF files from a sibling `-GGUF` repo and mislabel the model). `parse_blob_siblings` filters to `.gguf` files only, so it cannot see safetensors. Instead, read `siblings[].rfilename` directly from the already-fetched `response` (no extra HTTP call).
5. Ensure `hf_format` survives the DB round-trip: it is already persisted via `to_db_record`/`from_db_record` (`ModelConfig.hf_format`), so no schema change is needed.
6. Add `detect_hf_format` to the re-export list in `crates/tama-core/src/models/pull/mod.rs` (`pub use api::{...}`) if other modules or tests need it.
7. Do NOT change any existing GGUF behavior, migration, or backfill logic.

**Steps:**
- [ ] Write a failing unit test for `detect_hf_format`: given filenames containing `.safetensors` → `"transformers"`, containing `.gguf` → `"gguf"`, both → `"gguf"` (GGUF wins), neither → `"gguf"`. Also test extracting `rfilename` values from a `siblings` JSON array.
  - Run: `cargo nextest run --package tama-core -- pull::api` — did it fail (function missing)? If it passed unexpectedly, stop and investigate.
- [ ] Implement `detect_hf_format` in `pull/api.rs`, wire it into `lookup_hf_metadata` by reading `response["siblings"]`, and remove the hardcoded `Some("gguf")`.
- [ ] Run `cargo nextest run --package tama-core -- pull::api` — do all tests pass?
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: detect hf_format (gguf vs transformers) at pull time`

**Acceptance criteria:**
- [ ] `detect_hf_format` returns `"transformers"` for a safetensors-only repo
- [ ] `lookup_hf_metadata` reads `siblings[].rfilename` (no `list_gguf_files`/`parse_blob_siblings`) and sets `hf_format` correctly
- [ ] Existing GGUF pull behavior unchanged
- [ ] All pull API tests pass

---

### Task 2: Expose `hf_format` in the model editor DTO and load model detail

**Context:** The model-editor frontend needs to know whether a model is GGUF or safetensors to render the right form. Currently `ModelDetail` (the editor DTO in `crates/tama/src/pages/model_editor/types.rs`) does not include `hf_format`. The API endpoint that fills the editor (`GET /tama/v1/models/:id`) returns `ModelConfig` fields; `hf_format` is already on `ModelConfig` but must be surfaced into `ModelDetail` so the UI can branch on it.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/types.rs` (`ModelDetail` struct — add `hf_format`)
- Modify: `crates/tama/src/api/models/info.rs` (in `model_entry_json`, add `hf_format` to the JSON; this builder is used by both `list_models` and `get_model`)
- Modify: `crates/tama/src/pages/model_editor/api.rs` (the `ModelDetail` literal in `fetch_model` "new" branch ~line 28; and the detail fetch decode)
- Modify: `crates/tama/src/pages/model_editor/mod.rs` (the `ModelForm` populate `Effect` ~line 203 — thread `hf_format` from `ModelDetail` into `ModelForm`)
- Modify: `crates/tama/src/pages/model_editor/types.rs` (add `hf_format` to `ModelForm` if it is distinct from `ModelDetail`)
- Test: `crates/tama/src/pages/model_editor/types.rs` (create a `#[cfg(test)]` module — none exists yet)

**What to implement:**
1. Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub hf_format: Option<String>` to `ModelDetail` (and `ModelForm` if distinct), for backward compat with old serialized payloads.
2. Add `"hf_format": record.hf_format` to the `serde_json::json!()` block in `model_entry_json` (`crates/tama/src/api/models/info.rs` ~line 119). This function builds the JSON manually field-by-field and currently emits `hf_context_length`, `hf_architecture_type`, `hf_base_model`, etc. but **not** `hf_format` — it must be added explicitly. One edit here covers both `list_models` and `get_model`.
3. Update the two exhaustive struct literals that will otherwise fail to compile:
   - `fetch_model` "new" branch in `crates/tama/src/pages/model_editor/api.rs:28` constructs `ModelDetail { ... }` field-by-field (no `..Default`); add `hf_format: None`.
   - The `ModelForm` populate `Effect` in `crates/tama/src/pages/model_editor/mod.rs:~203` maps `ModelDetail` → `ModelForm`; add `hf_format: d.hf_format`.
4. Create a serialization round-trip test in a new `#[cfg(test)]` module in `crates/tama/src/pages/model_editor/types.rs` (there are currently **no tests in these editor files**): build a `ModelDetail` with `hf_format: Some("transformers")`, serialize → deserialize, assert it round-trips; assert a payload without `hf_format` deserializes to `None`.

**Steps:**
- [ ] Add `hf_format` to `ModelDetail` and `ModelForm`; create the round-trip test module in `types.rs`.
- [ ] Add `"hf_format"` to `model_entry_json` in `crates/tama/src/api/models/info.rs`; extend `test_model_entry_json_includes_hf_fields` if present.
- [ ] Fix the two struct literals (`fetch_model` "new" branch and the `ModelForm` populate Effect).
- [ ] Run `cargo nextest run --package tama --pages::model_editor` and `cargo nextest run --package tama -- api::models` — do all pass?
- [ ] Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: `feat: expose hf_format in model editor DTO`

**Acceptance criteria:**
- [ ] `ModelDetail`/`ModelForm` carry `hf_format`, populated from the API response
- [ ] `model_entry_json` emits `hf_format`
- [ ] Editor DTOs round-trip `hf_format`; missing field → `None`
- [ ] Both struct literals compile; tests pass; clippy clean

---

---

### Task 3: Format-specific model editor UI (GGUF vs safetensors/vLLM)

**Context:** Today the editor is GGUF-shaped: the top of the model editor shows a **pill-style tab navigation** (`.model-editor-pills` in `crates/tama/src/pages/model_editor/mod.rs:721`) with pills for **Settings, Context, Sampling, Files, Advanced**, and the Files form shows quants/mmproj/MTP. A safetensors model has none of those. When `hf_format == "transformers"` (and/or backend is a vLLM docker backend), the editor should (a) show a **GGUF vs safetensors (transformers) format pill** in the editor header so the format is visible at a glance, and (b) render a vLLM-oriented form while hiding GGUF-only sections.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/mod.rs` (format pill in `.model-editor-pills` header + switch which sections render based on `hf_format`)
- Modify: `crates/tama/src/pages/model_editor/sections.rs` (add a `Vllm` section variant; update exhaustive `name()`/`icon()` matches; add `is_transformers` gate usage)
- Create: `crates/tama/src/pages/model_editor/vllm_form.rs` (new vLLM/transformers form + `VllmSettings` + args helpers)
- Modify: `crates/tama/src/pages/model_editor/types.rs` (add vLLM fields to `ModelForm` / a `VllmSettings` sub-struct; add `format_label` helper)
- Test: `crates/tama/src/pages/model_editor/` (pure-function tests in `vllm_form.rs` and `types.rs`)

**What to implement:**
1. **Format pill in the editor header:** In the `model-editor-pills` row (`mod.rs:721`), render a **disabled/status pill** (or a distinct `model-editor-pill model-editor-pill--format`) showing the format label derived from `hf_format`:
   - `"transformers"` → label `"Safetensors"` (tooltip: `"transformers format / vLLM"`)
   - `"gguf"` → label `"GGUF"`
   - `None`/unknown → label `"Format unknown"` (muted)
   Add a pure helper `pub fn format_label(hf_format: Option<&str>) -> String` in `types.rs` returning one of these labels, for testability.
2. **Format-aware pill navigation:** gate existing GGUF-only sections (Files quants, Vision Projector, MTP Draft, and the llama.cpp Hardware/Context fields) so they render when `hf_format != "transformers"` (i.e., GGUF/legacy). Add a small pure helper `pub fn is_transformers(hf_format: Option<&str>) -> bool` (returns `hf_format == Some("transformers")`) in `types.rs` that both the editor and tests use. When `is_transformers` is true:
   - Replace the Files section (quants/mmproj/MTP) content with the vLLM form (or rename/add a `Vllm` section via the `sections.rs` enum).
   - Hide the llama.cpp-only fields in the Context/Hardware form.
3. **vLLM form fields** (in `vllm_form.rs`) that map to vLLM CLI args:
   - Quantization (`--quantization`): dropdown `none`/`fp8`/`awq` (free-form allowed)
   - KV cache dtype (`--kv-cache-dtype`): `auto`/`fp8`/`bf16`
   - Tensor parallel size (`--tensor-parallel-size`): number (default 1)
   - GPU memory utilization (`--gpu-memory-utilization`): 0.0–1.0
   - Max model len (`--max-model-len`): number
   - Max num batched tokens (`--max-num-batched-tokens`): number
   - Flag toggles: `--enable-prefix-caching`, `--trust-remote-code`
4. **Args sync protocol (must be deterministic):** `ModelForm.args` is a newline-joined `String` (edited free-form in `advanced_form.rs` via the `field-args` textarea; `mod.rs` joins/splits on save). Implement two pure, unit-testable functions in `vllm_form.rs`:
   - `pub fn args_to_vllm_form(args: &str) -> VllmSettings` — parse the newline-joined args string, extracting the vLLM-managed flags (names listed above, both `--flag value` and `--flag=value` forms) into a typed `VllmSettings`; ignore unknown flags.
   - `pub fn vllm_form_to_args(form: &VllmSettings, existing: &str) -> String` — take the existing newline-joined args, **remove** any lines that are vLLM-managed flags (by flag name), then append the current `VllmSettings` as flags, preserving all user free-form args. This keeps the Advanced textarea and the vLLM form in sync with one source of truth (the args string).
5. Store these as the newline-joined args string via the existing `ModelConfig.args` mechanism (no new DB columns).
6. Ensure `ModelEditorFilesForm` hides its quant table for transformers format (no `.gguf` quants exist).

**Steps:**
- [ ] Add `format_label` and `is_transformers` helpers + `VllmSettings` struct + `args_to_vllm_form`/`vllm_form_to_args` in `types.rs`/`vllm_form.rs`.
- [ ] Write pure-function tests: `format_label` (transformers→"Safetensors", gguf→"GGUF", none→"Format unknown"); `is_transformers`; `args_to_vllm_form` parses known flags; `vllm_form_to_args` replaces vLLM-managed flags without clobbering an unrelated free-form flag; round-trip `args → form → args` is stable for vLLM flags.
- [ ] Render the format pill in the `.model-editor-pills` row; add the `Vllm` section variant to the `sections.rs` enum (update exhaustive `name()`/`icon()` matches) and a `VllmSettingsForm` component.
- [ ] Add the format gate in `mod.rs`: `match hf_format { transformers => format pill + vllm form + hide GGUF sections, _ => existing pills/sections }`.
- [ ] This crate has **no Leptos component-render test harness** — do NOT write view-render assertions. Instead test the `is_transformers`/`format_label` predicates and the args helpers as pure functions.
- [ ] Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit: `feat: add format pill and vLLM/safetensors form to model editor`

**Acceptance criteria:**
- [ ] The model editor header shows a **format pill** (Safetensors / GGUF / Format unknown)
- [ ] Transformers-format models show a vLLM form + format pill; GGUF models show the existing forms/pills (no GGUF regression)
- [ ] vLLM fields round-trip through `ModelForm.args` without clobbering user free-form args
- [ ] `is_transformers`/`format_label` gates + args helpers have pure-function test coverage
- [ ] No new DB columns; existing GGUF editing unaffected; clippy (workspace + ssr) clean

---

### Task 4: Format-aware arg building in `build_full_args`

**Context:** `build_full_args` in `crates/tama-core/src/config/resolve/mod.rs` only injects the model file via `-m <path>` when `server.quant` is set (GGUF). For a safetensors model there is no quant entry, so no model path is emitted; vLLM needs the model **path as its first positional arg** (e.g. `/mnt/models/Qwen/Qwen3.6-27B-FP8`, which the docker layer rewrites to `/models/...`). This task makes the loader emit the correct model reference for transformers-format models.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs` (`build_full_args`)
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (docker path passes model path; already partially handled via `args` — verify)
- Test: `crates/tama-core/src/config/resolve/tests/*`

**What to implement:**
1. **Model reference for transformers format:** In `build_full_args` (`crates/tama-core/src/config/resolve/mod.rs`), when `hf_format` is `"transformers"` (or `server.quant` is `None` with a `server.model` repo id), emit the **model path** as the first positional arg: `repo_path(models_dir, model_id)` → e.g. `/mnt/models/Qwen/Qwen3.6-27B-FP8`. This is the host path that `rewrite_args_for_container` rewrites to `/models/...` inside the vLLM container.
   - **Precondition (documented):** the safetensors model files must exist on disk at `repo_path(models_dir, model_id)`, mirroring how GGUF models require their pulled quant files. This matches the established workflow of pulling the model to `/mnt/models/<org>/<repo>` via HF CLI first. In-Tama safetensors pull/download is explicitly **out of scope** for this plan (a future enhancement), because the pull pipeline (`list_gguf_files`/`parse_blob_siblings`/`pull_gguf_with_progress`) is GGUF-only.
   - **Do NOT emit the HF repo id as the positional arg** — vLLM would try to download it, and the intended mode is local disk. Keep HF repo id in `server.model` (the DB `repo_id`), but the positional arg is the local path.
2. **GGUF behavior preserved:** the existing `-m <quant_file>` / `--mmproj` / `--spec-draft-model` injection stays strictly on the non-transformers path.
3. **Gate llama.cpp-only flag injections:** `build_full_args` currently injects `-c` (context), `-np`, `-b`/`-ub`, and `-ngl` **unconditionally** when those fields are set. A transformers model with `context_length`/`num_parallel`/`n_batch`/`gpu_layers` set would get llama.cpp flags that vLLM rejects. Gate these injections on `hf_format != "transformers"` (or on the llama.cpp backend check). Transformers format must NOT get `-c`, `-np`, `-b`, `-ub`, `-ngl`, `-m`, `--mmproj`, or `--spec-draft-model`.
4. **Positional placement:** insert the positional model path in a defined position — immediately after any `default_args` subcommand (e.g. `serve`) and before the first `--` flag — rather than appending at the very end. Argparse tolerates intermixing, but test the position.
5. Keep HF_TOKEN injection (already in the docker lifecycle path) for any gated model, but note the primary mode is local disk.

**Steps:**
- [ ] Write failing tests in `crates/tama-core/src/config/resolve/tests/` (use `test_helpers::sample_server`/`sample_config` with `quant = None`, `hf_format = Some("transformers")`):
  - Transformers ModelConfig → result contains a positional model path arg (`/mnt/models/...`) and NO `-m`, `-c`, `-np`, `-b`, `-ub`, `-ngl` flags (even when `context_length`/`gpu_layers`/`num_parallel` are set).
  - GGUF ModelConfig → still returns `-m <quant_file>` (no regression).
  - Position test: the positional path appears in the expected slot.
  - Run `cargo nextest run --package tama-core -- config::resolve` — did they fail as expected?
- [ ] Implement the format-aware branch in `build_full_args` (positional path + flag gating).
- [ ] Run `cargo nextest run --package tama-core -- config::resolve` — do all pass?
- [ ] Run `cargo nextest run --package tama-core -- lifecycle` — no native llama.cpp regression?
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: `feat: build positional model path arg for transformers-format models`

**Acceptance criteria:**
- [ ] Transformers models get a positional model path arg (works with vLLM docker path)
- [ ] Transformers models get no llama.cpp-only flags (`-m`, `-c`, `-np`, `-b`, `-ub`, `-ngl`)
- [ ] GGUF models unchanged (still `-m <quant_file>`, full flag set)
- [ ] No regression in native llama.cpp loading
- [ ] Tests pass, clippy clean

---

## Cross-Cutting Notes

- **Format source of truth:** `ModelConfig.hf_format` — set correctly at pull (Task 1), surfaced in the editor (Task 2), drives UI (Task 3) and arg building (Task 4).
- **Refresh behavior:** existing rows were backfilled to `"gguf"` by `db/backfill/hf_metadata.rs`. Repos pulled before this change (including any safetensors repos) will need a **model refresh** (`POST /tama/v1/models/:id/refresh`) to be re-detected, since `detect_hf_format` always returns `Some` and the refresh COALESCE keeps the current value otherwise. No migration is required — just a refresh for previously-pulled safetensors models.
- **No new DB schema:** `hf_format` already exists; vLLM settings persist via `ModelConfig.args` (free-form). Structured vLLM columns are deliberately out of scope.
- **Safetensors pull is out of scope:** the pull pipeline is GGUF-only. Scheme: user pulls safetensors weights to `models_dir/<org>/<repo>` via HF CLI first (the established workflow). In-Tama safetensors download is a documented future enhancement.
- **Backward compatibility:** All GGUF behavior (pull, editor, arg building) is preserved; transformers format is purely additive.
- **Backends:** The vLLM docker backend from plan-080 is the target executor. The docker layer already handles device/env/host-path rewriting.
- **Testing:** frontend is Leptos — use pure-function tests (no render harness exists); use cargo-nextest for backend tests.
