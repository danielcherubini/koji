# Unified Model Metadata Plan

**Goal:** Unify model metadata (quant, KV cache dtype, context length, architecture) so Safetensors models display the same detail pills as GGUF models on the dashboard.

**Architecture:** Introduce a `ModelMetadata` struct that holds the unified view of common model fields. `ModelMetadata::resolve(&ModelConfig)` picks values from whichever source is populated (GGUF columns OR vLLM JSON). `ModelStateSnapshot` uses resolved values — fixes dashboard pills. `config.json` parsing populates metadata from the source of truth for transformers models.

**Tech Stack:** Rust (tama-core), Leptos (tama frontend), SQLite (model_configs table)

---

### Task 1: Add vLLM field resolution to `ModelStateSnapshot`

**Context:** Quick win that fixes the immediate problem — Safetensors models show no quant pill, no KV cache pill on the dashboard. `ModelStateSnapshot` (the dashboard SSE struct) only carries GGUF-specific fields (`quant`, `cache_type_k`, `cache_type_v`). `VllmConfig` stores `quantization`, `kv_cache_dtype`, `max_model_len` but they are never surfaced. This task adds resolution in `collect_model_state_snapshots()` so the effective value comes from whichever source is populated.

**Files:**
- Modify: `crates/tama-core/src/proxy/status.rs` — add inline resolution in `collect_model_state_snapshots()`
- Modify: `crates/tama-core/src/proxy/status.rs` — use resolution in `collect_model_state_snapshots()`
- Modify: `crates/tama/src/pages/dashboard/metrics.rs` — frontend mirror struct (no new fields needed, uses existing fields now populated correctly)
- Modify: `crates/tama/src/pages/dashboard/tests.rs` — update test helpers if needed

**What to implement:**

In `crates/tama-core/src/models/types.rs`, add a helper on `ModelStateSnapshot` or a standalone function:

In `crates/tama-core/src/proxy/status.rs`, in `collect_model_state_snapshots()`, add inline resolution (no separate functions needed — keep it simple and inline):

```rust
// Before:
quant: model_cfg.quant.clone(),
context_length: model_cfg.context_length,
cache_type_k: model_cfg.cache_type_k.clone(),
cache_type_v: model_cfg.cache_type_v.clone(),

// After — each field resolved independently with vLLM fallback:
quant: model_cfg.quant.clone().or_else(|| model_cfg.vllm.quantization.clone()),
context_length: model_cfg.context_length.or_else(|| model_cfg.vllm.max_model_len),
cache_type_k: model_cfg.cache_type_k.clone().or_else(|| model_cfg.vllm.kv_cache_dtype.clone()),
cache_type_v: model_cfg.cache_type_v.clone().or_else(|| model_cfg.vllm.kv_cache_dtype.clone()),
```

Note: `cache_type_k` and `cache_type_v` are resolved **independently** — each falls back to `vllm.kv_cache_dtype` only when its own column is `None`. This preserves the existing behavior where K and V can have different values (e.g., K=q8_0, V=q4_0). When only vLLM `kv_cache_dtype` is set, both K and V get the same value (which is correct for vLLM, as it uses a single `--kv-cache-dtype` flag).

**Steps:**
- [ ] Write a unit test in `crates/tama-core/src/proxy/status.rs` (in `#[cfg(test)]` module) that verifies the inline resolution logic:
  - When GGUF field is `Some`, it wins
  - When GGUF field is `None`, falls back to vLLM field
  - When both are `None`, returns `None`
  - Test each field independently: `quant`, `context_length`, `cache_type_k`, `cache_type_v`
- [ ] Run `cargo nextest run --package tama-core -- proxy::status`
  - Did it fail? If no tests exist yet, this is expected.
- [ ] Implement the inline resolution in `collect_model_state_snapshots()` in `crates/tama-core/src/proxy/status.rs`
- [ ] Run `cargo nextest run --package tama-core -- models::types`
  - Did all tests pass?
- [ ] Update `collect_model_state_snapshots()` in `crates/tama-core/src/proxy/status.rs` to use the resolution functions
- [ ] Run `cargo check --package tama-core`
  - Did it compile?
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --package tama-core`
  - Did all tests pass?
- [ ] Commit with message: "feat: resolve vLLM metadata fields in ModelStateSnapshot for dashboard pills"

**Acceptance criteria:**
- [ ] `ModelStateSnapshot.quant` is populated for Safetensors models (from `VllmConfig.quantization`)
- [ ] `ModelStateSnapshot.cache_type_k` and `cache_type_v` are populated for Safetensors models (from `VllmConfig.kv_cache_dtype`)
- [ ] `ModelStateSnapshot.context_length` falls back to `VllmConfig.max_model_len` when `context_length` is `None`
- [ ] GGUF models still work (GGUF fields take priority)
- [ ] All existing tests pass

---

### Task 2: Introduce `ModelMetadata` struct with `resolve()` method

**Context:** Structural fix that eliminates the fragmentation between GGUF-specific and vLLM-specific metadata storage. Currently, the same concepts (quant, KV cache dtype, context length, architecture, num layers, embedding length, head count, block count) exist in different locations depending on backend type. Every consumer (dashboard, REST API, model card UI, arg builder) must implement fallback logic. This task introduces a unified `ModelMetadata` struct with a single `resolve()` method.

**Files:**
- Create: `crates/tama-core/src/models/metadata.rs` — new `ModelMetadata` struct + `resolve()` method
- Modify: `crates/tama-core/src/models/mod.rs` — export `ModelMetadata`
- Modify: `crates/tama-core/src/models/types.rs` — remove ad-hoc `resolve_*` functions (replaced by `ModelMetadata::resolve()`)
- Modify: `crates/tama-core/src/proxy/status.rs` — use `ModelMetadata::resolve()` in `collect_model_state_snapshots()`
- Modify: `crates/tama/src/pages/models.rs` — remove ad-hoc `resolve_*` functions, use resolved values from API response
- Modify: `crates/tama-core/src/proxy/tama_handlers/models/utils.rs` — use `ModelMetadata::resolve()` in `build_model_entry()`

**What to implement:**

Create `crates/tama-core/src/models/metadata.rs`:

```rust
/// Unified model metadata — common fields that apply regardless of backend type.
/// Resolved from whichever source is populated: GGUF columns, vLLM config, or file parsing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelMetadata {
    /// Quantization name (e.g. "Q4_K_M", "fp8").
    pub quant: Option<String>,
    /// KV cache data type for K head (e.g. "f16", "q4_0", "fp8").
    /// Independent from kv_cache_v — llama.cpp supports different types per head.
    pub kv_cache_k: Option<String>,
    /// KV cache data type for V head (e.g. "f16", "q8_0", "fp8").
    /// Independent from kv_cache_k — llama.cpp supports different types per head.
    pub kv_cache_v: Option<String>,
    /// Context length in tokens.
    pub context_length: Option<u32>,
    /// Architecture type (e.g. "llama", "Qwen2ForCausalLM", "MoE", "Dense").
    pub architecture: Option<String>,
    /// Number of layers.
    pub num_layers: Option<u32>,
    /// Embedding/hidden dimension.
    pub embedding_length: Option<u32>,
    /// Number of attention heads.
    pub head_count: Option<u32>,
    /// Number of transformer blocks (same as num_layers for most architectures).
    pub block_count: Option<u32>,
}

impl ModelMetadata {
    /// Resolve unified metadata from a ModelConfig.
    /// Picks values from whichever source is populated:
    /// 1. GGUF columns (highest priority — explicit config)
    /// 2. vLLM config (fallback for transformers models)
    /// 3. HF metadata (fallback for architecture, context, layers)
    pub fn resolve(cfg: &crate::config::ModelConfig) -> Self {
        Self {
            quant: cfg.quant.clone().or_else(|| cfg.vllm.quantization.clone()),
            kv_cache_k: cfg.cache_type_k.clone().or_else(|| cfg.vllm.kv_cache_dtype.clone()),
            kv_cache_v: cfg.cache_type_v.clone().or_else(|| cfg.vllm.kv_cache_dtype.clone()),
            context_length: cfg.context_length
                .or_else(|| cfg.vllm.max_model_len)
                .or(cfg.hf_context_length),
            architecture: cfg.hf_architecture_type.clone(),
            num_layers: cfg.hf_num_layers,
            embedding_length: None,
            head_count: None,
            block_count: None,
        }
    }
}```
```

Update `crates/tama-core/src/models/mod.rs` to add module declaration and re-export:
```rust
pub mod metadata;
pub use metadata::ModelMetadata;
```

Same pattern for Task 3's `transformers.rs`:
```rust
pub mod transformers;
pub use transformers::{TransformersMetadata, parse_transformers_metadata};
```

In `crates/tama-core/src/proxy/status.rs`, replace the inline resolution from Task 1 with:
```rust
let meta = crate::models::ModelMetadata::resolve(model_cfg);
// ...
let status = crate::models::ModelStateSnapshot {
    quant: meta.quant,
    context_length: meta.context_length,
    cache_type_k: meta.kv_cache_k,
    cache_type_v: meta.kv_cache_v,
    // ... rest unchanged
};
```

In `crates/tama-core/src/proxy/tama_handlers/models/utils.rs`, in `build_model_entry()`:
```rust
let meta = crate::models::ModelMetadata::resolve(cfg);
// Use meta.quant, meta.context_length instead of cfg.quant, cfg.context_length
```

**DO NOT remove the frontend `resolve_*` functions from `crates/tama/src/pages/models.rs`.** The models page fetches from `/tama/v1/models` (served by `model_entry_json()` in `crates/tama/src/api/models/info.rs`), which returns raw DB values + `vllm` JSON. The frontend `resolve_*` functions correctly merge these. They can remain as-is — the backend resolution fixes the SSE/dashboard path, which is the actual gap. If desired, `model_entry_json()` could be updated to return pre-resolved values, but that is out of scope for this plan.

**Steps:**
- [ ] Write unit tests for `ModelMetadata::resolve()` in `crates/tama-core/src/models/metadata.rs`:
  - Test: GGUF fields take priority over vLLM fields
  - Test: vLLM fields used when GGUF fields are `None`
  - Test: HF metadata used as final fallback for context_length
  - Test: All `None` when no sources have data
- [ ] Run `cargo nextest run --package tama-core -- models::metadata`
  - Did it fail (expected — no implementation yet)?
- [ ] Implement `ModelMetadata` struct and `resolve()` method
- [ ] Run `cargo nextest run --package tama-core -- models::metadata`
  - Did all tests pass?
- [ ] Export `ModelMetadata` from `crates/tama-core/src/models/mod.rs`
- [ ] Update `collect_model_state_snapshots()` to use `ModelMetadata::resolve()`
- [ ] Update `build_model_entry()` to use `ModelMetadata::resolve()`
- [ ] Remove ad-hoc `resolve_*` functions from `crates/tama/src/pages/models.rs`
- [ ] Remove ad-hoc `resolve_*` functions from `crates/tama-core/src/models/types.rs` (replaced by `ModelMetadata`)
- [ ] Run `cargo check --workspace`
  - Did it compile?
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
  - Did all tests pass?
- [ ] Commit with message: "refactor: introduce ModelMetadata struct with unified resolve()"

**Acceptance criteria:**
- [ ] `ModelMetadata::resolve()` is the single source of truth for model metadata resolution
- [ ] No ad-hoc `resolve_*` functions remain in frontend or backend
- [ ] `ModelStateSnapshot` uses `ModelMetadata::resolve()`
- [ ] `build_model_entry()` uses `ModelMetadata::resolve()`
- [ ] All existing tests pass
- [ ] Dashboard and models page show consistent pills for both GGUF and Safetensors models

---

### Task 3: Add `config.json` parsing for transformers models

**Context:** GGUF models get rich, file-verified metadata from `parse_gguf_metadata()` (architecture, context_length, embedding_length, block_count, head_count, quantization). Safetensors models get zero file parsing — only HF API metadata and README heuristics. This adds `parse_transformers_metadata()` that reads `config.json` from the model directory, bringing Safetensors metadata quality to parity with GGUF.

**Files:**
- Create: `crates/tama-core/src/models/transformers.rs` — new `parse_transformers_metadata()` function
- Modify: `crates/tama-core/src/models/mod.rs` — export new module
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/start.rs` — call `parse_transformers_metadata()` after pull completion (parallel to `parse_gguf_metadata()`)
- Modify: `crates/tama-core/src/models/metadata.rs` — `ModelMetadata::resolve()` incorporates file-parsed data

**What to implement:**

Create `crates/tama-core/src/models/transformers.rs`:

```rust
/// Metadata extracted from a transformers model directory (config.json).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransformersMetadata {
    pub architectures: Vec<String>,          // e.g. ["Qwen2ForCausalLM"]
    pub hidden_size: Option<u32>,            // embedding_length
    pub num_hidden_layers: Option<u32>,      // block_count / num_layers
    pub num_attention_heads: Option<u32>,    // head_count
    pub max_position_embeddings: Option<u32>,// context_length
    pub quantization_method: Option<String>, // from quantization_config.quant_method
}

/// Parse `config.json` from a transformers model directory.
/// Returns `Err` only if the file cannot be read or is invalid JSON.
/// Individual missing keys are handled gracefully (fields are `None`/empty).
pub fn parse_transformers_metadata(model_dir: &Path) -> Result<TransformersMetadata> {
    let config_path = model_dir.join("config.json");
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config.json: {}", config_path.display()))?;
    
    let config: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse config.json: {}", config_path.display()))?;
    
    Ok(TransformersMetadata {
        architectures: config.get("architectures")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        hidden_size: config.get("hidden_size").and_then(|v| v.as_u64()).map(|v| v as u32),
        num_hidden_layers: config.get("num_hidden_layers").and_then(|v| v.as_u64()).map(|v| v as u32),
        num_attention_heads: config.get("num_attention_heads").and_then(|v| v.as_u64()).map(|v| v as u32),
        max_position_embeddings: config.get("max_position_embeddings").and_then(|v| v.as_u64()).map(|v| v as u32),
        quantization_method: config.get("quantization_config")
            .and_then(|qc| qc.get("quant_method"))
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}
```

In `crates/tama-core/src/proxy/tama_handlers/pull/start.rs`, after pull completion (near where `parse_gguf_metadata()` is called), add:

```rust
// After GGUF parsing — attempt transformers parsing only if GGUF failed
// (GGUF parsing succeeds on .gguf files, fails on .safetensors)
let transformers_metadata = if gguf_metadata.is_none() {
    parse_transformers_metadata(&dest_dir).ok()
} else {
    None
};

// Pass to setup_model_after_pull() alongside gguf_metadata
```

Note: Uses `dest_dir` (the actual variable name in `start.rs`), not `model_dir`. The check is `gguf_metadata.is_none()` — if GGUF parsing succeeded, we skip transformers parsing. If it failed (safetensors file), we try `config.json` parsing.

**`config.json` availability:** The pull flow downloads specific files by filename. `config.json` must be explicitly included in the pull request. For existing safetensors pulls that don't include `config.json`, `parse_transformers_metadata()` returns `Err` (handled gracefully with `.ok()` → `None`). The resolution chain falls back to HF API + README heuristics, which is the current behavior — no regression.

**Integration with `ModelMetadata::resolve()`:** The parsed metadata (`TransformersMetadata`) is used during pull completion to populate `ModelConfig` fields (same as GGUF metadata populates `hf_architecture_type`, `hf_context_length`, `hf_num_layers`). The `parse_transformers_metadata()` output maps to existing `ModelConfig` fields:

| `TransformersMetadata` field | → `ModelConfig` field |
|---|---|
| `architectures[0]` | `hf_architecture_type` |
| `max_position_embeddings` | `hf_context_length` |
| `num_hidden_layers` | `hf_num_layers` |
| `hidden_size` | (no column — stored in `ModelMetadata.embedding_length` at resolve time via `GgufMetadata` parallel) |
| `num_attention_heads` | (no column — stored in `ModelMetadata.head_count` at resolve time) |
| `quantization_method` | (no column — could populate `VllmConfig.quantization` if not already set) |

For fields without DB columns (`embedding_length`, `head_count`, `block_count`), the parsed data is used during pull completion to populate the `QuantEntry` in `model_toml` (same pattern as GGUF metadata populates `model_toml` entries). This avoids new DB columns.

**Steps:**
- [ ] Write unit tests for `parse_transformers_metadata()` in `crates/tama-core/src/models/transformers.rs`:
  - Test: Valid `config.json` with all fields → all fields populated
  - Test: `config.json` with missing fields → graceful `None`/empty
  - Test: Missing `config.json` file → `Err`
  - Test: Invalid JSON → `Err`
  - Test: `quantization_config.quant_method` extracted correctly
- [ ] Run `cargo nextest run --package tama-core -- models::transformers`
  - Did it fail (expected — no implementation yet)?
- [ ] Implement `TransformersMetadata` struct and `parse_transformers_metadata()`
- [ ] Run `cargo nextest run --package tama-core -- models::transformers`
  - Did all tests pass?
- [ ] Export from `crates/tama-core/src/models/mod.rs`
- [ ] Integrate into pull completion flow in `start.rs` (call after pull, parallel to `parse_gguf_metadata()`)
- [ ] Wire parsed metadata into `ModelMetadata::resolve()` (may require new `ModelConfig` fields or passing through resolution chain)
- [ ] Run `cargo check --workspace`
  - Did it compile?
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --workspace`
  - Did all tests pass?
- [ ] Commit with message: "feat: add config.json parsing for transformers model metadata"

**Acceptance criteria:**
- [ ] `parse_transformers_metadata()` reads `config.json` and extracts: architectures, hidden_size, num_hidden_layers, num_attention_heads, max_position_embeddings, quantization_method
- [ ] Missing fields handled gracefully (no panics on incomplete `config.json`)
- [ ] Called during pull completion for transformers-format models
- [ ] Parsed data flows into `ModelMetadata` (architecture shows actual name like "Qwen2ForCausalLM" instead of "Dense"/"MoE" heuristic)
- [ ] All existing tests pass

---

## Dependencies

- **Task 1** is independent — can be done first as a quick win
- **Task 2** depends on Task 1 being complete (refactors the resolution functions from Task 1 into `ModelMetadata`)
- **Task 3** depends on Task 2 (feeds into `ModelMetadata::resolve()`)

## Rollback

Each task is independently reversible:
- Task 1: Revert `status.rs` changes — falls back to GGUF-only fields
- Task 2: Revert to ad-hoc `resolve_*` functions (restore from Task 1)
- Task 3: Remove `transformers.rs` module and pull integration — falls back to HF API + README heuristics
