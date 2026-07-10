# Fix PUT /tama/v1/models/:id Wiping Optional Fields Plan

**Goal:** Fix `apply_model_body()` so `context_length`, `cache_type_k`, and `cache_type_v` preserve existing DB values when omitted from the PUT body, matching the documented partial-update semantics.

**Architecture:** Add `.or(base.field)` merge fallback to three fields in `apply_model_body()`, consistent with the pattern used by all other optional fields (e.g., `gpu_variant`, `num_parallel`, `api_name`). Add 6 tests covering preservation and override scenarios.

**Tech Stack:** Rust, existing `tama` crate tests

---

### Task 1: Fix the three fields in `apply_model_body()` and add tests

**Context:**
The `apply_model_body()` function in `crates/tama/src/api/models/crud/mod.rs` merges a `ModelBody` (from the PUT request) with an existing `ModelConfig` (from the DB). For most optional fields, the pattern is `body.field.or(base.field)` — if the body sends `None`, the existing value is preserved.

Three fields violate this pattern and use direct assignment, wiping the existing value to `None` whenever the body omits them:

1. **`context_length`** (line 116): `context_length: body.context_length,` — no `.or(base.context_length)`
2. **`cache_type_k`** (lines 154-157): `.map().filter()` chain with no `.or(base.cache_type_k)` fallback
3. **`cache_type_v`** (lines 158-161): same pattern as cache_type_k

The API docs (`docs/api/models.md`) state: "PUT /tama/v1/models/:id — Update an existing model. **Partial update — only provided fields change.**" This fix makes the code match the documented contract.

**Files:**
- Modify: `crates/tama/src/api/models/crud/mod.rs` (3 lines in `apply_model_body()`)
- Test: `crates/tama/src/api/models/crud/tests.rs` (6 new tests)

**What to implement:**

In `crates/tama/src/api/models/crud/mod.rs`, inside the `apply_model_body()` function, make the following three changes:

1. **Line 116** — `context_length`: Change from:
   ```rust
   context_length: body.context_length,
   ```
   To:
   ```rust
   context_length: body.context_length.or(base.context_length),
   ```

2. **Lines 154-157** — `cache_type_k`: Change from:
   ```rust
   cache_type_k: body
       .cache_type_k
       .map(|s| s.trim().to_string())
       .filter(|s| !s.is_empty() && s != "__custom"),
   ```
   To (add `.or(base.cache_type_k)` at the end):
   ```rust
   cache_type_k: body
       .cache_type_k
       .map(|s| s.trim().to_string())
       .filter(|s| !s.is_empty() && s != "__custom")
       .or(base.cache_type_k),
   ```

3. **Lines 158-161** — `cache_type_v`: Change from:
   ```rust
   cache_type_v: body
       .cache_type_v
       .map(|s| s.trim().to_string())
       .filter(|s| !s.is_empty() && s != "__custom"),
   ```
   To (add `.or(base.cache_type_v)` at the end):
   ```rust
   cache_type_v: body
       .cache_type_v
       .map(|s| s.trim().to_string())
       .filter(|s| !s.is_empty() && s != "__custom")
       .or(base.cache_type_v),
   ```

**Do NOT change:** `args`, `sampling`, or any other fields. Those are out of scope for this fix.

In `crates/tama/src/api/models/crud/tests.rs`, add the following 6 new tests at the bottom of the file (after the existing tests):

**Test 1: `test_apply_model_body_context_length_preserves_base_when_omitted`**
- Create a `ModelConfig` with `context_length: Some(4096)` and all other fields matching the existing test helpers (use `existing_with_size()` helper or construct inline)
- Create a `ModelBody` with `context_length: None` and `backend: "llama-cpp".to_string()` (required field)
- Call `apply_model_body(body, Some(existing))`
- Assert `result.context_length == Some(4096)`

**Test 2: `test_apply_model_body_cache_type_k_preserves_base_when_omitted`**
- Create a `ModelConfig` with `cache_type_k: Some("q4_0".to_string())`
- Create a `ModelBody` with `cache_type_k: None`
- Call `apply_model_body(body, Some(existing))`
- Assert `result.cache_type_k == Some("q4_0".to_string())`

**Test 3: `test_apply_model_body_cache_type_v_preserves_base_when_omitted`**
- Create a `ModelConfig` with `cache_type_v: Some("q8_0".to_string())`
- Create a `ModelBody` with `cache_type_v: None`
- Call `apply_model_body(body, Some(existing))`
- Assert `result.cache_type_v == Some("q8_0".to_string())`

**Test 4: `test_apply_model_body_cache_type_k_whitespace_preserves_base_when_existing`**
- Create a `ModelConfig` with `cache_type_k: Some("q4_0".to_string())`
- Create a `ModelBody` with `cache_type_k: Some("   ".to_string())` (whitespace-only)
- Call `apply_model_body(body, Some(existing))`
- Assert `result.cache_type_k == Some("q4_0".to_string())` (whitespace filtered to None, then falls back to base — this is the new behavior)

**Test 5: `test_apply_model_body_cache_type_v_whitespace_preserves_base_when_existing`**
- Same as Test 4 but for `cache_type_v` with base `Some("q8_0".to_string())`
- Assert `result.cache_type_v == Some("q8_0".to_string())`

**Test 6: `test_apply_model_body_context_length_body_wins_over_base`**
- Create a `ModelConfig` with `context_length: Some(4096)`
- Create a `ModelBody` with `context_length: Some(8192)`
- Call `apply_model_body(body, Some(existing))`
- Assert `result.context_length == Some(8192)` (body value wins when explicitly provided)

For all tests, use the same pattern as existing tests in the file: construct `ModelBody` and `ModelConfig` inline with all required fields. Use `existing_with_size()` helper where applicable to reduce boilerplate.

**Steps:**
- [ ] Write the 6 failing tests in `crates/tama/src/api/models/crud/tests.rs`
- [ ] Run `cargo nextest run --package tama -- api::models::crud::tests`
  - Tests 1-5 should FAIL (the bug causes them to assert `Some(...)` but get `None`)
  - Test 6 should PASS (body value already wins with direct assignment)
  - If tests 1-5 pass unexpectedly, stop and investigate — the bug may already be fixed
- [ ] Apply the 3 fixes in `crates/tama/src/api/models/crud/mod.rs` (the `.or(base.field)` additions)
- [ ] Run `cargo nextest run --package tama -- api::models::crud::tests`
  - All tests (existing + new) must pass
  - Existing tests pass `base = None`, so `.or(base.field)` is `.or(None)` = no-op for them
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama -- -D warnings`
  - Fix any clippy warnings
- [ ] Run `cargo nextest run --package tama` (full package test suite)
  - Ensure no regressions in other modules
- [ ] Commit with message: "fix: preserve context_length/cache_type_k/cache_type_v on partial PUT update"

**Acceptance criteria:**
- [ ] `context_length` preserves existing DB value when body sends `None`
- [ ] `cache_type_k` preserves existing DB value when body sends `None`
- [ ] `cache_type_v` preserves existing DB value when body sends `None`
- [ ] `cache_type_k` preserves existing DB value when body sends whitespace-only (new behavior)
- [ ] `cache_type_v` preserves existing DB value when body sends whitespace-only (new behavior)
- [ ] Explicit body values still override base values for all three fields
- [ ] All existing tests pass (no regressions)
- [ ] `cargo clippy --package tama -- -D warnings` passes clean

---

## Out of Scope

- `args` field — defaults to `vec![]` (not `Option`), requires different merge strategy (e.g., `body.args.if_empty().then(|| base.args)`)
- `sampling` field — same direct-assignment pattern, should be addressed in a follow-up
- `PATCH` endpoint — the backlog item for true PATCH is a separate, larger feature
