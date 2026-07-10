# PATCH Endpoints Plan

**Goal:** Add PATCH endpoints for models, config, and backends — truly surgical partial updates where only provided fields change.

**Architecture:** Three PATCH endpoints share a common pattern: a `*PatchBody` struct where every field is `Option<T>` with `#[serde(default)]`. A merge function walks the body and existing DB value, applying `body.field.or(base.field)` for `Option<T>` fields and `body.field.unwrap_or(base.field)` for bare `T` fields. `None` = preserve, `Some(value)` = replace. PUT endpoints remain unchanged (backward compatible).

**Tech Stack:** Rust, axum, SQLite, existing `tama` crate

---

### Task 1: PATCH /tama/v1/models/:id

**Context:**
The current PUT `/tama/v1/models/:id` acts as a partial update for most fields (post-plan-154 `.or(base.field)` merge), but `backend` is required, `args` defaults to `vec![]` (wipe on omit), and `sampling` uses direct assignment. PATCH fixes all three — `backend` becomes optional, `args` is `Option` (None = preserve, Some([]) = clear), and `sampling` gets `.or(base.sampling)`. This is the most complex PATCH because it must carry forward the non-trivial special-case logic from `apply_model_body` (quants size_bytes preservation, cache_type trim+filter, spec_decoding extraction).

**Files:**
- Modify: `crates/tama/src/api/models/crud/mod.rs` (add `ModelPatchBody`, `validate_model_patch`, `apply_model_patch`)
- Modify: `crates/tama/src/api/models/crud/update.rs` (add `patch_model` handler)
- Modify: `crates/tama/src/api/models/crud/tests.rs` (13+ new tests)
- Modify: `crates/tama/src/router.rs` (add `.patch()` to models/:id route)
- Modify: `crates/tama/src/api/models/mod.rs` (re-export `patch_model`)

**What to implement:**

In `crates/tama/src/api/models/crud/mod.rs`, add:

1. **`ModelPatchBody` struct** — mirror of `ModelBody` but with every field `Option<T>` and `#[serde(default)]`:
```rust
#[derive(serde::Deserialize)]
#[serde(default)]
pub struct ModelPatchBody {
    pub backend: Option<String>,         // was required String
    pub gpu_variant: Option<String>,
    pub gpu_device: Option<String>,
    pub model: Option<String>,
    pub quant: Option<String>,
    pub mmproj: Option<String>,
    pub mtp_model: Option<String>,
    pub args: Option<Vec<String>>,       // was Vec<String> (non-Option)
    pub sampling: Option<tama_core::profiles::SamplingParams>,
    pub enabled: Option<bool>,
    pub context_length: Option<u32>,
    pub num_parallel: Option<u32>,
    pub port: Option<u16>,
    pub api_name: Option<String>,
    pub display_name: Option<String>,
    pub gpu_layers: Option<u32>,
    pub quants: Option<std::collections::BTreeMap<String, tama_core::config::QuantEntry>>,
    pub modalities: Option<tama_core::config::ModelModalities>,
    pub kv_unified: Option<bool>,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
    pub spec_decoding: Option<tama_core::config::SpecDecodingConfig>,
}
```

2. **`validate_model_patch(body: &ModelPatchBody) -> Result<(), String>`** — validates only `Some` fields:
   - `backend.as_ref().map(|b| !b.is_empty())` — if Some and empty, return error
   - `backend` length ≤ `MAX_BACKEND` when Some
   - `model` non-empty and ≤ `MAX_MODEL` when Some
   - `quant` ≤ `MAX_QUANT` when Some and non-empty
   - `mmproj` ≤ `MAX_MMPROJ` when Some and non-empty
   - `api_name` ≤ `MAX_API_NAME` when Some and non-empty
   - `display_name` ≤ `MAX_DISPLAY_NAME` when Some and non-empty
   - `cache_type_k` when Some: trim, reject `__custom`, ≤ `MAX_CACHE_TYPE`
   - `cache_type_v` when Some: trim, reject `__custom`, ≤ `MAX_CACHE_TYPE`
   - An all-`None` body is valid (no-op)

3. **`apply_model_patch(body: ModelPatchBody, existing: &tama_core::config::ModelConfig) -> tama_core::config::ModelConfig`** — merge function (takes reference to avoid cloning the entire existing config):
   - Extract `existing_spec_decoding` before consuming (same pattern as `apply_model_body`)
   - `backend`: `body.backend.unwrap_or(base.backend.clone())`
   - `gpu_variant`: `body.gpu_variant.or(base.gpu_variant)`
   - `gpu_device`: `body.gpu_device.or(base.gpu_device)`
   - `model`: `body.model.or(base.model)`
   - `quant`: `body.quant.or(base.quant)`
   - `mmproj`: `body.mmproj.or(base.mmproj)`
   - `mtp_model`: `body.mtp_model.or(base.mtp_model)`
   - `args`: `body.args.unwrap_or_else(|| base.args.clone())`
   - `sampling`: `body.sampling.or(base.sampling)`
   - `enabled`: `body.enabled.unwrap_or(base.enabled)`
   - `context_length`: `body.context_length.or(base.context_length)`
   - `num_parallel`: `body.num_parallel.or(base.num_parallel)`
   - `port`: `body.port.or(base.port)`
   - `health_check`: `base.health_check` (server-side, always preserved)
   - `profile`: `base.profile.clone()` (**intentional deviation from `apply_model_body`** — PATCH preserves, PUT sets `None`)
   - `api_name`: `body.api_name.or(base.api_name)`
   - `gpu_layers`: `body.gpu_layers.or(base.gpu_layers)`
   - `modalities`: `body.modalities.or(base.modalities)`
   - `display_name`: `body.display_name.or(base.display_name)`
   - `quants`: **exact same size_bytes preservation logic as `apply_model_body`** — copy the full `body.quants.unwrap_or_else(...)` with the inner `size_bytes` preservation map loop. Do NOT simplify to a plain `.or()`.
   - `kv_unified`: `body.kv_unified.unwrap_or(base.kv_unified)`
   - `cache_type_k`: **exact same trim+filter logic as `apply_model_body`** — `.map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != "__custom").or(base.cache_type_k)`
   - `cache_type_v`: same pattern as cache_type_k
   - `hf_format`, `hf_base_model`, `hf_pipeline_tag`, `hf_total_params`, `hf_active_params`, `hf_architecture_type`, `hf_context_length`, `hf_num_layers`, `hf_last_modified`: all `base.field` (server-side, always preserved)
   - `db_id`: `base.db_id` (server-side, always preserved)
   - `spec_decoding`: `body.spec_decoding.unwrap_or_else(|| existing_spec_decoding.clone())`

In `crates/tama/src/api/models/crud/update.rs`, add:

4. **`patch_model` handler** — same structure as `update_model` but:
   - Accepts `Json(body): Json<ModelPatchBody>`
   - Calls `validate_model_patch(&body)` instead of `validate_model_body`
   - Calls `apply_model_patch(body, &existing)` instead of `apply_model_body`
   - Same error handling, same spawn_model_crud pattern

In `crates/tama/src/api/models/crud/mod.rs`, re-export:
5. `pub use update::patch_model;`

(The `models/mod.rs` already has `pub use crud::*;` so `patch_model` is automatically re-exported — no change needed there.)

In `crates/tama/src/router.rs`:
6. Add `.patch(api::patch_model)` to the `/tama/v1/models/:id` route (after `.put(api::update_model)`).

In `crates/tama/src/api/models/crud/tests.rs`, add tests:

7. **Test: `test_apply_model_patch_all_none_preserves_all_fields`** — all-None body preserves every field including `profile`, `args`, `context_length`, `cache_type_k`, `cache_type_v`, `sampling`, `spec_decoding`, `quants` (with size_bytes)
8. **Test: `test_apply_model_patch_single_field_changes_only_that_field`** — body with only `context_length: Some(8192)` changes only that field
9. **Test: `test_apply_model_patch_args_some_empty_clears`** — `args: Some(vec![])` clears args to empty
10. **Test: `test_apply_model_patch_args_none_preserves`** — `args: None` preserves existing args
11. **Test: `test_apply_model_patch_backend_none_preserves`** — `backend: None` preserves existing backend
12. **Test: `test_apply_model_patch_backend_some_overrides`** — `backend: Some("new")` overrides
13. **Test: `test_apply_model_patch_quants_size_bytes_preserved`** — quants size_bytes preserved per-key
14. **Test: `test_apply_model_patch_cache_type_custom_filtered`** — `cache_type_k: Some("__custom")` filtered to None, falls back to base
15. **Test: `test_apply_model_patch_profile_preserved`** — `profile` preserved (PATCH deviation from PUT)
16. **Test: `test_apply_model_patch_sampling_preserved`** — `sampling: None` preserves existing sampling
17. **Test: `test_validate_model_patch_empty_backend_rejected`** — `backend: Some("")` returns error
18. **Test: `test_validate_model_patch_all_none_valid`** — all-None body passes validation

**Steps:**
- [ ] Write the 13+ failing tests in `crates/tama/src/api/models/crud/tests.rs`
- [ ] Run `cargo nextest run --package tama -- api::models::crud::tests::test_apply_model_patch` — tests should fail (function doesn't exist yet)
- [ ] Implement `ModelPatchBody` struct in `crates/tama/src/api/models/crud/mod.rs`
- [ ] Implement `validate_model_patch` in `crates/tama/src/api/models/crud/mod.rs`
- [ ] Implement `apply_model_patch` in `crates/tama/src/api/models/crud/mod.rs`
- [ ] Run `cargo nextest run --package tama -- api::models::crud::tests::test_apply_model_patch` — unit tests should pass
- [ ] Implement `patch_model` handler in `crates/tama/src/api/models/crud/update.rs`
- [ ] Add re-exports in `mod.rs` and `models/mod.rs`
- [ ] Add `.patch(api::patch_model)` to router
- [ ] Run `cargo nextest run --package tama -- api::models::crud::tests` — all tests (existing + new) must pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama -- -D warnings` — fix any warnings
- [ ] Commit with message: "feat: add PATCH /tama/v1/models/:id for surgical model updates"

**Acceptance criteria:**
- [ ] PATCH with all-None body preserves every field (no-op)
- [ ] PATCH with single field changes only that field
- [ ] `args: Some([])` clears args, `args: None` preserves
- [ ] `backend: None` preserves existing backend
- [ ] `quants` size_bytes preserved per-key (security)
- [ ] `cache_type_k` `__custom` sentinel filtered in merge
- [ ] `profile` preserved (PATCH deviation from PUT)
- [ ] `sampling` preserved when None
- [ ] All existing tests pass (no regressions)
- [ ] `cargo clippy --package tama -- -D warnings` passes clean

---

### Task 2: PATCH /tama/v1/config/structured

**Context:**
The current POST `/tama/v1/config/structured` does a full replace — sends the entire Config, everything gets overwritten. PATCH provides deep recursive field-level merge: each section is `Option<SectionPatch>`, each `*Patch` has all fields as `Option<T>`. The `backends` field is dropped from the patch because `Config.backends` is read-only (not persisted by `to_db`). The handler must call `sync_proxy_config()` after persisting for hot-reload (matching `save_structured_config`).

**Files:**
- Modify: `crates/tama/src/types/config/mod.rs` (add `ConfigPatchBody` and `*Patch` structs)
- Modify: `crates/tama/src/api.rs` (add `patch_structured_config` handler, `merge_config_patch`)
- Modify: `crates/tama/src/router.rs` (add `.patch()` to config/structured route)

**What to implement:**

In `crates/tama/src/types/config/mod.rs`, add the following `*Patch` structs. Each field is `Option<T>` matching the corresponding field in the non-patch struct. Read the actual field types from the existing structs (General, Supervisor, ProxyConfig, CompactionConfig, SamplingParams) and mirror them as `Option<T>`.

1. **`GeneralPatch`** — mirror of `General` with all fields `Option<T>`. Read the actual `General` struct in `types/config/general.rs` for exact types:
```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GeneralPatch {
    pub log_level: Option<tama_core::config::LogLevel>,
    pub models_dir: Option<String>,
    pub logs_dir: Option<String>,
    pub hf_token: Option<String>,
    pub update_check_interval: Option<u32>,     // actual type is u32
}
```

2. **`SupervisorPatch`** — mirror of `Supervisor` with all fields `Option<T>`. Read actual types from `types/config/supervisor.rs`:
```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SupervisorPatch {
    pub restart_policy: Option<tama_core::config::RestartPolicy>,
    pub max_restarts: Option<u32>,
    pub restart_delay_ms: Option<u64>,
    pub health_check_interval_ms: Option<u64>,
    pub health_check_timeout_ms: Option<u64>,
    pub health_check_retries: Option<u32>,
}
```

3. **`ProxyConfigPatch`** — mirror of `ProxyConfig` with all fields `Option<T>`, including nested `OAuth2ConfigPatch`. Read actual types from `types/config/proxy.rs`:
```rust
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProxyConfigPatch {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub auto_unload: Option<bool>,
    pub idle_timeout_secs: Option<u64>,
    pub startup_timeout_secs: Option<u64>,
    pub circuit_breaker_threshold: Option<u32>,
    pub circuit_breaker_cooldown_seconds: Option<u64>,
    pub metrics_retention_secs: Option<u64>,
    pub download_queue_poll_interval_secs: Option<u64>,
    pub max_loaded_models: Option<u32>,
    pub authenticator_url: Option<String>,
    pub authenticator_skip_paths: Option<Vec<String>>,
    pub oauth2: Option<OAuth2ConfigPatch>,
    pub api_keys_enabled: Option<bool>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct OAuth2ConfigPatch {
    pub enabled: Option<bool>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub logout_url: Option<String>,
    pub redirect_uri: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub session_ttl_secs: Option<u64>,
}
```

4. **`CompactionConfigPatch`** — mirror of `CompactionConfig` with all fields `Option<T>`. Read actual struct from `types/config/compaction.rs` for fields and types. Add `#[serde(default)]` on the struct.

5. **`ConfigPatchBody`** — top-level patch body:
```rust
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ConfigPatchBody {
    #[serde(default)]
    pub general: Option<GeneralPatch>,
    // backends intentionally omitted — Config.backends is read-only (not persisted by to_db)
    #[serde(default)]
    pub supervisor: Option<SupervisorPatch>,
    #[serde(default)]
    pub sampling_templates: Option<std::collections::BTreeMap<String, SamplingParams>>,
    // SamplingParams already has all Option fields — reuse directly
    #[serde(default)]
    pub proxy: Option<ProxyConfigPatch>,
    #[serde(default)]
    pub compaction: Option<CompactionConfigPatch>,
}
```

6. Add `pub use` for all new `*Patch` types in the module.

In `crates/tama/src/api.rs`, add:

7. **`merge_config_patch(existing: crate::types::config::Config, patch: ConfigPatchBody) -> crate::types::config::Config`** — deep recursive merge operating on **mirror Config types** (not core types):
   - For each section (`general`, `supervisor`, `proxy`, `compaction`): if `patch.section.is_some()`, merge field-by-field using `.or()` / `unwrap_or()`; if `None`, keep existing section entirely
   - For `sampling_templates`: **upsert** — iterate patch map, for each key: if key exists in existing, merge field-by-field (each SamplingParams field is already `Option<T>` so `.or()` works); if key doesn't exist, insert new entry. Keys absent from patch are preserved.
   - For nested `oauth2` within `proxy`: same deep merge pattern — if `patch.oauth2.is_some()`, merge OAuth2 fields field-by-field; if `None`, keep existing oauth2

8. **`patch_structured_config` handler** — same structure as `save_structured_config` but:
   - Accepts `Json(body): Json<ConfigPatchBody>`
   - Loads existing config from DB (reuse `load_config_from_state`) — returns `tama_core::config::Config`
   - Converts core Config to mirror: `let existing_mirror: crate::types::config::Config = existing_core.into();`
   - Calls `merge_config_patch(existing_mirror, body)` to get merged mirror Config
   - Converts merged mirror Config back to core: `let merged_core: tama_core::config::Config = merged_mirror.into();`
   - Calls `to_db()` to persist (spawn_blocking)
   - Calls `sync_proxy_config(&state, merged_core)` for hot-reload
   - Returns `{"ok": true}`

In `crates/tama/src/router.rs`:
9. Add `.patch(api::patch_structured_config)` to the `/tama/v1/config/structured` route — chain it **before** `.layer(json_body_limit)` so the body limit applies to PATCH too:
```rust
.post(api::save_structured_config)
.patch(api::patch_structured_config)
.layer(json_body_limit),
```

**Steps:**
- [ ] Add `*Patch` structs in `crates/tama/src/types/config/mod.rs` (read actual types from existing structs in their submodules)
- [ ] Run `cargo build --package tama` — verify structs compile
- [ ] Write failing tests for `merge_config_patch` in `crates/tama/src/api.rs` (add a `#[cfg(test)] mod tests` section):
  - `test_merge_config_patch_all_none_preserves_all` — all-None body preserves entire config
  - `test_merge_config_patch_proxy_port_only` — patches `proxy.port`, preserves `proxy.oauth2.*`
  - `test_merge_config_patch_oauth2_client_id_deep_set` — patches `proxy.oauth2.client_id` only
  - `test_merge_config_patch_sampling_templates_upsert` — new key inserted, existing key merged
- [ ] Implement `merge_config_patch` in `crates/tama/src/api.rs`
- [ ] Run the merge tests — verify they pass
- [ ] Implement `patch_structured_config` handler in `crates/tama/src/api.rs`
- [ ] Add `.patch(api::patch_structured_config)` to router (before `.layer(json_body_limit)`)
- [ ] Run `cargo build --package tama` — verify build
- [ ] Run `cargo nextest run --package tama` — ensure no regressions
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama -- -D warnings` — fix any warnings
- [ ] Commit with message: "feat: add PATCH /tama/v1/config/structured with deep recursive merge"

**Acceptance criteria:**
- [ ] PATCH with all-None body preserves entire config (no-op)
- [ ] PATCH `proxy.port` only changes port, preserves all other proxy fields including oauth2.*
- [ ] PATCH `oauth2.client_id` deep-sets only that field
- [ ] PATCH `sampling_templates` with new key inserts it
- [ ] PATCH `sampling_templates` with existing key merges fields
- [ ] `sync_proxy_config` called after persist (hot-reload works)
- [ ] All existing tests pass (no regressions)
- [ ] `cargo clippy --package tama -- -D warnings` passes clean

---

### Task 3: PATCH /tama/v1/backends/:name

**Context:**
The current backend config updates are split across three POST endpoints (`default-args`, `default-env`, `source`), each surgical for one field. PATCH consolidates `default_args`, `default_env`, and `health_check_url` into one endpoint. `build_from_source` is excluded (managed via `POST /source` on a different table). The handler must preserve all three `save_config` args from existing when not patched (fixes the latent `health_check_url` clobber bug in existing handlers).

**Files:**
- Modify: `crates/tama/src/api/backends/manage/types.rs` (add `BackendPatchBody`)
- Modify: `crates/tama/src/api/backends/manage/config.rs` (add `patch_backend` handler)
- Modify: `crates/tama/src/api/backends/manage/mod.rs` (re-export)
- Modify: `crates/tama/src/router.rs` (add `.patch()` to backends/:name route)

**What to implement:**

In `crates/tama/src/api/backends/manage/types.rs`, add:

1. **`BackendPatchBody`**:
```rust
#[derive(serde::Deserialize)]
pub struct BackendPatchBody {
    pub default_args: Option<Vec<String>>,
    pub default_env: Option<Vec<String>>,            // Vec of "KEY=VALUE" strings
    pub health_check_url: Option<String>,           // None=preserve, Some(value)=set (clear via existing POST endpoints)
}
```

In `crates/tama/src/api/backends/manage/config.rs`, add:

2. **`patch_backend` handler** — follows the same pattern as `update_backend_default_args`:
   - Accepts `Path(backend_name): Path<String>`, `Query(query): Query<DefaultArgsQuery>` (reuse existing query param struct for `gpu_variant`), `Json(body): Json<BackendPatchBody>`
   - **Path-traversal validation** on `backend_name` (reject `/`, `\`, `..`) — same as existing handlers
   - Open `BackendManager` in spawn_blocking
   - Load existing values: `mgr.get_default_args()`, `mgr.get_default_env()`, (check if there's a `get_health_check_url` or equivalent — if not, read from the record directly)
   - Merge: `default_args = body.default_args.unwrap_or(existing_args)`, `default_env = body.default_env.unwrap_or(existing_env)`, `health_check_url = body.health_check_url.as_deref().or(existing_health_check_url.as_deref())`
   - Call `mgr.save_config(&backend_name, &gpu_variant, &default_args, &default_env, health_check_url)`
   - Return `{"success": true}`

In `crates/tama/src/api/backends/manage/mod.rs`:
3. Add `pub use config::patch_backend;` (or wherever it's defined).

In `crates/tama/src/router.rs`:
4. Add `.patch(patch_backend)` to the `/tama/v1/backends/:name` route (in `backend_routes`).

**Steps:**
- [ ] Write failing tests for `patch_backend` in `crates/tama/src/api/backends/manage/tests.rs`:
  - `test_patch_backend_path_traversal_rejected` — backend name with `/` returns 400
  - `test_patch_backend_all_none_preserves` — all-None body preserves all fields (no-op)
  - `test_patch_backend_default_args_only` — changes only args, preserves env
- [ ] Add `BackendPatchBody` in `types.rs`
- [ ] Implement `patch_backend` handler in `config.rs`
- [ ] Add re-export in `mod.rs`
- [ ] Add `patch_backend` to `use crate::api::backends::{...}` import in `router.rs`
- [ ] Add `.patch(patch_backend)` to the `/tama/v1/backends/:name` route in `backend_routes`
- [ ] Run `cargo nextest run --package tama -- api::backends::manage::tests` — all tests pass
- [ ] Run `cargo nextest run --package tama` — ensure no regressions
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama -- -D warnings` — fix any warnings
- [ ] Commit with message: "feat: add PATCH /tama/v1/backends/:name for consolidated backend config updates"

**Acceptance criteria:**
- [ ] PATCH with all-None body preserves all backend config fields (no-op)
- [ ] PATCH `default_args` changes only args, preserves env and health_check_url
- [ ] PATCH `health_check_url: Some("url")` sets health_check_url
- [ ] PATCH `health_check_url: None` preserves existing health_check_url
- [ ] Path-traversal validation rejects `/`, `\`, `..` in backend name
- [ ] All existing tests pass (no regressions)
- [ ] `cargo clippy --package tama -- -D warnings` passes clean

---

### Task 4: CORS, OpenAPI, and API Docs

**Context:**
PATCH is a new HTTP method that needs to be allowed by CORS (or browser preflight fails) and documented in the OpenAPI spec and API docs. This task is independent of the three PATCH endpoints (can be done in parallel or after).

**Files:**
- Modify: `crates/tama/src/router.rs` (CORS allow_methods)
- Modify: `crates/tama/src/api/openapi.rs` (add `patch_op_p` helper, register PATCH paths)
- Modify: `docs/api/models.md` (add PATCH /tama/v1/models/:id section)
- Modify: `docs/api/config.md` (add PATCH /tama/v1/config/structured section)
- Modify: `docs/api/backends.md` (add PATCH /tama/v1/backends/:name section)

**What to implement:**

In `crates/tama/src/router.rs`:

1. **CORS allow_methods** — add `axum::http::Method::PATCH` to both CorsLayer lists:
   - `csrf_routes` CorsLayer: add `PATCH` to existing `[GET, POST, PUT, DELETE]`
   - `backend_routes` CorsLayer: add `PATCH` to existing `[GET, POST, DELETE]`

In `crates/tama/src/api/openapi.rs`:

2. **`patch_op_p` helper** — mirror the **exact signature and return type** of the existing `put_op_p` function (read its signature from the file). The only difference is emitting `"patch"` as the method string instead of `"put"`. Read `put_op_p` to get the exact parameter list and return type (`serde_json::Value`).

3. **Register three PATCH paths** — add entries for:
   - `/tama/v1/models/{id}` — PATCH, summary "Update a model (partial/surgical)"
   - `/tama/v1/config/structured` — PATCH, summary "Update config (deep recursive merge)"
   - `/tama/v1/backends/{name}` — PATCH, summary "Update backend config (partial)"
   - **Note:** The existing OpenAPI `paths` HashMap has a pre-existing bug where duplicate keys are silently overwritten (multiple methods on the same path). This is a known issue — the PATCH entries will be added but may not all render in `/tama/v1/docs`. Fixing the HashMap merge is out of scope for this plan (separate fix needed).

In `docs/api/models.md`:

4. **Add PATCH /tama/v1/models/:id section** — document:
   - Description: "Update an existing model. Surgical partial update — only provided fields change, all others preserved."
   - Request body: `ModelPatchBody` — all fields optional, `backend` optional (was required for PUT), `args` is `Option` (None=preserve, Some([])=clear)
   - Response: `{"ok": true, "id": N}`
   - Errors: 404, 422

In `docs/api/config.md`:

5. **Add PATCH /tama/v1/config/structured section** — document:
   - Description: "Update config with deep recursive field-level merge. Only provided fields change."
   - Request body: `ConfigPatchBody` — each section is `Option<SectionPatch>`, each `*Patch` has all fields as `Option<T>`. `backends` section omitted (read-only).
   - Response: `{"ok": true}`
   - Errors: 422

In `docs/api/backends.md`:

6. **Add PATCH /tama/v1/backends/:name section** — document:
   - Description: "Update backend config fields (default_args, default_env, health_check_url) with partial merge."
   - Request body: `BackendPatchBody` — all fields `Option`. `health_check_url` is `Option<String>` (None=preserve, Some(value)=set).
   - Query params: `gpu_variant`
   - Response: `{"success": true}`
   - Errors: 404, 422

**Steps:**
- [ ] Add `PATCH` to CORS allow_methods in `router.rs` (both `csrf_routes` and `backend_routes` CorsLayer) — note: CSRF middleware already handles PATCH, no changes needed there
- [ ] Add `patch_op_p` helper to `openapi.rs` (mirror exact signature of `put_op_p`, read the function first)
- [ ] Register three PATCH paths in `openapi.rs`
- [ ] Update `docs/api/models.md` with PATCH section
- [ ] Update `docs/api/config.md` with PATCH section (document body as flat ConfigPatchBody, not wrapped in a `config` key)
- [ ] Update `docs/api/backends.md` with PATCH section
- [ ] Run `cargo build --package tama` — verify build
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama -- -D warnings` — fix any warnings
- [ ] Commit with message: "docs: add PATCH CORS, OpenAPI, and API documentation"

**Acceptance criteria:**
- [ ] PATCH requests pass CORS preflight (browser)
- [ ] `/tama/v1/docs` shows PATCH endpoints
- [ ] API docs describe all three PATCH endpoints with body schemas
- [ ] `cargo clippy --package tama -- -D warnings` passes clean

---

## Out of Scope

- Making PUT strict (full replace) — that's a future breaking change after clients migrate to PATCH
- `build_from_source` in backend PATCH — managed via existing `POST /backends/:name/source` (different table)
- PATCH for other resources (aliases, benchmarks, etc.) — can be added later using the same pattern
- Key deletion in config maps — use DELETE endpoints
