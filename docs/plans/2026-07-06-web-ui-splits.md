# Web UI File Splits Plan

**Goal:** Split 3 large web UI files in the `tama` crate into focused modules.

**Architecture:** Three independent tasks, each splitting one file. Task 1 splits `types/config.rs` (WASM mirror types). Task 2 splits `api/backends/manage.rs` (5 backend handlers). Task 3 splits `pages/config_editor.rs` (mirror types + 5 forms + page).

**Tech Stack:** Rust, Leptos (WASM + SSR)

---

### Task 1: Split `tama/src/types/config.rs` (986 lines) into modules

**Context:**
WASM mirror of `tama-core/src/config/types.rs`. Intentional duplication for WASM compatibility (BTreeMap vs HashMap). Should mirror the core types split (Task 1 of file-splits plan).

**Files:**
- Create: `crates/tama/src/types/config/general.rs`
- Create: `crates/tama/src/types/config/proxy.rs`
- Create: `crates/tama/src/types/config/model.rs`
- Create: `crates/tama/src/types/config/backend.rs`
- Create: `crates/tama/src/types/config/supervisor.rs`
- Create: `crates/tama/src/types/config/compaction.rs`
- Create: `crates/tama/src/types/config/sampling.rs`
- Create: `crates/tama/src/types/config/mod.rs`
- Delete: `crates/tama/src/types/config.rs`

**What to implement:**

Mirror the core types split structure. Each submodule contains the WASM-compatible version of the corresponding core type (using `BTreeMap` instead of `HashMap`, `wasm_bindgen` compatible types, etc.).

1. **`general.rs`**: `General` struct (WASM version).
2. **`proxy.rs`**: `ProxyConfig` struct (WASM version).
3. **`model.rs`**: `ModelConfig`, `QuantEntry`, `QuantKind`, `SpecDecodingConfig` (WASM versions).
4. **`backend.rs`**: `BackendConfig` struct (WASM version).
5. **`supervisor.rs`**: `Supervisor` struct (WASM version).
6. **`compaction.rs`**: `CompactionConfig` struct (WASM version).
7. **`sampling.rs`**: `SamplingParams` struct (WASM version).
8. **`mod.rs`**: `Config` struct (WASM version) + re-exports.

**Steps:**
- [ ] Read `types/config.rs` to map which lines belong to which submodule
- [ ] Create `types/config/` directory with `mod.rs`
- [ ] Create each submodule with the corresponding WASM types
- [ ] Create `mod.rs` with `Config` + re-exports
- [ ] Delete old `types/config.rs`
- [ ] Update `types/mod.rs` if needed
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "refactor: split tama/src/types/config.rs into focused modules"

**Acceptance criteria:**
- [ ] No single file exceeds 300 lines
- [ ] Structure mirrors `tama-core/src/config/types/` split
- [ ] All public types re-exported from `mod.rs`
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 2: Split `api/backends/manage.rs` (1,013 lines) into modules

**Context:**
Five handlers (update_backend, remove_backend_version, activate_backend_version, update_backend_default_args, update_backend_source) plus request types and ~150 lines of tests. All follow the same pattern: validate → open manager → resolve variant → do action → return response.

**Files:**
- Create: `crates/tama/src/api/backends/manage/update.rs`
- Create: `crates/tama/src/api/backends/manage/remove.rs`
- Create: `crates/tama/src/api/backends/manage/activate.rs`
- Create: `crates/tama/src/api/backends/manage/config.rs`
- Create: `crates/tama/src/api/backends/manage/types.rs`
- Create: `crates/tama/src/api/backends/manage/tests.rs`
- Create: `crates/tama/src/api/backends/manage/mod.rs`
- Delete: `crates/tama/src/api/backends/manage.rs`

**What to implement:**

1. **`types.rs`**: Shared request/response types (`UpdateQuery`, `RemoveVersionQuery`, `ActivateQuery`, `DefaultArgsQuery`, `SourceQuery`, `UpdateRequest`, `ActivateRequest`).

2. **`update.rs`**: `update_backend` handler (~120 lines).

3. **`remove.rs`**: `remove_backend_version` handler (~100 lines).

4. **`activate.rs`**: `activate_backend_version` handler (~80 lines).

5. **`config.rs`**: `update_backend_default_args` (~30 lines) + `update_backend_source` (~80 lines).

6. **`tests.rs`**: All existing tests (~150 lines).

7. **`mod.rs`**: Re-exports + route registration.

**Steps:**
- [ ] Read `manage.rs` to map which lines belong to which submodule
- [ ] Create `manage/` directory with `mod.rs`
- [ ] Create `types.rs` with shared request/response types
- [ ] Create `update.rs` with update_backend handler
- [ ] Create `remove.rs` with remove_backend_version handler
- [ ] Create `activate.rs` with activate_backend_version handler
- [ ] Create `config.rs` with default_args and source handlers
- [ ] Create `tests.rs` with all tests
- [ ] Create `mod.rs` with re-exports + route registration
- [ ] Delete old `manage.rs`
- [ ] Update `backends/mod.rs` if needed
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "refactor: split api/backends/manage.rs into update, remove, activate, config, types"

**Acceptance criteria:**
- [ ] No single file exceeds 200 lines
- [ ] Shared types in `types.rs`, each handler in its own file
- [ ] Route registration preserved in `mod.rs`
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass

---

### Task 3: Split `pages/config_editor.rs` (937 lines) into modules

**Context:**
Mirror types (~200 lines), main ConfigEditor component (~80 lines), and 5 form components (~600 lines) all in one file.

**Files:**
- Create: `crates/tama/src/pages/config_editor/types.rs`
- Create: `crates/tama/src/pages/config_editor/forms/general.rs`
- Create: `crates/tama/src/pages/config_editor/forms/proxy.rs`
- Create: `crates/tama/src/pages/config_editor/forms/supervisor.rs`
- Create: `crates/tama/src/pages/config_editor/forms/sampling.rs`
- Create: `crates/tama/src/pages/config_editor/forms/compaction.rs`
- Create: `crates/tama/src/pages/config_editor/mod.rs`
- Delete: `crates/tama/src/pages/config_editor.rs`

**What to implement:**

1. **`types.rs`**: Mirror types for the config editor (~200 lines).

2. **`forms/general.rs`**: `GeneralForm` component.

3. **`forms/proxy.rs`**: `ProxyForm` component.

4. **`forms/supervisor.rs`**: `SupervisorForm` component.

5. **`forms/sampling.rs`**: `SamplingForm` component.

6. **`forms/compaction.rs`**: `CompactionForm` component.

7. **`mod.rs`**: Main `ConfigEditor` component + re-exports.

**Steps:**
- [ ] Read `config_editor.rs` to map which lines belong to which submodule
- [ ] Create `config_editor/` directory with `mod.rs`
- [ ] Create `types.rs` with mirror types
- [ ] Create `forms/` directory
- [ ] Create `forms/general.rs` with GeneralForm
- [ ] Create `forms/proxy.rs` with ProxyForm
- [ ] Create `forms/supervisor.rs` with SupervisorForm
- [ ] Create `forms/sampling.rs` with SamplingForm
- [ ] Create `forms/compaction.rs` with CompactionForm
- [ ] Create `mod.rs` with main ConfigEditor component + re-exports
- [ ] Delete old `config_editor.rs`
- [ ] Update `pages/mod.rs` if needed
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compilation errors and re-run
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "refactor: split pages/config_editor.rs into types, forms, and main component"

**Acceptance criteria:**
- [ ] No single file exceeds 200 lines
- [ ] Mirror types in `types.rs`, each form in its own file
- [ ] Main ConfigEditor component in `mod.rs`
- [ ] `cargo build --workspace` succeeds with no warnings
- [ ] All tests pass
