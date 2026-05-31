# Extended Model Card Pips Plan

**Goal:** Add GPU variant (combined with backend), KV cache quant, and speculative decoding indicator pips to model cards on the dashboard and models pages.

**Architecture:** Four new fields flow from `ModelConfig` (core) → `gpu::ModelStatus` (serialized via SSE metrics stream) → frontend `ModelStatus` → `ModelCard` component. Two new CSS badge classes are added. The models page (`/tama/models`) uses a separate REST API and does not get these new pips (it already lacks architecture/base-model pips).

**Tech Stack:** Rust (tama-core, tama-web), Leptos (WASM), CSS

---

### Task 1: Add new fields to core ModelStatus and populate them

**Context:** The metrics SSE stream serializes `crate::gpu::ModelStatus` for each model. Four fields from `ModelConfig` are not currently included: `gpu_variant`, `cache_type_k`, `cache_type_v`, and `spec_decoding.spec_types`. This task adds them so the frontend can display the new pips.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs` — add 4 fields to `ModelStatus` struct
- Modify: `crates/tama-core/src/proxy/status.rs` — populate new fields in `collect_model_statuses`

**What to implement:**

In `crates/tama-core/src/gpu/system.rs`, add these fields to the `ModelStatus` struct (after `hf_base_model`):

```rust
/// GPU variant for the backend (e.g. "cpu", "cuda", "vulkan"). Display-only.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub gpu_variant: Option<String>,
/// KV cache quant for K head (e.g. "q4_0", "f16"). Display-only.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub cache_type_k: Option<String>,
/// KV cache quant for V head (e.g. "q8_0", "f16"). Display-only.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub cache_type_v: Option<String>,
/// Speculative decoding types (e.g. ["draft-mtp", "ngram-simple"]). Display-only.
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub spec_types: Vec<String>,
```

In `crates/tama-core/src/proxy/status.rs`, in the `collect_model_statuses` method, add these fields to the `out.push(crate::gpu::ModelStatus { ... })` block:

```rust
gpu_variant: model_cfg.gpu_variant.clone(),
cache_type_k: model_cfg.cache_type_k.clone(),
cache_type_v: model_cfg.cache_type_v.clone(),
spec_types: model_cfg.spec_decoding.spec_types.clone(),
```

Also update any tests that construct `crate::gpu::ModelStatus` directly (e.g., in `proxy/status.rs` tests and `proxy/server/mod.rs` tests) to include the new fields with `None` / `vec![]` defaults.

**Steps:**
- [ ] Add 4 new fields to `ModelStatus` struct in `crates/tama-core/src/gpu/system.rs`
- [ ] Populate the 4 new fields in `collect_model_statuses` in `crates/tama-core/src/proxy/status.rs`
- [ ] Fix any tests that construct `ModelStatus` directly — add the new fields with default values
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "feat: add gpu_variant, cache_type_k/v, spec_types to ModelStatus"

**Acceptance criteria:**
- [ ] `crate::gpu::ModelStatus` has the 4 new fields with `#[serde(default)]`
- [ ] `collect_model_statuses` populates all 4 fields from `model_cfg`
- [ ] All tests pass, clippy is clean, code is formatted

---

### Task 2: Add frontend ModelStatus fields and new badge CSS

**Context:** The frontend's `ModelStatus` struct (used for SSE metrics parsing) must mirror the new fields from the backend. Two new CSS badge classes are needed for the KV cache and spec decoding pips.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard/metrics.rs` — add 4 fields to frontend `ModelStatus`
- Modify: `crates/tama-web/css/06-badges-list-card.css` — add 2 new badge classes

**What to implement:**

In `crates/tama-web/src/pages/dashboard/metrics.rs`, add these fields to the `ModelStatus` struct (after `hf_base_model`):

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub gpu_variant: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub cache_type_k: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub cache_type_v: Option<String>,
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub spec_types: Vec<String>,
```

In `crates/tama-web/css/06-badges-list-card.css`, add these classes after `.badge-pill--base-model`:

```css
.badge-pill--kv-cache {
  background: rgba(88, 166, 255, 0.12);
  color: var(--accent-blue);
}

.badge-pill--spec-decoding {
  background: rgba(180, 120, 255, 0.12);
  color: #b478ff;
}
```

**Steps:**
- [ ] Add 4 new fields to `ModelStatus` in `crates/tama-web/src/pages/dashboard/metrics.rs`
- [ ] Add `.badge-pill--kv-cache` and `.badge-pill--spec-decoding` CSS classes in `crates/tama-web/css/06-badges-list-card.css`
- [ ] Run `cargo build --package tama-web`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Commit with message: "feat: add ModelStatus fields and badge CSS for new pips"

**Acceptance criteria:**
- [ ] Frontend `ModelStatus` has 4 new fields matching backend serialization names
- [ ] Two new CSS classes exist with the specified colors
- [ ] `tama-web` compiles without errors

---

### Task 3: Update ModelCard component and wire through dashboard

**Context:** The `ModelCard` component needs 4 new props and logic to render 3 new/modified pips. The dashboard page must pass the new props. The models page uses a different API without these fields — it should NOT be changed (it already doesn't show architecture/base-model pips).

**Files:**
- Modify: `crates/tama-web/src/components/model_card.rs` — add props, helper functions, pip rendering
- Modify: `crates/tama-web/src/pages/dashboard/mod.rs` — pass new props from `ModelStatus`
- Modify: `crates/tama-web/src/pages/models.rs` — pass new props (all `None` / defaults since the REST API doesn't provide them)

**What to implement:**

#### 3a. Add helper functions to `model_card.rs`

Add these two helper functions (before the `ModelCard` component):

```rust
/// Format KV cache quant display: "KV: q4_0/q8_0", "KV: f16/-", etc.
/// Returns None if both cache_type_k and cache_type_v are None.
pub(crate) fn format_kv_cache(k: Option<&str>, v: Option<&str>) -> Option<String> {
    let k_str = k.unwrap_or("-");
    let v_str = v.unwrap_or("-");
    if k.is_none() && v.is_none() {
        None
    } else {
        Some(format!("KV: {}/{}", k_str, v_str))
    }
}

/// Format speculative decoding display: "MTP", "Ngram", or "MTP+Ngram".
/// Returns None if spec_types is empty.
pub(crate) fn format_spec_decoding(spec_types: &[String]) -> Option<String> {
    if spec_types.is_empty() {
        return None;
    }
    let has_mtp = spec_types.iter().any(|s| s == "draft-mtp");
    let has_ngram = spec_types.iter().any(|s| s == "ngram-simple");
    match (has_mtp, has_ngram) {
        (true, true) => Some("MTP+Ngram".to_string()),
        (true, false) => Some("MTP".to_string()),
        (false, true) => Some("Ngram".to_string()),
        (false, false) => None, // Unknown types — don't show anything
    }
}
```

Also add a helper for the backend + GPU variant display:

```rust
/// Format backend display with optional GPU variant: "llama_cpp (cuda)" or "llama_cpp".
pub(crate) fn format_backend_with_variant(backend: &str, gpu_variant: Option<&str>) -> String {
    if let Some(variant) = gpu_variant {
        if !variant.is_empty() {
            return format!("{} ({})", backend, variant);
        }
    }
    backend.to_string()
}
```

#### 3b. Add new props to `ModelCard` component

Add these props to the `#[component]` signature:

```rust
#[prop(default = None)] gpu_variant: Option<String>,
#[prop(default = None)] cache_type_k: Option<String>,
#[prop(default = None)] cache_type_v: Option<String>,
#[prop(default)] spec_types: Vec<String>,
```

> **Note:** `spec_types` uses `#[prop(default)]` (not `#[prop(default = None)]`) because its type is `Vec<String>` which implements `Default` via empty vec. `None` would not compile for a `Vec` type.

#### 3c. Update line2 rendering

In the `line2` section of the view, modify the backend badge and add the new conditional pips. The order should be:

1. Quant (existing, unchanged)
2. Context length (existing, unchanged)
3. Backend + GPU variant (modified — use `format_backend_with_variant`)
4. Architecture type (existing, unchanged)
5. KV Cache (new — conditional, use `format_kv_cache`)
6. Spec Decoding (new — conditional, use `format_spec_decoding`)
7. Base model (existing, unchanged)

Clone the new values for closures — add these 4 lines after the existing `let hf_base_model_clone = ...` line:

```rust
let gpu_variant_clone = gpu_variant.clone();
let cache_type_k_clone = cache_type_k.clone();
let cache_type_v_clone = cache_type_v.clone();
let spec_types_clone = spec_types.clone();
```

The new pips should look like:

```rust
// KV Cache badge
{if let Some(kv_label) = format_kv_cache(cache_type_k_clone.as_deref(), cache_type_v_clone.as_deref()) {
    view! {
        <span class="badge-pill badge-pill--kv-cache">{kv_label}</span>
    }.into_any()
} else {
    view! { <span/> }.into_any()
}}
// Spec Decoding badge
{if let Some(spec_label) = format_spec_decoding(&spec_types_clone) {
    view! {
        <span class="badge-pill badge-pill--spec-decoding">{spec_label}</span>
    }.into_any()
} else {
    view! { <span/> }.into_any()
}}
```

The backend badge should change from:
```rust
<span class="badge-pill badge-pill--backend">{backend_clone}</span>
```
to:
```rust
<span class="badge-pill badge-pill--backend">{format_backend_with_variant(&backend_clone, gpu_variant_clone.as_deref())}</span>
```

#### 3d. Wire through dashboard

In `crates/tama-web/src/pages/dashboard/mod.rs`, update both `ModelCard` usages (active and inactive sections) to pass the new props. For each `<ModelCard ... />`, add:

```rust
gpu_variant=m.gpu_variant.clone()
cache_type_k=m.cache_type_k.clone()
cache_type_v=m.cache_type_v.clone()
spec_types=m.spec_types.clone()
```

#### 3e. Wire through models page

In `crates/tama-web/src/pages/models.rs`, the models page uses a different REST API response type that doesn't have these fields. Pass defaults so the component compiles:

```rust
gpu_variant=None
cache_type_k=None
cache_type_v=None
spec_types=vec![]
```

Do NOT modify the models page's API types or response — just pass defaults.

#### 3f. Add tests

Add unit tests for the three helper functions:

```rust
#[test]
fn test_format_kv_cache_both_set() {
    assert_eq!(format_kv_cache(Some("q4_0"), Some("q8_0")), Some("KV: q4_0/q8_0".to_string()));
}

#[test]
fn test_format_kv_cache_only_k_set() {
    assert_eq!(format_kv_cache(Some("f16"), None), Some("KV: f16/-".to_string()));
}

#[test]
fn test_format_kv_cache_only_v_set() {
    assert_eq!(format_kv_cache(None, Some("q8_0")), Some("KV: -/q8_0".to_string()));
}

#[test]
fn test_format_kv_cache_neither_set() {
    assert_eq!(format_kv_cache(None, None), None);
}

#[test]
fn test_format_spec_decoding_mtp_only() {
    assert_eq!(format_spec_decoding(&["draft-mtp".to_string()]), Some("MTP".to_string()));
}

#[test]
fn test_format_spec_decoding_ngram_only() {
    assert_eq!(format_spec_decoding(&["ngram-simple".to_string()]), Some("Ngram".to_string()));
}

#[test]
fn test_format_spec_decoding_both() {
    assert_eq!(format_spec_decoding(&["draft-mtp".to_string(), "ngram-simple".to_string()]), Some("MTP+Ngram".to_string()));
}

#[test]
fn test_format_spec_decoding_empty() {
    assert_eq!(format_spec_decoding(&[]), None);
}

#[test]
fn test_format_backend_with_variant() {
    assert_eq!(format_backend_with_variant("llama.cpp", Some("cuda")), "llama.cpp (cuda)");
    assert_eq!(format_backend_with_variant("llama.cpp", Some("")), "llama.cpp");
    assert_eq!(format_backend_with_variant("llama.cpp", None), "llama.cpp");
}
```

**Steps:**
- [ ] Add `format_kv_cache`, `format_spec_decoding`, `format_backend_with_variant` helper functions to `model_card.rs`
- [ ] Add unit tests for the three helper functions
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Add 4 new props to `ModelCard` component
- [ ] Update line2 rendering: modify backend badge, add KV cache and spec decoding pips in correct order
- [ ] Wire new props through dashboard's active and inactive `ModelCard` usages
- [ ] Wire default values through models page's `ModelCard` usage
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix failures and re-run.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix warnings and re-run.
- [ ] Commit with message: "feat: render GPU variant, KV cache, and spec decoding pips on model cards"

**Acceptance criteria:**
- [ ] Backend pip shows `backend_name (variant)` format when variant is set
- [ ] KV cache pip shows `KV: k/v` format (with `-` for unset), only when at least one is set
- [ ] Spec decoding pip shows `MTP`, `Ngram`, or `MTP+Ngram`, only when types are recognized and non-empty
- [ ] Pip order is: Quant → Context → Backend(variant) → Architecture → KV Cache → Spec → Base Model
- [ ] Dashboard passes real data; models page passes defaults
- [ ] All tests pass, clippy is clean, code is formatted

---

## Summary

| Task | Files | Description |
|------|-------|-------------|
| 1 | `gpu/system.rs`, `proxy/status.rs` | Add fields to core `ModelStatus` and populate |
| 2 | `dashboard/metrics.rs`, `css/06-badges-list-card.css` | Frontend types + CSS |
| 3 | `model_card.rs`, `dashboard/mod.rs`, `models.rs` | Component + wiring |

**Total estimated tasks:** 3 (each independently commitable)
