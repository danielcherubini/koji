# Model Batch/µ-batch Fields Plan

**Goal:** Promote batch (`-b`) and micro-batch (`-ub`) from free-text "additional params" to first-class typed model fields: stored in the DB, editable via dropdowns in the model editor, injected into llama.cpp spawn args when set, and prefilled in benchmark forms.
**Architecture:** Add nullable `n_batch`/`n_ubatch` columns to `model_configs` (migration `_0041`), surface through `ModelConfig` and the models API, inject `-b`/`-ub` in `resolve` only when `Some`. **Important:** the free-text field is `ModelConfig.args: Vec<String>`, persisted as a **JSON array string** in the `args` column — there is NO `additional_params` field anywhere. Migrations are pure SQL (`crates/tama-core/src/db/migrations.rs` — tuples of `(version, fk_off, sql)`, no Rust hooks), so flag extraction from `args` is done in **Rust** (see Task 1), not in SQL. `NULL` means unset — llama.cpp uses its own defaults.
**Tech Stack:** Rust, rusqlite migrations, Leptos model editor.

---

### Task 1: Migration + ModelConfig fields

**Context:**
`ModelConfig` lives in `crates/tama-core/src/config/types/model.rs` and is persisted in `model_configs` (see `crates/tama-core/src/config/types/model.rs` and the model queries under `crates/tama-core/src/db/queries/`). Migrations are numbered files in `crates/tama-core/src/db/migrations/`; the latest is `_0040_rename_app_supervisor_to_app_lifecycle.rs`, so the new one is `_0041_add_model_batch_sizes.rs`. Migration files are registered in the migrations mod — check `crates/tama-core/src/db/migrations.rs` or `migrations/mod.rs` for the registration list and `migrations_tests.rs` for the test pattern.

**Files:**
- Create: `crates/tama-core/src/db/migrations/_0041_add_model_batch_sizes.rs`
- Modify: `crates/tama-core/src/db/migrations.rs` (add `mod _0041...`, append to `MIGRATIONS` array, **bump `pub const LATEST_VERSION` from 40 to 41** — a compile-time registry test enforces this)
- Modify: `crates/tama-core/src/config/types/model.rs` (`ModelConfig`, `to_db_record`, `from_db_record`)
- Modify: `crates/tama-core/src/db/queries/types.rs` (`ModelConfigRecord` struct, `COLUMNS` const, `INSERT_COLUMNS` const, `from_row` positional indices)
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs` (INSERT values list **and the `ON CONFLICT DO UPDATE SET` clause** — add `n_batch = excluded.n_batch, n_ubatch = excluded.n_ubatch`)
- Modify: ~100+ literal construction sites across ~28 files — `ModelConfig { ... }` literals (`api/models/crud/tests.rs` alone has ~15, many more in `proxy/` tests, `crud/mod.rs`, `proxy/status.rs`, `db/mod.rs`, `backfill/initial_backfill.rs`, `llama_bench/mod.rs` `seed_test_db`, `info.rs` `make_config`) and `ModelConfigRecord { ... }` literals (~25 in tests). Neither struct derives `Default`, so every literal breaks until updated. The count here is illustrative — the `rg "ModelConfig \{"` / `rg "ModelConfigRecord \{"` sweep is authoritative. Strongly consider adding `#[derive(Default)]` to both structs so existing literals can add `..Default::default()` instead of naming the new fields.

**What to implement:**
1. Migration `_0041_add_model_batch_sizes.rs` (SQL only): `ALTER TABLE model_configs ADD COLUMN n_batch INTEGER;` + `ADD COLUMN n_ubatch INTEGER;` (nullable, no default). **Append the new columns at the END of the table** (after existing columns) to minimize `from_row` positional index churn — new `row.get(N)` indices go at the end.
2. `ModelConfig`: add `pub n_batch: Option<u32>` and `pub n_ubatch: Option<u32>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` matching neighboring optional fields. Update `to_db_record`/`from_db_record`.
3. **Rust-side normalization of legacy `args`** (NOT SQL): in `ModelConfig::from_db_record` (or a dedicated normalize fn it calls), scan the parsed `args` array for `-b`/`--batch-size` and `-ub`/`--ubatch-size` (both `--flag value` as two array elements and `--flag=value` as one). When found AND the corresponding new column is `None`: populate `n_batch`/`n_ubatch` from the parsed u32 and remove those elements from `args`. Skip unparseable values (leave in args). This runs at load time — write-through to DB happens naturally on next save; no data migration needed.
4. Query layer: `ModelConfigRecord` fields + `COLUMNS` + `INSERT_COLUMNS` + `from_row` + INSERT values + ON CONFLICT clause (see Files).

**Steps:**
- [ ] Write failing tests: (a) normalize test — `ModelConfig` whose `args` JSON array is `["-ngl","99","-b","2048","--ubatch-size=512"]` comes out of `from_db_record` with `n_batch == Some(2048)`, `n_ubatch == Some(512)`, `args == ["-ngl","99"]`; explicit column values win over args flags; (b) round-trip test — ModelConfig with `Some`/`None` values survives DB write/read.
- [ ] Run `cargo nextest run --package tama-core -- model_config` — confirm fail.
- [ ] Implement migration + fields + query mappings + normalization + all literal-site fixes.
- [ ] Run `cargo nextest run --package tama-core` then `cargo nextest run --workspace`
- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: "feat: n_batch/n_ubatch model fields with args normalization"

**Acceptance criteria:**
- [ ] Legacy `-b`/`-ub` args normalize into columns at load; rows without flags untouched
- [ ] ModelConfig round-trips the new fields; ON CONFLICT upsert updates them

---

### Task 2: Inject batch args at spawn (resolve)

**Context:**
Spawn-arg injection lives in `crates/tama-core/src/config/resolve/mod.rs` (e.g. spec args at :284-320 and :428-444). When `n_batch`/`n_ubatch` are `Some`, llama.cpp must get `--batch-size N` / `--ubatch-size N`. When `None`, pass nothing (llama.cpp default).

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/config/resolve/` test module (there are existing resolve tests — `rg -l "batch" crates/tama-core/src/config/resolve/` and the tests dir from plan-093 split)

**What to implement:**
- In `resolve_backend` (the fn building the `grouped` args vec in `crates/tama-core/src/config/resolve/mod.rs`), near the other model args (`-ngl` ~line 378, kv cache ~line 422), use the module's existing idiom: `grouped.push(format!("-b {}", b))` and `grouped.push(format!("-ub {}", ub))` when `Some`. **Use short flags `-b`/`-ub`** — llama-server accepts these (the resolve module's neighboring flags are all short forms; llama-bench also uses `-b`/`-ub` in `llama_bench/args.rs`). Verify llama-server's accepted flag names before finalizing.
- Guard against duplicates using the existing `crate::config::flag_name` helper pattern (as the spec-arg injections do, e.g. checking `.any(|e| matches!(flag_name(e), ...))`) so a leftover `-b` in `args` doesn't produce a duplicate flag.
- Do NOT remove `args` appending — leftover flags still pass through (Task 1 normalizes most; editor warning in Task 3 covers stragglers).

**Steps:**
- [ ] Write failing test: model with `n_batch: Some(4096), n_ubatch: Some(512)` produces args containing `-b 4096` and `-ub 512`; model with `None` produces neither; model with `Some(4096)` plus a leftover `-b 2048` in `args` produces only one `-b` (the typed field wins).
- [ ] Run `cargo nextest run --package tama-core -- resolve` — confirm fail.
- [ ] Implement.
- [ ] Run `cargo nextest run --package tama-core`
- [ ] `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Commit: "feat: inject -b/-ub from model config"

**Acceptance criteria:**
- [ ] Set values produce `-b`/`-ub` spawn args (no duplicates); unset values produce none

---

### Task 3: Model editor dropdowns + API surface

**Context:**
The model editor currently has batch/ubatch only via free-text additional params. Fields flow through the models API automatically once on `ModelConfig` (models API serializes the config — verify at `crates/tama/src/api/models/`; plan-154/155 established PATCH semantics so optional fields must not be wiped — include the new fields in any explicit field lists).

**Files:**
- Modify: model editor UI (find with `rg "additional" crates/tama/src/pages/ -l` — likely `crates/tama/src/pages/models/` editor components)
- Modify: `crates/tama/src/api/models/` DTOs if fields are enumerated explicitly (check `rg "ngl" crates/tama/src/api/models/` for the pattern)
- Modify: `crates/tama/css/` only if dropdown styling is missing (reuse existing select styles)

**What to implement:**
- Add two `<select>` dropdowns to the model editor (near other performance/advanced fields):
  - Batch: `Default` (unset) + 128, 256, 512, 1024, 2048, 4096, 8192
  - µ-batch: `Default` (unset) + 32, 64, 128, 256, 512, 1024, 2048, 4096
- Client-side validation: if both set and `ubatch > batch`, show a warning (llama.cpp requires ubatch ≤ batch). Warning only — don't block save.
- If `args` text still contains `-b`/`--batch-size`/`-ub`/`--ubatch-size` while a dropdown value is set, show a warning that the typed field takes precedence (resolve dedups in the typed field's favor).
- Ensure the fields are included in create/update/PATCH payloads and survive partial updates (mirror how `ngl` or `flash_attn` is handled).

**Steps:**
- [ ] Implement UI + any DTO field lists.
- [ ] Run `cargo check --package tama` and `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] `cargo fmt --all`
- [ ] Commit: "feat: batch/µ-batch dropdowns in model editor"

**Acceptance criteria:**
- [ ] Dropdowns round-trip through the API; Default = field absent from spawn args
- [ ] Warnings appear for ubatch > batch and for leftover text flags

---

### Task 4: Benchmark form prefill from model

**Context:**
The llama-bench form has batch/ubatch text inputs with presets (`crates/tama/src/pages/benchmarks/types.rs:87-134`). When a model with `n_batch`/`n_ubatch` set is selected, the inputs should prefill from the model; preset values remain the fallback. The model list JSON must carry the new fields (verify `crates/tama/src/api/models/info.rs` includes them — it serializes ModelConfig-derived data).

**Files:**
- Modify: `crates/tama/src/api/models/info.rs` (only if fields don't flow automatically)
- Modify: `crates/tama/src/pages/benchmarks/mod.rs` (model-select change handler) and possibly `crates/tama/src/pages/benchmarks/types.rs` (`parse_model`)

**What to implement:**
- **Explicitly add `"n_batch": m.n_batch, "n_ubatch": m.n_ubatch` to the `serde_json::json!({...})` block in `model_entry_json` (`crates/tama/src/api/models/info.rs`)** — the model JSON is built field-by-field (like `"mtp_model": m.mtp_model` at line 164); new ModelConfig fields do NOT flow automatically.
- Extend `parse_model` (`types.rs:23-56`) to read `n_batch`/`n_ubatch` from the model JSON.
- In the model-select `on:change`, if the chosen model has values set, write them into the batch/ubatch input signals (as strings); leave inputs untouched when unset.

**Steps:**
- [ ] Implement.
- [ ] Run `cargo check --package tama && cargo fmt --all && cargo clippy --package tama --all-targets -- -D warnings`
- [ ] Commit: "feat: prefill benchmark batch/µ-batch from model defaults"

**Acceptance criteria:**
- [ ] Selecting a model with stored values prefills the bench inputs
- [ ] Models without values leave current input contents alone
