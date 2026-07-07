# MTP Draft Model File Support

**Goal:** Add `mtp-*.gguf` draft model files as a first-class auxiliary file type, selectable via the Spec Decoding section of the model editor.

**Architecture:** Mirror the `mmproj` pattern — `QuantKind::Mtp` variant, `ModelConfig.mtp_model` field, DB column `selected_mtp_model`, `--mtp-model` flag injection gated on `draft-mtp` being in `spec_types`. MTP files are pulled via the web UI pull wizards and selected via a dropdown in the Spec Decoding form (not the Quants section).

**Tech Stack:** Rust (tama-core, tama-web), Leptos (web UI), SQLite (DB migration)

---

### Task 1: Core Data Model — QuantKind::Mtp, ModelConfig.mtp_model, DB Migration

**Context:**
Add the foundational types and DB schema for MTP file support. This is the base that all other tasks depend on. Follows the exact pattern established by `mmproj` support (Task 4 of the mmproj plan).

**Files:**
- Modify: `crates/tama-core/src/config/types.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs`
- Create: `crates/tama-core/src/db/migrations/_0028_add_selected_mtp_model.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: `crates/tama-core/src/db/migrations/migrations_tests.rs`

**What to implement:**

1. **`QuantKind` enum** (`config/types.rs`):
   - Add `Mtp` variant after `Mmproj`
   - Add doc comment: `/// An MTP draft model (mtp-*.gguf). Passed via --mtp-model to llama.cpp.`
   - Update `from_filename()` to check `mtp` AFTER `mmproj`:
     ```rust
     pub fn from_filename(filename: &str) -> Self {
         let lower = filename.to_lowercase();
         if lower.starts_with("mmproj") && lower.ends_with(".gguf") {
             QuantKind::Mmproj
         } else if lower.starts_with("mtp") && lower.ends_with(".gguf") {
             QuantKind::Mtp
         } else {
             QuantKind::Model
         }
     }
     ```

2. **`ModelConfig`** (`config/types.rs`):
   - Add field after `mmproj`:
     ```rust
     /// Which MTP draft model to use, if any. References a key in
     /// `quants` whose entry has `kind = Mtp`. When set AND `draft-mtp`
     /// is in `spec_decoding.spec_types`, the launch command gets
     /// `--mtp-model <path>` injected automatically.
     #[serde(default, skip_serializing_if = "Option::is_none")]
     pub mtp_model: Option<String>,
     ```
   - Update `to_db_record()` to include `selected_mtp_model: self.mtp_model.clone()`
   - Update `from_db_record()` to include `mtp_model: record.selected_mtp_model.clone().filter(|s| !s.is_empty())`

3. **`ModelConfigRecord`** (`db/queries/types.rs`):
   - Add field: `pub selected_mtp_model: Option<String>,` after `selected_mmproj`

4. **DB migration** (`_0028_add_selected_mtp_model.rs`):
   - Verify `_0028` is the next available number (check `migrations.rs` — currently last is `_0027_create_model_aliases`)
   - Create file with single `pub const MIGRATION` tuple (mirrors every existing migration):
     ```rust
     pub const MIGRATION: (i32, bool, &str) = (
         28,
         false,
         r#"
             ALTER TABLE model_configs ADD COLUMN selected_mtp_model TEXT COLLATE NOCASE;
         "#,
     );
     ```
   - No downgrade needed — SQLite doesn't support `DROP COLUMN` reliably and we don't run downgrades.

5. **Register migration** (`migrations.rs`):
   - Add `mod _0028_add_selected_mtp_model;` after `_0027_create_model_aliases`
   - Add `_0028_add_selected_mtp_model::MIGRATION,` to `MIGRATIONS` array after `_0027` entry
   - Update `pub const LATEST_VERSION: i32 = 27;` to `28;`

6. **Model config queries** (`db/queries/model_config_queries.rs`):
   Add `selected_mtp_model` as a new column AFTER `selected_mmproj` everywhere. This shifts `row.get(N)?` indices by +1 for all columns after `selected_mmproj` (indices 8+).

   **`upsert_model_config`** (line 12):
   - INSERT columns (line 15): add `selected_mtp_model` after `selected_mmproj`
   - VALUES params: add `?8` after `?7`, shift `?8` through `?32` to `?9` through `?33`
   - ON CONFLICT SET (line 33): add `selected_mtp_model = excluded.selected_mtp_model,` after `selected_mmproj` line
   - params![] (line 67): add `record.selected_mtp_model,` after `record.selected_mmproj,`

   **`get_model_config`** (line 100):
   - SELECT columns (line 108): add `selected_mtp_model` after `selected_mmproj`
   - row mapping (line 126): add `selected_mtp_model: row.get(8)?,` after `selected_mmproj: row.get(7)?`
   - Shift all `row.get(N)?` for N >= 8 to `row.get(N+1)?`

   **`get_model_config_by_repo_id`** (line 159):
   - Same pattern: add column to SELECT (line 167), add `row.get(8)?` mapping (line 185), shift indices 8+

   **`list_model_configs`** (line 215):
   - Same pattern: add column to SELECT (line 223), add `row.get(8)?` mapping (line 241), shift indices 8+

**Steps:**
- [ ] Write failing test for `QuantKind::from_filename` MTP detection in `crates/tama-core/src/tests/mmproj_detection_test.rs` (rename module to `auxiliary_file_detection_test` or add MTP tests to existing module):
  - `mtp-F16.gguf` → `Mtp`
  - `MTP-test.gguf` → `Mtp` (case-insensitive)
  - `mmproj-mtp-foo.gguf` → `Mmproj` (mmproj takes precedence)
  - `mtproj-foo.gguf` → `Model` (not a match)
  - `mtp-file.bin` → `Model` (not .gguf)
- [ ] Run `cargo test --package tama-core test_quant` — verify new MTP tests fail
- [ ] Implement `QuantKind::Mtp` variant and `from_filename()` update in `config/types.rs`
- [ ] Run `cargo test --package tama-core test_quant` — verify MTP tests pass
- [ ] Implement `ModelConfig.mtp_model` field, `to_db_record()`, `from_db_record()` in `config/types.rs`
- [ ] Implement `ModelConfigRecord.selected_mtp_model` in `db/queries/types.rs`
- [ ] Create migration `_0028_add_selected_mtp_model.rs` and register in `migrations.rs`
- [ ] Update model config queries in `db/queries/model_config_queries.rs`
- [ ] Run `cargo test --package tama-core -- migrations` — verify existing migration tests still pass (the `test_migrations_registry_is_ordered_and_complete` test will catch LATEST_VERSION mismatches)
- [ ] Update `test_upsert_and_get_config` in `crates/tama-core/src/models/manager_tests.rs` — add `mtp_model: None` to the `ModelConfig` literal
- [ ] Update `ModelConfigRecord` literals in `crates/tama-core/src/db/queries/tests.rs` (lines 318, 388, 423, 469) — add `selected_mtp_model: None` (or `Some(...)` for the test at line 318) after `selected_mmproj`
- [ ] Write `test_mtp_model_db_round_trip` test in `crates/tama-core/src/db/queries/tests.rs` (near line 318): create ModelConfig with `mtp_model: Some("mtp-F16.gguf")`, convert to DB record, upsert, fetch back, verify `mtp_model` preserved
- [ ] Add `test_migration_v28_adds_selected_mtp_model_column` in `crates/tama-core/src/db/migrations/migrations_tests.rs`: run migrations up to v28, verify `selected_mtp_model` column exists on `model_configs` (use `PRAGMA table_info(model_configs)` and assert column present)
- [ ] Write `test_mtp_model_toml_roundtrip` test: serialize ModelConfig with `mtp_model` to TOML, deserialize, verify preserved
- [ ] Run `cargo test --package tama-core` — all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: add QuantKind::Mtp and ModelConfig.mtp_model with DB migration"

**Acceptance criteria:**
- [ ] `QuantKind::from_filename("mtp-F16.gguf")` returns `QuantKind::Mtp`
- [ ] `QuantKind::from_filename("mmproj-mtp-foo.gguf")` returns `QuantKind::Mmproj` (mmproj precedence)
- [ ] `ModelConfig` round-trips `mtp_model` through DB record
- [ ] `ModelConfig` round-trips `mtp_model` through TOML serialization
- [ ] Migration `_0028` adds `selected_mtp_model TEXT COLLATE NOCASE` column
- [ ] All existing tests still pass

---

### Task 2: Backend Pull Support — PullRequest, Handler, Download Side Effects

**Context:**
Allow MTP files to be downloaded through the pull wizard. The backend must accept `mtp_filenames` in the pull request, chain them into validation, and apply download side effects (auto-select, stub creation) mirroring the mmproj pattern.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/types.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs`

**What to implement:**

1. **`PullRequest`** (`proxy/tama_handlers/types.rs`):
   - Add field: `pub mtp_filenames: Vec<String>` (with `#[serde(default)]`)
   - Add to the struct alongside `filenames` and `mmproj_filenames`

2. **Pull handler** (`proxy/tama_handlers/pull/handlers.rs`):
   - In `handle_tama_pull_model`, chain `mtp_filenames` into `all_files` for validation:
     ```rust
     let all_files = req.filenames.iter().chain(req.mmproj_filenames.iter()).chain(req.mtp_filenames.iter());
     ```
   - The filename allow-list check and duplicate check already work on `all_files` — no additional logic needed

3. **Download side effects** (`proxy/tama_handlers/pull/download.rs`):
   - Find the mmproj download side effects block (around lines 813-963, the section that auto-selects mmproj, creates stubs, etc.)
   - Add parallel MTP side effects:
     - When an MTP file download completes (`kind == QuantKind::Mtp`):
       - Auto-set `selected_mtp_model = Some(filename)` on the parent model config (same pattern as `selected_mmproj`)
       - Create stub model if no main quant exists yet (`quant=None, enabled=false`) — same pattern as mmproj stub
       - Tag `kind=Mtp` in the `model_files` DB row
     - Does NOT auto-enable `draft-mtp` in `spec_decoding.spec_types`
   - The `quant_key` derivation uses the existing `infer_quant_from_filename` / `unique_quant_key` logic — MTP files will get their key derived from filename (e.g. if `infer_quant_from_filename` returns `None`, the fallback uses the last component after splitting by `-` or `_`)

**Steps:**
- [ ] Add `mtp_filenames: Vec<String>` to `PullRequest` in `types.rs`
- [ ] Update `handle_tama_pull_model` in `handlers.rs` to chain `mtp_filenames` into `all_files`
- [ ] Find the mmproj download side effects in `download.rs` (search for `selected_mmproj` or `QuantKind::Mmproj`)
- [ ] Add parallel MTP side effects block mirroring the mmproj logic (mmproj block is at lines 813-963 of `download.rs`):
  - Auto-set `selected_mtp_model` on parent model config (same pattern as `selected_mmproj`)
  - Create stub model if no main quant exists yet (`quant=None, enabled=false`) — mirror the mmproj stub block (lines 916-958) verbatim, replacing `mmproj: None` with `mtp_model: None` and `mmproj: Some(...)` with `mtp_model: Some(...)`. Do NOT add image modalities — MTP does not affect input modalities.
  - Tag `kind=Mtp` in the `model_files` DB row
- [ ] Run `cargo test --package tama-core` — all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: add MTP filename support to pull request handler and download side effects"

**Acceptance criteria:**
- [ ] `PullRequest` deserializes `mtp_filenames` field
- [ ] `mtp_filenames` are included in `all_files` validation chain
- [ ] MTP file downloads auto-set `selected_mtp_model` on parent model config
- [ ] MTP-only pulls create stub model entries (same as mmproj)
- [ ] MTP files are tagged with `kind=Mtp` in model_files DB

---

### Task 3: Argument Injection — --mtp-model in build_full_args

**Context:**
Inject `--mtp-model <path>` into the server launch command when `mtp_model` is set AND `draft-mtp` is in `spec_types`. This is different from `--mmproj` (which always injects) — `--mtp-model` is gated on the spec decoding checkbox being checked.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/config/resolve/tests/spec_decoding/mtp.rs`

**What to implement:**

1. **`build_full_args()`** (`config/resolve/mod.rs`):
   - After the `--mmproj` injection block, add `--mtp-model` injection:
     ```rust
     // Inject --mtp-model from model card, only if:
     // 1. mtp_model is set
     // 2. The referenced quant has kind = Mtp
     // 3. draft-mtp is in spec_decoding.spec_types (user enabled it)
     if let (Some(ref model_id), Some(ref mtp_name)) = (&server.model, &server.mtp_model) {
         let has_draft_mtp = server.spec_decoding.spec_types.iter()
             .any(|t| t == "draft-mtp");
         if has_draft_mtp {
             if let Some(mtp_entry) = server.quants.get(mtp_name.as_str()) {
                 if mtp_entry.kind == crate::config::QuantKind::Mtp {
                     let models_dir = self.models_dir()?;
                     let mtp_path = repo_path(&models_dir, model_id).join(&mtp_entry.file);
                     let already_has_mtp = grouped
                         .iter()
                         .any(|e| matches!(crate::config::flag_name(e), Some("--mtp-model")));
                     if !already_has_mtp {
                         let path_str = mtp_path.to_string_lossy();
                         let quoted = crate::config::quote_value(&path_str);
                         grouped.push(format!("--mtp-model {}", quoted));
                     }
                 } else {
                     tracing::warn!(
                         "mtp_model '{}' for model '{}' has kind={:?}, expected Mtp",
                         mtp_name, model_id, mtp_entry.kind
                     );
                 }
             } else {
                 tracing::warn!(
                     "mtp_model '{}' not found in ModelConfig for model '{}'",
                     mtp_name, model_id
                 );
             }
         }
     }
     ```
   - No backend gate (follows mmproj precedent of no gate — works for any backend, silently ignored if not llama.cpp)

2. **Tests** (`config/resolve/tests/spec_decoding/mtp.rs`):
   - This file already exists with spec decoding tests — add MTP-specific tests

**Steps:**
- [ ] Write failing test `test_mtp_model_injected_when_draft_mtp_enabled` in `config/resolve/tests/spec_decoding/mtp.rs`:
  - ModelConfig with `mtp_model: Some("mtp-F16.gguf")`, `spec_types: ["draft-mtp"]`, quant with `kind=Mtp`
  - Verify `--mtp-model <path>` is in args
- [ ] Write failing test `test_mtp_model_not_injected_without_draft_mtp`:
  - ModelConfig with `mtp_model: Some(...)` but `spec_types: []` (empty)
  - Verify `--mtp-model` is NOT in args
- [ ] Write failing test `test_mtp_model_no_duplicate_when_in_args`:
  - ModelConfig with `args: ["--mtp-model", "/custom/path.gguf"]` and `mtp_model` set
  - Verify `--mtp-model` appears exactly once
- [ ] Write failing test `test_mtp_model_not_injected_when_none`:
  - ModelConfig with `mtp_model: None` (default), `spec_types: ["draft-mtp"]`
  - Verify `--mtp-model` is NOT in args
- [ ] Write failing test `test_mtp_model_warns_on_kind_mismatch`:
  - ModelConfig with `mtp_model` referencing a quant with `kind=Model`
  - Verify warning is logged (use tracing subscriber or check no panic)
- [ ] Run `cargo test --package tama-core -- spec_decoding::mtp` — verify new tests fail
- [ ] Implement `--mtp-model` injection in `build_full_args()` in `config/resolve/mod.rs`
- [ ] Run `cargo test --package tama-core -- spec_decoding::mtp` — verify all MTP tests pass
- [ ] Run `cargo test --package tama-core` — all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: inject --mtp-model flag gated on draft-mtp spec type"

**Acceptance criteria:**
- [ ] `--mtp-model <path>` injected when `mtp_model` set AND `draft-mtp` in `spec_types`
- [ ] `--mtp-model` NOT injected when `mtp_model` set but `draft-mtp` NOT in `spec_types`
- [ ] `--mtp-model` NOT injected when `mtp_model` is `None`
- [ ] No duplicate `--mtp-model` if user already has it in `args`
- [ ] Warning logged when referenced quant has wrong kind or doesn't exist

---

### Task 4: Web Types + Pull Wizards — Frontend QuantKind, Both Wizard Components

**Context:**
Add MTP support to the web frontend types and both pull wizard components. The standalone wizard (`pull_wizard/mod.rs`) and the model editor's modal wizard (`pull_quant_wizard.rs`) both need MTP bucketing, a third selection table, and `mtp_filenames` in the pull request.

**Files:**
- Modify: `crates/tama-web/src/components/pull_wizard/mod.rs`
- Modify: `crates/tama-web/src/components/pull_wizard/components/selection_step.rs`
- Modify: `crates/tama-web/src/components/pull_quant_wizard.rs`
- Modify: `crates/tama-web/src/pages/model_editor/types.rs`

**What to implement:**

1. **`QuantKind`** (`pull_wizard/mod.rs` and `model_editor/types.rs`):
   - Add `Mtp` variant to both frontend `QuantKind` enums (mirrors core)

2. **`PullRequest`** (`pull_wizard/mod.rs`):
   - Add `pub mtp_filenames: Vec<String>` field

3. **`pull_wizard/mod.rs`** (standalone wizard):
   - Add signals: `available_mtps: RwSignal<Vec<QuantEntry>>`, `selected_mtp_filenames: RwSignal<HashSet<String>>`
   - In quant fetch (where quants are split into model/mmproj), add MTP bucketing:
     ```rust
     for q in quants {
         match q.kind {
             QuantKind::Mmproj => mmprojs.push(q),
             QuantKind::Mtp => mtps.push(q),
             _ => model_quants.push(q),
         }
     }
     ```
   - Pass `available_mtps` and `selected_mtp_filenames` to `SelectionStep`
   - In `on_next` callback, include `mtp_filenames` in `PullRequest`
   - In reset effect, clear `selected_mtp_filenames`

4. **`selection_step.rs`**:
   - Add props: `available_mtps: Signal<Vec<QuantEntry>>`, `selected_mtp_filenames: RwSignal<HashSet<String>>`
   - Add third table after "Vision Projectors":
     ```html
     <Show when=move || !available_mtps.get().is_empty()>
         <div class="mt-4 mb-2">
             <h3 class="form-label">"MTP Draft Models"</h3>
             <p class="text-muted text-sm mb-2">"Select MTP draft model files for speculative decoding (mtp-*.gguf)."</p>
             <table class="data-table">
                 <!-- Same structure as Vision Projectors table -->
             </table>
         </div>
     </Show>
     ```
   - Update disabled guard on "Next" button to include `selected_mtp_filenames`:
     ```rust
     prop:disabled=move || selected_filenames.get().is_empty() && selected_mmproj_filenames.get().is_empty() && selected_mtp_filenames.get().is_empty()
     ```

5. **`pull_quant_wizard.rs`** (model editor modal):
   - Add signals: `available_mtps`, `selected_mtp_filenames`
   - In quant fetch (lines ~168 and ~307 where mmproj bucketing happens), add MTP bucketing
   - In `on_next` callback, include `mtp_filenames` in `PullRequest` (line ~358)
   - In reset effect, clear `selected_mtp_filenames`
   - In `on_complete` callback (model editor), add `kind == QuantKind::Mtp` detection:
     ```rust
     let kind = if lower.starts_with("mmproj") && lower.ends_with(".gguf") {
         QuantKind::Mmproj
     } else if lower.starts_with("mtp") && lower.ends_with(".gguf") {
         QuantKind::Mtp
     } else {
         QuantKind::Model
     };
     ```

6. **`model_editor/types.rs`**:
   - Add `Mtp` to `QuantKind` enum
   - Add `mtp_model: Option<String>` to `ModelForm` and `ModelDetail`

**Steps:**
- [ ] Add `Mtp` variant to `QuantKind` in `pull_wizard/mod.rs` and `model_editor/types.rs`
- [ ] Add `mtp_filenames` to `PullRequest` in `pull_wizard/mod.rs`
- [ ] Add `available_mtps` / `selected_mtp_filenames` signals to `pull_wizard/mod.rs`
- [ ] Add MTP bucketing in quant fetch in `pull_wizard/mod.rs`
- [ ] Add `mtp_filenames` to `PullRequest` in `pull_quant_wizard.rs`
- [ ] Add MTP bucketing in quant fetch in `pull_quant_wizard.rs`
- [ ] Add `available_mtps` / `selected_mtp_filenames` signals to `pull_quant_wizard.rs`
- [ ] Add `kind == QuantKind::Mtp` detection in `on_complete` callback in `pull_quant_wizard.rs`
- [ ] Add MTP table to `selection_step.rs` (with `<Show>` when mtps not empty)
- [ ] Update disabled guard on "Next" button to include `selected_mtp_filenames`
- [ ] Update the `<SelectionStep ... />` call sites in `pull_wizard/mod.rs` and `pull_quant_wizard.rs` to pass `available_mtps` and `selected_mtp_filenames` props (search for existing `SelectionStep` usage and add the new props alongside `available_mmprojs` / `selected_mmproj_filenames`)
- [ ] Add `mtp_model` to `ModelForm` and `ModelDetail` in `model_editor/types.rs`
- [ ] Run `cargo build --package tama-web --features ssr` and `cargo build --package tama-web --features hydrate` — verify both SSR and CSR targets compile
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add MTP support to pull wizards and web types"

**Acceptance criteria:**
- [ ] `QuantKind::Mtp` exists in both frontend type files
- [ ] Pull wizard splits quants into 3 buckets: Model, Mmproj, Mtp
- [ ] Selection step shows "MTP Draft Models" table when repo has MTP files
- [ ] `PullRequest` includes `mtp_filenames`
- [ ] `on_complete` callback detects MTP files and merges with `kind=Mtp`
- [ ] `ModelForm` / `ModelDetail` have `mtp_model` field

---

### Task 5: Model Editor UI — Spec Decoding MTP Dropdown

**Context:**
Add the MTP file selector dropdown to the Spec Decoding form. This is the primary UI interaction point for MTP files — users select which MTP draft model to use when `draft-mtp` is enabled.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor/spec_decoding_form.rs`
- Modify: `crates/tama-web/src/pages/model_editor/mod.rs`
- Modify: `crates/tama-web/src/pages/model_editor/api.rs`

**What to implement:**

1. **`spec_decoding_form.rs`**:
   - Add MTP Draft Model dropdown inside the `<Show when=move || has_draft_mtp.get()>` block, after the "Draft GPU Layers" input:
     ```html
     // After draft_ngl input, add:
     <div class="form-group">
         <label class="form-label" for="field-mtp-model">
             "MTP Draft Model"
             <div class="form-hint">"Select an MTP draft model file for speculative decoding"</div>
         </label>
         <select id="field-mtp-model" class="form-select" on:change=...>
             <option value="">"(none)"</option>
             {mtp_quants.iter().map(|(key, q)| {
                 view! { <option value={key.clone()} selected={...}>{q.file.clone()}</option> }
             }).collect::<Vec<_>>()}
         </select>
     </div>
     ```
   - The dropdown reads from `form.quants` filtered by `kind == QuantKind::Mtp`
   - `on:change` sets `form.mtp_model` to the selected key (or `None` for "(none)")
   - Dropdown is only shown when `has_draft_mtp` is true AND there are MTP quants available

2. **`mod.rs`** (model editor page):
   - In the form population effect, include `mtp_model` when building `ModelForm` from `ModelDetail`
   - In `save_action`, include `mtp_model` in `form_data`
   - In `delete_quant_action`, clear `mtp_model` when deleting an MTP quant (same pattern as clearing `mmproj`)

3. **`api.rs`** (model editor API):
   - In `save_model`, include `mtp_model` in the JSON payload sent to the backend (same pattern as `mmproj`)
   - In `fetch_model`, deserialize `mtp_model` from the backend response (same pattern as `mmproj`)

**Steps:**
- [ ] Add `has_mtp_quants` signal in `spec_decoding_form.rs`:
  ```rust
  let has_mtp_quants = Signal::derive(move || {
      form.get().as_ref().map(|f| {
          f.quants.iter().any(|(_, q)| q.kind == QuantKind::Mtp)
      }).unwrap_or(false)
  });
  ```
- [ ] Add MTP dropdown inside the `<Show when=move || has_draft_mtp.get()>` block, after Draft GPU Layers
- [ ] Wire `on:change` to set `form.mtp_model` (use `target_value` utility). In the handler: if `target_value(&e)` is empty string, set `form.mtp_model = None`; otherwise set `form.mtp_model = Some(val)`.
- [ ] Show "(none)" option + one option per MTP quant
- [ ] Update `mod.rs` form population to include `mtp_model: d.mtp_model`
- [ ] Update `mod.rs` save action to include `mtp_model` in `form_data`
- [ ] Update `mod.rs` delete_quant_action to clear `mtp_model` when deleting MTP quant
- [ ] Update `api.rs` save/fetch to include `mtp_model` (mirrors `mmproj` pattern)
- [ ] Run `cargo build --package tama-web` — verify compiles
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add MTP draft model dropdown to Spec Decoding form"

**Acceptance criteria:**
- [ ] MTP dropdown appears in Spec Decoding section when `draft-mtp` is checked
- [ ] MTP dropdown shows "(none)" + all MTP quants from `form.quants`
- [ ] Selecting an MTP file sets `form.mtp_model`
- [ ] Selecting "(none)" clears `form.mtp_model`
- [ ] MTP dropdown is hidden when `draft-mtp` is unchecked
- [ ] `mtp_model` is saved/loaded correctly through the API
- [ ] Deleting an MTP quant clears `mtp_model` if it was the selected one

---

## Verification

After all tasks are complete:

1. Run `cargo test --workspace` — all tests pass
2. Run `cargo clippy --workspace -- -D warnings` — no warnings
3. Run `cargo fmt --all` — no formatting issues
4. Run `cargo build --release --workspace` — release build succeeds
5. Manual test flow:
   - Pull a model with MTP files via the web UI
   - Verify MTP files appear in the "MTP Draft Models" selection table
   - After download, verify MTP files appear in the Spec Decoding dropdown
   - Select an MTP file, check `draft-mtp`, save
   - Verify `--mtp-model` is in the resolved args
   - Uncheck `draft-mtp`, verify `--mtp-model` is NOT in resolved args
