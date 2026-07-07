# MTP Draft Model Fixes Plan

**Goal:** Fix two bugs: (1) MTP Draft Model selector always returns "(none)" in the model editor, and (2) `--device` should also set `--device-draft` with the same value.

**Architecture:** Bug 1 is a missing field in the API response JSON builder — one-line addition. Bug 2 mirrors the existing `--device` injection pattern to also inject `--device-draft` at both the args-build stage (resolve/mod.rs) and the device-mapping override stage (lifecycle/mod.rs).

**Tech Stack:** Rust, SQLite, Leptos (web UI), llama.cpp CLI flags

---

### Task 1: Add `mtp_model` to the API response

**Context:** The `model_entry_json` function in `info.rs` builds the JSON response for `GET /tama/v1/models/:id` and `GET /tama/v1/models`. The DB stores `selected_mtp_model`, and `ModelConfig::from_db_record` correctly rehydrates it into `m.mtp_model`. However, the JSON builder omits `mtp_model` entirely, so the frontend always receives `null` and the dropdown shows "(none)". This task adds the missing field.

**Files:**
- Modify: `crates/tama-web/src/api/models/info.rs`

**What to implement:**
In the `model_entry_json` function, add `"mtp_model": m.mtp_model,` to the `serde_json::json!` macro, placed right after the existing `"mmproj": m.mmproj,` line. The field should use `m.mtp_model` (from the `ModelConfig` parameter), NOT `record.selected_mtp_model` (from the DB record), to be consistent with how other fields like `mmproj`, `quant`, and `model` are sourced.

**Steps:**
- [ ] In `crates/tama-web/src/api/models/info.rs`, find the `model_entry_json` function (line ~73)
- [ ] In the `serde_json::json!` block, add `"mtp_model": m.mtp_model,` after the `"mmproj": m.mmproj,` line
- [ ] Write a unit test at the bottom of `info.rs` in a `#[cfg(test)]` module:
  - Name: `test_model_entry_json_includes_mtp_model`
  - Create a `ModelConfigRecord` with `selected_mtp_model: Some("mtp-test.gguf".to_string())` and all other fields as minimal defaults (empty strings, `None`, `false` as appropriate)
  - Create a `ModelConfig` with `mtp_model: Some("mtp-test.gguf".to_string())` and minimal defaults for all other fields
  - Call `model_entry_json(1, &record, &config, &std::path::Path::new("."), None)`
  - Assert that `result.get("mtp_model").and_then(|v| v.as_str()) == Some("mtp-test.gguf")`
  - Also test the `None` case: create a `ModelConfig` with `mtp_model: None` and assert `result.get("mtp_model").and_then(|v| v.as_str()) == None` (or `result["mtp_model"].is_null()`)
- [ ] Run `cargo test --package tama-web -- mtp_model`
  - The new test should pass.
- [ ] Run `cargo test --package tama-core -- mtp_model`
  - The existing DB round-trip tests (`test_mtp_model_db_round_trip`, `test_mtp_model_toml_round_trip`) should still pass.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "fix: add mtp_model to API model response JSON"

**Acceptance criteria:**
- [ ] `GET /tama/v1/models/:id` returns `mtp_model` in the JSON response
- [ ] `GET /tama/v1/models` includes `mtp_model` for each model in the list
- [ ] Existing `mtp_model` DB round-trip tests still pass
- [ ] The model editor's "MTP Draft Model" dropdown correctly shows the previously selected value

---

### Task 2: Mirror `--device` to `--device-draft`

**Context:** When `gpu_device` is configured on a model, `--device <value>` is injected into the backend args. For MTP draft models, the draft model also needs a device assignment via `--device-draft`. Without this, the draft model may end up on a different (or default) GPU, causing suboptimal performance or errors on multi-GPU setups. This task mirrors the `--device` injection pattern to also inject `--device-draft` with the same value, at both the initial args-build stage and the device-mapping override stage.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`

**What to implement:**

**Part A — `resolve/mod.rs` (initial injection):**
After the existing `--device` injection block (around line 518), add a new block that injects `--device-draft` with the same value and the same guard pattern:
- Only for llama.cpp backends (`is_llama_cpp_backend`)
- Only when `server.gpu_device` is `Some` and non-empty after trimming
- Check if `--device-draft` is already present (via `flag_name(e) == Some("--device-draft")`) to avoid duplicating user-provided values
- Use `quote_value(trimmed)` for the value, matching the `--device` pattern

**Part B — `lifecycle/mod.rs` (mapped-device override):**
After the existing `override_arg(&mut args, "--device", &mapped_device);` call (around line 164), add:
```rust
override_arg(&mut args, "--device-draft", &mapped_device);
```
This ensures that when the position-based device (e.g. "GPU0") is mapped to the backend-specific name (e.g. "CUDA0"), `--device-draft` is updated to match. The `override_arg` function handles both insertion (if the flag doesn't exist) and replacement (if it does), so this works whether `--device-draft` was injected in Part A or provided by the user in extra args.

**Do NOT change:**
- The `resolve_gpu_device_to_backend_name` function or its return type
- Any other `override_arg` calls (host, port)
- The `--device` injection logic itself

**Steps:**
- [ ] In `crates/tama-core/src/config/resolve/mod.rs`, find the `--device` injection block (around line 514-523)
- [ ] Immediately after that block, add a `--device-draft` injection block with the same structure:

```rust
// Inject --device-draft (mirrors --device for MTP draft model placement).
if is_llama_cpp_backend {
    if let Some(ref device) = server.gpu_device {
        let trimmed = device.trim();
        if !trimmed.is_empty() {
            let already_has_device_draft = grouped
                .iter()
                .any(|e| matches!(crate::config::flag_name(e), Some("--device-draft")));
            if !already_has_device_draft {
                grouped.push(format!("--device-draft {}", crate::config::quote_value(trimmed)));
            }
        }
    }
}
```

- [ ] In `crates/tama-core/src/proxy/lifecycle/mod.rs`, find the `override_arg(&mut args, "--device", &mapped_device);` line (around line 164)
- [ ] Add `override_arg(&mut args, "--device-draft", &mapped_device);` immediately after it (still inside the `if let Some(mapped_device) = ...` block)
- [ ] Write a new test in `crates/tama-core/src/config/resolve/tests/gpu_device.rs`:
  - Name: `test_device_draft_injected_when_device_set`
  - Set up a `ModelConfig` with `gpu_device: Some("ROCm0".to_string())` and `backend: "llama_cpp"`
  - Call `build_full_args` and assert that both `["--device", "ROCm0"]` AND `["--device-draft", "ROCm0"]` appear in the args
- [ ] Write a second test: `test_device_draft_not_injected_when_device_none`
  - Same setup but with `gpu_device: None`
  - Assert that neither `--device` nor `--device-draft` appears in the args
- [ ] Write a third test: `test_device_draft_no_duplicate_when_already_set`
  - Set `args: vec!["--device-draft cuda1".to_string()]` and `gpu_device: Some("ROCm0".to_string())`
  - Assert that `--device-draft` appears exactly once and the user's value ("cuda1") is preserved
- [ ] Run `cargo test --package tama-core -- gpu_device`
  - All gpu_device tests (existing + new) should pass.
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "fix: mirror --device to --device-draft for MTP draft model placement"

**Acceptance criteria:**
- [ ] When `gpu_device` is set, both `--device` and `--device-draft` appear in built args with the same value
- [ ] When `gpu_device` is `None`, neither flag is injected
- [ ] If `--device-draft` is already in user args, it is not duplicated
- [ ] When the device is mapped (e.g. "GPU0" → "CUDA0"), both `--device` and `--device-draft` are overridden to the mapped value
- [ ] All existing `gpu_device` tests still pass
- [ ] New tests pass for `--device-draft` injection, no-injection, and no-duplicate cases
