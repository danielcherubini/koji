# DB Query `from_row` + `COLUMNS` Consolidation Plan

**Goal:** Eliminate the repeated `row.get(N)?` row-mapping closures and SELECT column lists in `crates/tama-core/src/db/queries/` by giving each duplicated record type one `from_row` associated function and one `COLUMNS` constant.

**Architecture:** Each record type gets `pub(crate) fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self>` plus `pub(crate) const COLUMNS: &str` defined in an `impl` block **alongside the record's own definition** (`db/queries/types.rs` for `ModelConfigRecord`/`ModelFileRecord`/`TtsConfigRecord`/`UpdateCheckRecord`; `alias_queries.rs` for `AliasResponse`; `metrics_queries.rs` for `SystemMetricsRow`). Query functions are rewritten as `stmt.query_map(params, ModelConfigRecord::from_row)` with the SQL built via `format!("SELECT {} FROM ...", ModelConfigRecord::COLUMNS)`. One task per record type — each is independently commitable and order-independent. Audit finding F30 (`docs/reviews/2026-07-18-codebase-improvement.md` #30).

**Tech Stack:** Rust, SQLite (rusqlite)

---

### Task 1: `ModelConfigRecord::from_row` + `COLUMNS` + `INSERT_COLUMNS`

**Context:**
`crates/tama-core/src/db/queries/model_config_queries.rs` repeats the identical 35-field mapping closure three times (`get_model_config` :110-152, `get_model_config_by_repo_id` :163-207, `get_all_model_configs` :219-273) and the identical 35-column SELECT list three times (:109, :162, :218). The 34-column INSERT column list in `upsert_model_config` (:12-62) appears only once, but defining `INSERT_COLUMNS` (all columns except `id`) next to `COLUMNS` keeps both lists in one place so a future column addition updates one site. Decisions: the `ON CONFLICT ... DO UPDATE SET` clause stays inline (single occurrence, and the `COALESCE` preservation semantics must stay visible); the record type lives in `db/queries/types.rs`, so the impl goes there. `ModelConfigRecord` is `#[deprecated]` in favor of Repository DTOs, but the crate has `#![allow(deprecated)]` (`crates/tama-core/src/lib.rs:11`) and the queries still back `ModelManager` + `db/queries/tests.rs`, so the refactor is worthwhile. Column order in `COLUMNS` MUST exactly match the current SELECT order (`id` first, then `repo_id` … `updated_at` last).

**Files:**
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs`

**What to implement:**

1. In `types.rs`, add `use rusqlite::Row;` and after the `ModelConfigRecord` struct add:
   ```rust
   impl ModelConfigRecord {
       /// All 35 columns in SELECT order (id first). Must match `from_row` index order.
       pub(crate) const COLUMNS: &str =
           "id, repo_id, display_name, backend, gpu_variant, gpu_device, enabled, selected_quant, \
            selected_mmproj, selected_mtp_model, context_length, num_parallel, kv_unified, gpu_layers, \
            cache_type_k, cache_type_v, port, args, \
            sampling, modalities, profile, api_name, health_check, \
            hf_format, hf_base_model, hf_pipeline_tag, hf_total_params, \
            hf_active_params, hf_architecture_type, hf_context_length, \
            hf_num_layers, hf_last_modified, spec_decoding, \
            created_at, updated_at";

       /// The 34 non-`id` columns in INSERT order. Must stay in sync with `COLUMNS` minus `id`.
       pub(crate) const INSERT_COLUMNS: &str =
           "repo_id, display_name, backend, gpu_variant, gpu_device, enabled, selected_quant, \
            selected_mmproj, selected_mtp_model, context_length, num_parallel, kv_unified, gpu_layers, \
            cache_type_k, cache_type_v, port, args, \
            sampling, modalities, profile, api_name, health_check, \
            hf_format, hf_base_model, hf_pipeline_tag, hf_total_params, \
            hf_active_params, hf_architecture_type, hf_context_length, \
            hf_num_layers, hf_last_modified, spec_decoding, \
            created_at, updated_at";

       /// Map a row selected with `COLUMNS` order into a record.
       pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
           // body = the existing 35-field closure verbatim (row.get(0)? … row.get(34)?),
           // keeping `row.get::<_, i32>(6)? != 0` for `enabled` and `row.get::<_, i32>(12)? != 0` for `kv_unified`
       }
   }
   ```
   Copy the mapping body verbatim from `model_config_queries.rs:111-151` — do not reorder or retype anything.

2. In `model_config_queries.rs`:
   - `upsert_model_config`: replace the inline INSERT column list (:13-19) with `format!("INSERT INTO model_configs ({}) VALUES (?1, ..., ?34) ON CONFLICT(repo_id) DO UPDATE SET ...", ModelConfigRecord::INSERT_COLUMNS)` — build the whole SQL string with `format!`, keep the `VALUES` placeholder list, the full `ON CONFLICT` clause (including the `COALESCE` HF-metadata block and `updated_at = strftime(...)`), and the `params![...]` list byte-identical. Keep the follow-up `SELECT id FROM model_configs WHERE repo_id = ?1` as-is (single column, not a record select).
   - `get_model_config`: SQL becomes `format!("SELECT {} FROM model_configs WHERE id = ?1", ModelConfigRecord::COLUMNS)`; mapper becomes `stmt.query_map([id], ModelConfigRecord::from_row)?;` keeping the existing `match rows.next()` tail.
   - `get_model_config_by_repo_id`: same treatment with `WHERE repo_id = ?1` and `[repo_id]`.
   - `get_all_model_configs`: `format!("SELECT {} FROM model_configs", ModelConfigRecord::COLUMNS)`; `stmt.query_map([], ModelConfigRecord::from_row)?;` keeping the existing `.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)` tail.
   - Do NOT touch `delete_model_config` or the module doc comment.

3. Add a drift-guard test at the bottom of `model_config_queries.rs` in a new `#[cfg(test)] mod tests`:
   ```rust
   #[test]
   fn test_model_config_columns_match_insert_columns() {
       let select: Vec<&str> = ModelConfigRecord::COLUMNS.split(',').map(str::trim).collect();
       let insert: Vec<&str> = ModelConfigRecord::INSERT_COLUMNS.split(',').map(str::trim).collect();
       assert_eq!(select.len(), 35);
       assert_eq!(insert.len(), 34);
       assert_eq!(select[0], "id");
       assert_eq!(&select[1..], insert.as_slice());
   }
   ```
   (Needs `use super::types::ModelConfigRecord;` — already imported at file top.)

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- db::queries` — confirm the existing tests are green BEFORE changing anything (baseline; `db/queries/tests.rs::test_upsert_and_get_model_config`, `test_mtp_model_db_round_trip`, `test_get_all_model_configs`, `test_delete_model_config` exercise these queries)
- [ ] Add the new `test_model_config_columns_match_insert_columns` test first and run `cargo nextest run --package tama-core -- db::queries::model_config` — it FAILS to compile (consts don't exist yet)
- [ ] Implement the `impl ModelConfigRecord` block in `types.rs` and rewrite the four query functions in `model_config_queries.rs` per above
- [ ] Run `cargo nextest run --package tama-core -- db::queries` — all pass, including the new guard test
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (catches regressions in `models/manager.rs`, `db/repository.rs`, `db/backfill/*` which consume these queries)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: dedupe ModelConfigRecord row mapping via from_row + COLUMNS"

**Acceptance criteria:**
- [ ] `model_config_queries.rs` contains zero `Ok(ModelConfigRecord {` closures and zero inline SELECT column lists; all three SELECTs use `ModelConfigRecord::COLUMNS` via `format!`
- [ ] `upsert_model_config` uses `ModelConfigRecord::INSERT_COLUMNS`; the `ON CONFLICT` clause text is unchanged
- [ ] `db/queries/tests.rs` model-config tests and `models/manager_tests.rs` pass unmodified
- [ ] The new column-count/order guard test passes
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 2: `AliasResponse::from_row` + `COLUMNS` + `FROM_JOIN`

**Context:**
`crates/tama-core/src/db/queries/alias_queries.rs` repeats the `AliasResponse` mapping closure twice (`get_all_aliases` :52-61, `get_alias_by_id` :79-88) with the identical 8-item SELECT list over the same `model_aliases a JOIN model_configs m` join (:44-50, :71-77). The audit says "×3", but the third function `load_aliases_for_cache` (:27-39) maps a `(String, String)` tuple, not an `AliasResponse` — leave it as-is. The SELECT list includes the computed column `COALESCE(m.api_name, m.repo_id)` (aliased table prefixes are required by the JOIN); this still fits the `COLUMNS` const pattern because both queries share the exact same FROM/JOIN clause. Decisions: define the impl directly in `alias_queries.rs` (where `AliasResponse` is defined); add a `FROM_JOIN` const so the join clause is also single-sourced. Do NOT change `insert_alias`/`update_alias`/`delete_alias`.

**Files:**
- Modify: `crates/tama-core/src/db/queries/alias_queries.rs`

**What to implement:**

1. After the `AliasResponse` struct definition (:9-19) add:
   ```rust
   impl AliasResponse {
       /// Select list including the computed `model_name` column. Requires the `FROM_JOIN` clause.
       pub(crate) const COLUMNS: &str =
           "a.id, a.name, a.model_id, COALESCE(m.api_name, m.repo_id), \
            a.description, a.enabled, a.created_at, a.updated_at";

       /// Shared FROM/JOIN clause every `AliasResponse` query uses.
       pub(crate) const FROM_JOIN: &str =
           "FROM model_aliases a JOIN model_configs m ON m.id = a.model_id";

       /// Map a row selected with `COLUMNS` order into a response.
       pub(crate) fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
           Ok(AliasResponse {
               id: row.get(0)?,
               name: row.get(1)?,
               model_id: row.get(2)?,
               model_name: row.get(3)?,
               description: row.get(4)?,
               enabled: row.get::<_, i32>(5)? != 0,
               created_at: row.get(6)?,
               updated_at: row.get(7)?,
           })
       }
   }
   ```

2. `get_all_aliases`: SQL becomes `format!("SELECT {} {} ORDER BY a.name ASC", AliasResponse::COLUMNS, AliasResponse::FROM_JOIN)`; mapper becomes `stmt.query_map([], AliasResponse::from_row)?;` keeping the collect tail.
3. `get_alias_by_id`: SQL becomes `format!("SELECT {} {} WHERE a.id = ?1", AliasResponse::COLUMNS, AliasResponse::FROM_JOIN)`; mapper becomes `stmt.query_map([id], AliasResponse::from_row)?;` keeping the `match rows.next()` tail.
4. Leave `load_aliases_for_cache` (tuple mapper), `insert_alias`, `update_alias`, `delete_alias`, and the inline `mod tests` untouched.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- db::queries::alias` — confirm the existing 5 inline tests are green BEFORE changing anything (baseline)
- [ ] Implement the `impl AliasResponse` block and rewrite `get_all_aliases` + `get_alias_by_id` per above
- [ ] Run `cargo check --package tama-core` — compiles
- [ ] Run `cargo nextest run --package tama-core -- db::queries::alias` — all 5 tests pass unmodified (they round-trip through both rewritten functions)
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: dedupe AliasResponse row mapping via from_row + COLUMNS"

**Acceptance criteria:**
- [ ] `alias_queries.rs` contains exactly one `Ok(AliasResponse {` construction (inside `from_row`) and zero inline alias SELECT column lists
- [ ] Both rewritten queries produce identical SQL semantics (JOIN + `COALESCE` model_name); `test_insert_and_get_alias` and `test_update_alias` pass unmodified
- [ ] `load_aliases_for_cache` is byte-identical to before
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 3: `TtsConfigRecord::from_row` + `COLUMNS`

**Context:**
`crates/tama-core/src/db/queries/tts_config_queries.rs` repeats the 8-field `TtsConfigRecord` mapping twice (`get_tts_config` :50-59, `get_all_tts_configs` :75-84) with the identical 8-column SELECT list (:44-47, :69-72). The record type is defined in `db/queries/types.rs` (:118-128), so the impl goes there. `upsert_tts_config`'s 7-column INSERT list appears once — leave it inline (single occurrence, no `INSERT_COLUMNS` const needed; the INSERT omits `id` and its order matches `COLUMNS` minus `id`, but there is no duplication to fix). Note: `get_all_tts_configs` is flagged as dead code by audit F38 — do NOT delete it in this plan; that belongs to the dead-code batch.

**Files:**
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/tts_config_queries.rs`

**What to implement:**

1. In `types.rs`, after the `TtsConfigRecord` struct add:
   ```rust
   impl TtsConfigRecord {
       /// All 8 columns in SELECT order (id first). Must match `from_row` index order.
       pub(crate) const COLUMNS: &str =
           "id, engine, default_voice, speed, format, enabled, created_at, updated_at";

       /// Map a row selected with `COLUMNS` order into a record.
       pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
           Ok(TtsConfigRecord {
               id: row.get(0)?,
               engine: row.get(1)?,
               default_voice: row.get(2)?,
               speed: row.get(3)?,
               format: row.get(4)?,
               enabled: row.get::<_, i32>(5)? != 0,
               created_at: row.get(6)?,
               updated_at: row.get(7)?,
           })
       }
   }
   ```

2. `get_tts_config`: SQL becomes `format!("SELECT {} FROM tts_configs WHERE engine = ?1", TtsConfigRecord::COLUMNS)`; mapper becomes `stmt.query_map([engine], TtsConfigRecord::from_row)?;` keeping the `match rows.next()` tail.
3. `get_all_tts_configs`: SQL becomes `format!("SELECT {} FROM tts_configs ORDER BY engine ASC", TtsConfigRecord::COLUMNS)`; mapper becomes `stmt.query_map([], TtsConfigRecord::from_row)?;` keeping the collect tail.
4. Do NOT touch `upsert_tts_config`, `delete_tts_config`, or the inline `mod tests` (:120+).

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- db::queries::tts` — confirm the existing inline tests are green BEFORE changing anything (baseline)
- [ ] Implement the `impl TtsConfigRecord` block in `types.rs` and rewrite the two query functions per above
- [ ] Run `cargo nextest run --package tama-core -- db::queries::tts` — all pass unmodified
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: dedupe TtsConfigRecord row mapping via from_row + COLUMNS"

**Acceptance criteria:**
- [ ] `tts_config_queries.rs` contains zero `Ok(TtsConfigRecord {` closures outside `types.rs::from_row` and zero inline SELECT column lists
- [ ] Existing TTS round-trip tests pass unmodified
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 4: `UpdateCheckRecord::from_row` + `COLUMNS`

**Context:**
`crates/tama-core/src/db/queries/update_check_queries.rs` repeats the 9-field `UpdateCheckRecord` mapping twice (`get_all_update_checks` :53-63, `get_update_check` :74-84) with the identical 9-column SELECT list (:48-51, :69-72). The record type is defined in `db/queries/types.rs` (:131-141), so the impl goes there. `UpdateCheckParams`/`upsert_update_check`, `delete_update_check`, `delete_update_checks_by_pattern`, and `get_oldest_check_time` have no duplicated mapping — leave them all untouched (`get_oldest_check_time` selects `MIN(checked_at)`, a scalar, not a record).

**Files:**
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/update_check_queries.rs`

**What to implement:**

1. In `types.rs`, after the `UpdateCheckRecord` struct add:
   ```rust
   impl UpdateCheckRecord {
       /// All 9 columns in SELECT order. Must match `from_row` index order.
       pub(crate) const COLUMNS: &str =
           "item_type, item_id, current_version, latest_version, update_available, \
            status, error_message, details_json, checked_at";

       /// Map a row selected with `COLUMNS` order into a record.
       pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
           Ok(UpdateCheckRecord {
               item_type: row.get(0)?,
               item_id: row.get(1)?,
               current_version: row.get(2)?,
               latest_version: row.get(3)?,
               update_available: row.get::<_, i32>(4)? != 0,
               status: row.get(5)?,
               error_message: row.get(6)?,
               details_json: row.get(7)?,
               checked_at: row.get(8)?,
           })
       }
   }
   ```

2. `get_all_update_checks`: SQL becomes `format!("SELECT {} FROM update_checks ORDER BY item_type, item_id", UpdateCheckRecord::COLUMNS)`; mapper becomes `stmt.query_map([], UpdateCheckRecord::from_row)?;` keeping the collect tail.
3. `get_update_check`: SQL becomes `format!("SELECT {} FROM update_checks WHERE item_type = ?1 AND item_id = ?2", UpdateCheckRecord::COLUMNS)`; mapper becomes `stmt.query_map((item_type, item_id), UpdateCheckRecord::from_row)?;` keeping the `match rows.next()` tail.
4. This file has no inline tests — coverage lives in `db/queries/tests.rs` (`test_upsert_and_get_update_check` :7, `test_get_all_update_checks` :68, `test_delete_update_check` :109, `test_get_oldest_check_time` :136, `test_delete_update_checks_by_pattern*` :177/:251). Do not add tests; those must pass unmodified.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- db::queries::tests` — confirm the 7 update-check tests are green BEFORE changing anything (baseline)
- [ ] Implement the `impl UpdateCheckRecord` block in `types.rs` and rewrite the two query functions per above
- [ ] Run `cargo nextest run --package tama-core -- db::queries::tests` — all pass unmodified
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (catches regressions in `updates/checker/*` which consume these queries)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: dedupe UpdateCheckRecord row mapping via from_row + COLUMNS"

**Acceptance criteria:**
- [ ] `update_check_queries.rs` contains zero `Ok(UpdateCheckRecord {` closures and zero inline SELECT column lists
- [ ] All 7 update-check tests in `db/queries/tests.rs` pass unmodified
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 5: `ModelFileRecord::from_row` + `COLUMNS`

**Context:**
`crates/tama-core/src/db/queries/model_queries.rs` repeats the 11-field `ModelFileRecord` mapping twice (`get_model_files` :139-152, `get_all_model_files` :166-179) with the identical 11-column SELECT list (:132-134, :159-161). The record type is defined in `db/queries/types.rs` (:65-89, also `#[deprecated]`, allowed crate-wide). The mapper has one non-trivial bit: `verified_ok` is read as `Option<i64>` first (`let verified_ok: Option<i64> = row.get(9)?;`) then converted with `.map(|v| v != 0)` — preserve that exactly inside `from_row`. Also in this file: `get_model_pull` (:31-46) maps `ModelPullRecord` exactly once — NOT duplicated, leave it as-is. `upsert_model_pull`, `upsert_model_file`, `update_verification`, `delete_model_records`, `delete_model_file`, `log_pull` stay untouched. Outside this file, `crates/tama-core/src/backup/archive.rs:160` does a partial-column select (`repo_id, quant, size_bytes FROM model_files`) — leave it as-is; it cannot use the full 11-column `COLUMNS`.

**Files:**
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/model_queries.rs`

**What to implement:**

1. In `types.rs`, after the `ModelFileRecord` struct add:
   ```rust
   impl ModelFileRecord {
       /// All 11 columns in SELECT order (id first). Must match `from_row` index order.
       pub(crate) const COLUMNS: &str =
           "id, model_id, repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at, \
            last_verified_at, verified_ok, verify_error";

       /// Map a row selected with `COLUMNS` order into a record.
       pub(crate) fn from_row(row: &Row) -> rusqlite::Result<Self> {
           let verified_ok: Option<i64> = row.get(9)?;
           Ok(ModelFileRecord {
               id: row.get(0)?,
               model_id: row.get(1)?,
               repo_id: row.get(2)?,
               filename: row.get(3)?,
               quant: row.get(4)?,
               lfs_oid: row.get(5)?,
               size_bytes: row.get(6)?,
               downloaded_at: row.get(7)?,
               last_verified_at: row.get(8)?,
               verified_ok: verified_ok.map(|v| v != 0),
               verify_error: row.get(10)?,
           })
       }
   }
   ```

2. `get_model_files`: SQL becomes `format!("SELECT {} FROM model_files WHERE model_id = ?1", ModelFileRecord::COLUMNS)`; mapper becomes `stmt.query_map([model_id], ModelFileRecord::from_row)?;` keeping the collect tail.
3. `get_all_model_files`: SQL becomes `format!("SELECT {} FROM model_files", ModelFileRecord::COLUMNS)`; mapper becomes `stmt.query_map([], ModelFileRecord::from_row)?;` keeping the collect tail.
4. Update the file's import: `use super::types::{ModelFileRecord, ModelPullRecord, PullLogEntry};` stays as-is (still needed for `ModelPullRecord`/`PullLogEntry`); remove `ModelFileRecord` from it only if clippy flags it unused after the rewrite — it is still needed for the `format!` calls, so it stays.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- db::queries::model` — confirm the 3 inline tests + `db::queries::tests` model tests are green BEFORE changing anything (baseline)
- [ ] Implement the `impl ModelFileRecord` block in `types.rs` and rewrite the two query functions per above
- [ ] Run `cargo nextest run --package tama-core -- db::queries` — all pass unmodified (`test_delete_model_file*` round-trips go through `get_model_files`)
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes (catches regressions in `models/verify.rs`, `models/update.rs`, `updates/checker/model.rs`, `db/repository.rs`)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: dedupe ModelFileRecord row mapping via from_row + COLUMNS"

**Acceptance criteria:**
- [ ] `model_queries.rs` contains zero `Ok(ModelFileRecord {` closures and zero inline model_files SELECT column lists; `get_model_pull`'s single `ModelPullRecord` mapper is unchanged
- [ ] The `verified_ok` Option<i64>→Option<bool> conversion semantics are preserved exactly
- [ ] `backup/archive.rs:160` partial select is untouched
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 6: `SystemMetricsRow::from_row` + `COLUMNS`

**Context:**
`crates/tama-core/src/db/queries/metrics_queries.rs` repeats the 14-field `SystemMetricsRow` mapping twice (`get_system_metrics_since` :78-93, `get_recent_system_metrics` :114-129) with the identical 14-column SELECT list (:68-73, :103-108). `SystemMetricsRow` is defined at the top of this same file (:8-25), so the impl goes here too — NOT in `types.rs`. `get_recent_system_metrics` additionally reverses the DESC result to return oldest-first and validates `limit >= 0` — preserve both behaviors exactly. `insert_system_metric`'s 14-column INSERT list appears once; leave it inline (no duplication). The only other consumer, `crates/tama-core/src/proxy/server/metrics.rs`, uses the query functions (not raw SQL), so nothing else changes.

**Files:**
- Modify: `crates/tama-core/src/db/queries/metrics_queries.rs`

**What to implement:**

1. After the `SystemMetricsRow` struct add:
   ```rust
   impl SystemMetricsRow {
       /// All 14 columns in SELECT order. Must match `from_row` index order.
       pub(crate) const COLUMNS: &str =
           "ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib, \
            gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded, \
            tps, prompt_tps, cache_hit_pct, spec_accept_pct, \
            net_rx_bytes, net_tx_bytes";

       /// Map a row selected with `COLUMNS` order into a row struct.
       pub(crate) fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Self> {
           Ok(SystemMetricsRow {
               ts_unix_ms: row.get(0)?,
               cpu_usage_pct: row.get(1)?,
               ram_used_mib: row.get(2)?,
               ram_total_mib: row.get(3)?,
               gpu_utilization_pct: row.get(4)?,
               vram_used_mib: row.get(5)?,
               vram_total_mib: row.get(6)?,
               models_loaded: row.get(7)?,
               tps: row.get(8)?,
               prompt_tps: row.get(9)?,
               cache_hit_pct: row.get(10)?,
               spec_accept_pct: row.get(11)?,
               net_rx_bytes: row.get(12)?,
               net_tx_bytes: row.get(13)?,
           })
       }
   }
   ```

2. `get_system_metrics_since`: SQL becomes `format!("SELECT {} FROM system_metrics_history WHERE ts_unix_ms > ?1 ORDER BY ts_unix_ms ASC", SystemMetricsRow::COLUMNS)`; mapper becomes `stmt.query_map([since_ms], SystemMetricsRow::from_row)?;` keeping the collect tail.
3. `get_recent_system_metrics`: keep the `if limit < 0 { bail!(...) }` guard; SQL becomes `format!("SELECT {} FROM system_metrics_history ORDER BY ts_unix_ms DESC LIMIT ?1", SystemMetricsRow::COLUMNS)`; mapper becomes `stmt.query_map([limit], SystemMetricsRow::from_row)?;` keeping the collect + `rows.reverse()` tail verbatim.
4. Do NOT touch `insert_system_metric` or the inline `mod tests`.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- db::queries::metrics` — confirm the 11 inline tests are green BEFORE changing anything (baseline)
- [ ] Implement the `impl SystemMetricsRow` block and rewrite the two query functions per above
- [ ] Run `cargo nextest run --package tama-core -- db::queries::metrics` — all 11 tests pass unmodified
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: dedupe SystemMetricsRow row mapping via from_row + COLUMNS"

**Acceptance criteria:**
- [ ] `metrics_queries.rs` contains exactly one `Ok(SystemMetricsRow {` construction (inside `from_row`) and zero inline SELECT column lists
- [ ] All 11 inline tests pass unmodified, including `test_get_recent_system_metrics_ordered` (proves the reverse tail survived)
- [ ] `cargo clippy --workspace -- -D warnings` is clean
