# Docker Backend Callbacks Fix Plan

**Goal:** Extend the `backend_name` fix from plan-186 to the remaining docker backend callbacks (update, delete, check-updates, build-method) so they use the actual DB key instead of `r#type = "docker"`.

**Architecture:** Same mechanical change as plan-186's Task 1: pass `backend_name` through the card props for the four remaining callbacks, and use it in URL construction instead of `r#type`. The `backend_name` field already exists on both DTOs from plan-186.

**Tech Stack:** Leptos (Rust WASM frontend), axum (backend API)

---

### Task 1: Pass `backend_name` through remaining docker callbacks

**Context:**
Plan-186 fixed `on_default_args_change`, `on_default_env_change`, and `on_version_change` to use `backend_name` instead of `r#type`. Four callbacks remain that still construct URLs from `r#type = "docker"` for docker backends:

- `on_update` — `POST /tama/v1/backends/{bt}/update?gpu_variant={gv}` (Update button)
- `on_delete` — `DELETE /tama/v1/backends/{bt}?gpu_variant={gv}` (Uninstall button)
- `on_check_updates` — `POST /tama/v1/updates/check/backend/{bt}?gpu_variant={gv}` (Check for updates button)
- `on_build_method_change` — `POST /tama/v1/backends/{bt}/source?gpu_variant={gv}` (Build from source toggle)

For docker backends, `bt = "docker"` but the DB key is the actual name (e.g., `"vllm"`). The fix passes `backend_name` through these callbacks and uses it in URL construction.

**Files:**
- Modify: `crates/tama/src/components/backend_card.rs` — Add `backend_name` to four remaining callback invocations
- Modify: `crates/tama/src/pages/backends.rs` — Accept `backend_name` in four remaining callbacks, use in URLs with `url_encode()`

**What to implement:**

1. **`backend_card.rs` — Update callback props and invocations:**
   - Change `on_update` from `Callback<(String, String)>` (backend_type, gpu_variant) to `Callback<(String, String, String)>` (backend_name, backend_type, gpu_variant). The `backend_type` is kept for the display name in error messages.
   - Change `on_delete` from `Callback<(String, String)>` to `Callback<(String, String, String)>` same shape.
   - Change `on_check_updates` from `Callback<(String, String)>` to `Callback<(String, String, String)>` same shape.
   - Change `on_build_method_change` from `Callback<(String, String, bool)>` to `Callback<(String, String, bool)>` — just replace the first element from backend_type to backend_name (same 3-tuple shape).
   - In the card body, use `backend_name.clone()` as the first element for all four callbacks instead of `type_update`, `type_delete`, etc.

2. **`backends.rs` — Update page callbacks:**
   - `on_update_click`: Accept `(backend_name, _backend_type, gpu_variant)`. Use `url_encode(&backend_name)` in the update URL.
   - `on_delete_click`: Accept `(backend_name, _backend_type, gpu_variant)`. Use `url_encode(&backend_name)` in the delete URL.
   - `on_check_updates_click`: Accept `(backend_name, _backend_type, gpu_variant)`. Use `url_encode(&backend_name)` in the check-updates URL.
   - `on_build_method_change`: Already accepts 3-tuple — change first element from backend_type to backend_name. Use `url_encode(&backend_name)` in the source URL.

**Steps:**
- [ ] In `backend_card.rs`, change `on_update`, `on_delete`, `on_check_updates` callback types to include `backend_name` as first element: `Callback<(String, String, String)>` (backend_name, backend_type, gpu_variant)
- [ ] Change `on_build_method_change` first element from backend_type to backend_name (remains 3-tuple with bool): `Callback<(String, String, bool)>` (backend_name, gpu_variant, build_from_source)
- [ ] Update all four callback invocations in the card body to use `backend_name.clone()` as first element
- [ ] In `backends.rs`, update `on_update_click` to accept 3-tuple and use `url_encode(&backend_name)` in URL
- [ ] Update `on_delete_click` same pattern
- [ ] Update `on_check_updates_click` same pattern
- [ ] Update `on_build_method_change` to use backend_name (first element) with `url_encode()`
- [ ] Remove any remaining unused bindings (`type_update`, `type_delete`, `type_check`) from card component
- [ ] Run `cargo check --package tama`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama --features ssr --all-targets -- -D warnings`
- [ ] Run `cargo nextest run --package tama`
- [ ] Commit with message: "fix: use backend_name for docker update/delete/check/source callbacks"

**Acceptance criteria:**
- [ ] All four remaining callbacks (update, delete, check-updates, build-method) use `backend_name` in URL construction
- [ ] Docker backend uninstall/update/check buttons hit the correct DB key (e.g., `/tama/v1/backends/vllm/...`)
- [ ] No unused-variable warnings (old `type_*` bindings removed from card)
- [ ] Native and custom backends continue to work unchanged
- [ ] Code compiles, passes clippy `--all-targets`, and passes all tests
