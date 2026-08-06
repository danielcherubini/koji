# Fix /v1/opencode Endpoint Plan

**Goal:** Fix two bugs in the `/v1/opencode/models` endpoint: (1) context_length not populated for vLLM models, (2) model IDs incorrectly lowercased.

**Architecture:** The opencode handler currently queries `/props` for capabilities only. We extend it to also query `/v1/models` from each backend to extract per-model context lengths (`max_model_len` for vLLM, `meta.n_ctx` for llama.cpp). The `build_model_entry` function gains a `backend_context_length` parameter that fills the gap when `cfg.context_length` is None. Model IDs are returned as-is without lowercasing.

**Tech Stack:** Rust, axum, serde_json

---

### Task 1: Extract context_length from backend /v1/models and thread through to build_model_entry

**Context:**
For vLLM backends, the context length is stored as `max_model_len` in the backend's `/v1/models` response. For llama.cpp, it's `meta.n_ctx`. The opencode handler currently only queries `/props` (llama.cpp-only endpoint) for capabilities, so vLLM models get `context_length: null`. We need to query `/v1/models` from each backend and extract the context length per model.

The existing `fetch_models_from_backend` in `crates/tama-core/src/proxy/handlers/models.rs` already does this fetch — we can reuse it. The `BackendModelEntry` struct captures the response with `extra: serde_json::Map<String, serde_json::Value>` which contains the unknown fields like `max_model_len` and `meta`.

**Important notes for the executing agent:**
- `fetch_models_from_backend` takes `(state: &ProxyState, backend_url: &str)` — not `(&reqwest::Client, ...)` like the capabilities helper.
- It uses a 10s timeout.
- Existing tests that only mock `/props` will see 404 from wiremock for `/v1/models`, and `fetch_models_from_backend` degrades to an empty `Vec` — existing tests pass unchanged.
- Multiple configs can share the same backend URL — deduplicate URLs before fetching.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/opencode.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/tests/opencode.rs`

**What to implement:**

1. In `opencode.rs`, add the following import at the top:
   ```rust
   use crate::proxy::handlers::models::{
       BackendModelEntry, find_model_in_entries, fetch_models_from_backend,
   };
   ```
   After fetching capabilities from `/props`, also call `fetch_models_from_backend(&state, url)` for each **unique** backend URL. Build a `HashMap<String, Vec<BackendModelEntry>>` keyed by backend_url.

2. Create a helper function `extract_context_length_from_backend_entry(&BackendModelEntry) -> Option<u32>` that:
   - Checks `entry.extra.get("max_model_len").and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok())` (vLLM)
   - Falls back to `entry.extra.get("meta").and_then(|m| m.get("n_ctx")).and_then(|v| v.as_u64()).and_then(|v| u32::try_from(v).ok())` (llama.cpp)
   - Use `u32::try_from` (not `as u32`) to avoid silent truncation on values > u32::MAX.

3. In `opencode.rs`, when iterating configs, use `find_model_in_entries` to find the matching backend entry for each config, and extract its context length via the helper from step 2.

4. Change `build_model_entry` signature to accept `backend_context_length: Option<u32>` as a new parameter.

5. In `build_model_entry`, update the context_length resolution order:
   ```
   cfg.context_length (highest priority)
   → backend_context_length (from /v1/models)
   → model_toml (lowest priority, existing fallback)
   ```

6. In the alias branch (line ~88 of `opencode.rs`), when calling `build_model_entry` for the alias, look up the backend context length for the target config key `key` (the same way capabilities are looked up via `cap_map.get(key)`). This ensures aliases inherit backend-derived context length from their target model.

7. What NOT to change: Do not modify `fetch_models_from_backend` or `BackendModelEntry` — reuse them as-is.

**Steps:**
- [ ] Write a test in `tests/opencode.rs` that mocks a vLLM backend `/v1/models` returning `max_model_len: 32768` and verifies the opencode response has `context_length: 32768`
  - [ ] Run `cargo nextest run --package tama-core -- opencode` — verify new test fails (context_length is null)
- [ ] Implement `extract_context_length_from_backend_entry` helper in `opencode.rs`
- [ ] Modify `handle_opencode_list_models` to fetch `/v1/models` from each backend and build the context_length map
- [ ] Update `build_model_entry` signature to accept `backend_context_length: Option<u32>`
- [ ] Update `build_model_entry` to use the new resolution order for context_length
- [ ] Update the alias branch to pass backend context length for the target config
- [ ] Run `cargo nextest run --package tama-core -- opencode` — verify all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core --all-targets -- -D warnings`
- [ ] Commit with message: "fix: extract context_length from backend /v1/models for opencode endpoint"

**Acceptance criteria:**
- [ ] vLLM models with `max_model_len` in `/v1/models` response get correct `context_length` in opencode response
- [ ] llama.cpp models with `meta.n_ctx` in `/v1/models` response get correct `context_length`
- [ ] Config-level `context_length` still takes precedence over backend value
- [ ] Alias entries inherit backend-derived `context_length` from their target model
- [ ] All existing opencode tests still pass

---

### Task 2: Stop lowercasing model IDs in opencode response

**Context:**
The `build_model_entry` function in `utils.rs` force-lowercases the `api_id` (line ~131), and the alias handling in `opencode.rs` force-lowercases the alias name (line ~87). This breaks clients that expect the original casing (e.g., "Unsloth/Qwen3.5-35B-A3B-GGUF" becomes "unsloth/qwen3.5-35b-a3b-gguf").

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/opencode.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/tests/opencode.rs`

**What to implement:**

1. In `utils.rs`, change the `api_id` computation from:
   ```rust
   let api_id = cfg
       .api_name
       .as_ref()
       .map(|s| s.to_lowercase())
       .or_else(|| cfg.model.as_ref().map(|s| s.to_lowercase()));
   ```
   to:
   ```rust
   let api_id = cfg.api_name.clone().or_else(|| cfg.model.clone());
   ```

2. In `opencode.rs`, change alias ID from `entry.id = Some(alias_name.to_lowercase())` to `entry.id = Some(alias_name.clone())`.

3. In `opencode.rs`, change `seen_ids` tracking from lowercase to exact match:
   - `seen_ids.insert(api_id.to_lowercase())` → `seen_ids.insert(api_id.to_string())`
   - `seen_ids.contains(&alias_name.to_lowercase())` → `seen_ids.contains(alias_name.as_str())`
   - `seen_ids.insert(alias_name.to_lowercase())` → `seen_ids.insert(alias_name.clone())`

   **Note:** This is a behavioral change — exact-case dedup means aliases differing only in case from a model id can both appear. This is acceptable because routing uses `eq_ignore_ascii_case` anyway, and it matches the main `/v1/models` handler's behavior.

4. The alias resolution lookup (finding target config by resolved_model) currently uses `to_lowercase()` for comparison — keep this behavior (it's for matching, not for the output ID).

5. What NOT to change: Do not modify the alias resolution lookup logic (the `resolved_lower` comparison) — only change the output IDs and seen_ids tracking.

**Steps:**
- [ ] Write a test in `tests/opencode.rs` that creates a model with mixed-case api_name (e.g., "Unsloth/Qwen3.5-35B") and verifies the opencode response preserves the original casing
  - [ ] Run `cargo nextest run --package tama-core -- opencode` — verify new test fails (ID is lowercased)
- [ ] Remove `.to_lowercase()` from `api_id` in `utils.rs`
- [ ] Remove `.to_lowercase()` from alias `entry.id` in `opencode.rs`
- [ ] Update `seen_ids` to use exact case in `opencode.rs`
- [ ] Run `cargo nextest run --package tama-core -- opencode` — verify all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core --all-targets -- -D warnings`
- [ ] Commit with message: "fix: preserve original casing for model IDs in opencode endpoint"

**Acceptance criteria:**
- [ ] Model IDs in opencode response preserve original casing from `api_name`/`model` config fields
- [ ] Alias IDs preserve original casing from the alias name
- [ ] Deduplication still works (no exact duplicate IDs in response)
- [ ] All existing opencode tests still pass
