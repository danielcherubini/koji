# Codebase Improvement Report — 2025-07-15

**Focus:** Model metadata fragmentation — unified model truth vs. backend-specific storage

## Summary

5 findings across 3 categories. 2 high, 2 medium, 1 low.

## Context

- CONTEXT.md: loaded
- ADRs reviewed: 6 (none relevant to model metadata architecture)
- Plans reviewed: none

## Findings

### 🔴 High Severity

#### 1. `ModelStateSnapshot` surfaces only GGUF fields — vLLM metadata is invisible on dashboard

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/models/types.rs:8-60`, `crates/tama-core/src/proxy/status.rs:145-168`
- **Severity:** High
- **Confidence:** High
- **Problem:** `ModelStateSnapshot` (the dashboard SSE struct) has `quant`, `cache_type_k`, and `cache_type_v` fields — all of which are llama.cpp/GGUF-specific columns. It has **no** `vllm_quantization`, `vllm_kv_cache_dtype`, or `vllm_max_model_len` fields. The `VllmConfig` struct (`crates/tama-core/src/config/types/model.rs:278-297`) stores `quantization`, `kv_cache_dtype`, and `max_model_len`, but they are never surfaced to `ModelStateSnapshot`. This means Safetensors models show no quant pill, no KV cache pill, and potentially no context length on the dashboard.
- **Evidence:** `status.rs:145-168` constructs `ModelStateSnapshot` from `model_cfg.quant`, `model_cfg.cache_type_k`, `model_cfg.cache_type_v` — never touches `model_cfg.vllm`. The dashboard `ModelCard` component renders pills only when these fields are `Some`, so Safetensors models get blank pills.
- **Proposal:** Add unified resolution in `collect_model_state_snapshots()` that picks the effective value from whichever source is populated (GGUF column OR vLLM config). Either add new fields to `ModelStateSnapshot` (`effective_quant`, `effective_kv_cache`) or resolve inline before assignment.

#### 2. Common model metadata stored in backend-specific silos instead of unified fields

- **Lens:** Weak Abstractions
- **Files:** `crates/tama-core/src/config/types/model.rs:28-130`, `crates/tama-core/src/db/queries/types.rs:14-60`, `crates/tama-core/src/models/gguf.rs:10-23`
- **Severity:** High
- **Confidence:** High
- **Problem:** The same model metadata concepts exist in different locations depending on backend type:

| Concept | GGUF location | vLLM location | Same DB column? |
|---------|---------------|---------------|-----------------|
| Quantization | `ModelConfig.quant` → `selected_quant` column | `VllmConfig.quantization` → `vllm_config` JSON | **No** |
| KV cache dtype | `ModelConfig.cache_type_k/v` → dedicated columns | `VllmConfig.kv_cache_dtype` → `vllm_config` JSON | **No** |
| Context length | `ModelConfig.context_length` → dedicated column | `VllmConfig.max_model_len` → `vllm_config` JSON | **No** |
| Architecture | `GgufMetadata.architecture` → `hf_architecture_type` column | README heuristics → `hf_architecture_type` column | Yes (but different quality) |

This means every consumer (dashboard, REST API, model card UI, arg builder) must know about both storage locations and implement fallback logic. The `models.rs` frontend page already has `resolve_quant()`, `resolve_cache_k()`, `resolve_cache_v()`, `resolve_context_length()` — ad-hoc resolution functions that duplicate logic.

- **Evidence:** `crates/tama/src/pages/models.rs:110-136` — four separate resolution functions that try top-level field first, then vLLM JSON fallback. `crates/tama/src/pages/dashboard/mod.rs:655-690` — dashboard has NO resolution functions, passing raw fields directly.
- **Proposal:** Introduce a `ModelMetadata` struct that holds the unified view of common fields (quant, kv_cache_dtype, context_length, architecture, num_layers, embedding_length). `ModelConfig` would contain one `metadata: ModelMetadata` instead of scattered fields. Resolution logic lives in one place: `ModelMetadata::resolve(model_cfg)` picks values from whichever source is populated. Both `ModelStateSnapshot` and the REST API response use this single resolution.

### 🟡 Medium Severity

#### 3. No Safetensors or `config.json` parsing — metadata quality gap vs GGUF

- **Lens:** Inconsistent Patterns
- **Files:** `crates/tama-core/src/models/gguf.rs:26` (GGUF parsing exists), no equivalent for Safetensors
- **Severity:** Medium
- **Confidence:** High
- **Problem:** GGUF models get rich, file-verified metadata from `parse_gguf_metadata()` (architecture, context_length, embedding_length, block_count, head_count, quantization, name, nextn_predict_count). Safetensors models get **zero** file parsing — only HF API metadata and README heuristics. This means:
  - Architecture is "Dense" or "MoE" (README heuristic) vs actual "llama", "deepseek2" (GGUF header)
  - Quant is user-configured in `VllmConfig` vs file-verified from GGUF header
  - Context length is from `max_model_len` backend response or README vs file-verified from GGUF header
  - No embedding_length, block_count, head_count at all for Safetensors

Safetensors files have a JSON metadata header (first 8KB), and `config.json` in the model directory has structured metadata (`architectures`, `hidden_size`, `num_hidden_layers`, `max_position_embeddings`, `quantization_config`).

- **Evidence:** `crates/tama-core/src/models/gguf.rs` has `parse_gguf_metadata()`. No `parse_safetensors_metadata()` or `parse_config_json()` exists anywhere in the codebase. `grep -r "config.json" crates/tama-core/` returns no results.
- **Proposal:** Add `parse_transformers_metadata()` that reads `config.json` from the model directory and extracts: architectures, hidden_size, num_hidden_layers, max_position_embeddings, quantization_config. Call it during pull completion (parallel to `parse_gguf_metadata()`) and populate the unified `ModelMetadata`.

#### 4. Dashboard and models page have different resolution paths — inconsistent pill display

- **Lens:** Inconsistent Patterns
- **Files:** `crates/tama/src/pages/dashboard/mod.rs:655-690`, `crates/tama/src/pages/models.rs:310-360`
- **Severity:** Medium
- **Confidence:** High
- **Problem:** The models page (`models.rs`) has resolution functions (`resolve_quant`, `resolve_cache_k`, etc.) that fall back from top-level fields to `vllm` JSON. The dashboard page (`dashboard/mod.rs`) passes raw `ModelStateSnapshot` fields directly with **no** resolution. This means the same model can show different pills on the dashboard vs the models page.

- **Evidence:** `models.rs:110-136` has four resolution functions. `dashboard/mod.rs:665-670` passes `m.quant.clone()`, `m.cache_type_k.clone()` directly.
- **Proposal:** Either (a) fix `ModelStateSnapshot` to include resolved values (recommended — fixes the root cause from Finding #1), or (b) add the same resolution functions to the dashboard page (band-aid).

### 🟢 Low Severity

#### 5. `GgufMetadata` struct fields don't map cleanly to `ModelConfig` — manual field-by-field assignment

- **Lens:** Coupling Issues
- **Files:** `crates/tama-core/src/models/gguf.rs:10-23`, pull completion handler
- **Severity:** Low
- **Confidence:** Medium
- **Problem:** `GgufMetadata` has fields like `embedding_length`, `block_count`, `head_count` that have no corresponding columns in `ModelConfig` or the database. The mapping from `GgufMetadata` to `ModelConfig` is manual, field-by-field, and some GGUF data is silently dropped. This is a symptom of the deeper issue (Finding #2) — without a unified `ModelMetadata` type, each new GGUF field requires a new DB column migration and manual wiring.

- **Proposal:** Addressed by Finding #2's `ModelMetadata` struct. Additional GGUF-only fields can live in a `GgufSpecificMetadata` sub-struct without requiring new DB columns.

## Top Recommendation

**Finding #2 (unified `ModelMetadata` struct)** is the highest-impact change. It addresses the root cause of Findings #1, #4, and #5 simultaneously. The implementation would:

1. Create `ModelMetadata` struct with common fields: `quant`, `kv_cache_dtype`, `context_length`, `architecture`, `num_layers`, `embedding_length`, `head_count`, `block_count`
2. Add `ModelMetadata::resolve(&ModelConfig)` that picks values from GGUF columns OR vLLM JSON (single source of truth)
3. `ModelStateSnapshot` uses resolved values — fixes dashboard pills
4. REST API response uses resolved values — fixes models page (can remove ad-hoc `resolve_*` functions)
5. `ModelCard` component gets consistent data from both paths

Finding #3 (Safetensors/config.json parsing) is the next priority — it populates the unified metadata with actual file data rather than user-configured or heuristic values.
