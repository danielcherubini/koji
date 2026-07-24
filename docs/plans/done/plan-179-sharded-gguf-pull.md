# Sharded GGUF Pull Support

**Goal:** Support pulling sharded GGUF models where multiple files (shards) in a subdirectory belong to a single quant (e.g., `UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf` + 2 more shards).

**Architecture:** Sharded GGUF files are organized in subdirectories where the directory name IS the quant name. All `.gguf` files within a subdirectory are shards of that quant. The primary shard (first by sorted filename, typically `00001-of-0000N`) is what llama.cpp loads — it auto-discovers remaining shards in the same directory. The listing API groups shards into a single `QuantEntry`; the frontend shows one checkbox per quant; the backend pulls all shards but only the primary shard's filename is stored as `QuantInfo.file` in the model card.

**Tech Stack:** Rust, tama-core (proxy library), tama (Leptos SSR frontend), HuggingFace Hub API, SQLite (pull_queue, model_files tables).

---

### Task 1: Add `shards` field to `QuantEntry` types and update constructions

**Context:**
The listing API currently returns one `QuantEntry` per GGUF file. For sharded models, this means 3+ entries for a single quant. We need a `shards` field so the frontend can display one entry per quant and know which files to pull together. There are three `QuantEntry` types with a `filename` field that must all get the field: the core API response type in `tama_handlers/types.rs`, the frontend API response type in `api/hf.rs`, and the frontend wizard deserialization type in `pull_wizard/mod.rs`. Additionally, any code that constructs these types must be updated to include the new field.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/types.rs` — `QuantEntry` struct (line 22)
- Modify: `crates/tama/src/api/hf.rs` — private `QuantEntry` struct (line 73)
- Modify: `crates/tama/src/components/pull_wizard/mod.rs` — `QuantEntry` struct (line 42)
- Modify: `crates/tama-core/src/proxy/tama_handlers/tests.rs` — `QuantEntry` construction (line 302, in `test_quant_entry_serializes`)

**What to implement:**
Add `pub shards: Vec<String>` to all three `QuantEntry` structs. The field is empty for single-file quants and contains all shard rfilenames (sorted, full path with directory prefix) for sharded quants. The `filename` field is always the primary shard (first by sort order).

- `tama_handlers/types.rs`: Add `pub shards: Vec<String>` (this struct derives `Serialize` only — `#[serde(default)]` is a no-op but harmless for consistency)
- `api/hf.rs`: Add `shards: Vec<String>` (private struct, derives `Serialize` only)
- `pull_wizard/mod.rs`: Add `#[serde(default)] pub shards: Vec<String>` (this struct derives `Deserialize` — `#[serde(default)]` ensures old API responses without `shards` still deserialize)
- `tests.rs:302`: Add `shards: Vec::new()` to the `QuantEntry` construction

Note: `QuantEntry` in `config/types/model.rs` (line 40, has `file` field) and `QuantInfo` in `models/card.rs` (line 38, has `file` field) are model config types, NOT API response types. They do NOT need a `shards` field — the model card only references the primary file via `QuantInfo.file`.

**Steps:**
- [ ] Add `pub shards: Vec<String>` to `QuantEntry` in `tama-core/src/proxy/tama_handlers/types.rs`
- [ ] Add `shards: Vec<String>` to private `QuantEntry` in `tama/src/api/hf.rs`
- [ ] Add `#[serde(default)] pub shards: Vec<String>` to `QuantEntry` in `tama/src/components/pull_wizard/mod.rs`
- [ ] Add `shards: Vec::new()` to `QuantEntry` construction in `tama-core/src/proxy/tama_handlers/tests.rs:302`
- [ ] Run `cargo build --workspace` to verify compilation

**Acceptance criteria:**
- [ ] All three `QuantEntry` types (with `filename` field) have a `shards: Vec<String>` field
- [ ] `#[serde(default)]` is present on the deserializing struct (`pull_wizard/mod.rs`)
- [ ] `tests.rs` compiles with the new field
- [ ] `cargo build --workspace` succeeds

---

### Task 2: Add `group_sharded_quants` function with testable helper

**Context:**
The listing API (`handle_hf_list_quants` / `hf_metadata`) currently iterates over blob metadata and creates one `QuantEntry` per file. We need a pure function that groups sharded files (those with `/` in the rfilename) by their directory prefix, treating the directory name as the quant name. Files without `/` are single-file quants. This function must be testable without network calls.

**Files:**
- Modify: `crates/tama-core/src/models/pull/api.rs` — add `group_sharded_quants` function and `GroupedQuant` struct
- Modify: `crates/tama-core/src/models/pull/mod.rs` — add re-export (line 512)

**What to implement:**
- New struct `GroupedQuant` with `#[derive(Debug, Clone)]`:
  ```rust
  pub struct GroupedQuant {
      pub filename: String,       // primary shard (first by sort order)
      pub quant: Option<String>,  // directory name for sharded, inferred for single-file
      pub shards: Vec<String>,    // all shard rfilenames (sorted); empty for single-file
      pub size_bytes: Option<i64>, // sum of all shard sizes
      pub kind: crate::config::QuantKind, // QuantKind::from_filename(primary_shard)
  }
  ```
  Note: `QuantKind` is accessible via `crate::config::QuantKind` (re-exported from `config/mod.rs` line 17). No new import needed — use the full path in the struct definition.

- New function `group_sharded_quants(blobs: HashMap<String, BlobInfo>) -> Vec<GroupedQuant>`
- Logic:
  1. Iterate over blobs. For each filename:
     - If it contains `/`: extract directory prefix (everything before last `/`). Group with other files in the same directory. Quant name = directory name.
     - If no `/`: single-file quant. Quant name = `infer_quant_from_filename(filename)`.
  2. For each group: sort shard filenames, primary = first, `shards` = all shard filenames, `size_bytes` = sum of all shard sizes.
  3. For single-file: `shards` = empty, `filename` = the file, `size_bytes` = individual file size.
- Exclude non-GGUF files (already handled by `parse_blob_siblings` which only returns `.gguf` files).
- Re-export `group_sharded_quants` and `GroupedQuant` from `mod.rs` (add to the `pub use api::{...}` block at line 512).

**Steps:**
- [ ] Write failing test in `api.rs` test module with fixture data matching Laguna-S-2.1-GGUF structure (mixed single-file + sharded quants with known sizes)
- [ ] Run `cargo nextest run --package tama-core -- models::pull::api::tests::test_group_sharded_quants`
  - Did it fail with "function not found"? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `group_sharded_quants` and `GroupedQuant` in `api.rs`
- [ ] Add re-export in `mod.rs`
- [ ] Run `cargo nextest run --package tama-core -- models::pull::api::tests::test_group_sharded_quants`
  - Did all tests pass? If not, fix the failures and re-run before continuing.
- [ ] Run `cargo fmt`

**Acceptance criteria:**
- [ ] `group_sharded_quants` correctly groups 3 shards under `UD-Q4_K_XL/` into one entry with `shards` containing all 3 filenames
- [ ] Single-file quants (no `/`) get `shards = []`
- [ ] `size_bytes` for sharded quants = sum of all shard sizes
- [ ] Primary shard (first by sort order) is in `filename` field
- [ ] Quant name for sharded files = directory name
- [ ] Non-GGUF files (README.md, etc.) are excluded
- [ ] `cargo nextest run --package tama-core -- models::pull::api` passes

---

### Task 3: Update listing handlers to use grouped quants

**Context:**
Two handlers serve the HF quant listing API. The web UI handler (`tama/src/api/hf.rs::hf_metadata`) is the one actually used in production (the tama-core version is excluded from the unified router per `router.rs` line 160 comment). Both need updating to call `group_sharded_quants` and map results to `QuantEntry` with the `shards` field.

**Files:**
- Modify: `crates/tama/src/api/hf.rs` — `hf_metadata` function (line 50)
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs` — `handle_hf_list_quants` function (line 53)

**What to implement:**
In `hf_metadata` (api/hf.rs):
- Replace the `blobs.into_values().map(...)` with `tama_core::models::pull::group_sharded_quants(blobs)` (note: `tama_core::` prefix, NOT `crate::` — this is the `tama` crate)
- Map `GroupedQuant` → `QuantEntry` including `shards` field
- Keep the existing `quants.sort_by(|a, b| a.filename.cmp(&b.filename))` sort

In `handle_hf_list_quants` (system.rs):
- Same change: use `crate::models::pull::group_sharded_quants(blobs)` (note: `crate::` prefix — this is the `tama-core` crate)
- Map `GroupedQuant` → `QuantEntry` including `shards` field
- Keep the existing sort

**Steps:**
- [ ] Update `hf_metadata` in `api/hf.rs` to use `group_sharded_quants` with `tama_core::` prefix
- [ ] Update `handle_hf_list_quants` in `system.rs` to use `group_sharded_quants` with `crate::` prefix
- [ ] Run `cargo build --workspace`

**Acceptance criteria:**
- [ ] Both handlers return grouped quant entries with `shards` field
- [ ] Sort is preserved
- [ ] `cargo build --workspace` succeeds

---

### Task 4: Update path validation + add subdirectory creation in download.rs

**Context:**
`start_pull_from_queue` in `download.rs` validates filenames with `is_safe_path_component`, which rejects any filename containing `/`. Sharded files have paths like `UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf`. We need to use the already-implemented `is_safe_relative_path` instead, which allows `/` but still blocks `..`, `\`, and null bytes. Additionally, `dest_path` for sharded files includes a subdirectory (e.g., `models_dir/org/repo/UD-Q4_K_XL/file.gguf`) that doesn't exist yet — we must create it before downloading.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs` — import (line 5), filename validation (line 39), dest_path parent creation (after `let dest_path = dest_dir.join(&filename_clone);`)

**What to implement:**
1. Change import on line 5 from:
   ```rust
   use crate::proxy::tama_handlers::types::{is_safe_path_component, QuantDownloadSpec};
   ```
   to:
   ```rust
   use crate::proxy::tama_handlers::types::{
       is_safe_path_component, is_safe_relative_path, QuantDownloadSpec,
   };
   ```
   (Keep `is_safe_path_component` — it's still used on line 58 for `repo_id` validation)

2. Change line 39 from `is_safe_path_component(&filename_clone)` to `is_safe_relative_path(&filename_clone)`

3. After `let dest_path = dest_dir.join(&filename_clone);`, add:
   ```rust
   if let Some(parent) = dest_path.parent() {
       if let Err(e) = std::fs::create_dir_all(parent) {
           // Handle error same as dest_dir creation failure (the block above
           // that creates dest_dir at line ~119)
           let mut jobs = pull_jobs_arc.write().await;
           if let Some(job) = jobs.get_mut(&job_id_clone) {
               job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
               job.error = Some(format!("Failed to create dest subdir: {}", e));
           }
           drop(jobs);
           if let Some(ref svc) = state_clone.pull_queue {
               let _ = svc.update_status(
                   &job_id_clone,
                   "failed",
                   0,
                   None,
                   Some(&format!("Failed to create dest subdir: {}", e)),
                   None,
               );
           }
           return;
       }
   }
   ```
   Note: Do NOT include `in_flight_clone.lock().await.remove(&dest_path);` here — the in-flight dedup guard hasn't been entered yet at this point (it's at line ~145).

**Steps:**
- [ ] Update import to include both `is_safe_path_component` and `is_safe_relative_path`
- [ ] Change line 39 validation to `is_safe_relative_path`
- [ ] Add subdirectory creation after `dest_path` is set
- [ ] Run `cargo build --package tama-core`

**Acceptance criteria:**
- [ ] `is_safe_relative_path` is used for filename validation (line 39)
- [ ] `is_safe_path_component` is still used for repo_id validation (line 58)
- [ ] Sharded file subdirectory is created before download
- [ ] `cargo build --package tama-core` succeeds

---

### Task 5: Add primary shard detection to verification

**Context:**
`run_verification` in `verify.rs` already fetches blob metadata via `fetch_blob_metadata` to get the expected SHA. We can reuse this same call to determine if the current file is the primary shard (first by sorted filename within its directory). This avoids a redundant API call. The result is added to `VerificationOutcome` so `start_pull_from_queue` can decide whether to insert the quant into the model card.

To make this testable without network calls, extract a pure helper function `determine_primary_shard(filename: &str, blobs: &HashMap<String, BlobInfo>) -> bool`.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs` — `VerificationOutcome` struct and `run_verification` function

**What to implement:**
- Add `is_primary_shard: bool` field to `VerificationOutcome`
- Add `use std::collections::HashMap;` to imports (check if already present — `verify.rs` line 1 has `use std::collections::HashMap;`)
- Add `use crate::models::pull::BlobInfo;` to imports (BlobInfo is defined in `models/pull/mod.rs` and re-exported there)
- Add pure helper function:
  ```rust
  fn determine_primary_shard(filename: &str, blobs: &HashMap<String, BlobInfo>) -> bool {
      // Single-file quant (no directory) is always primary
      if !filename.contains('/') {
          return true;
      }
      // Extract directory prefix (everything before last '/')
      let dir_prefix = filename.rsplit_once('/').unwrap().0;
      // Find all blobs in the same directory, sort, check if current is first
      let mut siblings: Vec<&String> = blobs.keys()
          .filter(|k| k.starts_with(&format!("{}/", dir_prefix)))
          .collect();
      siblings.sort();
      siblings.first().map(|f| *f == filename).unwrap_or(true)
  }
  ```
- In `run_verification`, restructure the existing `fetch_blob_metadata` call to retain the blobs HashMap:
  ```rust
  // Replace the existing match block at line 52:
  let blobs_result = crate::models::pull::fetch_blob_metadata(&repo_id).await;
  let expected_sha: Option<String> = blobs_result
      .as_ref()
      .ok()
      .and_then(|blobs| blobs.get(&filename).and_then(|b| b.lfs_sha256.clone()));
  if let Err(e) = &blobs_result {
      tracing::warn!(job_id = %job_id, repo = %repo_id, error = %e,
          "Failed to fetch HF blob metadata for verification");
  }
  let is_primary_shard = match blobs_result.as_ref() {
      Ok(blobs) => determine_primary_shard(&filename, blobs),
      Err(_) => true, // fail-safe: default to primary for single-file quants
  };
  ```
  Then add `is_primary_shard` to both `VerificationOutcome` return paths (passed=true and passed=false).

**Steps:**
- [ ] Add `is_primary_shard: bool` field to `VerificationOutcome`
- [ ] Create `#[cfg(test)] mod tests { use super::*; ... }` at the bottom of `verify.rs` if it doesn't already exist
- [ ] Write failing test for `determine_primary_shard` with fixture data (sharded + single-file)
- [ ] Run `cargo nextest run --package tama-core -- pull::verify::tests::test_determine_primary_shard`
  - Did it fail? If it passed unexpectedly, stop and investigate why.
- [ ] Implement `determine_primary_shard` helper
- [ ] Restructure `run_verification` to retain blobs and compute `is_primary_shard`
- [ ] Run `cargo nextest run --package tama-core -- pull::verify`
- [ ] Run `cargo fmt`

**Acceptance criteria:**
- [ ] `VerificationOutcome` has `is_primary_shard: bool` field
- [ ] `determine_primary_shard` returns `true` for single-file quants (no `/`)
- [ ] `determine_primary_shard` returns `true` for the first shard (by sorted order)
- [ ] `determine_primary_shard` returns `false` for non-primary shards
- [ ] When `fetch_blob_metadata` fails, `is_primary_shard` defaults to `true`
- [ ] No additional API call beyond the existing `fetch_blob_metadata`

---

### Task 6: Skip card quant insert for non-primary shards + update test calls

**Context:**
`start_pull_from_queue` calls `setup_model_after_pull` for every completed pull. `setup_model_after_pull` is idempotent (finds existing model configs by `model == repo_id`), so calling it for non-primary shards is safe — it will find or create the model config. However, the card quant insert (`card.quants.insert(...)`) should only happen for the primary shard, otherwise it would overwrite the primary file reference with a non-primary shard's filename.

The race condition (non-primary shard completing before primary) is handled by always calling `setup_model_after_pull` — whichever shard completes first creates the model config, and the primary shard's completion sets the correct `QuantInfo.file`.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs` — `start_pull_from_queue` function (pass `outcome.is_primary_shard` to `setup_model_after_pull`)
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/verify.rs` — `_setup_model_after_pull_with_config` and `setup_model_after_pull` signatures + `card.quants.insert` guard
- Modify: `crates/tama-core/src/proxy/tama_handlers/tests.rs` — update all 5 calls to `_setup_model_after_pull_with_config` to pass `is_primary_shard: true`

**What to implement:**
1. In `verify.rs`:
   - Add `is_primary_shard: bool` parameter to `_setup_model_after_pull_with_config` (after `gguf_metadata` parameter)
   - Add `is_primary_shard: bool` parameter to `setup_model_after_pull` (after `gguf_metadata` parameter)
   - Pass `is_primary_shard` from `setup_model_after_pull` to `_setup_model_after_pull_with_config`
   - In `_setup_model_after_pull_with_config`, wrap `card.quants.insert(...)` (line 271) in `if is_primary_shard { ... }`
   - The `QuantInfo.file` for non-primary shards is not inserted into the card, so the primary shard's entry is preserved.

2. In `download.rs`:
   - Pass `outcome.is_primary_shard` as the new parameter to `setup_model_after_pull` (line ~435)
   - `upsert_file` and `update_verification` already run for all shards inside the `if outcome.passed` block — no change needed.

3. In `tests.rs`:
   - Update all 5 calls to `_setup_model_after_pull_with_config` (lines 39, 102, 122, 176, 202) to pass `true` for `is_primary_shard` (all test scenarios use single-file quants where the file IS the primary shard).

**Steps:**
- [ ] Add `is_primary_shard: bool` parameter to `_setup_model_after_pull_with_config`
- [ ] Add `is_primary_shard: bool` parameter to `setup_model_after_pull`
- [ ] Wrap `card.quants.insert(...)` in `if is_primary_shard` check
- [ ] Pass `outcome.is_primary_shard` to `setup_model_after_pull` in `download.rs`
- [ ] Update all 5 calls in `tests.rs` to pass `true`
- [ ] Run `cargo build --package tama-core`

**Acceptance criteria:**
- [ ] `setup_model_after_pull` accepts `is_primary_shard` parameter
- [ ] Card quant insert only happens for primary shard (`is_primary_shard = true`)
- [ ] `upsert_file` and `update_verification` still run for all shards
- [ ] Model config is created regardless of which shard completes first (idempotent)
- [ ] `tests.rs` compiles with the new parameter
- [ ] `cargo build --package tama-core` succeeds

---

### Task 7: Make `untracked_ggufs` recursive

**Context:**
`ModelRegistry::untracked_ggufs` in `registry.rs` only scans the top-level of the model directory. Sharded GGUFs live in subdirectories (e.g., `UD-Q4_K_XL/`). We need recursive scanning to detect untracked shard files.

**Files:**
- Modify: `crates/tama-core/src/models/registry.rs` — `untracked_ggufs` method (line 108)

**What to implement:**
- Replace flat `read_dir` with recursive directory walk
- Walk all subdirectories, collect `.gguf` files
- Compute relative paths (e.g., `UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf`)
- Skip files already tracked in the card's `quants` map (by `file` field)
- Note: `untracked_ggufs` is only called in tests currently (no production caller). Non-primary shards are not in the card's `quants` map, so they would appear as "untracked" — this is a known limitation. Acceptable since the function is test-only.

**Steps:**
- [ ] Write failing test with nested directory structure
- [ ] Run test, verify it fails
- [ ] Implement recursive scan using `std::fs::read_dir` with a helper function
- [ ] Run test, verify it passes
- [ ] Run `cargo fmt`

**Acceptance criteria:**
- [ ] `.gguf` files in subdirectories are detected as untracked
- [ ] Already-tracked files (in card's `quants`) are excluded
- [ ] Returned paths are relative to `model_dir` (include subdirectory)
- [ ] `cargo nextest run --package tama-core -- models::registry` passes

---

### Task 8: Update frontend wizard for grouped quants

**Context:**
The frontend wizard's `SelectionStep` shows one checkbox per `QuantEntry.filename`. With grouped quants, each `QuantEntry` represents a quant (possibly sharded). When a sharded quant is selected, all shard filenames must be sent to the pull API. The `selected_filenames` set currently stores individual filenames — we need it to expand to all shards when a sharded quant is selected.

**Files:**
- Modify: `crates/tama/src/components/pull_wizard/components/selection_step.rs` — checkbox toggle and select-all logic
- Modify: `crates/tama/src/components/pull_quant_wizard.rs` — `on_complete` Effect: add primary-shard filter

**What to implement:**

In `selection_step.rs`:
- The checkbox `on:change` handler should toggle all shards: when checking, insert `q.filename` AND all `q.shards` into `selected_filenames`; when unchecking, remove all of them
- The `is_checked` function should check if `q.filename` is in the set (primary shard presence indicates the quant is selected)
- "Select All" button should insert all `q.filename` + `q.shards` for all available quants

In `pull_quant_wizard.rs`:
- The `on_next` callback already collects `selected_filenames` into `filenames: Vec<String>` — this will now include all shard filenames (no change needed)
- The `on_complete` Effect (lines 72-95) emits `CompletedQuant` for every job. Since `CompletedQuant` is matched by `filename` in `model_editor/mod.rs`, and non-primary shards have different filenames, they will be processed individually. The model editor should handle this — non-primary shard completions will try to find a matching quant by `cq.quant` key and overwrite `row.file`. To prevent this, filter `on_complete` to only emit for primary shards:
  ```rust
  // Capture all three signals at the top of the Effect (untracked):
  let quants_listing = available_quants.get_untracked();
  let mmprojs = available_mmprojs.get_untracked();
  let mtps = available_mtps.get_untracked();
  // In the completed mapping, filter to only primary shards:
  .filter(|j| {
      quants_listing.iter().any(|q| q.filename == j.filename)
          || mmprojs.iter().any(|q| q.filename == j.filename)
          || mtps.iter().any(|q| q.filename == j.filename)
  })
  ```
  This ensures only the primary shard's `CompletedQuant` is emitted, preventing non-primary shards from overwriting the primary file reference in the model editor form.

**Steps:**
- [ ] Update `selection_step.rs` checkbox toggle to handle shards (clone `q.shards` alongside `q.filename` before the `view!` closure since `q` is consumed by `.map()`)
- [ ] Update `selection_step.rs` "Select All" to include shards
- [ ] Update `pull_quant_wizard.rs` `on_complete` to filter primary shards only
- [ ] Run `cargo build` for the tama crate

**Acceptance criteria:**
- [ ] Selecting a sharded quant adds all shard filenames to `selected_filenames`
- [ ] Deselecting removes all shard filenames
- [ ] "Select All" includes all shards
- [ ] `on_complete` only emits `CompletedQuant` for primary shards
- [ ] `cargo build` succeeds

---

### Task 9: Run full test suite, clippy, and fmt

**Context:**
After all changes, run the full verification gate.

**Files:**
- None (verification only)

**Steps:**
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
- [ ] Fix any failures

**Acceptance criteria:**
- [ ] `cargo fmt --all` succeeds with no changes
- [ ] `cargo clippy --workspace -- -D warnings` succeeds
- [ ] `cargo nextest run --workspace` passes all tests

---

### Known Limitations
- **`max_concurrent_pulls` limit**: The backend rejects requests with more than `max_concurrent_pulls()` files (default 8). With sharded quants, each quant contributes multiple files. Selecting 3 sharded quants × 3 shards = 9 files > 8 would be rejected. The frontend should warn the user. (Follow-up task)
- **`untracked_ggufs` and non-primary shards**: Non-primary shards are not in the card's `quants` map, so they appear as "untracked" in recursive scanning. This is acceptable since the function is test-only.
- **Non-primary shard `model_files` tracking**: Non-primary shards are recorded in `model_files` via `upsert_file` with the model_id found by `setup_model_after_pull` (which is idempotent). If the primary shard hasn't completed yet, `setup_model_after_pull` will create the model config on-the-fly for the non-primary shard.
