# GPU Device Selection Plan

**Goal:** Allow each model to be assigned to a specific GPU (e.g., `ROCm0`, `ROCm1`) so multiple models can run on separate GPUs.

**Architecture:** Add `gpu_device: Option<String>` to `ModelConfig`. When `gpu_device` is set and the backend is llama.cpp-compatible, inject `--device <value>` into the server's command-line args during `build_full_args()`. The value is a device name (e.g., `ROCm0`, `CUDA1`) as reported by `llama-server --list-devices`.

**Tech Stack:** Rust, SQLite, llama.cpp `--device` flag

---

### Task 1: Add `gpu_device` field to ModelConfig, DB schema, and queries

**Context:**
The `ModelConfig` struct and its DB-backed storage (`model_configs` table) need a new `gpu_device` column. This is the core data model change — everything else (arg injection, CLI flag) depends on it. Follows the same pattern as recent additions like `selected_mtp_model` (migration _0028), `cache_type_k/v` (migration _0018), and `gpu_variant` (migration _0021).

**Files:**
- Modify: `crates/tama-core/src/config/types.rs` — add `gpu_device` to `ModelConfig`, `to_db_record()`, `from_db_record()`
- Modify: `crates/tama-core/src/db/queries/types.rs` — add `gpu_device` to `ModelConfigRecord`
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs` — add `gpu_device` to all SQL queries (upsert, get_by_id, get_by_repo_id, get_all)
- Create: `crates/tama-core/src/db/migrations/_0029_add_gpu_device.rs` — ALTER TABLE migration (no COLLATE NOCASE — consistent with `cache_type_k/v` and `gpu_variant`)
- Modify: `crates/tama-core/src/db/migrations.rs` — register new migration, bump `LATEST_VERSION` to 29
- Modify: `crates/tama-core/src/db/migrations/migrations_tests.rs` — add `test_migration_v29_adds_gpu_device_column` regression test
- Modify: `crates/tama-core/src/models/card.rs` — add `default_gpu_device: Option<String>` to `ModelMeta` (model card default, analogous to `default_gpu_layers`)
- Modify: `crates/tama-cli/src/commands/model/pull.rs` — add `gpu_device: None` to all `ModelConfigRecord` constructions (×6 sites: lines ~344, ~505, ~569, ~625, ~673, ~713)
- Modify: `crates/tama-core/src/models/manager_tests.rs` — add `gpu_device: None` to `make_test_record()` helper
- Modify: `crates/tama-core/src/db/queries/tests.rs` — add `gpu_device: None` to all `ModelConfigRecord` constructions (×4 sites)
- Modify: `crates/tama-core/src/db/backfill/hf_metadata.rs` — add `gpu_device: None` to `ModelConfigRecord` constructions (×2 sites)
- Modify: `crates/tama-core/src/db/backfill/initial_backfill.rs` — add `gpu_device: None` to `ModelConfigRecord` constructions (×2 sites)
- Modify: `crates/tama-web/src/types/config.rs` — add `gpu_device: Option<String>` to web's `ModelConfig` mirror type and both `From` impls
- Modify: `crates/tama-web/src/api/models/crud/mod.rs` — add `gpu_device: Option<String>` to `ModelBody`, add to `ModelConfig` constructions, pass through in apply logic

**What to implement:**

1. In `config/types.rs`, add to `ModelConfig`:
   ```rust
   /// GPU device name for this model (e.g. "ROCm0", "CUDA1").
   /// Passed as `--device` to llama.cpp backends.
   /// When None, the backend uses its default device selection.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub gpu_device: Option<String>,
   ```
   Place it near `gpu_variant` and `gpu_layers` for logical grouping (after `gpu_variant` field).

2. In `config/types.rs`, update `to_db_record()` — add `gpu_device: self.gpu_device.clone()` to the `ModelConfigRecord` construction.

3. In `config/types.rs`, update `from_db_record()` — add `gpu_device: record.gpu_device.clone()` to the `Self { ... }` construction.

4. In `db/queries/types.rs`, add to `ModelConfigRecord`:
   ```rust
   pub gpu_device: Option<String>,
   ```
   Place it after `gpu_variant` for logical grouping.

5. In `db/queries/model_config_queries.rs`, update ALL four SQL functions:
   - `upsert_model_config`: Add `gpu_device` to INSERT columns, VALUES, and ON CONFLICT DO UPDATE
   - `get_model_config`: Add `gpu_device` to SELECT columns and row mapping
   - `get_model_config_by_repo_id`: Same as get_model_config
   - `get_all_model_configs`: Same as get_model_config
   The column should be placed after `gpu_variant` in all queries for consistency.

6. Create `db/migrations/_0029_add_gpu_device.rs`:
   ```rust
   /// v29 — Add gpu_device column to model_configs.
   /// Stores the GPU device name (e.g. "ROCm0", "CUDA1") for per-model GPU placement.
   /// Passed as `--device` to llama.cpp backends.
   pub const MIGRATION: (i32, bool, &str) = (
       29,
       false,
       r#"
           ALTER TABLE model_configs ADD COLUMN gpu_device TEXT COLLATE NOCASE;
       "#,
   );
   ```

7. In `db/migrations.rs`:
   - Add `mod _0029_add_gpu_device;` to the module declarations
   - Add `_0029_add_gpu_device::MIGRATION,` to the `MIGRATIONS` array
   - Bump `LATEST_VERSION` from 28 to 29

8. In `models/card.rs`, add to `ModelMeta`:
   ```rust
   /// Default GPU device for this model (e.g. "ROCm0", "CUDA1").
   /// Passed as `--device` to llama.cpp backends when the model config
   /// does not override it.
   #[serde(default)]
   pub default_gpu_device: Option<String>,
   ```

**Steps:**
- [ ] Add `gpu_device: Option<String>` field to `ModelConfig` in `config/types.rs` (place after `gpu_variant` for logical grouping, with `#[serde(default, skip_serializing_if = "Option::is_none")]`)
- [ ] Add `gpu_device: Option<String>` to `ModelConfigRecord` in `db/queries/types.rs` (place after `gpu_variant`)
- [ ] Update `to_db_record()` in `config/types.rs` — add `gpu_device: self.gpu_device.clone()` to the record construction
- [ ] Update `from_db_record()` in `config/types.rs` — add `gpu_device: record.gpu_device.clone()` to the `Self { ... }` construction
- [ ] Create migration file `db/migrations/_0029_add_gpu_device.rs` with:
  ```rust
  pub const MIGRATION: (i32, bool, &str) = (
      29, false,
      r#"ALTER TABLE model_configs ADD COLUMN gpu_device TEXT;"#,
  );
  ```
  (No `COLLATE NOCASE` — consistent with `cache_type_k/v` and `gpu_variant` migrations.)
- [ ] Register migration in `db/migrations.rs`: add `mod _0029_add_gpu_device;`, add to `MIGRATIONS` array, bump `LATEST_VERSION` to 29
- [ ] Update all 4 SQL functions in `db/queries/model_config_queries.rs` — add `gpu_device` to INSERT columns, VALUES params, ON CONFLICT DO UPDATE, SELECT columns, and row mapping (place after `gpu_variant` in all queries)
- [ ] Add `default_gpu_device: Option<String>` to `ModelMeta` in `models/card.rs`
- [ ] Add `gpu_device: None` to ALL `ModelConfigRecord` direct constructions:
  - `crates/tama-cli/src/commands/model/pull.rs` — ×6 sites (lines ~344, ~505, ~569, ~625, ~673, ~713)
  - `crates/tama-core/src/models/manager_tests.rs` — `make_test_record()` helper
  - `crates/tama-core/src/db/queries/tests.rs` — ×4 sites
  - `crates/tama-core/src/db/backfill/hf_metadata.rs` — ×2 sites
  - `crates/tama-core/src/db/backfill/initial_backfill.rs` — ×2 sites
- [ ] Add migration regression test in `db/migrations/migrations_tests.rs`:
  ```rust
  #[test]
  fn test_migration_v29_adds_gpu_device_column() {
      let conn = Connection::open_in_memory().unwrap();
      run(&conn).unwrap();
      let col_exists: i64 = conn.query_row(
          "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='gpu_device'",
          [], |row| row.get(0),
      ).unwrap();
      assert_eq!(col_exists, 1, "gpu_device column must exist after migration v29");
  }
  ```
- [ ] Update web crate mirror types in `crates/tama-web/src/types/config.rs`:
  - Add `pub gpu_device: Option<String>` to the web `ModelConfig` struct (place after `gpu_variant`)
  - Add `gpu_device: b.gpu_device` to `From<BackendModelConfig>` impl
  - Add `gpu_device: m.gpu_device` to both `From<ModelConfig>` impls
- [ ] Update web API in `crates/tama-web/src/api/models/crud/mod.rs`:
  - Add `pub gpu_device: Option<String>` to `ModelBody` struct
  - Add `gpu_device: body.gpu_device.clone()` to `ModelConfig` constructions (×2-3 sites)
- [ ] Extend the existing `test_model_config_round_trip` test in `config/types.rs` to also set and assert `gpu_device` (this test uses `..Default::default()` so it won't fail to compile, but it should exercise the new field)
- [ ] Run `cargo build --workspace`
  - Did it compile? If not, fix compilation errors (likely missing `gpu_device: None` at some construction site).
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add gpu_device field to ModelConfig and DB schema"

**Acceptance criteria:**
- [ ] `ModelConfig` has `gpu_device: Option<String>` field
- [ ] `ModelConfigRecord` has `gpu_device: Option<String>` field
- [ ] Migration _0029 adds `gpu_device` column to `model_configs` table
- [ ] `to_db_record()` / `from_db_record()` round-trip preserves `gpu_device`
- [ ] All 4 SQL queries include `gpu_device` in SELECT/INSERT/UPDATE
- [ ] `ModelMeta` has `default_gpu_device: Option<String>` field
- [ ] `cargo test --package tama-core` passes
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 2: Inject `--device` flag during server launch

**Context:**
When `gpu_device` is set on a `ModelConfig`, the `--device <value>` flag must be injected into the args passed to `llama-server`. This happens in `build_full_args()` in `config/resolve/mod.rs`, which already injects many flags (`-m`, `-c`, `-ngl`, `--kv-unified`, etc.). The injection follows the same pattern: check if the flag is already present in args, and if not, append it.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs` — inject `--device` in `build_full_args()`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` — no changes needed (uses `build_full_args()` which now handles `--device`)

**What to implement:**

In `config/resolve/mod.rs`, inside `build_full_args()`, add a new injection block **after** the `--alias` injection and **before** the sampling merge. The block should:

1. Only apply to llama.cpp backends (gate on `is_llama_cpp_backend`)
2. Check if `server.gpu_device` is `Some` and non-empty
3. Check if `--device` is already present in args (to allow manual override)
4. If not present, append `--device <gpu_device>` to the grouped args

```rust
// Inject --device (GPU device selection) when configured.
if is_llama_cpp_backend {
    if let Some(ref device) = server.gpu_device {
        let trimmed = device.trim();
        if !trimmed.is_empty() {
            let already_has_device = grouped.iter().any(|e| {
                matches!(
                    crate::config::flag_name(e),
                    Some("--device")
                )
            });
            if !already_has_device {
                grouped.push(format!("--device {}", trimmed));
            }
        }
    }
}
```

Place this block after the `--alias` injection (after the `if is_llama_cpp_backend { let alias_value = ... }` block, around line ~510) and before the sampling merge section. Match only `--device` — the `-dev` short form does not exist in llama.cpp.

**Steps:**
- [ ] Create test file `config/resolve/tests/gpu_device.rs` with 4 tests:
  1. `test_gpu_device_injected_for_rocm` — When `gpu_device = Some("ROCm0")` and backend is `llama_cpp`, `--device ROCm0` appears in built args
  2. `test_gpu_device_none_no_injection` — When `gpu_device = None`, no `--device` flag is added
  3. `test_gpu_device_no_duplicate_when_already_set` — When `--device` is already in `server.args`, it is NOT duplicated
  4. `test_gpu_device_not_injected_for_non_llama_cpp` — When `gpu_device` is set but backend is `ik_llama`, no `--device` flag is added
  5. `test_gpu_device_empty_string_no_injection` — When `gpu_device = Some("   ")`, no `--device` flag is added (mirrors `test_kv_cache_type_args_not_injected_for_empty_string` pattern)
- [ ] Register the new test module by adding `mod gpu_device;` to `config/resolve/tests/mod.rs`
- [ ] Run `cargo test --package tama-core -- config::resolve::tests::gpu_device`
  - Did the tests fail (expected — feature not implemented yet)?
- [ ] Implement the `--device` injection in `build_full_args()` in `config/resolve/mod.rs`:
  - Place the block after the `--alias` injection (around line ~510, after the `if is_llama_cpp_backend { let alias_value = ... }` block) and before the sampling merge
  - Match only `--device` (not `-dev` — that short form does not exist in llama.cpp)
  - Use `trimmed = device.trim(); if !trimmed.is_empty()` pattern (consistent with `cache_type_k/v` handling)
- [ ] Run `cargo test --package tama-core -- config::resolve::tests::gpu_device`
  - Did all tests pass?
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: inject --device flag for gpu_device in build_full_args"

**Acceptance criteria:**
- [ ] When `ModelConfig.gpu_device = Some("ROCm0")` and backend is llama.cpp, `--device ROCm0` appears in built args
- [ ] When `gpu_device = None`, no `--device` flag is added
- [ ] When `--device` is already in manual args, it is NOT duplicated
- [ ] When backend is NOT llama.cpp, `--device` is NOT injected even if `gpu_device` is set
- [ ] When `gpu_device = Some("   ")` (whitespace-only), no `--device` flag is added
- [ ] All existing tests still pass
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 3: CLI support — `--gpu-device` flag for server add/edit

**Context:**
The CLI needs a `--gpu-device` flag so users can specify GPU placement when adding or editing servers. This follows the same pattern as `--port`, `--ctx`, `--quant`, etc. — extracted in `flags.rs`, used in `server/add.rs` and `server/edit.rs`.

**Files:**
- Modify: `crates/tama-cli/src/flags.rs` — add `gpu_device` to `ExtractedFlags`, parse `--gpu-device`
- Modify: `crates/tama-cli/src/handlers/server/add.rs` — use `extracted.gpu_device` when building `ModelConfig`
- Modify: `crates/tama-cli/src/handlers/server/edit.rs` — use `extracted.gpu_device` when updating `ModelConfig`

**What to implement:**

1. In `crates/tama-cli/src/flags.rs`, add to `ExtractedFlags`:
   ```rust
   /// GPU device name (e.g. "ROCm0", "CUDA1")
   pub gpu_device: Option<String>,
   ```

2. In `extract_tama_flags()`, handle `--gpu-device` / `-gd` in both `--flag=value` and `--flag value` syntaxes:
   ```rust
   "--gpu-device" | "-gd" => {
       gpu_device = Some(value.to_string());
       i += 1;
   }
   ```
   And for the traditional syntax:
   ```rust
   "--gpu-device" | "-gd" => {
       if i + 1 >= args.len() {
           anyhow::bail!("--gpu-device/-gd flag requires a value");
       }
       gpu_device = Some(args[i + 1].clone());
       i += 2;
   }
   ```

3. In `crates/tama-cli/src/handlers/server/add.rs`, add to the `ModelConfig` construction:
   ```rust
   gpu_device: extracted.gpu_device.clone(),
   ```
   Place it near `gpu_variant` for logical grouping.

4. In `crates/tama-cli/src/handlers/server/edit.rs`, add selective merge:
   ```rust
   if let Some(ref gpu_device) = extracted.gpu_device {
       srv.gpu_device = Some(gpu_device.clone());
   }
   ```

**Steps:**
- [ ] Add `gpu_device` field to `ExtractedFlags` in `flags.rs`
- [ ] Add `--gpu-device` / `-gd` parsing in both `--flag=value` and `--flag value` branches of `extract_tama_flags()`
- [ ] Update `Ok(ExtractedFlags { ... })` to include `gpu_device`
- [ ] Add `gpu_device: extracted.gpu_device.clone()` to `ModelConfig` construction in `server/add.rs`
- [ ] Add selective merge for `gpu_device` in `server/edit.rs`
- [ ] Run `cargo build --package tama-cli`
  - Did it compile? If not, fix errors.
- [ ] Run `cargo test --package tama-cli`
  - Did all tests pass?
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-cli -- -D warnings`
- [ ] Commit with message: "feat: add --gpu-device CLI flag for server add/edit"

**Acceptance criteria:**
- [ ] `--gpu-device ROCm0` is parsed correctly by `extract_tama_flags()`
- [ ] `-gd ROCm0` is parsed correctly (short form)
- [ ] `--gpu-device=ROCm0` is parsed correctly (equals syntax)
- [ ] Server add includes `gpu_device` in the saved `ModelConfig`
- [ ] Server edit updates `gpu_device` when `--gpu-device` is provided
- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

### Task 4: Resolve `gpu_device` from model card default (optional fallback)

**Context:**
When a model config does not specify `gpu_device`, fall back to the model card's `default_gpu_device` (added in Task 1). This mirrors how `default_gpu_layers` and `default_context_length` work — the model card provides defaults that the model config can override.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` — resolve effective `gpu_device` in `load_model()`

**What to implement:**

In `proxy/lifecycle/mod.rs::load_model()`, the `_model_card` parameter is currently unused. Remove the `_` prefix and use it for the fallback. The approach: resolve the effective `gpu_device` at the call site (where both model config and model card are available), clone the server config, set the resolved value, and pass the modified copy to `build_full_args()`.

**Exact changes in `load_model()` (after resolving `server_config`, before `build_full_args`):**

1. Rename `_model_card` to `model_card` in the function signature (line ~21):
   ```rust
   pub async fn load_model(
       &self,
       model_name: &str,
       model_card: Option<&crate::models::card::ModelCard>,  // remove _ prefix
   ) -> Result<String> {
   ```

2. After resolving `server_config` and `backend_config` (around line ~50), before building args:
   ```rust
   // Resolve effective gpu_device: model config > model card default
   let effective_gpu_device = server_config.gpu_device.clone().or_else(|| {
       model_card.and_then(|card| card.model.default_gpu_device.clone())
   });

   // Build a modified server config with the resolved gpu_device.
   // This ensures build_full_args() sees the effective value.
   let server_config = if effective_gpu_device.is_some() && server_config.gpu_device.is_none() {
       let mut modified = server_config.clone();
       modified.gpu_device = effective_gpu_device;
       modified
   } else {
       server_config.clone()
   };
   ```

3. The existing `build_full_args()` call (around line ~97) already uses `server_config`, so it will pick up the resolved value automatically. No change needed to `build_full_args()` signature.

**Note on call sites:** `load_model()` is called from multiple places. Most pass `None` for the model card (e.g., `crates/tama-core/src/proxy/tama_handlers/models.rs:162`). The fallback simply won't apply on those paths — the model config's `gpu_device` is used directly. This is acceptable: the fallback is a convenience for model card authors, not a requirement.

**Steps:**
- [ ] Rename `_model_card` to `model_card` in `load_model()` function signature in `proxy/lifecycle/mod.rs`
- [ ] Add the effective `gpu_device` resolution logic (as shown above) before the `build_full_args()` call
- [ ] Write a test that verifies the fallback chain:
  - Create a `ModelCard` with `model.default_gpu_device = Some("ROCm0")`
  - Create a `ModelConfig` with `gpu_device = None`
  - Verify that `load_model()` resolves `--device ROCm0` in the spawned process args
  - (This is an integration test — may need to mock the backend binary)
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: resolve gpu_device from model card default"

---

## Execution Order

Tasks 1-3 are independent (can be done in parallel or sequentially). Task 4 depends on Task 1 (model card field) and Task 2 (injection logic).

Recommended order: Task 1 → Task 2 → Task 3 → Task 4 (sequential, each builds on the previous).

**Minimum viable delivery:** Tasks 1-3 are sufficient for the core feature. Task 4 (model card fallback) is a nice-to-have convenience.

## Rollback

Each task is independently reversible:
- Task 1: Revert migration (SQLite doesn't support DROP COLUMN in older versions, but the column is harmless if unused)
- Task 2: Remove the injection block from `build_full_args()`
- Task 3: Remove the flag parsing and usage
- Task 4: Remove the fallback logic
