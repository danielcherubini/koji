# vLLM Speculative Decoding Settings Plan

**Goal:** Add speculative decoding configuration for safetensors (transformers-format) models in the vLLM backend, exposing `--speculative-config` JSON through a guided form in the model editor.

**Architecture:** New `VllmSpecConfig` struct in `tama-core` serializes to the JSON blob for `--speculative-config`. Frontend mirror `VllmSpecForm` in `tama` provides a Leptos form with method dropdown, conditional drafter model input, and advanced options. Config is nested inside the existing `vllm_config` JSON DB column — no migration needed.

**Tech Stack:** Rust (tama-core + tama), Leptos (WASM frontend), serde JSON serialization

---

### Task 1: Add `VllmSpecConfig` to `tama-core` and extend `VllmConfig`

**Context:**
This task adds the core Rust types that represent vLLM's speculative decoding configuration. `VllmSpecConfig` serializes to the JSON passed to `--speculative-config`. It's nested inside `VllmConfig` so it's stored in the existing `vllm_config` DB column. `attention_backend` is also added to `VllmConfig` (not `VllmSpecConfig`) because it's a top-level vLLM engine arg, not a speculative config key.

**Files:**
- Modify: `crates/tama-core/src/config/types/model.rs`
- Modify: `crates/tama-core/src/config/types/model_tests.rs`
- Modify: `crates/tama-core/src/db/backfill/vllm_config.rs`

**What to implement:**

1. Add `VllmSpecConfig` struct in `model.rs` (near `VllmConfig`):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VllmSpecConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_speculative_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection_sample_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_tensor_parallel_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_sample_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_padded_drafter_batch: Option<bool>,
}

impl VllmSpecConfig {
    pub fn is_empty(&self) -> bool {
        self.method.is_none()
            && self.model.is_none()
            && self.num_speculative_tokens.is_none()
            && self.rejection_sample_method.is_none()
            && self.draft_tensor_parallel_size.is_none()
            && self.draft_sample_method.is_none()
            && self.disable_padded_drafter_batch.is_none()
    }
}
```

2. Extend `VllmConfig` with two new fields:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub attention_backend: Option<String>,
#[serde(default)]
pub spec_decoding: VllmSpecConfig,
```

Note: `attention_backend` is a top-level vLLM arg (`--attention-backend`), NOT a field of `VllmSpecConfig`.

3. Update `VllmConfig::is_empty()` to include:
```rust
&& self.attention_backend.is_none()
&& self.spec_decoding.is_empty()
```

4. Update `VllmConfig::to_args()` to emit:
- `--attention-backend <value>` (after existing args, before `--enable-prefix-caching`):
```rust
if let Some(ref v) = self.attention_backend {
    args.push("--attention-backend".to_string());
    args.push(v.clone());
}
```
- `--speculative-config <json>` as the **last** arg, after `--trust-remote-code`. **Critical: emit as a single grouped, shlex-quoted entry** to survive `flatten_args` (which uses `shlex::split` and strips quotes from raw JSON):
```rust
if !self.spec_decoding.is_empty() {
    if let Ok(json) = serde_json::to_string(&self.spec_decoding) {
        args.push(format!("--speculative-config {}", crate::config::quote_value(&json)));
    }
}
```
`quote_value` is exported at `crate::config::quote_value` and round-trips losslessly through `split_arg_entry`. This matches the existing convention used for `-ctk`/`-ctv` in `resolve/mod.rs:506`.

5. Fix `merge_vllm_config` in `crates/tama-core/src/db/backfill/vllm_config.rs:171` — it constructs `VllmConfig` with an exhaustive struct literal (no `..Default::default()`). Add:
```rust
attention_backend: existing.attention_backend.clone().or_else(|| extracted.attention_backend.clone()),
spec_decoding: merge_vllm_spec_decoding(&existing.spec_decoding, &extracted.spec_decoding),
```
Add a helper `merge_vllm_spec_decoding` that prefers existing non-default values. Also fix the two test literals in the same file (add `..Default::default()` or include new fields).

6. Add tests in `model_tests.rs`:
- `VllmSpecConfig::is_empty()` — empty and non-empty cases
- `VllmConfig::is_empty()` includes spec_decoding and attention_backend
- `to_args()` emits `--speculative-config` as a **single grouped, shlex-quoted entry** (e.g., `"--speculative-config \"{\"method\":\"mtp\",...}\""`) that survives `flatten_args`
- `to_args()` emits nothing when spec_decoding is empty
- `to_args()` emits `--attention-backend <value>` when set
- Legacy `vllm_config` JSON without `spec_decoding` key deserializes (serde default)
- Emitted JSON contains only set fields (no nulls)

**Steps:**
- [ ] Fix `merge_vllm_config` in `backfill/vllm_config.rs` (add new fields + merge helper + fix test literals)
- [ ] Write failing test for `VllmSpecConfig::is_empty()` in `model_tests.rs`
- [ ] Run `cargo nextest run --package tama-core -- vllm`
  - Did it fail with compilation error (type not found)? If not, investigate.
- [ ] Implement `VllmSpecConfig` struct in `model.rs`
- [ ] Run `cargo nextest run --package tama-core -- vllm`
  - Did tests pass? If not, fix and re-run.
- [ ] Write failing test for `VllmConfig` extended with `spec_decoding` and `attention_backend`
- [ ] Run `cargo nextest run --package tama-core -- vllm`
  - Did it fail? If not, investigate.
- [ ] Extend `VllmConfig` with new fields, update `is_empty()` and `to_args()`
- [ ] Run `cargo nextest run --package tama-core -- vllm`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core --all-targets -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add VllmSpecConfig for vLLM speculative decoding"

**Acceptance criteria:**
- [ ] `VllmSpecConfig` serializes to JSON with only set fields (no nulls)
- [ ] `VllmConfig::is_empty()` returns true when spec_decoding is empty
- [ ] `VllmConfig::to_args()` emits `--speculative-config <json>` last
- [ ] `VllmConfig::to_args()` emits `--attention-backend <value>` when set
- [ ] Legacy `vllm_config` JSON deserializes without `spec_decoding` key
- [ ] All tests pass, clippy clean, fmt clean

---

### Task 2: Add `VllmSpecForm` to frontend types, extend `VllmSettings`, and manage `--attention-backend`

**Context:**
The WASM frontend cannot use `tama-core` types directly, so we need a mirror struct. This task adds `VllmSpecForm` and extends `VllmSettings` in the frontend types module. It also makes `--attention-backend` a managed flag (it was previously unmanaged/preserved in args) so the new typed field populates correctly on load and is stripped from Extra Args on save.

The API layer (`crud/mod.rs`) already serializes `vllm` as JSON — the new fields will flow through automatically once the core types are updated.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/types.rs`
- Modify: `crates/tama/src/pages/model_editor/vllm_form.rs`

**What to implement:**

1. Add `VllmSpecForm` struct in `types.rs` (near `VllmSettings`):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct VllmSpecForm {
    pub method: Option<String>,
    pub model: Option<String>,
    pub num_speculative_tokens: Option<u32>,
    pub rejection_sample_method: Option<String>,
    pub draft_tensor_parallel_size: Option<u32>,
    pub draft_sample_method: Option<String>,
    pub disable_padded_drafter_batch: Option<bool>,
}
```

2. Extend `VllmSettings` with:
```rust
pub attention_backend: Option<String>,
#[serde(default)]
pub spec_decoding: VllmSpecForm,
```

3. Make `--attention-backend` a managed flag:
- Add `"--attention-backend"` to `MANAGED_FLAGS`
- Add arm in `parse_flag_into_form`:
```rust
"--attention-backend" => {
    if !value.is_empty() && !value.contains(char::is_whitespace) {
        form.attention_backend = Some(value.to_string());
    } else {
        form.attention_backend = None;
    }
}
```
- `--attention-backend` is NOT a boolean flag (it takes a value)
- Update existing tests that assert `--attention-backend` is preserved: `test_args_to_vllm_form_flattened` and `test_strip_managed_flags_flattened` must now expect it to be extracted/stripped

4. Update `args_to_vllm_form` to parse `--speculative-config` JSON:
- Add `"--speculative-config"` to `MANAGED_FLAGS`
- When `--speculative-config` is encountered, read the next value (JSON string), attempt `serde_json::from_str::<VllmSpecForm>()`. On success, populate `form.spec_decoding`. On failure, do nothing (preserves malformed JSON in args).
- Handle all three forms: `--speculative-config {json}` (same line), `--speculative-config={json}` (inline), `--speculative-config` + `{json}` on next line (flattened).
- Strip surrounding quotes from JSON value before parsing.

5. Update `strip_managed_flags` — both `--attention-backend` and `--speculative-config` are now in `MANAGED_FLAGS`, so they will be stripped automatically by existing logic. Verify: the value on the next line (flattened) or same line must also be consumed.

6. Update `classify_managed_line`/`can_parse_managed_value`:
- For `--speculative-config`: attempt `serde_json::from_str::<VllmSpecForm>(value)`. Return true on success, false on failure (so malformed JSON is preserved, matching existing behavior for unparseable values).
- For `--attention-backend`: standard string check (non-empty, no whitespace).

7. Update `merge_vllm_settings` to merge `spec_decoding` and `attention_backend`:
```rust
spec_decoding: merge_vllm_spec_settings(&existing.spec_decoding, &extracted.spec_decoding),
attention_backend: existing.attention_backend.clone().or(extracted.attention_backend.clone()),
```

Add helper `merge_vllm_spec_settings` that prefers existing non-default values, fills gaps from extracted.

8. Add tests in `vllm_form.rs` test module:
- `args_to_vllm_form` parses `--attention-backend ROCM_AITER_UNIFIED_ATTN` (now managed)
- `strip_managed_flags` removes `--attention-backend` and its value
- `args_to_vllm_form` parses `--speculative-config '{"method":"mtp","num_speculative_tokens":8}'`
- `args_to_vllm_form` handles `--speculative-config` in all three forms (grouped, inline, flattened)
- `strip_managed_flags` removes `--speculative-config` and its JSON value
- Malformed JSON preserved (not stripped) by `strip_managed_flags`
- `merge_vllm_settings` correctly merges `spec_decoding` and `attention_backend` fields

**Steps:**
- [ ] Write failing test for `args_to_vllm_form` parsing `--speculative-config` in `vllm_form.rs`
- [ ] Run `cargo nextest run --package tama -- vllm_form`
  - Did it fail? If not, investigate.
- [ ] Add `VllmSpecForm` to `types.rs`, extend `VllmSettings`
- [ ] Update `args_to_vllm_form` to parse `--speculative-config` JSON
- [ ] Update `MANAGED_FLAGS`, `parse_flag_into_form`, `is_boolean_flag`, `strip_managed_flags`, `classify_managed_line`, `can_parse_managed_value` for both `--attention-backend` and `--speculative-config`
- [ ] Update `merge_vllm_settings` with `spec_decoding` and `attention_backend` merge
- [ ] Update existing tests: `test_args_to_vllm_form_flattened` and `test_strip_managed_flags_flattened` must now expect `--attention-backend` to be extracted/stripped
- [ ] Run `cargo nextest run --package tama -- vllm_form`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Run `cargo build --package tama`
- [ ] Commit with message: "feat: add VllmSpecForm and args parsing for speculative config"

**Acceptance criteria:**
- [ ] `args_to_vllm_form` parses `--speculative-config` JSON in all three forms
- [ ] Malformed JSON preserved (not stripped) by `strip_managed_flags`
- [ ] `merge_vllm_settings` correctly merges `spec_decoding` fields
- [ ] All tests pass, clippy clean, fmt clean

---

### Task 3: Build the Speculative Decoding UI in `advanced_form.rs`

**Context:**
This task adds the Leptos UI components for configuring speculative decoding. It sits inside the existing vLLM `<Show>` block (which only renders for transformers-format models). The form uses conditional rendering: method dropdown controls visibility of drafter model input, and an "Advanced" section is collapsible.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/advanced_form.rs`
- Modify: `crates/tama/src/pages/model_editor/mod.rs`

**What to implement:**

Add a "Speculative Decoding" subsection inside the existing vLLM `<Show>` block, **below** the prefix caching / trust remote code checkboxes and **above** the "Extra Args" textarea.

**Save-time validation in `mod.rs`:** In the save action (before `save_model`), add normalization:
- If method is empty (disabled): clear entire `spec_decoding` (all fields None)
- If method is `mtp` or `ngram`: clear `model` field
- If method is `dflash`, `eagle3`, or `draft_model`: if `model` is empty, set `save_status` to error ("Drafter model required for this method") and abort save
- If `num_speculative_tokens` is not set: default to 5

UI structure:

```
<h3 class="form-section-title">"Speculative Decoding"</h3>
<div class="form-grid">
    // Method dropdown
    <label class="form-label">"Method"</label>
    <select id="field-vllm-spec-method" class="form-select">
        <option value="">"(disabled)"</option>
        <option value="mtp">"mtp" — Multi-Token Prediction (no drafter needed)"</option>
        <option value="ngram">"ngram" — N-gram matching (no drafter needed)"</option>
        <option value="dflash">"dflash" — Diffusion block prediction (needs drafter)"</option>
        <option value="eagle3">"eagle3" — EAGLE-3 autoregressive (needs drafter)"</option>
        <option value="draft_model">"draft_model" — Any smaller model (needs drafter)"</option>
    </select>
    <div class="form-hint">"MTP requires model family support (DeepSeek, Qwen3, Gemma 4, etc.)"</div>

    // Shown when method is not empty
    <Show when=move || has_method.get()>
        // num_speculative_tokens
        <label class="form-label" for="field-vllm-spec-tokens">"Speculative tokens"</label>
        <input id="field-vllm-spec-tokens" type="number" min="1" placeholder="5" />
        <div class="form-hint">"Tokens to propose per step. Default: 5. Values above 8 may reduce quality."</div>

        // Drafter model — shown only for dflash, eagle3, draft_model
        <Show when=move || needs_drafter.get()>
            <label class="form-label" for="field-vllm-spec-model">"Drafter model"</label>
            <input id="field-vllm-spec-model" type="text" placeholder="owner/repo or /path/to/model" />
            <div class="form-hint">"HF repo ID or local path to the drafter/speculator model"</div>
        </Show>

        // Advanced — collapsible
        <details>
            <summary>"Advanced"</summary>
            // rejection_sample_method
            <label class="form-label">"Rejection method"</label>
            <select>
                <option value="">"(default)"</option>
                <option value="standard">"standard"</option>
                <option value="synthetic">"synthetic"</option>
                <option value="block">"block"</option>
            </select>

            // draft_sample_method
            <label class="form-label">"Draft sample method"</label>
            <select>
                <option value="">"(default)"</option>
                <option value="greedy">"greedy"</option>
                <option value="probabilistic">"probabilistic"</option>
            </select>

            // draft_tensor_parallel_size
            <label class="form-label">"Draft TP size"</label>
            <input type="number" min="1" placeholder="1" />

            // disable_padded_drafter_batch
            <div class="form-check">
                <input type="checkbox" id="field-vllm-spec-disable-padded" />
                <label class="form-check-label" for="field-vllm-spec-disable-padded">
                    "Disable padded drafter batch"
                    <div class="form-hint">"Use unpadded draft batches (EAGLE only)"</div>
                </label>
            </div>
        </details>
    </Show>
</div>
```

**Save-time validation (in the form submit handler):**
- If method is empty (disabled): clear entire `spec_decoding` (all fields None)
- If method is `mtp` or `ngram`: clear `model` field
- If method is `dflash`, `eagle3`, or `draft_model`: `model` is required (show inline error if empty)
- If `num_speculative_tokens` is not set: default to 5
- `attention_backend` is handled in Task 2 (made managed in `vllm_form.rs`) and Task 4 (UI field). Not repeated here.

**Effect for populating form on load (use correct types):**
Add to the existing `Effect` that populates form fields:
- `set_input_value("field-vllm-spec-method", &form.spec_decoding.method.clone().unwrap_or_default())`
- `set_input_value("field-vllm-spec-tokens", &form.spec_decoding.num_speculative_tokens.map(|v| v.to_string()).unwrap_or_default())`
- `set_input_value("field-vllm-spec-model", &form.spec_decoding.model.clone().unwrap_or_default())`
- `set_input_value("field-vllm-spec-rejection-method", &form.spec_decoding.rejection_sample_method.clone().unwrap_or_default())`
- `set_input_value("field-vllm-spec-draft-sample-method", &form.spec_decoding.draft_sample_method.clone().unwrap_or_default())`
- `set_input_value("field-vllm-spec-draft-tp-size", &form.spec_decoding.draft_tensor_parallel_size.map(|v| v.to_string()).unwrap_or_default())`
- `set_checked("field-vllm-spec-disable-padded", form.spec_decoding.disable_padded_drafter_batch.unwrap_or(false))`

**Steps:**
- [ ] Write the Leptos view! macro for the Speculative Decoding subsection in `advanced_form.rs`
- [ ] Add `has_method` and `needs_drafter` derived signals
- [ ] Add event handlers for all inputs (on:change / on:input)
- [ ] Add save-time validation logic (clear model for mtp/ngram, require model for drafter methods)
- [ ] Add form population in the existing Effect
- [ ] Run `cargo build --package tama`
  - Did it compile? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit with message: "feat: add speculative decoding UI to vLLM form"

**Acceptance criteria:**
- [ ] Method dropdown with 5 methods + disabled option
- [ ] Drafter model input shown only for dflash/eagle3/draft_model
- [ ] Advanced section collapsible, collapsed by default
- [ ] Save-time validation: method None clears spec, drafter methods require model
- [ ] Form populates correctly on load from existing config
- [ ] Builds and clippy clean

---

### Task 4: Add `attention_backend` UI field to the vLLM form

**Context:**
`attention_backend` is a top-level vLLM engine arg (`--attention-backend`), not a speculative config key. Task 2 made it a managed flag in the parser. This task adds the UI field so users can set it from the editor.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/advanced_form.rs`

**What to implement:**

Add an "Attention backend" text input in the vLLM section (above or below spec decoding):

```rust
<label class="form-label" for="field-vllm-attention-backend">"Attention backend"</label>
<input
    id="field-vllm-attention-backend"
    class="form-input"
    type="text"
    placeholder="e.g. ROCM_AITER_UNIFIED_ATTN"
    on:input=move |e| { ... }
/>
<div class="form-hint">"vLLM --attention-backend — overrides the default attention kernel"</div>
```

Update the existing Effect to populate the field on load.

**Steps:**
- [ ] Add attention_backend text input to the vLLM section in `advanced_form.rs`
- [ ] Add on:input handler that updates `form.vllm.attention_backend`
- [ ] Add form population in the existing Effect
- [ ] Run `cargo build --package tama`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit with message: "feat: add attention_backend field to vLLM form"

**Acceptance criteria:**
- [ ] Attention backend text input in vLLM section
- [ ] Value flows through to `VllmSettings.attention_backend`
- [ ] Emits `--attention-backend <value>` via `VllmConfig::to_args()`
- [ ] Builds and clippy clean

---

### Task 5: Integration tests and verification

**Context:**
This task adds end-to-end integration tests that verify the full round-trip: args → form → args, the DB serialization/deserialization, and the arg injection in `build_full_args`.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/tests/transformers_format.rs`
- Modify: `crates/tama/src/pages/model_editor/vllm_form.rs` (tests only)

**What to implement:**

1. In `transformers_format.rs` (tama-core):
- `test_build_full_args_vllm_spec_decoding` — Full args for transformers model with spec_decoding includes `--speculative-config` with intact JSON (after `flatten_args`). Use `h::sample_server` harness. Assert the flat args contain the intact JSON token.
- `test_build_full_args_vllm_attention_backend` — Full args include `--attention-backend ROCM_AITER_UNIFIED_ATTN`

2. In `vllm_form.rs` (tama) tests:
- Round-trip: args with `--speculative-config` → form → strip → reparse stable
- Mixed args: `--quantization fp8` + `--speculative-config {...}` → both parsed correctly
- `--attention-backend` round-trip: args → form → strip → reparse stable
- Malformed JSON: `--speculative-config '{bad'` → preserved in args (not stripped)

**NOTE:** Task 5 does NOT duplicate Task 1/2 tests. Task 1 owns `model_tests.rs` tests (struct-level). Task 2 owns `vllm_form.rs` tests (frontend parser). Task 5 owns only `build_full_args` integration tests and cross-cutting round-trips.

**Out of scope:** Core-side extraction (`vllm_args.rs::extract_vllm_args` + startup backfill in `backfill/vllm_config.rs`) is NOT updated. Existing models with these flags in args will self-heal only when the user opens and saves the model in the editor. Runtime correctness is preserved (`merge_args` dedupes, typed column wins). This is acceptable because: (1) no existing models have `--speculative-config` (new feature), and (2) `--attention-backend` in existing args will continue to work at runtime via the args column while the typed field is empty.

**Steps:**
- [ ] Write failing integration tests in `transformers_format.rs` (build_full_args for vLLM spec_decoding and attention_backend)
- [ ] Run `cargo nextest run --package tama-core -- resolve`
  - Did they fail? If not, investigate.
- [ ] Write failing round-trip tests in `vllm_form.rs`
- [ ] Run `cargo nextest run --package tama -- vllm_form`
  - Did they fail? If not, investigate.
- [ ] Verify all tests pass (implementation from Tasks 1-4 should make them pass)
- [ ] Run `cargo nextest run --workspace` (full test suite)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Commit with message: "test: add integration tests for vLLM speculative decoding"

**Acceptance criteria:**
- [ ] All new tests pass
- [ ] Full workspace test suite passes
- [ ] Clippy clean across workspace
- [ ] No regressions in existing tests
