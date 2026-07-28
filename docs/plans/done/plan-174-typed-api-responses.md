# Typed API Responses Plan

**Goal:** Give the three highest-traffic untyped wire formats real Rust types — the `/status` payload, the opencode `ModelEntry`, and the repeated `json!({"ok": true})` success bodies — incrementally, with golden tests that pin the exact JSON shape before and after (audit F19).

**Architecture:** The `/status` endpoint already has a partial type: `StatusResponse` in `crates/tama-core/src/proxy/handlers/status.rs:20`, but `handle_status` gets it via a lossy round-trip (`build_status_response() -> serde_json::Value` then `serde_json::from_value`) and its `models` map is still `BTreeMap<String, serde_json::Value>`. This plan types the per-model entry (`StatusModelEntry`), makes `build_status_response` return the struct directly, types `build_model_entry`'s output as `ModelEntry`, introduces `BackendModelEntry` (with `#[serde(flatten)]` passthrough so no backend field is lost) for `find_model_in_entries`, and defines shared `OkResponse`/`ModelMutationResponse` in tama-core for the 10 plain success bodies. **The live wire shape is the contract**: `StatusResponse` currently skips `gpu_utilization_pct`/`vram` when `None` (that is what clients actually receive today — the intermediate `Value` emitting `"vram": null` never reaches the wire), so the typed version must skip them too. Explicitly OUT OF SCOPE: typing all 145 `json!` sites, rich one-off payloads (`crud/delete.rs:120`, `models/files.rs:169`), codegen from OpenAPI, utoipa adoption, and the `state: "loading"` literal alignment with `ModelState::Starting` (plan-173 sequel).

**Tech Stack:** Rust, Axum, serde/serde_json, tokio, tower (tests)

---

### Task 1: Type the `/status` response (`StatusModelEntry` + direct `StatusResponse`)

**Context:**
`build_status_response` (`crates/tama-core/src/proxy/status.rs:94`) hand-assembles a `serde_json::Map` from 4 state domains; `handle_status` then re-parses it into `StatusResponse`. Two name collisions to navigate: a DTO named `ProxyMetrics` already exists in `handlers/status.rs:43`, distinct from the atomic runtime counters `proxy::types::ProxyMetrics` (`proxy/types.rs:197`) — the DTO moves and is renamed `ProxyMetricsSnapshot` (internal name only; JSON unchanged). The per-model `state` field uses string literals (`"ready"`, `"loading"`, `"unloading"`, `"failed"`, `"idle"`) — these become a `StatusModelState` enum with lowercase serde so the values are locked at compile time (the `"loading"`→`"starting"` alignment is deliberately NOT done here). The golden test compares parsed `serde_json::Value`s (order-insensitive) with volatile time fields masked, capturing the shape the endpoint emits TODAY.

**Files:**
- Modify: `crates/tama-core/src/proxy/status.rs`
- Modify: `crates/tama-core/src/proxy/handlers/status.rs`
- Modify: `crates/tama-core/src/proxy/mod.rs` (test updates)

**What to implement:**

1. **Move + extend the types into `proxy/status.rs`** (top-level, above the `impl ProxyState` block):
   ```rust
   use serde::{Deserialize, Serialize};

   /// Lifecycle state label for one model in the `/status` payload.
   /// NOTE: values are the CURRENT wire literals; aligning `Loading` with
   /// `ModelState::Starting` (CONTEXT.md canon) is a follow-up to plan-173.
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(rename_all = "lowercase")]
   pub enum StatusModelState {
       Ready,
       Loading,
       Unloading,
       Failed,
       Idle,
   }

   /// Per-model entry in the `/status` response `models` map.
   /// Every key is always present on the wire; absent values serialize as null.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct StatusModelEntry {
       pub id: Option<i64>,
       pub display_name: Option<String>,
       pub backend: String,
       pub backend_path: Option<String>,
       pub model: Option<String>,
       pub quant: Option<String>,
       pub context_length: Option<u32>,
       pub enabled: bool,
       pub api_name: Option<String>,
       pub state: StatusModelState,
       pub backend_pid: Option<u32>,
       pub load_time_secs: Option<u64>,
       pub last_accessed_secs_ago: Option<u64>,
       pub idle_timeout_remaining_secs: Option<u64>,
       pub consecutive_failures: Option<u32>,
   }

   /// VRAM sub-object of the status response.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct VramStatus {
       pub used_mib: u64,
       pub total_mib: u64,
   }

   /// Serializable snapshot of the proxy's atomic request counters
   /// (`proxy::types::ProxyMetrics` is the live atomic form — different type).
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct ProxyMetricsSnapshot {
       pub total_requests: u64,
       pub successful_requests: u64,
       pub failed_requests: u64,
       pub models_loaded: u64,
       pub models_unloaded: u64,
   }

   /// Typed response for the `/status` endpoint.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct StatusResponse {
       pub cpu_usage_pct: f32,
       pub ram_used_mib: u64,
       pub ram_total_mib: u64,
       #[serde(skip_serializing_if = "Option::is_none")]
       pub gpu_utilization_pct: Option<u8>,
       #[serde(skip_serializing_if = "Option::is_none")]
       pub vram: Option<VramStatus>,
       pub auto_unload: bool,
       pub idle_timeout_secs: u64,
       pub metrics: ProxyMetricsSnapshot,
       pub models: std::collections::BTreeMap<String, StatusModelEntry>,
   }
   ```

2. **Rewrite `build_status_response`** to return `StatusResponse` (signature: `pub async fn build_status_response(&self) -> StatusResponse`). Logic is a mechanical translation of the existing code:
   - The five match arms construct `StatusModelEntry` directly. Ready arm: `state: StatusModelState::Ready`, `backend_pid: Some(*backend_pid)`, `load_time_secs: Some(load_secs)`, `last_accessed_secs_ago: Some(secs_ago)`, `idle_timeout_remaining_secs: remaining` (the existing `Option<u64>` — `None` when auto_unload is off, previously `Value::Null`), `consecutive_failures: Some(consecutive_failures.load(Relaxed))`. Starting arm: `state: StatusModelState::Loading`, `consecutive_failures: Some(...)`, all other runtime fields `None`. Unloading/Failed arms: respective state, everything else `None`. `_` (idle) arm: `state: StatusModelState::Idle`, `consecutive_failures: None`.
   - `vram: sys_metrics.vram.map(|v| VramStatus { used_mib: v.used_mib, total_mib: v.total_mib })` (check `VramInfo` is `Copy`; if not, `.as_ref()`/clone as needed).
   - `metrics: ProxyMetricsSnapshot { total_requests: metrics.total_requests.load(Relaxed), … }`.
   - Insert into a `BTreeMap<String, StatusModelEntry>` (the current `serde_json::Map` is also sorted — `preserve_order` is not enabled — so ordering is unchanged).

3. **`handlers/status.rs`:** delete the local `StatusResponse`/`VramStatus`/`ProxyMetrics` structs; `use crate::proxy::status::StatusResponse;` — simplify `handle_status` to:
   ```rust
   #[axum::debug_handler]
   pub async fn handle_status(state: State<Arc<ProxyState>>) -> Json<StatusResponse> {
       Json(state.build_status_response().await)
   }
   ```
   (infallible now — drop the `Result`/`from_value` error arm). Check `proxy/status.rs` module visibility: `status` is declared `mod status;` or `pub mod status;` in `proxy/mod.rs` — widen to `pub mod status;` if needed for the import.

4. **Update the 3 existing tests in `proxy/mod.rs`** (`test_build_status_response` :64, `test_build_status_response_model_fields` :97, `test_build_status_response_backend_path_null` :359) to the typed API: e.g. `let response = state.build_status_response().await;` then `assert!(!response.auto_unload); assert!(response.models.is_empty());` etc. For `test_build_status_response` — the old assertion `"vram key should be present (even if null)"` tested the INTERMEDIATE shape; the live endpoint shape omits it. Replace with a serialization assertion: `let v = serde_json::to_value(&response).unwrap(); assert!(v.get("vram").is_none() || !v["vram"].is_null());` plus a comment: `// live wire shape: vram/gpu_utilization_pct are OMITTED when None`.

5. **Golden test** (new, in `proxy/mod.rs` tests): `test_build_status_response_golden_shape`:
   - Fixture: `Config::default()` + `config.backends.insert("llama_cpp", BackendConfig { path: Some("/opt/llama/llama-server".into()), version: None, gpu_variant: None })` (match `BackendConfig` field types at `config/types/backend.rs`); two model configs (`"idle-model"` and `"ready-model"`, backend `llama_cpp`, `display_name: Some("Ready Model")`, `model: Some("test/model")`, `db_id: Some(7)` on the ready one); one `BackendState::Ready` runtime entry for `"ready-model"` with `load_time: std::time::UNIX_EPOCH` (deterministic `load_time_secs: 0`), `backend_pid: 4242`.
   - Set `state.system_metrics.write().await.vram = Some(VramInfo { used_mib: 100, total_mib: 200, .. })` (read `VramInfo`'s full field list at `gpu/vram.rs` first; if it has more fields, fill them) and `cpu_usage_pct = 12.5`.
   - `let v = serde_json::to_value(state.build_status_response().await).unwrap();`
   - Mask volatility: `v["models"]["ready-model"]["last_accessed_secs_ago"] = json!(0);` (also `idle_timeout_remaining_secs` — with default config `auto_unload: false` it is already null).
   - Assert `v == serde_json::json!({ ... })` with the full expected literal: top-level keys exactly `auto_unload, cpu_usage_pct, gpu_utilization_pct?, idle_timeout_secs, metrics, models, ram_total_mib, ram_used_mib, vram` (gpu key present only because the fixture sets it — default `SystemMetrics` may have `gpu_utilization_pct: Some(0)`; READ `gpu::SystemMetrics::default()` first and set both GPU fields to known values so the literal is stable), each model entry with exactly the 15 keys from `StatusModelEntry`, `"ready-model"` entry `{"state": "ready", "backend_pid": 4242, "load_time_secs": 0, …}` and `"idle-model"` entry with all-null runtime fields and `"state": "idle"`, metrics counters all `0`.
   - Second assertion block: same fixture but `vram = None` and `gpu_utilization_pct = None` → serialized value has NO `vram` and NO `gpu_utilization_pct` keys.

**Steps:**
- [ ] Write the golden test against the CURRENT implementation (it passes by construction — this is the contract capture); run `cargo nextest run --package tama-core -- proxy:: -- test_build_status_response_golden_shape` to confirm
- [ ] Implement items 1–3 (types move, `build_status_response` rewrite, `handle_status` simplification)
- [ ] Update the 3 existing tests per item 4
- [ ] Run `cargo nextest run --package tama-core -- proxy` — all pass including the unchanged golden
- [ ] Run `rg "StatusResponse|build_status_response" crates/ -g "*.rs"` — no stale imports; `cargo nextest run --package tama-core` — full pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: type the /status response (StatusModelEntry + StatusResponse)"

**Acceptance criteria:**
- [ ] `build_status_response` returns `StatusResponse`; no `serde_json::Map` assembly remains in `proxy/status.rs`
- [ ] Golden test proves the serialized shape is key-for-key identical to the pre-refactor endpoint output (including key omission when `vram`/`gpu_utilization_pct` are `None`)
- [ ] Only one `ProxyMetrics`-family DTO exists (`ProxyMetricsSnapshot` in `proxy/status.rs`); the atomic `proxy::types::ProxyMetrics` is untouched
- [ ] `cargo nextest run --package tama-core` passes; clippy clean

---

### Task 2: Type the opencode `ModelEntry` and backend-model matching (`BackendModelEntry`)

**Context:**
`build_model_entry` (`proxy/tama_handlers/models/utils.rs:63`) builds the opencode discovery entry as a `serde_json::Value` with string-key mutations in `opencode.rs` (:64 reads `entry["id"]`, :88-94 writes `entry["id"]`/`entry["name"]`), and `find_model_in_entries` (`proxy/handlers/models.rs:21`) string-matches entries from a backend's `/v1/models` by `id`/`aliases`. These are two DIFFERENT shapes — do not unify them. `ModelEntry` types the opencode shape (the `modalities` key is emitted only when present — model it with `skip_serializing_if`); `BackendModelEntry` types the llama-server shape with `#[serde(flatten)] extra` so unknown backend fields keep passing through to clients byte-identically (dropping them would be a behavior change). Existing behavior quirks to preserve: entries missing `id` are pushed without an `id` key (never as `"id": null`), `ready` is injected only on loaded entries, and `temperature` is always literal `true` in opencode entries.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/opencode.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/mod.rs`
- Modify: `crates/tama-core/src/proxy/handlers/models.rs`
- Modify: `crates/tama-core/src/proxy/handlers/list_models_tests.rs`

**What to implement:**

1. **`ModelEntry` + `OpencodeModelsResponse`** in `tama_handlers/models/utils.rs` (or `types.rs` if the module prefers — keep in `utils.rs` next to `build_model_entry`):
   ```rust
   /// Context/output limits sub-object of an opencode model entry.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct ModelLimit {
       pub context: Option<u32>,
       pub output: Option<u32>,
   }

   /// One model entry in the `/v1/opencode/models` discovery response.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct ModelEntry {
       pub id: Option<String>,
       pub name: String,
       pub model: Option<String>,
       pub backend: String,
       pub context_length: Option<u32>,
       pub limit: ModelLimit,
       pub quant: Option<String>,
       pub gpu_layers: Option<String>,
       #[serde(skip_serializing_if = "Option::is_none")]
       pub modalities: Option<crate::config::ModelModalities>,
       pub tool_call: bool,
       pub reasoning: bool,
       pub attachment: bool,
       pub temperature: bool,
   }

   /// Response wrapper for `/v1/opencode/models`.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct OpencodeModelsResponse {
       pub models: Vec<ModelEntry>,
   }
   ```
   (`crate::config::ModelModalities` at `config/types/model.rs:391` already derives Serialize/Deserialize — reuse it; the current code emits `{"input": ..., "output": ...}` which matches.) Export both from `tama_handlers/models/mod.rs` (`pub use`).

2. **Rewrite `build_model_entry`** to return `Option<ModelEntry>`: identical logic (hf_repo guard, context_length resolution via `get_model_card`… — NOTE: if plan-173 has landed this is `get_model_toml`; use whichever name exists), `output_limit = context_length.map(|ctx| (ctx / 8).clamp(16384, 32768))`, pretty-name construction, `attachment` derivation, capabilities default `(true, false)`, `temperature: true` literal. Construct the struct directly instead of the `json!` + 4 key-insertions.

3. **`opencode.rs`:** `handle_opencode_list_models` returns `Json<OpencodeModelsResponse>`. The `seen_ids` read at :64 becomes `if let Some(api_id) = entry.id.as_deref() { seen_ids.insert(api_id.to_lowercase()); }`. The alias arm (:86-95) becomes field assignment: `entry.id = Some(alias_name.to_lowercase()); entry.name = alias_display;`. Final return `Json(OpencodeModelsResponse { models })` instead of `json!({ "models": models })`.

4. **`BackendModelEntry`** in `proxy/handlers/models.rs`:
   ```rust
   /// A model entry as returned by a backend's own `/v1/models`.
   /// Unknown fields pass through untouched via `extra` — clients of the proxy
   /// see exactly what the backend sent (plus normalized `id`/`ready`).
   #[derive(Debug, Clone, Default, Serialize, Deserialize)]
   pub struct BackendModelEntry {
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub id: Option<String>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub aliases: Option<Vec<String>>,
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub ready: Option<bool>,
       #[serde(flatten)]
       pub extra: serde_json::Map<String, serde_json::Value>,
   }
   ```
   - `parse_models_response(body: &[u8]) -> Vec<BackendModelEntry>`: parse `Value`, take `data` array, `serde_json::from_value::<BackendModelEntry>` per element, skipping elements that fail (current code keeps anything; a malformed element failing to deserialize should be skipped with a `warn!` — the only acceptable behavior delta, note it in the commit message; llama-server entries always have string `id` so this should never fire).
   - `fetch_models_from_backend` returns `Vec<BackendModelEntry>` (unchanged otherwise).
   - `find_model_in_entries(entries: &[BackendModelEntry], config_model: Option<&str>) -> Option<BackendModelEntry>`: same algorithm; matching reads `entry.id.as_deref()` and `entry.aliases` instead of string keys.
   - `handle_get_model`: the mutation becomes `entry.id = Some(response_id.to_string()); entry.ready = Some(true;)` — preserve the exact semantics including the is_alias branch.
   - `handle_list_models`: alias-normalization block reads `entry.aliases` (iterate `alias_str` as before), writes `entry.id = Some(new_id)`, `entry.ready = Some(true)`; the duplicate-id check uses `entry.id.as_deref().unwrap_or("")` (matches the current `unwrap_or("")`); Phase-4/5 `json!` constructions for unloaded models and aliases stay as-is (typed in Task… NO — leave them; they are the proxy's own OpenAI-shape entries, not the opencode shape; typing them is explicitly out of scope).

5. **Update `list_models_tests.rs`** — the `parse_models_response` tests assert on `Vec<Value>`; change assertions to typed field reads (`result[0].id.as_deref() == Some("…")`, `result[0].extra.get("…")`). Add one passthrough test: body with an entry containing a custom key (`{"id": "m", "custom_field": 42}`) round-trips through parse → serialize with `custom_field` intact.

**Steps:**
- [ ] Add a failing passthrough test in `list_models_tests.rs` (flatten round-trip) — run `cargo nextest run --package tama-core -- proxy::handlers` (it fails to compile until `BackendModelEntry` exists)
- [ ] Implement items 1–4
- [ ] Update the existing `parse_models_response`/opencode tests per item 5
- [ ] Run `cargo nextest run --package tama-core -- proxy::tama_handlers::models` and `-- proxy::handlers` — all pass
- [ ] Run `cargo nextest run --package tama-core` — full pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: type opencode ModelEntry and backend model entries (flatten passthrough)"

**Acceptance criteria:**
- [ ] `build_model_entry` returns `Option<ModelEntry>`; no string-key indexing remains in `opencode.rs`
- [ ] `BackendModelEntry.extra` preserves unknown backend fields (proven by the passthrough test)
- [ ] The opencode wire shape is unchanged (`models` array; `modalities` omitted when absent; `temperature: true` literal) — existing opencode tests pass unmodified in their assertions
- [ ] `cargo nextest run --package tama-core` passes; clippy clean

---

### Task 3: Shared `OkResponse` and `ModelMutationResponse` for plain success bodies

**Context:**
`json!({"ok": true})` is repeated at 6 plain sites and `json!({"ok": true, "id": <i64>})` at 4 CRUD sites (all verified). The OpenAPI schema `OkResponse` (openapi.rs:641) already documents this shape but drifted (`"id": {"type": "string"}` — the handlers emit an i64). Decision: two structs in tama-core (the common dependency of both crates) — `OkResponse { ok: bool }` with an `OkResponse::OK` const for the no-id sites, and `ModelMutationResponse { ok: bool, id: i64 }` for the model CRUD sites. Rich one-off payloads (`crud/delete.rs:120`'s `ok+id+quant_key+deleted_file`, `models/files.rs:169`) are NOT converted. The spawn_blocking closures in the CRUD handlers have inferred return types, so swapping the `Ok(json!(...))` tail for `Ok(Json(ModelMutationResponse { … }))` just works (`Result<Json<_>, (StatusCode, Json<Value>)>` still implements `IntoResponse`).

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/types.rs`
- Modify: `crates/tama-core/src/proxy/handlers/status.rs`
- Modify: `crates/tama/src/api.rs` (sites :162, :449)
- Modify: `crates/tama/src/api/updates.rs` (site :331)
- Modify: `crates/tama/src/api/benchmarks/history.rs` (site :317)
- Modify: `crates/tama/src/api/models/crud/create.rs` (:130), `crud/update.rs` (:97, :180), `crud/rename.rs` (:116), `crud/delete.rs` (:261)

**What to implement:**

1. **Types** in `crates/tama-core/src/proxy/tama_handlers/types.rs`:
   ```rust
   /// Plain success body for management endpoints that return nothing else.
   #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
   pub struct OkResponse {
       pub ok: bool,
   }

   impl OkResponse {
       /// The canonical `{"ok": true}` body.
       pub const OK: Self = Self { ok: true };
   }

   /// Success body for model create/update/rename — carries the affected DB id.
   #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
   pub struct ModelMutationResponse {
       pub ok: bool,
       pub id: i64,
   }
   ```
   Add unit tests in the same file's test module: `serde_json::to_value(OkResponse::OK) == json!({"ok": true})`; `serde_json::to_value(ModelMutationResponse { ok: true, id: 7 }) == json!({"ok": true, "id": 7})`.

2. **Plain sites** → `Json(OkResponse::OK).into_response()`:
   - `crates/tama-core/src/proxy/handlers/status.rs:78` (`handle_reload_configs` Ok arm)
   - `crates/tama/src/api.rs:162` and `:449` (config save + config patch Ok arms)
   - `crates/tama/src/api/updates.rs:331`
   - `crates/tama/src/api/benchmarks/history.rs:317`
   - `crates/tama/src/api/models/crud/delete.rs:261`
   Import: `use tama_core::proxy::tama_handlers::OkResponse;` in the tama files (check the re-export path — `tama_handlers/mod.rs` re-exports `types::*`; if `OkResponse` lands in the glob, `tama_core::proxy::tama_handlers::OkResponse` resolves; otherwise add it to the explicit re-export list at `tama_handlers/mod.rs:31`).

3. **CRUD id sites** → replace the closure-tail `Ok(serde_json::json!({ "ok": true, "id": model_id }))` with `Ok(Json(ModelMutationResponse { ok: true, id: model_id }))` at `crud/create.rs:130`, `crud/update.rs:97` (`new_model_id`) and `:180`, `crud/rename.rs:116`. The closures' inferred `Ok` type changes from `Value` to `Json<ModelMutationResponse>`; the surrounding `match`/`.await` plumbing needs no edit because both `Value` and `Json<_>` implement `IntoResponse` — VERIFY each handler's tail still compiles (some may do `Ok(Json(body))` wrapping; if a site wraps the value in `Json(...)` again, flatten it).

**Steps:**
- [ ] Add the two structs + shape tests; run `cargo nextest run --package tama-core -- proxy::tama_handlers` — pass
- [ ] Convert the 6 plain sites; run `cargo nextest run --package tama-core -- proxy::handlers` and `cargo nextest run --package tama` — pass
- [ ] Convert the 4 CRUD sites; run `cargo nextest run --package tama -- api::models` — pass
- [ ] Run `rg '"ok": true' crates/tama/src crates/tama-core/src -g "*.rs"` — remaining hits only at the exempt rich payloads (`crud/delete.rs:120`, `models/files.rs:169`) and test literals
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: shared OkResponse/ModelMutationResponse for plain success bodies"

**Acceptance criteria:**
- [ ] All 10 listed sites construct the typed structs; wire bytes unchanged (`{"ok": true}` / `{"ok": true, "id": <int>}`)
- [ ] Rich one-off payloads untouched
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 4: Drift-guard tests — one typed round-trip per module

**Context:**
Typed structs only stay honest if something asserts the endpoints actually emit them. One test per touched surface, each building the real router (or calling the handler) and deserializing the response body INTO the new type — a future shape change fails the test. These sit beside the code they guard (in-crate `#[cfg(test)]` modules), reuse the fixtures from Task 1–3, and require no network.

**Files:**
- Modify: `crates/tama-core/src/proxy/mod.rs` (status guard, extends Task 1 tests)
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/tests/opencode.rs`
- Create: `crates/tama/src/api/models/crud/tests_typed.rs` — NO: append to the existing `crates/tama/src/api/models/crud/tests.rs`
- Modify: `crates/tama/src/api.rs` (test module for the config PUT guard)

**What to implement:**

1. **`/status` guard** (`proxy/mod.rs` tests): `test_status_endpoint_deserializes_into_status_response` — build `Router::new().route("/status", get(handle_status)).with_state(state)` (state from the Task-1 golden fixture), `oneshot` GET `/status`, assert 200, then `serde_json::from_slice::<StatusResponse>(&body_bytes)` succeeds AND `serde_json::to_value(&parsed) == serde_json::from_slice::<Value>(&body_bytes)` (round-trip lossless — catches any field the struct drops or invents).
2. **Opencode guard** (`tama_handlers/models/tests/opencode.rs`): `test_opencode_response_deserializes_into_typed` — reuse `create_state_with_model` + `call_list_models`; assert `serde_json::from_value::<OpencodeModelsResponse>(result.clone())` succeeds and `serde_json::to_value(&parsed) == result` (lossless round-trip; catches e.g. a future field added to the json but not the struct).
3. **Model CRUD guard** (`crates/tama/src/api/models/crud/tests.rs`): `test_create_model_response_deserializes_into_mutation_response` — drive `create_model` via the established crud test harness (read the file's existing helpers first — `apply_model_body_*` tests show the pattern; if no router harness exists, call the handler fn directly with a constructed `State`/`Json`) and assert the body deserializes into `ModelMutationResponse` with `ok && id > 0`. Add a second assertion for the delete path → `OkResponse`.
4. **Config guard** (`crates/tama/src/api.rs` test module — create `#[cfg(test)] mod tests` if absent): `test_put_config_response_deserializes_into_ok_response` — PUT `/tama/v1/config` via `build_web_routes` + CSRF pair (pattern from `api/backends/manage/tests.rs`) with a minimal valid config body (mirror shape — read the handler's expected body type first; `Config::default()` serialized through the WASM mirror is the safest fixture) → 200 → body deserializes into `OkResponse` with `ok`.

**Steps:**
- [ ] Write the 4 guard tests; run each with `cargo nextest run --package <crate> -- <filter>` — all pass against the Task 1–3 implementations (write them AFTER Tasks 1–3 land; they are the regression net, not TDD drivers)
- [ ] Deliberately break one struct (add a dummy field) in a scratch checkout to confirm the guards fail — revert immediately
- [ ] Run `cargo nextest run --workspace` — pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "test: drift-guard typed round-trips for status/opencode/crud/config responses"

**Acceptance criteria:**
- [ ] 4 guard tests exist and perform full lossless round-trips (not just "parses")
- [ ] Guards fail when a struct and the wire shape diverge (verified by the scratch break)
- [ ] `cargo nextest run --workspace` passes; clippy clean

---

### Task 5: Update `openapi.rs` for the newly-typed endpoints only

**Context:**
`crates/tama/src/api/openapi.rs` (1,123 lines, hand-maintained) already contains an `OkResponse` schema (:641) whose `"id": {"type": "string"}` contradicts the i64 the handlers emit — and the endpoints typed in Tasks 1–3 deserve schemas sourced from the structs. Scope is strictly the newly-typed surfaces: the `OkResponse` schema fix, a new `ModelMutationResponse` schema, `StatusResponse`/`StatusModelEntry` schemas IF the spec documents `/status` (check — `/status` is a tama-core proxy route and may not be in this management-API spec at all), `OpencodeModelsResponse`/`ModelEntry` IF the spec documents `/v1/opencode/models`, and re-pointing the CRUD paths' 200 responses at `ModelMutationResponse`. The known `ErrorResponse` schema contradiction is plan-161's — do NOT touch it here.

**Files:**
- Modify: `crates/tama/src/api/openapi.rs`

**What to implement:**

1. **Fix `OkResponse`** (:641): `"id": {"type": "string"}` → `"id": {"type": "integer", "format": "int64"}` (keep `required: ["ok"]` — id is absent on plain sites).
2. **Add `ModelMutationResponse`** next to it: `{"type": "object", "required": ["ok", "id"], "properties": {"ok": {"type": "boolean", "example": true}, "id": {"type": "integer", "format": "int64"}}}`.
3. **Re-point CRUD 200 responses**: find the path entries for model create (`POST /tama/v1/models`), update (`PUT /tama/v1/models/:id` — read the exact path strings in the file), and rename (`POST /tama/v1/models/:id/rename`); change their 200 `"$ref": "#/components/schemas/OkResponse"` (or inline shape) to `ModelMutationResponse`. Leave every other `OkResponse` reference as-is.
4. **Conditional additions** — `rg -n '"/status"|opencode' crates/tama/src/api/openapi.rs`: if a `/status` path exists, add `StatusResponse` + `StatusModelEntry` + `ProxyMetricsSnapshot` + `VramStatus` schemas mirroring the Task-1 structs field-for-field (all 15 `StatusModelEntry` keys, `additionalProperties: false`) and `$ref` them; if an opencode path exists, add `OpencodeModelsResponse`/`ModelEntry`/`ModelLimit`/`ModelModalities` schemas and `$ref` them. If neither path exists in the spec, add NOTHING for them (spec documents management API only) and note that in the commit message.
5. **Consistency test**: extend the guard from Task 4 (or add `test_openapi_ok_response_schema_matches_struct` in `crates/tama/src/api.rs` tests) asserting the spec's `OkResponse.required == ["ok"]` and `properties.id.type == "integer"` — cheap protection against re-drift on the schema we just fixed. Do NOT add a full spec-vs-code diff harness (out of scope).

**Steps:**
- [ ] Apply items 1–3; check the rendered spec: `rg -n "ModelMutationResponse" crates/tama/src/api/openapi.rs` — 1 schema + N path refs
- [ ] Apply item 4 conditionally after the `rg` check
- [ ] Apply item 5's schema test; run `cargo nextest run --package tama -- api` — pass
- [ ] Run `cargo nextest run --workspace` — pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "docs: align openapi.rs with newly typed responses (OkResponse id type, ModelMutationResponse)"

**Acceptance criteria:**
- [ ] `OkResponse` schema's `id` is `integer/int64`; `ModelMutationResponse` exists and is referenced by the create/update/rename paths
- [ ] No other schemas touched (ErrorResponse stays plan-161's); no utoipa/codegen introduced
- [ ] Schema-shape test guards the fix
- [ ] `cargo nextest run --workspace` passes; clippy clean
