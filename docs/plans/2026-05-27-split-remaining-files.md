# Split Remaining Long Files Plan

**Goal:** Split 3 files > 1,000 LOC into focused sub-modules for readability and maintainability.

**Architecture:** Each large file is split by responsibility area into independent sub-modules. Test files are grouped by feature/endpoint. Each split is a single commit that compiles and passes all tests.

**Tech Stack:** Rust, cargo

**Dependencies:** None — tasks are independent of each other.

---

### Task 1: Split config/resolve/tests/args_building.rs (2,256 → 7 files)

**Context:**
The `args_building.rs` file contains 29 tests for `build_full_args`, all in a single 2,256 LOC file. The tests are grouped by feature area (basic args, context/num_parallel, unified slots, spec decoding, aliases) but not organized into separate files. This makes the file hard to navigate and slow to compile during test iterations.

**Files:**
- Create: `crates/tama-core/src/config/resolve/tests/basic.rs`
- Create: `crates/tama-core/src/config/resolve/tests/context_np.rs`
- Create: `crates/tama-core/src/config/resolve/tests/unified_slots.rs`
- Create: `crates/tama-core/src/config/resolve/tests/spec_decoding/mod.rs`
- Create: `crates/tama-core/src/config/resolve/tests/spec_decoding/mtp.rs`
- Create: `crates/tama-core/src/config/resolve/tests/spec_decoding/general.rs`
- Create: `crates/tama-core/src/config/resolve/tests/aliases.rs`
- Modify: `crates/tama-core/src/config/resolve/tests/mod.rs` (add module declarations)
- Modify: `crates/tama-core/src/config/resolve/tests/args_building.rs` (replace with `mod.rs`-style declarations only)

**What to implement:**

The existing `args_building.rs` contains 29 tests. Group them as follows:

1. **`basic.rs`** — 6 tests: `test_build_full_args_unified`, `test_build_full_args_ctx_override`, `test_build_full_args_no_sampling`, `test_build_full_args_no_quants`, `test_build_args_sampling_overrides_inline_temp_in_args`, `test_build_full_args_returns_flat_tokens_with_quoted_path`

2. **`context_np.rs`** — 6 tests: `test_build_full_args_context_multiplied_by_num_parallel`, `test_build_full_args_context_saturating_overflow`, `test_build_full_args_context_no_num_parallel_defaults_to_one`, `test_build_full_args_injects_np_flag`, `test_build_full_args_no_np_when_auto`, `test_build_full_args_np_when_one`

3. **`unified_slots.rs`** — 5 tests: `test_build_full_args_unified_n_slots`, `test_build_full_args_non_unified_n_slots`, `test_build_full_args_unified_default`, `test_build_full_args_ctx_override_unified`, `test_build_full_args_kv_unified_not_duplicated_when_in_user_args`

4. **`spec_decoding/mod.rs`** — Module declarations only:
   ```rust
   mod general;
   mod mtp;
   ```

5. **`spec_decoding/mtp.rs`** — 5 tests: `test_spec_decoding_flags_injected`, `test_spec_decoding_no_duplicate_when_in_args`, `test_spec_decoding_draft_ngl_only_for_mtp`, `test_spec_decoding_draft_ngl_value_99`, `test_spec_decoding_empty_types_no_flags`

6. **`spec_decoding/general.rs`** — 3 tests: `test_spec_decoding_multi_type_comma_separated`, `test_spec_decoding_non_llama_backend_no_flags`, `test_spec_decoding_all_already_has_checks`

7. **`aliases.rs`** — 4 tests: `test_build_full_args_injects_alias_for_llama_cpp`, `test_build_full_args_no_alias_for_non_llama_cpp`, `test_build_full_args_alias_falls_back_to_model`, `test_build_full_args_respects_user_alias`

Each new file must include the imports it needs from the original file. The shared imports are:

For the flat files (`basic.rs`, `context_np.rs`, `unified_slots.rs`, `aliases.rs`):
```rust
use std::collections::BTreeMap;
use tempfile::tempdir;
use crate::config::types::{QuantEntry, SpecDecodingConfig};
use super::super::*;
```

For the `spec_decoding/` sub-files (`mtp.rs`, `general.rs`), they are nested one level deeper so they need **three** levels up:
```rust
use std::collections::BTreeMap;
use tempfile::tempdir;
use crate::config::types::{QuantEntry, SpecDecodingConfig};
use super::super::super::*;
```

In `mod.rs`, add the new module declarations:
```rust
mod basic;
mod context_np;
mod unified_slots;
mod spec_decoding;
mod aliases;
```
Remove the `mod args_building;` line (or rename args_building.rs to just contain the old module declarations if preferred — but cleaner to remove it entirely and add new `mod` lines).

**Steps:**
- [ ] Read `crates/tama-core/src/config/resolve/tests/args_building.rs` completely
- [ ] Create `basic.rs` with the 6 basic tests and necessary imports
- [ ] Create `context_np.rs` with the 6 context/np tests and necessary imports
- [ ] Create `unified_slots.rs` with the 5 unified slots tests and necessary imports
- [ ] Create `spec_decoding/mod.rs` with module declarations
- [ ] Create `spec_decoding/mtp.rs` with the 5 MTP tests and necessary imports
- [ ] Create `spec_decoding/general.rs` with the 3 general tests and necessary imports
- [ ] Create `aliases.rs` with the 4 alias tests and necessary imports
- [ ] In `mod.rs`, add `mod basic; mod context_np; mod unified_slots; mod spec_decoding; mod aliases;`
- [ ] In `mod.rs`, remove `mod args_building;`
- [ ] Delete the original file `crates/tama-core/src/config/resolve/tests/args_building.rs` (all its contents have been moved to the 6 new files)
- [ ] Run `cargo test --package tama-core -- config::resolve::tests`
  - Did all tests pass? If not, fix missing imports and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "refactor(core): split args_building tests into 6 focused modules"

**Acceptance criteria:**
- [ ] `args_building.rs` no longer exists
- [ ] 7 new files exist (basic.rs, context_np.rs, unified_slots.rs, spec_decoding/mod.rs, spec_decoding/mtp.rs, spec_decoding/general.rs, aliases.rs)
- [ ] All 29 tests still pass: `cargo test --package tama-core -- config::resolve::tests`
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 2: Split proxy/handlers/tests.rs (1,530 → 5 files)

**Context:**
The `proxy/handlers/tests.rs` file contains 1,530 LOC of integration tests for the proxy handlers. Tests are grouped by endpoint (parse models, list models, get model, forward, aliases) but all live in one file. The handlers themselves are already split into modules (chat.rs, forward.rs, models.rs, status.rs), so the tests should follow the same pattern.

**Files:**
- Create: `crates/tama-core/src/proxy/handlers/list_models_tests.rs`
- Create: `crates/tama-core/src/proxy/handlers/get_model_tests.rs`
- Create: `crates/tama-core/src/proxy/handlers/forward_tests.rs`
- Create: `crates/tama-core/src/proxy/handlers/alias_tests.rs`
- Modify: `crates/tama-core/src/proxy/handlers/tests.rs` (replace with shared helpers + module declarations)
- Modify: `crates/tama-core/src/proxy/handlers/mod.rs` (add new module declarations if needed)

**What to implement:**

**CRITICAL:** New test files are placed in `crates/tama-core/src/proxy/handlers/` **alongside** `tests.rs`, NOT inside a `tests/` subdirectory. They are sibling modules of `tests`.

Read `tests.rs` completely. The file has shared helpers at the top (`create_test_state`, `create_forward_post_request`, `create_forward_get_request`, `create_state_with_two_backends`) and then tests grouped by area.

1. **`tests.rs`** — Keep as-is but convert to module style:
   - Keep all shared helper functions (`create_test_state`, `create_forward_post_request`, `create_forward_get_request`, `create_state_with_two_backends`)
   - Keep all imports
   - Add module declarations at the bottom:
     ```rust
     mod list_models_tests;
     mod get_model_tests;
     mod forward_tests;
     mod alias_tests;
     ```
   - Remove all test functions (they move to new files)

In `crates/tama-core/src/proxy/handlers/mod.rs`, add the new module declarations alongside the existing `#[cfg(test)] mod tests;`:
```rust
#[cfg(test)]
mod list_models_tests;
#[cfg(test)]
mod get_model_tests;
#[cfg(test)]
mod forward_tests;
#[cfg(test)]
mod alias_tests;
```

2. **`list_models_tests.rs`** — Tests for GET /v1/models:
   - `test_parse_models_response_valid_data`
   - `test_parse_models_response_invalid_json`
   - `test_parse_models_response_missing_data_field`
   - `test_parse_models_response_data_not_array`
   - `test_parse_models_response_empty_data_array`
   - `test_parse_models_response_empty_body`
   - `test_parse_models_response_data_is_object`
   - `test_handle_list_models_returns_api_name`
   - `test_handle_list_models_merges_backend_responses_with_meta`
   - `test_handle_list_models_unloaded_from_config`
   - `test_handle_list_models_deduplicates_model_ids`
   - `test_handle_list_models_backend_failure_fallback`
   - `test_handle_list_models_response_shape`
   - `test_handle_list_models_alias_deduplication`
   - `test_handle_list_models_no_alias_no_normalization`
   - Needs: `use super::tests::*;` for shared helpers, plus handler imports

3. **`get_model_tests.rs`** — Tests for GET /v1/models/{id}:
   - `test_handle_get_model_by_config_key_returns_api_name`
   - `test_handle_get_model_by_api_name_returns_api_name`
   - `test_handle_get_model_without_api_name_falls_back_to_config_key`
   - `test_handle_get_model_fetches_from_backend_with_meta`
   - `test_handle_get_model_fallback_to_config_when_not_loaded`
   - `test_handle_get_model_404_for_unknown_model`
   - `test_handle_get_model_matches_by_model_field_when_multiple`
   - `test_handle_get_model_backend_failure_fallback`
   - `test_handle_get_model_normalizes_id_from_alias`
   - `test_find_model_in_entries_matches_by_alias`
   - `test_find_model_in_entries_single_entry`
   - Needs: `use super::tests::*;` for shared helpers

4. **`forward_tests.rs`** — Tests for wildcard forwarding:
   - `test_handle_forward_post_model_extraction_from_json_body`
   - `test_handle_forward_post_non_json_body_does_not_crash`
   - `test_handle_forward_post_missing_model_no_servers_returns_503`
   - `test_handle_forward_post_load_model_error_returns_500`
   - `test_handle_forward_get_no_servers_returns_503`
   - `test_handle_forward_get_empty_body_does_not_crash`
   - Needs: `use super::tests::*;` for shared helpers

5. **`alias_tests.rs`** — Tests for alias resolution:
   - `test_chat_completions_resolves_alias`
   - `test_list_models_includes_aliases`
   - `test_get_model_resolves_alias`
   - Needs: `use super::tests::*;` for shared helpers

Each new file must use `use super::tests::*;` to access shared helpers from tests.rs. They also need the handler imports from the original tests.rs.

**Steps:**
- [ ] Read `crates/tama-core/src/proxy/handlers/tests.rs` completely
- [ ] Identify all shared helper functions and their exact signatures
- [ ] Create `list_models_tests.rs` with 15 list-models tests + `use super::tests::*;`
- [ ] Create `get_model_tests.rs` with 11 get-model tests + `use super::tests::*;`
- [ ] Create `forward_tests.rs` with 6 forward tests + `use super::tests::*;`
- [ ] Create `alias_tests.rs` with 3 alias tests + `use super::tests::*;`
- [ ] In `tests.rs`, remove all test functions, keep only imports + shared helpers + add `mod` declarations
- [ ] Run `cargo test --package tama-core -- proxy::handlers`
  - Did all tests pass? If not, fix missing imports and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "refactor(core): split proxy handler tests into 4 focused modules"

**Acceptance criteria:**
- [ ] `tests.rs` contains only shared helpers + module declarations (no test functions)
- [ ] 4 new test files exist (list_models_tests.rs, get_model_tests.rs, forward_tests.rs, alias_tests.rs)
- [ ] All proxy handler tests still pass: `cargo test --package tama-core -- proxy::handlers`
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 3: Split db/backfill.rs (1,023 → 3 files)

**Context:**
The `db/backfill.rs` file contains 1,023 LOC of database backfill logic. It has 4 distinct public functions: `run_initial_backfill`, `migrate_backend_registry_toml`, `migrate_backend_config_from_toml`, `repair_orphaned_model_files`, plus `backfill_hf_metadata`. The first 4 are one-time migration logic, while `backfill_hf_metadata` is a distinct HF metadata backfill concern.

**Files:**
- Create: `crates/tama-core/src/db/initial_backfill.rs`
- Create: `crates/tama-core/src/db/hf_metadata.rs`
- Modify: `crates/tama-core/src/db/backfill.rs` (replace with module declarations + re-exports)
- Modify: `crates/tama-core/src/db/mod.rs` (add new module declarations)

**What to implement:**

Read `backfill.rs` completely. Identify the boundaries:

1. **`backfill.rs`** — Convert to module style:
   - Keep shared types: `LegacyRegistryData`, `LegacyBackendInfo` (ensure visibility is sufficient for child modules — default private is fine since child modules can access parent's items)
   - Add module declarations:
     ```rust
     mod initial_backfill;
     mod hf_metadata;
     ```
   - Re-export public functions:
     ```rust
     pub use initial_backfill::*;
     pub use hf_metadata::*;
     ```
   - Remove all function implementations (they move to new files)

2. **`initial_backfill.rs`** — One-time migration logic:
   - `pub async fn run_initial_backfill`
   - `pub fn migrate_backend_registry_toml`
   - `pub fn migrate_backend_config_from_toml`
   - `pub fn repair_orphaned_model_files`
   - Needs: `use super::backfill::{LegacyRegistryData, LegacyBackendInfo};` for shared types
   - Include the `#[cfg(test)]` module if any tests are specific to these functions

3. **`hf_metadata.rs`** — HF metadata backfill:
   - `pub async fn backfill_hf_metadata`
   - `mod tests` (if tests exist for this function)
   - Needs: `use super::backfill::*;` for shared types if needed

**Steps:**
- [ ] Read `crates/tama-core/src/db/backfill.rs` completely
- [ ] Identify exact line boundaries for each function and shared types
- [ ] Create `initial_backfill.rs` with the 4 migration functions + necessary imports
- [ ] Create `hf_metadata.rs` with `backfill_hf_metadata` + its tests + necessary imports
- [ ] In `backfill.rs`, keep only shared types + add `mod` declarations + `pub use` re-exports
- [ ] Verify `db/mod.rs` already has `pub mod backfill;` (no change needed if it does)
- [ ] Run `cargo check --package tama-core`
  - Did it succeed? If not, fix missing imports and re-run.
- [ ] Run `cargo test --package tama-core -- db::backfill`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "refactor(core): split db/backfill.rs into initial_backfill and hf_metadata modules"

**Acceptance criteria:**
- [ ] `backfill.rs` contains only shared types + module declarations + re-exports
- [ ] `initial_backfill.rs` exists with 4 migration functions
- [ ] `hf_metadata.rs` exists with `backfill_hf_metadata`
- [ ] `cargo check --package tama-core` passes
- [ ] All backfill tests pass: `cargo test --package tama-core -- db::backfill`
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

## Verification

After all 3 tasks are complete:

```bash
# Full workspace check
cargo build --workspace
cargo test --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
```

**Expected LOC reduction:**
- `args_building.rs` (2,256) → max single file ~500 LOC
- `handlers/tests.rs` (1,530) → max single file ~700 LOC (tests.rs with helpers)
- `backfill.rs` (1,023) → max single file ~450 LOC
