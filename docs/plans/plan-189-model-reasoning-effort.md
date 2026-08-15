# Model Reasoning Effort Plan

**Goal:** Expose per-model reasoning-effort capability (`supportsReasoningEffort` boolean derived from a stored `reasoningLevels` array) on all client-facing model info (`/v1/opencode/models`, `/v1/models`, model detail API), editable via a comma-separated text input in the model editor, and consumed by pi clients through the `pi-provider-tama` plugin.

**Architecture:** One new JSON TEXT column on `model_configs` (`reasoning_levels`, pi-vocabulary: `off, minimal, low, medium, high, xhigh, max`). The `supportsReasoningEffort` boolean is derived everywhere ("levels non-empty") and never stored (ADR-0008). Client endpoints also emit opencode-canonical `reasoning_options` (derived, `off`→`none`) (ADR-0009). The chat forwarder normalizes incoming `reasoning_effort: "off"` → `"none"` because no backend accepts `"off"`. The pi plugin maps the fields to pi's `Model.reasoning` + `compat.supportsReasoningEffort` + `thinkingLevelMap` (explicit `null` holes; `off` → wire value `"none"`).

**Tech Stack:** Rust (Axum, serde, rusqlite, Leptos WASM UI), TypeScript (pi extension), SQLite migration, vitest.

**References:** `docs/research/reasoning-effort-model-info.md` · ADR-0008 · ADR-0009

**Conventions (from AGENTS.md):** TDD (failing test → implement → pass); targeted `cargo nextest run --package <crate> -- <filter>` while coding; full gate (fmt + clippy all-targets + SSR clippy + nextest workspace) before the final commit. Commit prefixes: `feat:`, `fix:`, `chore:`, `docs:`.

---

### Task 1: Storage — `ModelConfig` field, migration `_0049`, DB record plumbing, derived helper

**Context:**
This task creates the persistence foundation. A model's accepted reasoning-effort levels are stored as a JSON array in a new nullable TEXT column on `model_configs` — the same pattern as `modalities` and `vllm_config` (JSON-encoded TEXT columns, nullable so existing rows are unaffected). The `supportsReasoningEffort` boolean is deliberately NOT stored: it is derived as "levels non-empty" via a single helper method that all later serialization points call (ADR-0008). NOTE: migration numbers `_0044`…`_0048` are already taken; this one is **`_0049`**.

**Files:**
- Modify: `crates/tama-core/src/config/types/model.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: backup schema/restore column lists (search `crates/tama-core/src/backup/` for `MODEL_CONFIGS_FULL_SCHEMA` and the explicit-column `INSERT INTO model_configs` statements in restore/merge)
- Create: `crates/tama-core/src/db/migrations/_0049_add_reasoning_levels.rs`
- Test: existing `#[cfg(test)]` modules in the files above (create at file bottom if absent)

**What to implement:**

1. `ModelConfig` (`crates/tama-core/src/config/types/model.rs:27`): add, next to the existing `modalities` field (line ~128):
   ```rust
   /// Reasoning effort levels this model accepts (pi vocabulary:
   /// off, minimal, low, medium, high, xhigh, max). When non-empty,
   /// the model advertises `supportsReasoningEffort: true` on client
   /// model-info endpoints. Stored as a JSON array in a TEXT column.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub reasoning_levels: Option<Vec<String>>,
   ```
2. `ModelConfig` impl: add the single derivation point (ADR-0008 — all callers must use this, never inline the check):
   ```rust
   /// True when the model advertises adjustable reasoning effort
   /// (i.e. at least one reasoning level is configured).
   pub fn supports_reasoning_effort(&self) -> bool {
       self.reasoning_levels.as_ref().is_some_and(|levels| !levels.is_empty())
   }
   ```
3. `to_db_record` (model.rs:~200-287): add next to the `modalities` serialization:
   ```rust
   reasoning_levels: self
       .reasoning_levels
       .as_ref()
       .and_then(|v| serde_json::to_string(v).ok()),
   ```
4. `from_db_record` (model.rs:~294-375): add next to the `modalities` parse (parse error → `None`, matching the existing pattern):
   ```rust
   reasoning_levels: record
       .reasoning_levels
       .as_deref()
       .and_then(|s| serde_json::from_str(s).ok()),
   ```
5. `ModelConfigRecord` (`crates/tama-core/src/db/queries/types.rs:14`): add `pub reasoning_levels: Option<String>, // raw JSON string` after `provider_name`. Update:
   - the doc comment "All 39 columns" → "All 40 columns"
   - the index-map table: add `| 39    | reasoning_levels     |` row (the table currently ends at index 38)
   - the doc note after the table: add `Migration _0049 appended `reasoning_levels` (index 39).`
   - `COLUMNS` const: append `, reasoning_levels` at the end (after `provider_name`)
   - `INSERT_COLUMNS` const: append `, reasoning_levels` at the end, AND update its doc comment "The 38 non-`id` columns" → "The 39 non-`id` columns"
   - `from_row`: add `.get(39)` for the new column (follow the existing index-order mapping)
6. `upsert_model_config` (`crates/tama-core/src/db/queries/model_config_queries.rs`) — the ONLY production insert/update path for model configs; it has hardcoded placeholders/params that `cargo check` will NOT catch (runtime SQL):
   - add `?39` to the `VALUES (...)` placeholder list in the `INSERT INTO model_configs ({}) VALUES (...)` statement
   - add `record.reasoning_levels` to the `params![...]` list (position matching the column order)
   - add `reasoning_levels = excluded.reasoning_levels` to the `ON CONFLICT(repo_id) DO UPDATE SET` clause — plain overwrite is intentional (unlike the adjacent `COALESCE` HF-metadata fields): the editor always sends a non-NULL array — `[]` to clear — so `COALESCE` would make clearing impossible, and scan/pull upserts never populate this column
   - update `test_model_config_columns_match_insert_columns`: its column-count assertions change from 39/38 to 40/39
7. Backup/restore round-trip: the PRODUCTION restore column list is the hard-coded `model_configs_columns: &[&str]` array in `merge_database` (`crates/tama-core/src/backup/merge.rs`, ~line 180, interpolated into `INSERT OR IGNORE INTO model_configs ({cols}) SELECT {cols}`) — add `"reasoning_levels"` to it. ALSO update the test-only `MODEL_CONFIGS_FULL_SCHEMA` constant in the same file's `#[cfg(test)]` module so the backup tests round-trip the new column. (Pre-existing gap — flag in the PR description, do NOT fix here: that array already omits `provider_name` from `_0047`.)
8. Create `crates/tama-core/src/db/migrations/_0049_add_reasoning_levels.rs` — copy the structure of `_0044_add_vllm_config.rs` exactly:
   ```rust
   //! Migration v49: Add reasoning_levels column to model_configs.
   //!
   //! Stores the JSON array of reasoning effort levels the model accepts
   //! (pi vocabulary: off, minimal, low, medium, high, xhigh, max).
   //! Nullable so existing rows remain unaffected.

   pub const MIGRATION: (i32, bool, &str) = (
       49,
       false, // does not require FKs off
       r#"
   ALTER TABLE model_configs ADD COLUMN reasoning_levels TEXT DEFAULT NULL;
   "#,
   );
   ```
   plus the inline `#[cfg(test)]` test following `_0044`'s test exactly: `run_up_to(&conn, 48)` → assert column count for `reasoning_levels` is 0 → `run_up_to(&conn, 49)` → assert 1.
9. Register in `crates/tama-core/src/db/migrations.rs`: add `mod _0049_add_reasoning_levels;` with the other `mod` declarations (alphabetical order), add `_0049_add_reasoning_levels::MIGRATION,` as the last entry of the `MIGRATIONS` array (line ~92ff, after `_0048_rename_backend_to_provider::MIGRATION,`), and bump `pub const LATEST_VERSION: i32 = 48;` → `49` (two tests assert this constant equals the last migration version: `migrations/migrations_tests.rs` and `db/mod.rs` `test_user_version_updated`).
10. Catch-all for exhaustive struct literals — adding fields to `ModelConfig` AND `ModelConfigRecord` breaks many exhaustive literals, mostly **in test code that plain `cargo check --workspace` does not build**. Known affected sites (add `reasoning_levels: None` or the appropriate value):
    - non-test: `tama-core/src/proxy/tama_handlers/pull/verify.rs` (two `ModelConfig` literals), `tama/src/api/models/info.rs` (`make_record()` builds `ModelConfigRecord`), **`tama/src/types/config/core_conv.rs` (~line 421, the WASM/SSR mirror→core `From` impl builds the core `ModelConfig` exhaustively — this file is gated behind `#[cfg(feature = "ssr")]` and is NOT caught by `cargo check --workspace --all-targets`; the mirror type itself does NOT get the field, only the mirror→core direction needs `reasoning_levels: None`)**
    - test modules: `ModelConfig` literals in `config/types/model_tests.rs`, `models/manager_tests.rs`, `proxy/server/tests.rs`, `proxy/handlers/{tests,alias_tests,get_model_tests,list_models_tests}.rs`, `proxy/lifecycle/tests.rs`, `proxy/status.rs`, `tama_handlers/models/tests/*`, `bench/llama_bench/mod.rs` (`seed_test_db` ~line 437); `ModelConfigRecord` literals in `db/queries/tests.rs`, `db/repository.rs` (`insert_model_config`), `db/backfill/{hf_metadata,initial_backfill,vllm_config}.rs`, `models/manager_tests.rs` (`make_test_record` ~line 10), `updates/checker/orchestration_tests.rs` (~lines 29 and 256), `tama/src/api/aliases/mod.rs`, `tama/src/api/models/crud/tests.rs`
    Run `cargo check --workspace --all-targets` (NOT plain `cargo check --workspace` — it skips test targets) AND `cargo check --package tama --features ssr --all-targets` (for the SSR-gated `core_conv.rs`), and fix every resulting error.

**Do NOT:** add a `supports_reasoning_effort` column or field anywhere; store the boolean; validate the level values here (validation is Task 2's job).

**Steps:**
- [ ] Write the migration inline test (item 8's test) in `crates/tama-core/src/db/migrations/_0049_add_reasoning_levels.rs`
- [ ] Write unit tests in `crates/tama-core/src/config/types/model.rs` test module: `supports_reasoning_effort` returns false for `None`, false for `Some(vec![])`, true for `Some(vec!["low"])`; `to_db_record` → `from_db_record` round-trip preserves `Some(vec!["off","low"])` and maps `None` → `None`
- [ ] Run `cargo nextest run --package tama-core -- reasoning`
  - Did it fail with [compile errors — field/methods don't exist yet]? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–9 above
- [ ] Run `cargo nextest run --package tama-core -- reasoning` — did all tests pass?
- [ ] Run `cargo nextest run --package tama-core -- migrations` (migration suite still green after registration)
- [ ] Run `cargo nextest run --package tama-core -- db::queries` (catches `upsert_model_config` placeholder/param mismatch and the column-count test — this is a RUNTIME SQL failure, not a compile error)
- [ ] Run `cargo check --workspace --all-targets` (fix every exhaustive-literal error per item 10)
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: store per-model reasoning levels (migration _0049)"

**Acceptance criteria:**
- [ ] `model_configs` has a nullable `reasoning_levels TEXT` column (migration `_0049` registered and passing its inline test)
- [ ] `ModelConfig.reasoning_levels: Option<Vec<String>>` round-trips through the DB record
- [ ] `ModelConfig::supports_reasoning_effort()` exists and is the only place the boolean is derived
- [ ] `cargo nextest run --package tama-core` fully green; `cargo fmt --all --check` clean

---

### Task 2: Management API — request bodies, validation, detail JSON

**Context:**
The model editor (Task 5) reads and writes model config through the management API, so this task makes `reasoningLevels` a first-class field of the create/update/patch bodies and of the model detail response. Level values are validated at this boundary because the backends are stricter than our editor could be: vLLM 400s on any `reasoning_effort` value outside its enum, so a typo saved in the editor would otherwise fail at chat time with a confusing error. The valid set is pi's 7-level vocabulary (storage speaks pi's words — ADR-0009). The detail endpoint exposes ONLY the raw stored array; the derived boolean and `reasoning_options` are client-endpoint concerns (Task 3).

**Clear contract (matches the `modalities` merge pattern):** `null`/absent in the body = preserve existing; `[]` (empty array) = clear. The editor (Task 5) sends `[]` — never `null` — when the input is empty, so clearing works through PUT.

**Files:**
- Modify: `crates/tama/src/api/models/crud/mod.rs`
- Modify: `crates/tama/src/api/models/crud/create.rs` (validator call site)
- Modify: `crates/tama/src/api/models/crud/update.rs` (validator call sites)
- Modify: `crates/tama/src/api/models/info.rs`
- Test: `#[cfg(test)]` module in `crates/tama/src/api/models/crud/mod.rs` (create at file bottom if absent)

**What to implement:**

1. Shared normalizer in `crates/tama/src/api/models/crud/mod.rs` (single source for the rules, used by both validators):
   ```rust
   /// Valid reasoning level values (pi thinking-level vocabulary).
   pub(crate) const VALID_REASONING_LEVELS: &[&str] =
       &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

   /// Trim + lowercase each value, drop empties, dedupe preserving order.
   /// Returns an error (naming the offenders and the valid set) when any
   /// value is outside `VALID_REASONING_LEVELS`.
   pub(crate) fn normalize_reasoning_levels(
       levels: &[String],
   ) -> Result<Vec<String>, String> { ... }
   ```
   Behavior: `[" Off ", "low", "LOW", "xhig"]` → error mentioning `xhig` and listing the valid set. `["off", "low", "low"]` → `["off", "low"]`. Empty input → `Ok(vec![])`.
2. `ModelBody` (crud/mod.rs:~32): add `#[serde(default)] pub reasoning_levels: Option<Vec<String>>,` next to `modalities`. (These bodies derive `Deserialize` only — do NOT add `skip_serializing_if`, it would be a no-op.)
3. `ModelPatchBody` (crud/mod.rs:~84): same field.
4. `apply_model_body` (crud/mod.rs:~192): `reasoning_levels: body.reasoning_levels.or(base.reasoning_levels.clone())` — next to the `modalities` line. ALSO add `reasoning_levels: None,` to the `base` struct literal in that function (~line 204-241) if it constructs a full `ModelConfig`.
5. `apply_model_patch` (crud/mod.rs:~112): `reasoning_levels: body.reasoning_levels.or_else(|| existing.reasoning_levels.clone()),` — next to the `modalities` line.
6. `validate_model_body` (crud/mod.rs:~329) and `validate_model_patch` (crud/mod.rs:~395): when the field is `Some(levels)`, run `normalize_reasoning_levels` and surface the error using the **exact error style the existing per-field validations in those functions use** (match how `modalities`/length-check errors are reported). Error message shape: `invalid reasoning level(s): <offenders> — valid values: off, minimal, low, medium, high, xhigh, max`. On success, store the normalized value back into the body (so trim/lowercase/dedupe are persisted). Because both validators currently take immutable references, this requires:
   - changing signatures to `fn validate_model_body(body: &mut ModelBody) -> Result<...>` and `pub fn validate_model_patch(body: &mut ModelPatchBody) -> Result<...>` (keep the existing return/error types)
   - updating call sites: `create.rs` (`validate_model_body(&body.model)` → `let mut body = body; ... validate_model_body(&mut body.model)`) and `update.rs` (both PUT and PATCH paths: `let mut body = body;` before the validate call, since the body is moved into `apply_model_*` later)
7. Model detail JSON in `crates/tama/src/api/models/info.rs` (~line 160-210, the `json!` block that includes `"modalities"` at ~line 186): add `"reasoningLevels": m.reasoning_levels,`.

**Do NOT:** add the derived boolean or `reasoning_options` to the management API responses; validate in the serialization layer (Task 3); touch the editor (Task 5).

**Steps:**
- [ ] Write unit tests for `normalize_reasoning_levels` in `crates/tama/src/api/models/crud/mod.rs` test module: happy path (mixed case/whitespace/dupes), empty → empty, invalid token → error naming the offender, all-valid single value
- [ ] Run `cargo nextest run --package tama -- normalize_reasoning`
  - Did it fail with [function doesn't exist]? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–7
- [ ] Run `cargo nextest run --package tama -- crud` (and any existing crud/validate tests)
  - Did all tests pass? If not, fix and re-run.
- [ ] If there are existing API-level tests for PUT/PATCH model (search `crates/tama/src/api/models/` tests), add cases: PUT with `reasoningLevels: ["off","low"]` persists; PUT with `reasoningLevels: ["bogus"]` → 400; **PUT with `reasoningLevels: []` on a model that already has levels clears them** (detail then shows `[]` and no `supportsReasoningEffort`); PATCH without the field leaves it unchanged; PATCH with `[]` clears it
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: reasoningLevels in model create/update/patch APIs with validation"

**Acceptance criteria:**
- [ ] `POST/PUT/PATCH /tama/v1/models` accept `reasoningLevels`; invalid values → 400 naming the valid set
- [ ] Clear contract holds: `null`/absent preserves, `[]` clears (both PUT and PATCH)
- [ ] Values are normalized (trim/lowercase/dedupe) before persisting
- [ ] `GET /tama/v1/models/:id` returns `reasoningLevels` (raw array or null)
- [ ] `cargo nextest run --package tama` green; fmt clean

---

### Task 3: Client-facing serialization — opencode entries, `/v1/models`, derived fields

**Context:**
This is the feature's visible surface. Three endpoints advertise the new fields to clients: `/v1/opencode/models` (opencode-shaped; consumed by the pi plugin), `/v1/models` (OpenAI-shaped list), and (from Task 2) the detail API. Per the approved design: flat fields use camelCase (`supportsReasoningEffort`, `reasoningLevels`) because the JS-side consumers (pi) expect it, while the opencode-canonical field keeps opencode's own snake_case spelling `reasoning_options` so it is byte-compatible with the models.dev catalog. The opencode entry's existing `reasoning` boolean becomes `props-computed OR derived` — which also fixes vLLM-served thinking models (vLLM has no `/props`, so the extraction defaults to `false`). `reasoning_options` is derived at serialization time (never stored): `[{ "type": "effort", "values": <levels with off→none> }]`, present only when levels are non-empty (ADR-0009).

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/mod.rs` (re-export for cross-module use)
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/opencode.rs`
- Modify: `crates/tama-core/src/proxy/handlers/models.rs`
- Test: `crates/tama-core/src/proxy/tama_handlers/models/tests/opencode.rs`, plus the existing `/v1/models` tests (search `crates/tama-core/src/proxy/server/tests.rs` and `crates/tama-core/src/proxy/handlers/` for `handle_list_models` tests)

**What to implement:**

1. `ModelEntry` (`crates/tama-core/src/proxy/tama_handlers/models/utils.rs:34-51`): add three fields:
   ```rust
   /// Derived at serialization: true when the model has configured
   /// reasoning levels (see ADR-0008). Wire name is camelCase.
   #[serde(rename = "supportsReasoningEffort")]
   pub supports_reasoning_effort: bool,
   /// Raw stored levels (pi vocabulary). Absent when None.
   #[serde(rename = "reasoningLevels", skip_serializing_if = "Option::is_none")]
   pub reasoning_levels: Option<Vec<String>>,
   /// Opencode-canonical derived field (snake_case on purpose —
   /// byte-compatible with the models.dev catalog). Absent when None.
   #[serde(skip_serializing_if = "Option::is_none")]
   pub reasoning_options: Option<serde_json::Value>,
   ```
2. Helper in `utils.rs` (one place builds the canonical field), plus a re-export so the OTHER module tree (`proxy/handlers/models.rs`) can call it — `mod utils;` in `models/mod.rs` is private, so add to `crates/tama-core/src/proxy/tama_handlers/models/mod.rs`:
   ```rust
   pub(crate) use utils::reasoning_options_from_levels;
   ```
   ```rust
   /// Build the opencode-canonical `reasoning_options` value from stored
   /// levels: [{ "type": "effort", "values": [...] }] with `off` mapped to
   /// `none` (ADR-0009). Returns None for empty/absent levels.
   pub(crate) fn reasoning_options_from_levels(
       levels: &Option<Vec<String>>,
   ) -> Option<serde_json::Value> { ... }
   ```
3. `build_model_entry` (utils.rs:~60-196): populate the three new fields from the `ModelConfig` it already receives:
   - `supports_reasoning_effort: cfg.supports_reasoning_effort()`
   - `reasoning_levels: cfg.reasoning_levels.clone()`
   - `reasoning_options: reasoning_options_from_levels(&cfg.reasoning_levels)`
   - **Effective reasoning** (the OR merge approved in design): wherever the `reasoning` flag is currently set from the `/props`-derived capabilities, change it to `props_reasoning || cfg.supports_reasoning_effort()`.
4. `/v1/models` handler (`crates/tama-core/src/proxy/handlers/models.rs`, `handle_list_models`): import the helper as `use crate::proxy::tama_handlers::models::reasoning_options_from_levels;` and inject the three fields **only when the model's config has non-empty levels** (entries without levels stay byte-identical to today):
   - **Phase 3 (loaded models, ~line 240-252):** `BackendModelEntry` has a flattened `extra` map. After `entry.ready = Some(true)`, look up `all_configs.get(config_name)`; if it has non-empty `reasoning_levels`, insert into `entry.extra`: `"supportsReasoningEffort": true`, `"reasoningLevels": <array>`, `"reasoning_options": <helper output>`.
   - **Phase 4 (unloaded models, the `json!` literal ~line 262-270):** build the value as `let mut entry = json!({...})` and conditionally insert the same three keys from `server_cfg`.
   - **Phase 5 (aliases, ~line 285-305):** extend the existing metadata-inheritance block (which already inherits `context_length`/`modalities` from the target config) with the same three keys when the target has non-empty levels.
5. Alias entries in `/v1/opencode/models` inherit everything automatically (whole-entry copy in opencode.rs:~102-139) — do not change that code; cover it with a test.

**Do NOT:** rename any existing wire fields; add the fields to the management API; clamp/validate levels here (validated at save time in Task 2); emit `reasoning_options` when levels are empty.

**Steps:**
- [ ] Write failing tests in `crates/tama-core/src/proxy/tama_handlers/models/tests/opencode.rs`:
  - config with `reasoning_levels = Some(["off","low","medium","xhigh"])` → entry has `supportsReasoningEffort: true`, `reasoningLevels: [...]`, `reasoning_options: [{type:"effort", values:["none","low","medium","xhigh"]}]`
  - config without levels → `supportsReasoningEffort: false`, no `reasoningLevels` key in serialized JSON, no `reasoning_options` key
  - effective `reasoning`: (props reasoning=false + levels set) → true; (props reasoning=true + no levels) → true; (both false) → false
  - alias of a leveled model → inherits all three fields
- [ ] Run `cargo nextest run --package tama-core -- proxy::tama_handlers::models`
  - Did it fail with [fields don't exist]? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–4
- [ ] Update the drift-guard test `test_opencode_response_deserializes_into_typed` if it pins the old shape (it round-trips against `OpencodeModelsResponse` — verify it still compiles/passes; if it uses a local mirror struct, update that struct too)
- [ ] Add `/v1/models` tests: unloaded model with levels → 3 keys present; unloaded without → byte-identical to before (no keys); alias of a leveled model → keys inherited; loaded-model merge (if a test harness for ready backends exists — otherwise cover loaded-merge via the Phase 3 unit path or note the gap)
- [ ] Run `cargo nextest run --package tama-core -- models` (both handler test modules)
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: advertise reasoning effort levels on client model-info endpoints"

**Acceptance criteria:**
- [ ] `/v1/opencode/models` entries expose `supportsReasoningEffort` (always emitted — it's a plain bool), `reasoningLevels` (only when set), `reasoning_options` (off→none, only when set), with `reasoning` = props-OR-derived
- [ ] `/v1/models` entries expose the same three keys only when levels are configured (loaded, unloaded, and alias entries)
- [ ] `/v1/models` entries for models without levels are byte-identical to before this change (NOTE: opencode entries are NOT byte-identical — `supportsReasoningEffort: false` is always emitted; the drift-guard test passes because it round-trips against the updated typed struct)
- [ ] `cargo nextest run --package tama-core` green; fmt clean

---

### Task 4: Chat-path `off` → `none` normalization

**Context:**
No backend accepts `"off"` as `reasoning_effort`: vLLM is a strict enum and 400s; llama.cpp special-cases only `"none"` and silently passes any other string to the chat template (so `"off"` would leave thinking ON with no error). The pi plugin never sends `"off"` (it maps off→`"none"` in its thinkingLevelMap — Task 7), but other clients might. This task adds a one-value safety-net normalization at the point where the request body is already a mutable `serde_json::Value`: rewrite `reasoning_effort: "off"` to `"none"` before forwarding. No other translation or clamping in v1 — all other values pass through verbatim (vLLM surfaces invalid ones with a clear 400; llama.cpp feeds them to the template, and Qwen3.8's template implements low/medium/xhigh).

**Files:**
- Modify: `crates/tama-core/src/proxy/forward/request.rs`
- Modify: `crates/tama-core/src/proxy/handlers/chat.rs`
- Test: `#[cfg(test)]` module in `crates/tama-core/src/proxy/forward/request.rs` (create at file bottom if absent)

**What to implement:**

1. In `forward_request` (`crates/tama-core/src/proxy/forward/request.rs`), at the body-build site (~line 214-235, where `body` is `serde_json::Value` and already mutated for `model` + langfuse `stream_options`):
   ```rust
   // ADR-0009: no backend accepts "off" as reasoning_effort — the
   // ecosystem off-word is "none" (vLLM 400s on "off"; llama.cpp would
   // silently leave thinking on). Normalize for all clients.
   if body.get("reasoning_effort").and_then(serde_json::Value::as_str) == Some("off") {
       body["reasoning_effort"] = serde_json::json!("none");
   }
   ```
2. Extract that as a small testable helper that returns whether it changed anything, so callers can preserve zero-copy behavior:
   ```rust
   /// Rewrite `reasoning_effort: "off"` → `"none"` (ADR-0009).
   /// Returns true if the body was modified.
   pub(crate) fn normalize_reasoning_effort_body(body: &mut serde_json::Value) -> bool { ... }
   ```
   Call it from both sites (the local forwarder and the remote branch).
3. Remote-provider branch: the remote forward happens in the SHARED `resolve_and_load_server` (chat.rs ~line 63-90) — ONE change point covers both the non-streaming and streaming handlers (both call `resolve_and_load_server`; verify rather than assuming, but do not duplicate the logic in each handler). There: parse the bytes to `Value`, call the helper, and re-serialize **only if the helper returned true** (otherwise forward the original bytes untouched, preserving today's zero-copy behavior).

**Do NOT:** validate/clamp other effort values; touch the response path; add per-model level enforcement.

**Steps:**
- [ ] Write failing unit tests in `request.rs` test module: `{"reasoning_effort":"off"}` → `"none"`; `"low"` unchanged; absent unchanged; `{"reasoning_effort": 42}` unchanged (non-string)
- [ ] Run `cargo nextest run --package tama-core -- normalize_reasoning_effort`
  - Did it fail with [function doesn't exist]? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–3
- [ ] Run `cargo nextest run --package tama-core -- forward` plus any existing chat-forward tests
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: normalize reasoning_effort off→none before backend forwarding"

**Acceptance criteria:**
- [ ] Local and remote chat paths rewrite `reasoning_effort: "off"` → `"none"` before the backend
- [ ] All other bodies (incl. remote) are forwarded byte-identical to before
- [ ] `cargo nextest run --package tama-core` green; fmt clean

---

### Task 5: Model editor UI — "Reasoning levels" text input

**Context:**
The editor is where users set levels (first model to be seeded: the Qwen3.8 row, `off, low, medium, xhigh`). Per the approved design there is ONE control — a comma-separated text input (no checkbox; the derived boolean makes "has levels" the only state, ADR-0008). The form follows the existing `args` precedent exactly: `ModelForm` holds a raw `String`, the stored array is comma-joined on load, and the string is parsed into `Option<Vec<String>>` at save time. Client-side validation mirrors the server rule (Task 2) so users see the error before hitting 400.

**Files:**
- Modify: `crates/tama/src/pages/model_editor/types.rs`
- Modify: `crates/tama/src/pages/model_editor/mod.rs`
- Modify: `crates/tama/src/pages/model_editor/settings_form.rs`
- Modify: `crates/tama/src/pages/model_editor/api.rs`
- Test: `#[cfg(test)]` module for the parse helper (in `types.rs` or `mod.rs` where `args` parsing lives)

**What to implement:**

1. `types.rs`: `ModelDetail` gains the field **with an explicit serde rename** (the detail API emits camelCase `reasoningLevels`; omitting the rename would make serde silently ignore the key — the editor would then always show an empty input and a save would send `[]`, wiping configured levels):
   ```rust
   #[serde(rename = "reasoningLevels", default)]
   pub reasoning_levels: Option<Vec<String>>,
   ```
   (next to `modalities` at ~line 100). `ModelForm` gains `reasoning_levels_input: String` (raw text, like `args: String` at ~line 207).
2. Parse helper (put it where the existing `args` multiline→`Vec<String>` conversion lives, ~mod.rs:469-474, or in `types.rs` if that's more discoverable):
   ```rust
   /// Parse the comma-separated reasoning-levels input: split on commas
   /// and whitespace, trim, lowercase, drop empties, dedupe preserving
   /// order. Empty result → None. Error names invalid tokens and lists
   /// the valid set (mirror the server rule from Task 2 exactly — same
   /// valid set: off, minimal, low, medium, high, xhigh, max).
   fn parse_reasoning_levels_input(raw: &str) -> Result<Option<Vec<String>>, String> { ... }
   ```
3. `settings_form.rs`: add a text input near the modalities checkbox block, following the `field-display-name` input pattern (~line 151-168) with `set_input_value` init (~line 121-135):
   - Label: `Reasoning levels`
   - Placeholder/hint text: `off, low, medium, xhigh  — valid: off, minimal, low, medium, high, xhigh, max (empty = none)`
   - Bound to `form.reasoning_levels_input`; on save-click, run `parse_reasoning_levels_input` — on error, show the message using the form's existing inline-error mechanism and abort the save (mirror how other validation errors surface in this form).
4. `mod.rs`: form init (~line 203-252): `reasoning_levels_input: d.reasoning_levels.map(|v| v.join(", ")).unwrap_or_default()`; `save_action` (~line 505-534) has TWO things to update:
   - the `form_data = ModelForm { ... }` literal (~line 483) is exhaustive — add `reasoning_levels_input: initial_form.reasoning_levels_input.clone()` (or `String::new()`)
   - parse the input string (abort on error per item 3) and normalize to a `Vec<String>` before calling `save_model`: `match parsed { Err(e) => { /* surface e, abort */ } Ok(Some(v)) => v, Ok(None) => vec![] }` (empty input → `[]` = CLEAR per Task 2's contract; `null` would preserve)
   ALSO: `fetch_model`'s `id == "new"` branch constructs `ModelDetail` **exhaustively** — add `reasoning_levels: None` there; and the `ModelDetail` test literal in `types.rs` (`test_model_detail_hf_format_round_trip` or similar) needs `reasoning_levels: None` too.
5. `api.rs` (`save_model`, ~line 140-165): the parsed levels live in `save_action` (they are NOT a `ModelForm` field), so `save_model` gains a parameter — `pub async fn save_model(args: Vec<String>, form: ModelForm, is_new: bool, reasoning_levels: Vec<String>)` — and its single call site in `mod.rs` passes the normalized `Vec<String>` from item 4's match. In the JSON body add `"reasoningLevels": serde_json::json!(reasoning_levels)` (always an array; `[]` clears on the server).
6. Unit tests for `parse_reasoning_levels_input`: `""` → `Ok(None)`; `"off, low, medium, xhigh"` → `Ok(Some([off, low, medium, xhigh]))`; `" Off ,LOW, low "` → `Ok(Some([off, low]))`; `"off, bogus"` → `Err` naming `bogus`.

**Do NOT:** add a checkbox; auto-validate on every keystroke (parse at save); reorder or "sort" the user's level order (preserve input order — the list order is meaningful to the user).

**Steps:**
- [ ] Write failing unit tests for `parse_reasoning_levels_input` (item 6)
- [ ] Run `cargo nextest run --package tama -- parse_reasoning_levels`
  - Did it fail with [function doesn't exist]? If it passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–5 (incl. the `save_model` signature change and the two exhaustive `ModelDetail` literals in item 4)
- [ ] Run `cargo check --package tama` and `cargo check --package tama --features ssr` (the Leptos form compiles under both targets)
- [ ] Manually verify in dev mode (`make dev`): open a model, type `off, low, medium, xhigh`, save; reopen — the input shows the same text; clear it, save — the detail JSON shows `reasoningLevels: []` (cleared, NOT null — null would preserve) and `/v1/opencode/models` no longer reports `supportsReasoningEffort: true` for it; type `off, bogus` — inline error, save aborted
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: reasoning levels input in model editor"

**Acceptance criteria:**
- [ ] The editor shows one comma-separated text input; values round-trip (load → unchanged → save → unchanged)
- [ ] Invalid tokens produce an inline error and block the save
- [ ] Empty input CLEARS levels (saves `[]` per Task 2's clear contract; the model no longer advertises `supportsReasoningEffort`)
- [ ] `cargo check` green on both `tama` targets; fmt clean

---

### Task 6: API docs

**Context:**
`docs/api/models.md` documents the create/update payload table and response shapes; it is the reference for API consumers (and for agents making curl calls per AGENTS.md). The new field must be documented there so the contract is discoverable.

**Files:**
- Modify: `docs/api/models.md`

**What to implement:**
- In the create/update payload table (next to the `modalities` row, ~line 149): `reasoningLevels` — `string[] | null` — "Reasoning effort levels the model accepts. Valid values: `off, minimal, low, medium, high, xhigh, max`. Stored as given (trim/lowercase/dedupe applied). When non-empty, the model advertises `supportsReasoningEffort: true` on client model-info endpoints."
- In the model detail response section: add `reasoningLevels` (array or null).
- Note the client-facing derivation in one sentence (detail = raw only; `/v1/opencode/models` + `/v1/models` additionally emit derived `supportsReasoningEffort` and opencode-canonical `reasoning_options` with `off`→`none`).

**Steps:**
- [ ] Make the doc edits
- [ ] Re-read the changed sections for accuracy against the implemented field names
- [ ] Commit with message: "docs: document reasoningLevels in models API"

**Acceptance criteria:**
- [ ] `docs/api/models.md` payload table and response section include `reasoningLevels` with the valid-value set and the derived-field note

---

### Task 7: pi-provider-tama plugin — map the fields into pi Models (SEPARATE REPO)

**Context:**
This task lives in the **separate repo** `/home/daniel/Coding/Javascript/pi-provider-tama` (its own git repo, branch, and PR — do NOT commit these changes in the tama repo). The plugin fetches `GET /v1/opencode/models` and converts each entry into a pi `Model`. Today it hardcodes `reasoning: false` (`src/tama-api.ts:162`) and `supportsReasoningEffort: false` (`src/tama-api.ts:21-24` DEFAULT_COMPAT), so pi never offers thinking for tama models. With the new fields (Tasks 1–3) the mapping becomes data-driven. Critical pi semantics (verified against installed pi-ai 0.81.0): for a `reasoning: true` model, a `thinkingLevelMap` key that is ABSENT means "supported via provider default" — levels must be hidden with explicit `null`; `xhigh`/`max` only appear when explicitly mapped; map VALUES are the wire strings sent as `reasoning_effort` (this is where the `off` → `"none"` translation lives, ADR-0009). `thinkingFormat` stays at pi's default (`"openai"` → top-level `reasoning_effort`); the `qwen`/`qwen-chat-template` formats are boolean-only and for direct-to-backend use — NOT for the pi→tama hop.

**Files (in `/home/daniel/Coding/Javascript/pi-provider-tama`):**
- Modify: `src/types.ts`
- Modify: `src/tama-api.ts`
- Modify: `test/tama-api.test.ts`
- Modify: `README.md`
- Modify: `package.json` (version bump)

**What to implement:**

1. `src/types.ts`:
   - `TamaModel` (lines ~2-18): add `supportsReasoningEffort?: boolean` and `reasoningLevels?: string[]`.
   - Pi types: **import** `ThinkingLevelMap` (and `ModelThinkingLevel` if you need it for `PI_THINKING_LEVELS` typing) from `@earendil-works/pi-ai` — verified these are genuinely exported by the installed version. Fall back to local declarations matching the pi-ai source only if the named exports are unavailable:
     ```ts
     export type ModelThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
     export type ThinkingLevelMap = Partial<Record<ModelThinkingLevel, string | null>>
     ```
   - `PiModel` (lines ~35-52): add `thinkingLevelMap?: ThinkingLevelMap`.
   - `PiCompat` (lines ~27-33): no change needed (`supportsReasoningEffort` already exists).
2. `src/tama-api.ts`:
   ```ts
   const PI_THINKING_LEVELS: ModelThinkingLevel[] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
   const PI_KNOWN_LEVEL_SET = new Set<string>(PI_THINKING_LEVELS)

   /// Build pi's thinkingLevelMap from tama's reasoningLevels.
   /// - each pi level in the list → itself, EXCEPT "off" → "none"
   ///   (ADR-0009: no backend accepts "off" as reasoning_effort)
   /// - each pi level NOT in the list → null (explicit hole — absent
   ///   keys would mean "supported" in pi)
   /// - levels outside pi's vocabulary are dropped (tama validates them
   ///   server-side; this is defensive)
   /// - absent/empty input → undefined (pi defaults apply)
   function buildThinkingLevelMap(levels?: string[]): ThinkingLevelMap | undefined { ... }
   ```
3. `transformModel` (`src/tama-api.ts:141-177`):
   - `reasoning: model.supportsReasoningEffort ?? false` (replaces hardcoded `false` at line 162)
   - compat merge: keep `{ ...DEFAULT_COMPAT, ...backendCompat }`, and when `reasoning` is true also set `supportsReasoningEffort: true` (spread order so the per-model value wins; do NOT clobber `BACKEND_COMPAT`'s `maxTokensField`/`requiresToolResultName` — e.g. `compat: { ...DEFAULT_COMPAT, ...backendCompat, ...(reasoning ? { supportsReasoningEffort: true } : {}) }`)
   - `...(thinkingLevelMap ? { thinkingLevelMap } : {})` in the returned object
   - Do NOT set `thinkingFormat` (pi's default `"openai"` is correct)
4. `test/tama-api.test.ts`:
   - Update the pinned assertions that currently assert `reasoning: false` / `supportsReasoningEffort: false` (lines ~71, 78, 91, 102, 111, 170-172) — for models WITHOUT levels the expectations stay the same (false), so most may only need the explicit-key assertions tightened; fix any that break. Also rename the now-misleading test `it('always sets reasoning to false', ...)` (~line 170) to something accurate (e.g. `"defaults reasoning to false when supportsReasoningEffort is absent"`), keeping its assertion.
   - New case — qwen-like model `{ supportsReasoningEffort: true, reasoningLevels: ["off","low","medium","xhigh"] }` → `reasoning: true`, `compat.supportsReasoningEffort: true`, `thinkingLevelMap` deep-equals `{ off: "none", minimal: null, low: "low", medium: "medium", high: null, xhigh: "xhigh", max: null }`
   - New case — no levels → `reasoning: false`, `thinkingLevelMap` key absent
   - New case — unknown level `["off", "ultra"]` → map = `{ off: "none", minimal: null, low: null, medium: null, high: null, xhigh: null, max: null }` (ultra dropped)
5. `package.json`: bump `version` `0.13.0` → `0.14.0` (new capability). `README.md`: update the sections documenting the hardcoded `reasoning: false` (~line 164) and compat merging (~lines 119-121, 166) to describe the data-driven mapping.

**Do NOT:** touch pi packages in node_modules; set `thinkingFormat`; change the fetch/timeout logic; commit anything in the tama repo.

**Steps:**
- [ ] In `/home/daniel/Coding/Javascript/pi-provider-tama`: write the new/updated tests first (item 4)
- [ ] Run `npm run test:run`
  - Did the new cases fail (old mapping still hardcoded)? If they passed unexpectedly, stop and investigate why.
- [ ] Implement items 1–3, 5
- [ ] Run `npm run test:run` — did all tests pass?
- [ ] Run `npm run typecheck` and `npm run lint` — did both succeed?
- [ ] Commit (in the pi-provider-tama repo) with message: "feat: map tama reasoning levels to pi thinkingLevelMap"

**Acceptance criteria:**
- [ ] A tama model with levels `off, low, medium, xhigh` yields a pi Model with `reasoning: true`, `compat.supportsReasoningEffort: true`, and the exact 7-key thinkingLevelMap above (so pi's selector shows off/low/medium/xhigh and sends `reasoning_effort: none|low|medium|xhigh`)
- [ ] Models without levels behave exactly as before
- [ ] typecheck, lint, and vitest all green in the plugin repo

---

## Final verification (after Task 7, before PR)

- [ ] In `tama` repo, full gate (matches CI exactly):
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
  - `cargo nextest run --workspace`
- [ ] In `pi-provider-tama` repo: `npm run typecheck && npm run lint && npm run test:run`
- [ ] End-to-end smoke (dev mode): seed the Qwen3.8 model row with `reasoningLevels: ["off","low","medium","xhigh"]` (via editor or `curl -X PUT -H "Authorization: Bearer $TAMA_TOKEN" -d '{"reasoningLevels":["off","low","medium","xhigh"]}' "$TAMA_URL/tama/v1/models/<id>"`), then verify:
  - `curl -s -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/v1/opencode/models" | jq '.models[] | select(.reasoningLevels != null)'` shows `supportsReasoningEffort: true`, the levels array, and `reasoning_options` with `none`
  - `curl -s -H "Authorization: Bearer $TAMA_TOKEN" "$TAMA_URL/v1/models" | jq` shows the same keys on the model and its aliases
  - With pi + the updated plugin: the model's thinking selector shows off/low/medium/xhigh; picking a level sends `reasoning_effort` in the chat body (visible in langfuse logs or `tama` server logs)
- [ ] Open PRs: tama repo (Tasks 1–6) and pi-provider-tama repo (Task 7), linked to each other
