# /v1/opencode/models Capability Enrichment

**Goal:** Add `tool_call`, `reasoning`, `attachment`, and `temperature` fields to `/v1/opencode/models` response so opencode's provider config gets full capability metadata per model.

**Architecture:** Query each loaded backend's `/props` endpoint to get `chat_template_caps` and `default_generation_settings`, then merge capability flags into the opencode model entry. For unloaded models, derive from config fields and sensible defaults.

**Tech Stack:** Rust, axum, reqwest (already in use)

---

### Problem

The opencode `ProviderConfig` per-model schema supports these capability fields that the current `/v1/opencode/models` response does not provide:

| Field | Type | Current source | Needed for opencode behavior |
|-------|------|----------------|------------------------------|
| `tool_call` | `boolean` | ❌ Missing | Controls whether opencode sends tool definitions to the model |
| `reasoning` | `boolean` | ❌ Missing | Controls whether opencode uses reasoning mode |
| `attachment` | `boolean` | ❌ Missing (inferable from modalities) | Controls whether opencode allows image/file uploads |
| `temperature` | `boolean` | ❌ Missing | Controls whether opencode sends temperature params |

The `/props` endpoint on each llama.cpp backend provides `chat_template_caps` with `supports_tool_calls`, `supports_tools`, `supports_preserve_reasoning`, etc. This is the authoritative source for model capabilities.

### Design

**For loaded models:** Query the backend's `/props` endpoint and extract capability flags.

**For unloaded models:** Derive from config + sensible defaults:
- `tool_call: true` — all modern GGUF models support tool calling via chat template
- `reasoning: false` — only set true if `sampling.reasoning_format` is configured
- `attachment: true` if `modalities.input` contains `"image"`
- `temperature: true` — always true for local models

**No new ModelConfig fields needed** — these are computed at response time from backend state + config. No DB migration required.

---

### Task 1: Add `/props` fetch helper

**Context:**
We need to query each loaded backend's `/props` endpoint to get capability data. This is the same pattern used in the `/v1/models` enrichment (plan 2026-05-20-v1-models-meta.md) — query backend, merge, inject.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models.rs`

**What to implement:**

1. **Pure parsing function** (unit-testable):

```rust
/// Extract capability flags from a /props response body.
/// Returns (tool_call, reasoning) tuple. Defaults to (true, false) on any error.
fn extract_capabilities(body: &[u8]) -> (bool, bool) {
    // Parse as serde_json::Value
    // Extract chat_template_caps.supports_tool_calls → tool_call (default true)
    // Extract chat_template_caps.supports_preserve_reasoning → reasoning (default false)
    // Also check default_generation_settings.params.reasoning_format != "none"
    // Return (true, false) on any parse error
}
```

2. **Async HTTP helper** (uses real client):

```rust
/// Query a single backend's /props endpoint and extract capability flags.
/// Returns (tool_call, reasoning) tuple. Defaults to (true, false) on any error.
async fn fetch_capabilities_from_backend(
    client: &reqwest::Client,
    backend_url: &str,
) -> (bool, bool) {
    // Build URL: {backend_url}/props
    // Send GET with 3-second timeout
    // Parse response using extract_capabilities
    // Return (true, false) on any error
}
```

Key details:
- Use `client.get(url).timeout(Duration::from_secs(3)).send().await` — **MUST have timeout**
- `extract_capabilities` is a pure function — unit-testable
- `fetch_capabilities_from_backend` returns `(true, false)` on any error (safe defaults)
- Do NOT propagate errors — the handler should never fail because `/props` is unavailable

**Steps:**
- [ ] Implement `extract_capabilities` in `models.rs`
- [ ] Write unit tests for `extract_capabilities`:
  - Valid response with `chat_template_caps.supports_tool_calls: true` → `(true, false)`
  - Valid response with `supports_preserve_reasoning: true` → `(true, true)`
  - Valid response with `reasoning_format: "xml"` → `(true, true)`
  - Missing `chat_template_caps` → `(true, false)`
  - Invalid JSON → `(true, false)`
  - Empty body → `(true, false)`
- [ ] Implement `fetch_capabilities_from_backend` in `models.rs`
- [ ] Run `cargo test --package tama-core extract_capabilities` — all tests must pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add /props capability fetch helper for opencode enrichment"

**Acceptance criteria:**
- [ ] `extract_capabilities` is a pure function, unit-testable without HTTP
- [ ] `fetch_capabilities_from_backend` has a 3-second timeout
- [ ] Both default to `(true, false)` on any error
- [ ] All existing tests pass

---

### Task 2: Enrich `build_model_entry` with capability fields

**Context:**
The `build_model_entry` function constructs the JSON for each model in the opencode response. We need to add `tool_call`, `reasoning`, `attachment`, and `temperature` fields.

For **loaded models**, we pass capability flags from the `/props` fetch. For **unloaded models**, we derive from config + defaults.

**Critical design decisions:**
- **No new ModelConfig fields** — capabilities are computed, not persisted
- **Default `tool_call: true`** — all modern GGUF models support tool calling via chat template
- **Default `reasoning: false`** — conservative default; only true if backend confirms
- **`attachment` derived from modalities** — if `modalities.input` contains `"image"`, then `attachment: true`
- **`temperature: true`** — always true for local models
- **Lock discipline** — snapshot backend URLs under lock, drop locks before HTTP

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models.rs`

**What to implement:**

1. **Refactor `build_model_entry` to accept optional capabilities:**

```rust
async fn build_model_entry(
    state: &ProxyState,
    id: &str,
    cfg: &ModelConfig,
    capabilities: Option<&ModelCapabilities>,  // new parameter
) -> Option<serde_json::Value> {
    // ... existing logic ...

    // Derive attachment from modalities
    let attachment = modalities.as_ref().is_some_and(|m|
        m.input.iter().any(|s| s == "image")
    );

    // Use provided capabilities or config-derived defaults
    let (tool_call, reasoning) = capabilities
        .map(|c| (c.tool_call, c.reasoning))
        .unwrap_or_else(|| (true, cfg.sampling.as_ref().is_some_and(|s| s.reasoning_format.is_some())));

    // ... add to model_json ...
    model_json["tool_call"] = serde_json::json!(tool_call);
    model_json["reasoning"] = serde_json::json!(reasoning);
    model_json["attachment"] = serde_json::json!(attachment);
    model_json["temperature"] = serde_json::json!(true);

    Some(model_json)
}
```

2. **Add `ModelCapabilities` struct:**

```rust
#[derive(Debug, Clone, Copy, Default)]
struct ModelCapabilities {
    tool_call: bool,
    reasoning: bool,
}
```

3. **Rewrite `handle_opencode_list_models` to fetch capabilities for loaded models:**

```rust
pub async fn handle_opencode_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    // 1. Snapshot data under locks
    let (loaded_models, all_configs): (HashMap<_, _>, _) = {
        let models = state.models.read().await;
        let configs = state.model_configs.read().await;
        // Collect (config_name, backend_url) for Ready backends
        let loaded: HashMap<_, _> = models.iter()
            .filter_map(|(name, ms)| {
                if let ModelState::Ready { backend_url, .. } = ms {
                    Some((name.clone(), backend_url.clone()))
                } else {
                    None
                }
            })
            .collect();
        (loaded, configs.clone())
    }; // locks dropped

    // 2. Fetch capabilities for all loaded backends concurrently
    let futures: Vec<_> = loaded_models.values()
        .map(|url| fetch_capabilities_from_backend(&state.client, url))
        .collect();
    let capabilities: Vec<(bool, bool)> = futures::future::join_all(futures).await;

    // Build a map: config_name → ModelCapabilities
    let cap_map: HashMap<_, _> = loaded_models.keys()
        .zip(capabilities.into_iter())
        .map(|(name, (tool_call, reasoning))| {
            (name.clone(), ModelCapabilities { tool_call, reasoning })
        })
        .collect();

    // 3. Build model entries with capabilities
    for (id, cfg) in all_configs.iter().filter(|(_, cfg)| cfg.enabled) {
        let caps = cap_map.get(id);
        if let Some(entry) = build_model_entry(&state, id, cfg, caps).await {
            models.push(entry);
        }
    }

    // 4. Add alias entries (same capability inheritance as target model)
    // ... existing alias logic, but pass capabilities from cap_map ...

    Json(serde_json::json!({ "models": models }))
}
```

**Steps:**
- [ ] Add `ModelCapabilities` struct to `models.rs`
- [ ] Refactor `build_model_entry` to accept `Option<&ModelCapabilities>` parameter
- [ ] Add `tool_call`, `reasoning`, `attachment`, `temperature` to the JSON output
- [ ] Rewrite `handle_opencode_list_models` to fetch capabilities for loaded models
- [ ] Ensure alias entries inherit capabilities from their target model
- [ ] Write an integration test:
  - Mock a backend with `/props` returning known capabilities
  - Verify `tool_call`, `reasoning` appear in the opencode response
- [ ] Write a test for unloaded model defaults:
  - Model in config, no backend → `tool_call: true`, `reasoning: false`
- [ ] Write a test for attachment derivation:
  - Model with `modalities.input: ["text", "image"]` → `attachment: true`
  - Model with `modalities.input: ["text"]` → `attachment: false`
- [ ] Run `cargo test --package tama-core` — all tests must pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: enrich /v1/opencode/models with tool_call, reasoning, attachment, temperature"

**Acceptance criteria:**
- [ ] Loaded models get capabilities from `/props`
- [ ] Unloaded models get sensible defaults (`tool_call: true`, `reasoning: false`)
- [ ] `attachment` is derived from `modalities.input`
- [ ] `temperature` is always `true`
- [ ] Alias entries inherit capabilities from target model
- [ ] Locks are dropped before HTTP requests
- [ ] Backend queries are concurrent (`join_all`)
- [ ] All existing tests pass
- [ ] Response shape matches opencode `ProviderConfig` model schema

---

### Task 3: Update tests and verify

**Context:**
Ensure all existing tests pass and the new fields appear in the response.

**Files:**
- Modify: `crates/tama-core/src/proxy/tama_handlers/models.rs` (tests)

**What to implement:**
- Verify the existing handler tests still pass
- Add tests for the new capability fields
- Verify the response shape matches opencode's expectations

**Steps:**
- [ ] Run `cargo test --workspace --features web-ui` — all tests must pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "test: update tests for /v1/opencode/models capability enrichment"

**Acceptance criteria:**
- [ ] All workspace tests pass
- [ ] No clippy warnings
- [ ] Code is formatted

---

## Verification

After all tasks:

```bash
cargo test --workspace --features web-ui
cargo clippy --workspace -- -D warnings
cargo fmt --all
```

Manual verification:
1. Start proxy with a loaded backend
2. `curl http://localhost:11434/v1/opencode/models | jq` — verify new fields:
   - `tool_call: true` for loaded models
   - `reasoning: false` (or true if reasoning format is active)
   - `attachment: true` if model has image input modality
   - `temperature: true`
3. Verify unloaded models have defaults: `tool_call: true`, `reasoning: false`
4. Verify alias entries inherit capabilities from target model
5. Verify opencode-tama plugin picks up the new fields in its model config

## Response shape (after)

```json
{
  "models": [
    {
      "id": "unsloth/qwen3.6-27b-mtp-gguf",
      "name": "Unsloth: Qwen3.6 27B MTP",
      "model": "unsloth/Qwen3.6-27B-MTP-GGUF",
      "backend": "llama_cpp",
      "context_length": 262144,
      "limit": {
        "context": 262144,
        "output": 32768
      },
      "modalities": {
        "input": ["text", "image"],
        "output": ["text"]
      },
      "quant": "UD-Q5_K_XL",
      "gpu_layers": null,
      "tool_call": true,
      "reasoning": false,
      "attachment": true,
      "temperature": true
    }
  ]
}
```
