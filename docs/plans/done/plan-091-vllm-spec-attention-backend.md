# Add attention_backend to vLLM Speculative Decoding Config

**Goal:** Add `attention_backend` field to `VllmSpecForm` so it appears in both `--attention-backend` (top-level flag) and `--speculative-config` JSON, with two independent UI inputs.

**Architecture:** `VllmSpecForm` gains `attention_backend: Option<String>`. The existing top-level input in `advanced_form.rs` (`VllmSettings.attention_backend`) stays as-is. A second input is added inside the Speculative Decoding → Advanced section (`VllmSpecForm.attention_backend`). Serialization automatically includes the field in `--speculative-config` JSON.

**Tech Stack:** Rust, Leptos, serde.

---

### Task 1: Add attention_backend to VllmSpecForm type

**Context:** `VllmSpecForm` is the struct that serializes into the `--speculative-config` JSON passed to vLLM. It currently lacks `attention_backend`, so even when the user sets it on the command line, it can't be managed through the form. Adding it allows the field to round-trip through the form and be included in the speculative config JSON.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/types.rs`
- Modify: `crates/tama/src/pages/model_editor/vllm_form.rs`

**What to implement:**

1. In `types.rs`, add `attention_backend: Option<String>` to `VllmSpecForm`:
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct VllmSpecForm {
    pub method: Option<String>,
    pub model: Option<String>,
    pub num_speculative_tokens: Option<u32>,
    pub rejection_sample_method: Option<String>,
    pub draft_tensor_parallel_size: Option<u32>,
    pub draft_sample_method: Option<String>,
    pub disable_padded_drafter_batch: Option<bool>,
    pub attention_backend: Option<String>,  // NEW
}
```

2. In `vllm_form.rs`, update `merge_vllm_spec_settings` to include the new field:
```rust
fn merge_vllm_spec_settings(existing: &VllmSpecForm, extracted: &VllmSpecForm) -> VllmSpecForm {
    VllmSpecForm {
        method: existing.method.clone().or_else(|| extracted.method.clone()),
        model: existing.model.clone().or_else(|| extracted.model.clone()),
        num_speculative_tokens: existing.num_speculative_tokens.or(extracted.num_speculative_tokens),
        rejection_sample_method: existing.rejection_sample_method.clone().or_else(|| extracted.rejection_sample_method.clone()),
        draft_tensor_parallel_size: existing.draft_tensor_parallel_size.or(extracted.draft_tensor_parallel_size),
        draft_sample_method: existing.draft_sample_method.clone().or_else(|| extracted.draft_sample_method.clone()),
        disable_padded_drafter_batch: existing.disable_padded_drafter_batch.or(extracted.disable_padded_drafter_batch),
        attention_backend: existing.attention_backend.clone().or_else(|| extracted.attention_backend.clone()),
    }
}
```

3. `normalize_vllm_spec` does NOT need changes — `attention_backend` is a free-form string with no validation requirements.

**What NOT to change:**
- Do NOT modify `VllmSettings.attention_backend` (the top-level field stays as-is)
- Do NOT modify `args_to_vllm_form` or `parse_flag_into_form` (the `--speculative-config` JSON parsing automatically handles the new field through serde)

**Steps:**
- [ ] Add `attention_backend: Option<String>` field to `VllmSpecForm` in `types.rs`
- [ ] Add `attention_backend` merge logic to `merge_vllm_spec_settings` in `vllm_form.rs`
- [ ] Run `cargo build --package tama --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Run `cargo nextest run --package tama --features ssr`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Commit with message: "feat: add attention_backend to VllmSpecForm"

**Acceptance criteria:**
- [ ] `VllmSpecForm` has `attention_backend: Option<String>` field
- [ ] `merge_vllm_spec_settings` merges `attention_backend` (existing wins, extracted fills gaps)
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes
- [ ] `cargo nextest run --package tama --features ssr` passes (all existing tests still pass)

---

### Task 2: Add attention_backend input to Speculative Decoding form

**Context:** The Advanced tab already has a top-level "Attention backend" input for `VllmSettings.attention_backend`. A second input is needed inside the Speculative Decoding section for `VllmSpecForm.attention_backend`. This allows the user to set different values for the top-level flag vs the speculative config JSON.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/advanced_form.rs`

**What to implement:**

1. In the `Effect::new` that populates input values, add initialization for the new field:
```rust
set_input_value(
    "field-vllm-spec-attention-backend",
    &f.vllm.spec_decoding.attention_backend.clone().unwrap_or_default(),
);
```

2. Inside the Speculative Decoding section, within the `<Show when=move || has_method.get()>` block, add the input. Place it after "Speculative tokens" and before the drafter model section (it's a general spec config setting, not drafter-specific):

```rust
// attention_backend (inside spec config)
<label class="form-label" for="field-vllm-spec-attention-backend">
    "Attention backend (spec)"
</label>
<input
    id="field-vllm-spec-attention-backend"
    class="form-input"
    type="text"
    placeholder="e.g. ROCM_AITER_UNIFIED_ATTN"
    on:input=move |e| {
        let val = target_value(&e);
        form.update(|f| {
            if let Some(form) = f {
                form.vllm.spec_decoding.attention_backend = if val.is_empty() {
                    None
                } else {
                    Some(val)
                };
            }
        });
    }
/>
<div class="form-hint">
    "Attention backend inside --speculative-config JSON. Separate from the top-level --attention-backend flag."
</div>
```

**What NOT to change:**
- Do NOT modify the existing top-level "Attention backend" input (`VllmSettings.attention_backend`)
- Do NOT change the label or hint of the existing input

**Steps:**
- [ ] Add `set_input_value` for `field-vllm-spec-attention-backend` in the Effect
- [ ] Add the attention_backend input inside the `<Show when=move || has_method.get()>` block, after "Speculative tokens"
- [ ] Run `cargo build --package tama --features ssr`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "feat: add attention_backend input to vLLM spec decoding form"

**Acceptance criteria:**
- [ ] Second "Attention backend (spec)" input appears in Speculative Decoding section when a method is selected
- [ ] Input populates `vllm.spec_decoding.attention_backend` (not `vllm.attention_backend`)
- [ ] Existing top-level "Attention backend" input still works unchanged
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes

---

### Task 3: Update tests and verification

**Context:** Existing tests for `VllmSpecForm` need to account for the new field. The `deny_unknown_fields` attribute means old JSON without `attention_backend` still works (it defaults to `None`), but tests that construct `VllmSpecForm` with all fields should include it.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/vllm_form.rs` (tests module)

**What to implement:**

1. In `test_args_to_vllm_form_speculative_config_all_fields`, add `attention_backend` to the JSON and assertion:
```rust
let args = r#"--speculative-config {"method":"eagle","model":"draft.gguf","num_speculative_tokens":8,"rejection_sample_method":"top_k","draft_tensor_parallel_size":2,"draft_sample_method":"greedy","disable_padded_drafter_batch":true,"attention_backend":"ROCM_AITER_UNIFIED_ATTN"}"#;
// ...
assert_eq!(
    form.spec_decoding.attention_backend,
    Some("ROCM_AITER_UNIFIED_ATTN".to_string())
);
```

2. Add a test for `merge_vllm_spec_settings` with `attention_backend`:
```rust
#[test]
fn test_merge_vllm_spec_attention_backend_existing_wins() {
    let existing = VllmSpecForm {
        attention_backend: Some("FLASH_ATTN".to_string()),
        ..Default::default()
    };
    let extracted = VllmSpecForm {
        attention_backend: Some("ROCM_AITER_UNIFIED_ATTN".to_string()),
        ..Default::default()
    };
    let merged = merge_vllm_spec_settings(&existing, &extracted);
    assert_eq!(merged.attention_backend, Some("FLASH_ATTN".to_string()));
}
```

3. Add a test for JSON round-trip with `attention_backend`:
```rust
#[test]
fn test_speculative_config_with_attention_backend_roundtrip() {
    let args = r#"--speculative-config {"method":"mtp","attention_backend":"ROCM_AITER_UNIFIED_ATTN"}"#;
    let form = args_to_vllm_form(args);
    assert_eq!(form.spec_decoding.method, Some("mtp".to_string()));
    assert_eq!(
        form.spec_decoding.attention_backend,
        Some("ROCM_AITER_UNIFIED_ATTN".to_string())
    );
    // Verify it's stripped from args
    let stripped = strip_managed_flags(args);
    assert!(!stripped.contains("--speculative-config"));
}
```

**Steps:**
- [ ] Update `test_args_to_vllm_form_speculative_config_all_fields` with `attention_backend`
- [ ] Add `test_merge_vllm_spec_attention_backend_existing_wins`
- [ ] Add `test_speculative_config_with_attention_backend_roundtrip`
- [ ] Run `cargo nextest run --package tama --features ssr`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr -- -D warnings`
- [ ] Commit with message: "test: add attention_backend tests for VllmSpecForm"

**Acceptance criteria:**
- [ ] All existing tests pass
- [ ] New tests cover: JSON parse, merge, and round-trip for `attention_backend`
- [ ] `cargo clippy --package tama --features ssr -- -D warnings` passes
- [ ] `cargo nextest run --package tama --features ssr` passes

---

### Task 4: Final verification — full gate

**Context:** All individual tasks pass their local checks. This task runs the full validation gate matching CI.

**Files:**
- No files to modify — verification only.

**Steps:**
- [ ] Run `cargo fmt --all --check`
  - Did it pass? If not, run `cargo fmt --all` and re-check.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
  - Did it pass? If not, fix clippy errors and re-run.
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
  - Did it pass? If not, fix clippy errors and re-run.
- [ ] Run `cargo nextest run --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Commit with message: "chore: verification gate for vllm spec attention_backend"

**Acceptance criteria:**
- [ ] All four gate commands pass with zero errors
- [ ] Workspace builds and tests cleanly
