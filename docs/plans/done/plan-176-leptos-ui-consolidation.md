# Leptos UI Consolidation Plan

**Goal:** Single-source the mirrored core types and quant inference between `tama-core` and the Leptos WASM frontend, and eliminate the ~350 lines of copy-pasted DOM helpers, request helpers, and benchmark-form boilerplate in `crates/tama`.

**Architecture:** `tama-core` cannot compile to wasm32 (unconditional `rusqlite`/`sysinfo`/`tokio` deps; it is an optional ssr-only dep of `tama`, and ADR-0010 forbids a fourth crate), so pure types are single-sourced at the **file level**: a new dependency-free `tama_core::types` leaf module (serde+std only) is included into the `tama` crate via `#[path]` attributes for csr builds, while ssr builds re-export the identical types from `tama_core::types` — so on ssr the UI and server share ONE type, and on csr the WASM bundle compiles the SAME source file. On top of that: `gpu_types.rs` becomes pure re-exports, `config_editor/types.rs` collapses onto `types/config`, the pull wizard's drifted csr quant table is deleted, the 5 model-editor forms share `crate::utils` DOM helpers, and the 3 benchmark forms share a form-state module. Audit findings F29 + F31 (`docs/reviews/2026-07-18-codebase-improvement.md` #29, #31).

**Tech Stack:** Rust, Leptos 0.7 (csr/ssr feature split), gloo-net, wasm-bindgen, Trunk (wasm32-unknown-unknown)

---

### Task 1: Create the `tama_core::types` pure leaf module

**Context:**
The types the WASM frontend mirrors live scattered inside impure modules: `RestartPolicy`/`LogLevel`/`CompactionDevice` in `config/types/enums.rs`, `GpuVendor`/`ModelState` in `gpu/types.rs:11,39`, `QuantKind`/`QuantEntry` in `config/types/model.rs:15,41`, and `infer_quant_from_filename` in `models/pull/quant.rs:3`. Every one of these is pure serde+std code EXCEPT `impl From<LogLevel> for tracing::Level` (`config/types/enums.rs:101-112`), which stays behind. Decisions: the new module is `crates/tama-core/src/types/` (the name is free — `lib.rs` has no top-level `types`); old import paths keep working via re-exports, so ZERO callers change in this task; the ~18 quant tests move with the function. `QuantKind::from_filename` and `ModelState::from_str_fallback` move with their types. COORDINATION: plan-172 may delete `ModelState::from_str_fallback` (skip moving it if already gone); plan-173 renames `ModelState::Loading`→`Starting` — whichever plan lands second adapts (if 173 first, move the renamed variant; if 176 first, 173 edits `types/gpu.rs` instead of `gpu/types.rs`).

**Files:**
- Create: `crates/tama-core/src/types/mod.rs`
- Create: `crates/tama-core/src/types/enums.rs`
- Create: `crates/tama-core/src/types/gpu.rs`
- Create: `crates/tama-core/src/types/quant.rs`
- Modify: `crates/tama-core/src/lib.rs` (add `pub mod types;` after :12)
- Modify: `crates/tama-core/src/config/types/enums.rs` (delete moved items, keep tracing impl, re-export)
- Modify: `crates/tama-core/src/gpu/types.rs` (delete moved enums, re-export)
- Modify: `crates/tama-core/src/config/types/model.rs` (delete `QuantKind`/`QuantEntry`, re-export)
- Modify: `crates/tama-core/src/models/pull/quant.rs` (delete function + tests, re-export)

**What to implement:**

1. `types/enums.rs` — move `RestartPolicy` (incl. `as_str`, inherent `from_str -> Option<Self>`, `impl FromStr`), `LogLevel` (same shape), and `CompactionDevice` (incl. custom `Serialize`/`Deserialize`, `as_str`, `from_str -> Option<Self>`, `impl FromStr`) verbatim from `config/types/enums.rs`, including their `#[cfg(test)] mod tests` (all 20 tests). Allowed imports ONLY: `serde::{Deserialize, Serialize}`, `std::str::FromStr`. Do NOT move `impl From<LogLevel> for tracing::Level`.
2. `types/gpu.rs` — move `GpuVendor` (:11-34, incl. `as_str`, `try_from_str`) and `ModelState` (:39-78, incl. `as_str`, `from_str_fallback`) verbatim from `gpu/types.rs`, with any inline tests. Imports: serde only. Leave `VramInfo`, `ModelStatus`, `SystemMetrics`, etc. in `gpu/types.rs`.
3. `types/quant.rs` — move `QuantKind` + `QuantEntry` (`config/types/model.rs:7-52`, incl. `QuantKind::from_filename`) and `infer_quant_from_filename` + its full test module (`models/pull/quant.rs`, ~290 lines total). Imports: serde only (the function is std-only).
4. `types/mod.rs`:
   ```rust
   //! Pure, dependency-free types shared between the server and the WASM frontend.
   //!
   //! Everything in this module must compile on `wasm32-unknown-unknown` with only
   //! `serde` and `std` — no tokio, rusqlite, axum, reqwest, sysinfo, or tracing.
   //! The `tama` crate includes these exact files via `#[path]` for csr builds
   //! (see `crates/tama/src/core_shared.rs`), so adding a non-wasm dependency here
   //! breaks the frontend build. Keep it pure.
   pub mod enums;
   pub mod gpu;
   pub mod quant;
   ```
5. Re-export shims (preserve every existing path):
   - `config/types/enums.rs` → body becomes `pub use crate::types::enums::{CompactionDevice, LogLevel, RestartPolicy};` PLUS the retained `impl From<LogLevel> for tracing::Level` block.
   - `gpu/types.rs` → add `pub use crate::types::gpu::{GpuVendor, ModelState};` (check `gpu/mod.rs:20` `pub use types::{...}` still resolves — it re-exports from `gpu::types`, which now re-exports; no change needed there).
   - `config/types/model.rs` → delete the two types, add `pub use crate::types::quant::{QuantEntry, QuantKind};` (`config/mod.rs:17` re-exports via `config::types`, so `tama_core::config::{QuantKind, QuantEntry}` keeps working — verify the re-export chain compiles).
   - `models/pull/quant.rs` → body becomes `pub use crate::types::quant::infer_quant_from_filename;` (paths `tama_core::models::infer_quant_from_filename` via `models/mod.rs:12` and `crate::models::pull::quant::infer_quant_from_filename` via `pull/mod.rs:518` keep working).
6. Verify purity: `grep -n "^use " crates/tama-core/src/types/*.rs` shows only `serde`/`std` imports.

**Steps:**
- [ ] Run `cargo nextest run --package tama-core -- config::types` and `cargo nextest run --package tama-core -- models::pull::quant` — confirm green baseline (20 enum tests + ~18 quant tests)
- [ ] Create the four `types/` files per above; apply the four re-export shims; add `pub mod types;` to `lib.rs`
- [ ] Run `cargo check --package tama-core` — compiles (fix only import mistakes; the moved tests' `use super::*;` keeps working)
- [ ] Run `cargo nextest run --package tama-core` — all pass with the same total test count as baseline
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: add pure tama_core::types leaf module for wasm-shared types"

**Acceptance criteria:**
- [ ] `crates/tama-core/src/types/{mod,enums,gpu,quant}.rs` exist and import only serde/std
- [ ] `tama_core::config::{LogLevel, RestartPolicy, CompactionDevice, QuantKind, QuantEntry}`, `tama_core::gpu::{GpuVendor, ModelState}`, and `tama_core::models::infer_quant_from_filename` all still resolve — no caller edits anywhere in the workspace
- [ ] Same test count as baseline (tests moved, none deleted)
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 2: Add `crate::core_shared` to the `tama` crate (dual csr/ssr mechanism)

**Context:**
The `tama` crate builds two ways from the same source: ssr (`default = ["ssr"]`, has `tama-core`) and csr (`trunk build --no-default-features --features csr`, wasm32, NO `tama-core`). To give both builds the same types we use a cfg-dual module: on ssr, `core_shared` re-exports `tama_core::types::*` (literally the server's types — no conversion code needed); on csr, `core_shared` includes the SAME source files via `#[path]` (they are pure after Task 1). This is the ADR-0010-safe mechanism — no new crate, no new dependency. Decisions: one flat re-export surface (`crate::core_shared::{LogLevel, …}`) so callers don't care which branch is active; the wasm32 compile check becomes a mandatory step (Makefile already installs the target, `Makefile:13`).

**Files:**
- Create: `crates/tama/src/core_shared.rs`
- Modify: `crates/tama/src/lib.rs` (add `pub mod core_shared;` next to :50, NOT cfg-gated)

**What to implement:**

`crates/tama/src/core_shared.rs`, exactly this shape:
```rust
//! Types shared with `tama-core`, compiled into BOTH csr and ssr builds.
//!
//! On ssr these are re-exports of `tama_core::types` — the same types the
//! server uses, so no conversion code exists on the server boundary.
//! On csr the identical source files are included via `#[path]` (they are
//! pure serde+std, see `crates/tama-core/src/types/mod.rs`), giving the WASM
//! bundle structurally identical types without depending on tama-core.
//!
//! DO NOT add types here. Shared types live in `tama_core::types`; this
//! module only re-exports/includes them.

#[cfg(feature = "ssr")]
mod imp {
    pub use tama_core::types::enums::{CompactionDevice, LogLevel, RestartPolicy};
    pub use tama_core::types::gpu::{GpuVendor, ModelState};
    pub use tama_core::types::quant::{infer_quant_from_filename, QuantEntry, QuantKind};
}

#[cfg(not(feature = "ssr"))]
mod imp {
    #[path = "../../tama-core/src/types/enums.rs"]
    mod enums;
    #[path = "../../tama-core/src/types/gpu.rs"]
    mod gpu;
    #[path = "../../tama-core/src/types/quant.rs"]
    mod quant;

    pub use enums::{CompactionDevice, LogLevel, RestartPolicy};
    pub use gpu::{GpuVendor, ModelState};
    pub use quant::{infer_quant_from_filename, QuantEntry, QuantKind};
}

pub use imp::*;
```
`#[path]` notes for the executing agent: paths are relative to the directory of the FILE containing the inline `mod imp { }` (`crates/tama/src/`), so `../../tama-core/src/types/enums.rs` is correct. The included files' `#[cfg(test)] mod tests` never compile in csr (no wasm test runner) — leave them alone.

**Steps:**
- [ ] Create `core_shared.rs`; register `pub mod core_shared;` in `lib.rs`
- [ ] Run `cargo check --package tama` (ssr branch) — compiles
- [ ] Run `cargo check --package tama --target wasm32-unknown-unknown --no-default-features --features csr` (csr branch) — compiles; if the target is missing run `rustup target add wasm32-unknown-unknown` first
- [ ] Run `cargo nextest run --package tama` — all pass (nothing uses the module yet; this proves no regression)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean (if clippy flags the `imp` module as unused on either branch, add `#[allow(unused_imports)]` to the `pub use imp::*;` line ONLY as a last resort — first check it is genuinely unused-this-task and remove the allow in Task 3)
- [ ] Commit with message: "feat: add crate::core_shared dual csr/ssr type bridge to tama_core::types"

**Acceptance criteria:**
- [ ] Both `cargo check --package tama` (ssr) and the wasm32 csr check compile
- [ ] `crate::core_shared::{CompactionDevice, GpuVendor, LogLevel, ModelState, QuantEntry, QuantKind, RestartPolicy, infer_quant_from_filename}` resolve on BOTH feature sets
- [ ] `cargo nextest run --package tama` passes

---

### Task 3: Rewrite `gpu_types.rs` as re-exports; adopt Option-returning `from_str`

**Context:**
`crates/tama/src/gpu_types.rs` (165 lines) hand-mirrors `GpuVendor`, `ModelState`, `LogLevel`, `RestartPolicy`, `CompactionDevice` with a semantic drift: its `from_str` returns `Self` (default-on-unknown) where core returns `Option<Self>`, and its `CompactionDevice::Deserialize` silently maps unknown strings to `Cpu` where core errors. Decision (per audit): adopt the CORE semantics — `from_str -> Option<Self>`, strict `Deserialize`. The module body becomes re-exports from `core_shared`, so `use crate::gpu_types::{ModelState, …}` keeps working at all 9 import sites UNCHANGED except the three `from_str` callers, which gain `.unwrap_or_default()` to preserve current behavior (unknown → default; the `<select>`s only emit known values, so this is unreachable in practice). COORDINATION with plan-173 Task 6, which renames `gpu_types.rs` → `core_mirrors.rs`: whichever lands second adapts (if 173 first, this task edits `core_mirrors.rs`; if 176 first, 173's `git mv` moves the rewritten file and updates the same import sites).

**Files:**
- Modify: `crates/tama/src/gpu_types.rs` (replace body with re-exports)
- Modify: `crates/tama/src/pages/config_editor/forms/compaction.rs:42`
- Modify: `crates/tama/src/pages/config_editor/forms/general.rs:24`
- Modify: `crates/tama/src/pages/config_editor/forms/supervisor.rs:25`

**What to implement:**

1. `gpu_types.rs` new body:
   ```rust
   //! Mirror types from tama-core that can be used from WASM.
   //!
   //! These are re-exports of `crate::core_shared` (which bridges to
   //! `tama_core::types` on ssr and includes the same source files on csr).
   //! Kept as a stable module name so existing `crate::gpu_types::*` imports
   //! keep working; plan-173 renames this module to `core_mirrors`.
   pub use crate::core_shared::{CompactionDevice, GpuVendor, LogLevel, ModelState, RestartPolicy};
   ```
   Delete everything else (all 5 mirror enums and their impls).
2. Fix the three from_str call sites to Option semantics:
   - `forms/compaction.rs:42`: `c.compaction.device = CoreCompactionDevice::from_str(&v).unwrap_or_default();`
   - `forms/general.rs:24`: `c.general.log_level = CoreLogLevel::from_str(&v).unwrap_or_default();`
   - `forms/supervisor.rs:25`: `c.supervisor.restart_policy = CoreRestartPolicy::from_str(&v).unwrap_or_default();`
3. Grep for any other user of the deleted mirror-only behaviors: `rg "gpu_types::" crates/tama/src` — every hit must still compile (the re-exported types are a superset of the old mirrors: core adds `try_from_str`/`from_str_fallback`/`FromStr`, removes nothing except the lossy `from_str` signature). Note the intentional strictness change in the commit message: an unknown `CompactionDevice`/`ModelState` string from the server now fails deserialization instead of silently becoming `Cpu`/`Idle` (server only writes valid values).

**Steps:**
- [ ] Run `cargo nextest run --package tama` — green baseline
- [ ] Rewrite `gpu_types.rs`; fix the 3 call sites
- [ ] Run `cargo check --package tama` AND `cargo check --package tama --target wasm32-unknown-unknown --no-default-features --features csr` — both compile
- [ ] Run `cargo nextest run --package tama` — all pass (dashboard tests at `pages/dashboard/tests.rs` exercise `ModelState`)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: gpu_types.rs re-exports core_shared; from_str uses core Option semantics"

**Acceptance criteria:**
- [ ] `gpu_types.rs` is ≤ 15 lines and defines zero types
- [ ] Both feature-set checks compile; `cargo nextest run --package tama` passes
- [ ] `rg "fn from_str" crates/tama/src/gpu_types.rs` — zero hits (no mirror impls remain)

---

### Task 4: Collapse the pull wizard's csr quant inference + local `QuantKind`

**Context:**
`crates/tama/src/components/pull_wizard/mod.rs` carries: (a) a local `QuantKind` mirror (:9-17), and (b) a `#[cfg(not(feature = "ssr"))]` re-implementation of `infer_quant_from_filename` (:128-195) that has ALREADY DRIFTED from core: it lacks ~30 patterns core has (`UD-Q*_K_XL`, `UD-Q4_0/4_1/5_0/5_1/6_0/8_1`, standard `Q*_K_XL`, `Q2_K`–`Q6_0`, `Q8_1`, `F16`, `F32`, `BF16`), lacks the separator-aware matching core added (`-` `.` `_` prefixes prevent false matches like `XQ4_K_M`), and returns `None` where core falls back to the last `-`/`_` component. Decision: delete the csr copy and the cfg-dual fn entirely; both builds call `crate::core_shared::infer_quant_from_filename` (same source on both sides after Task 2). This is a deliberate behavior fix for the csr build, not just a dedup — call it out in the commit message. The wizard's API-DTO `QuantEntry` (:41-48, shape `{filename, quant, size_bytes, kind}` — NOT the config `QuantEntry`) stays untouched.

**Files:**
- Modify: `crates/tama/src/components/pull_wizard/mod.rs`
- Modify: `crates/tama/src/components/pull_wizard/components.rs` (only if it imports the local `QuantKind` — verify with rg; most sites use `super::QuantKind` and switch to `crate::core_shared::QuantKind`)

**What to implement:**

1. Delete the local `QuantKind` enum (:5-17) and replace with `pub use crate::core_shared::QuantKind;` so `super::QuantKind` imports in `pull_wizard/components.rs` keep compiling. (Keep it `pub use`, not a private import.)
2. Delete BOTH `infer_quant_from_filename` definitions (:113-127 ssr wrapper, :129-195 csr copy) and replace with `pub use crate::core_shared::infer_quant_from_filename;`.
3. Grep `crates/tama/src/components/pull_wizard/` for remaining references; everything else untouched (`HfModelMetadata`, `QuantEntry`, `PullRequest`, `CompletedQuant`, `ContextSettings`, `KV_QUANT_OPTIONS`, `format_bytes`, `step_class`, `is_selection_empty` all stay).

**Steps:**
- [ ] Write the failing check first: `cargo check --package tama --target wasm32-unknown-unknown --no-default-features --features csr` compiles today — after deleting the csr fn but BEFORE adding the re-export it must FAIL (proves the csr path actually used the deleted copy)
- [ ] Add the two `pub use` re-exports
- [ ] Run both checks: `cargo check --package tama` and the wasm32 csr check — compile
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: pull wizard csr quant inference uses shared core impl (fixes drifted pattern table)"

**Acceptance criteria:**
- [ ] `pull_wizard/mod.rs` contains zero `#[cfg(feature = "ssr")]`/`#[cfg(not(feature = "ssr"))]` quant fns and zero local enum definitions; `rg "APEX-IQ2_XXS" crates/tama/src` — zero hits (the drifted table is gone)
- [ ] csr and ssr builds both compile; wizard quant inference behavior on csr now matches core (separator-aware + fallback + full pattern table)
- [ ] `cargo nextest run --package tama` passes

---

### Task 5: Collapse `config_editor/types.rs` onto `crate::types::config`

**Context:**
The config struct tree exists twice: `crates/tama/src/types/config/` (1,321 lines, ssr-only because every file `use`s `tama_core::…` for `From` conversions and enum field types) and `crates/tama/src/pages/config_editor/types.rs` (326 lines, csr+ssr, with an in-code NOTE admitting hand-sync and a regression test for the `api_keys_enabled` drop bug). Field-by-field the section structs are IDENTICAL (`General`, `BackendConfig`, `Supervisor`, `OAuth2Config`, `ProxyConfig`, `CompactionConfig`, `LangfuseConfig`, `SamplingParams`) — the only differences are the enum provenance (`tama_core::config::*` vs `crate::gpu_types::*`, now unified by `core_shared`) and the ssr-only `From` impls. Decision: make `types/config` compile on BOTH feature sets by (a) switching enum field types to `crate::core_shared::*` (identical types on ssr), (b) moving every `From<tama_core::…>` impl into a new `#[cfg(feature = "ssr")]` submodule `types/config/core_conv.rs`, (c) keeping `patch.rs` + `StructuredConfigBody` ssr-only (only `api.rs` uses them), then delete the 326-line copy and repoint the page. The two regression tests MOVE to `types/config/mod.rs` (they are csr-compatible — pure serde round-trips).

**Files:**
- Modify: `crates/tama/src/types/config/mod.rs` (un-gate structs, gate `patch`/`StructuredConfigBody`, declare `core_conv`)
- Modify: `crates/tama/src/types/config/{general,supervisor,compaction,quant,model,health,backend,proxy,sampling,langfuse}.rs` (import swap + From-impl extraction)
- Create: `crates/tama/src/types/config/core_conv.rs`
- Delete: `crates/tama/src/pages/config_editor/types.rs`
- Modify: `crates/tama/src/pages/config_editor/mod.rs` + `crates/tama/src/pages/config_editor/forms/*.rs` (repoint imports)
- Modify: `crates/tama/src/lib.rs:19-20` (remove `#[cfg(feature = "ssr")]` from `pub mod types;`)

**What to implement:**

1. In each `types/config/*.rs` struct file: replace `use tama_core::config::X as CoreX;` with `use crate::core_shared::X as CoreX;` (keep the alias so field types/defaults don't change). Cut every `impl From<tama_core::…> for …` / `impl From<…> for tama_core::…` block (incl. the ones in `mod.rs` :70-258, `model.rs` :87-248, `health.rs`, `quant.rs` :35-89) and paste them verbatim into `core_conv.rs` (they still reference `tama_core::config::*` — valid there under ssr). In `quant.rs`: delete the mirror `QuantKind`/`QuantEntry` structs AND their four `From` impls entirely; replace with `pub use crate::core_shared::{QuantEntry, QuantKind};` — on ssr these ARE the core types, so the impls become identity conversions (delete, don't move). The `quant.rs` serialization tests stay with the file (they exercise the shared type now — keep them, they pass).
2. `types/config/mod.rs`: remove tama_core imports from the top; keep `StructuredConfigBody` but gate it `#[cfg(feature = "ssr")]`; gate `mod patch;` + its `pub use` as `#[cfg(feature = "ssr")]`; add `#[cfg(feature = "ssr")] mod core_conv;`. Move the two regression tests from `config_editor/types.rs:236-326` (`api_keys_enabled_round_trips_through_form_config`, `full_config_round_trip_preserves_every_field`, plus the `server_response_with_all_fields` helper) into a `#[cfg(test)] mod tests` at the bottom of `types/config/mod.rs`, verbatim — they compile on both feature sets (pure serde).
3. `lib.rs`: `pub mod types;` loses its cfg gate.
4. `pages/config_editor/types.rs`: delete the file. In `pages/config_editor/mod.rs` replace `mod types;` + `pub use types::*;` (:1-2) with `pub use crate::types::config::*;` — the glob re-export means most page code keeps compiling unchanged. Then repoint every explicit path reference (verified list): the `config: RwSignal<Option<crate::pages::config_editor::types::Config>>` prop type in `forms/compaction.rs:10`, `forms/general.rs:9`, `forms/proxy/advanced.rs:8`, `forms/proxy/basic.rs:9`, `forms/sampling.rs:73`, `forms/supervisor.rs:9`, `forms/langfuse.rs:9` → `crate::types::config::Config`; plus any other hit from `rg "config_editor::types" crates/tama/src`. The `CoreLogLevel`/`CoreRestartPolicy`/`CoreCompactionDevice` aliases in forms now import from `crate::core_shared` (Task 3 already touched these files — merge carefully).
5. Dead-code caution (do NOT act, just don't be confused): `components/{sampling_templates_section,supervisor_section,general_section}.rs` also import `crate::types::config` — they are dead components (audit F26) but still compiled; they must keep compiling after your changes.

**Steps:**
- [ ] Run `cargo nextest run --package tama` — green baseline (incl. the two config round-trip regression tests at their old location)
- [ ] Apply the changes in order: struct-file import swaps → `core_conv.rs` extraction → `mod.rs` gating → `lib.rs` un-gate → delete `config_editor/types.rs` + repoint page imports → move the two tests
- [ ] Run `cargo check --package tama` AND `cargo check --package tama --target wasm32-unknown-unknown --no-default-features --features csr` — BOTH compile (the csr check is the one that proves the hand-sync copy is truly gone, not just unreachable)
- [ ] Run `cargo nextest run --package tama -- config` — the two moved regression tests pass at their new location
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: collapse config_editor/types.rs onto crate::types::config (single config mirror)"

**Acceptance criteria:**
- [ ] `pages/config_editor/types.rs` no longer exists; `rg "config_editor::types" crates/tama/src` — zero hits
- [ ] `types/config/` compiles on BOTH feature sets; only `core_conv.rs`, `patch.rs`, and `StructuredConfigBody` are ssr-gated
- [ ] The two config round-trip regression tests pass from `types/config/mod.rs`
- [ ] wasm32 csr check compiles (this is the failing-first proof)
- [ ] `cargo clippy --workspace -- -D warnings` is clean

---

### Task 6: Shared DOM helpers + `patch_request` in `crate::utils`; convert the 5 raw-call sites

**Context:**
`set_input_value`/`set_checked` are copy-pasted into all 5 model-editor forms (`settings_form.rs:10-35`, `files_form.rs:10-35`, `sampling_form.rs:10-35`, `hardware_form.rs:9-34`, `advanced_form.rs:8-33`) — byte-identical except `advanced_form.rs` falls back to `HtmlTextAreaElement` instead of `HtmlSelectElement`. The shared version (next to `target_value` in `utils/mod.rs:117`) tries input → select → textarea, a superset of both variants. Separately, `utils/mod.rs` has `get/post/put/delete_request` (:61-95) but no PATCH, so `pages/keys/api.rs:41-53` hand-rolls `Request::patch` with manual CSRF, and `pages/logs.rs:72` + `components/self_update_section.rs:38` use raw `Request::get` WITHOUT injecting the stored CSRF token (logs.rs at least extracts it from the response; self_update_section doesn't even do that). Decisions: add `patch_request`; convert the 3 raw sites to the shared helpers (adding `extract_and_store_csrf_token` to self_update_section's success path); document the two fetching idioms (`LocalResource` vs `spawn_local`) as a comment in `utils/mod.rs` but do NOT migrate any page between them — out of scope (audit F31 explicitly defers pattern standardization).

**Files:**
- Modify: `crates/tama/src/utils/mod.rs`
- Modify: `crates/tama/src/pages/model_editor/{settings,files,sampling,hardware,advanced}_form.rs`
- Modify: `crates/tama/src/pages/keys/api.rs`
- Modify: `crates/tama/src/pages/logs.rs`
- Modify: `crates/tama/src/components/self_update_section.rs`

**What to implement:**

1. In `utils/mod.rs` (after `delete_request`, :95), add:
   ```rust
   /// Build a PATCH request with X-CSRF-Token header injected.
   pub fn patch_request(url: &str) -> RequestBuilder {
       let mut builder = Request::patch(url);
       if let Some(token) = get_csrf_token() {
           builder = builder.header("X-CSRF-Token", &token);
       }
       builder
   }
   ```
2. In `utils/mod.rs` (after `target_value`), add:
   ```rust
   /// Set an input's value by DOM id. Handles input, select, and textarea elements.
   pub fn set_input_value(id: &str, value: &str) {
       use wasm_bindgen::JsCast;
       if let Some(el) = web_sys::window()
           .and_then(|w| w.document())
           .and_then(|d| d.get_element_by_id(id))
       {
           if let Ok(input) = el.clone().dyn_into::<web_sys::HtmlInputElement>() {
               input.set_value(value);
               return;
           }
           if let Ok(select) = el.clone().dyn_into::<web_sys::HtmlSelectElement>() {
               select.set_value(value);
               return;
           }
           if let Ok(textarea) = el.dyn_into::<web_sys::HtmlTextAreaElement>() {
               textarea.set_value(value);
           }
       }
   }

   /// Set a checkbox's checked state by DOM id.
   pub fn set_checked(id: &str, checked: bool) {
       use wasm_bindgen::JsCast;
       if let Some(el) = web_sys::window()
           .and_then(|w| w.document())
           .and_then(|d| d.get_element_by_id(id))
       {
           if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
               input.set_checked(checked);
           }
       }
   }
   ```
   (web-sys `HtmlTextAreaElement` is already in the crate's feature list — verify at `Cargo.toml:20`; add `"HtmlTextAreaElement"` there if missing.)
3. In each of the 5 forms: delete the two local fns and add `use crate::utils::{set_checked, set_input_value};` (merge with the existing `use crate::utils::target_value;` import). Nothing else changes in the forms.
4. `pages/keys/api.rs:41-53` (`update_key_scopes`): replace the hand-rolled block with:
   ```rust
   let resp = patch_request(&format!("/tama/v1/keys/{}", id))
       .header("Content-Type", "application/json")
       .body(serde_json::to_string(&body).map_err(|e| e.to_string())?)
       .map_err(|e| e.to_string())?
       .send()
       .await
       .map_err(|e| e.to_string())?;
   extract_and_store_csrf_token(&resp);
   ```
   and import `patch_request` from `crate::utils` (delete the now-unused `get_csrf_token` import if nothing else in the file uses it).
5. `pages/logs.rs:72`: `Request::get("/tama/v1/logs")` → `get_request("/tama/v1/logs")` (keep the existing `extract_and_store_csrf_token(&resp)` and status handling; remove the now-unused `gloo_net::http::Request` import if unused).
6. `components/self_update_section.rs:38`: `Request::get("/tama/v1/self-update/check")` → `get_request("/tama/v1/self-update/check")`, and ADD `extract_and_store_csrf_token(&resp);` as the first statement inside the `Ok(resp) if resp.ok()` arm (it is missing today — first-ever CSRF extraction on this page); import both from `crate::utils`.
7. Add this comment block at the top of `utils/mod.rs` (documentation only, no code):
   ```rust
   //! Fetching patterns used across pages (both are accepted; do not mix within one fetch):
   //! 1. `LocalResource::new(|| async …)` + `Suspend`/manual `Option` handling — used by
   //!    `pages/dashboard` and `pages/models` for read-on-mount data.
   //! 2. `spawn_local` + `RwSignal` (loading/error signals managed by hand) — the dominant
   //!    pattern (~18 files). Use the `*_request` helpers below for CSRF injection and
   //!    always call `extract_and_store_csrf_token` on 2xx GET responses.
   ```
   (Verify the dashboard/models claim against the actual files before writing the comment; adjust the named pages to the real `LocalResource` users — `rg "LocalResource" crates/tama/src/pages` tells you.)

**Steps:**
- [ ] Run `cargo nextest run --package tama` — green baseline
- [ ] Add the two helpers + `patch_request` + the doc comment to `utils/mod.rs`
- [ ] Convert the 5 forms, then keys/api.rs, logs.rs, self_update_section.rs
- [ ] Run `cargo check --package tama` AND the wasm32 csr check — both compile (catches unused-import fallout, e.g. `wasm_bindgen::JsCast` still needed in forms for other code)
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: shared DOM/request helpers in utils; convert 5 forms + 3 raw gloo sites"

**Acceptance criteria:**
- [ ] `rg "fn set_input_value|fn set_checked" crates/tama/src/pages` — zero hits; the 5 forms import from `crate::utils`
- [ ] `rg "Request::patch|Request::get" crates/tama/src/pages/keys crates/tama/src/pages/logs.rs crates/tama/src/components/self_update_section.rs` — zero hits
- [ ] `self_update_section.rs` calls `extract_and_store_csrf_token` on the check response
- [ ] Both feature-set checks compile; `cargo nextest run --package tama` passes

---

### Task 7: Extract shared benchmark-form state into `benchmarks/utils.rs`

**Context:**
The three benchmark tabs triplicate ~200 lines: the fetch-models `Effect` (`mod.rs:206-231`, `spec_bench.rs:178-203`, `mtp_bench.rs:47-68`), the fetch-backends `spawn_local` block (`mod.rs:232-255` simple variant; `spec_bench.rs:204-252` and `mtp_bench.rs:71-119` installed+variant variant — byte-identical to each other), `parse_sizes` (`mod.rs:279-286` closure, `spec_bench.rs:101-108`, `mtp_bench.rs:14-21`), `format_mean_stddev` (`mod.rs:23-29`, `spec_bench.rs:111-118`), the auto-select-first-quant `Effect` (`mod.rs:418-430`, `spec_bench.rs:257-271`, `mtp_bench.rs:121-135`), and the submit-time `"id:quant"` split (`mod.rs:301-310`, `spec_bench.rs:309-317`, `mtp_bench.rs:142-150`) + `"name:variant"` split (`spec_bench.rs:319-326`, `mtp_bench.rs:152-159`). Decisions: put plain fns (`parse_sizes`, `format_mean_stddev`, `split_id_quant`, `split_name_variant`) and a `BenchmarkFormState` struct + `use_benchmark_form_state()` constructor in the EXISTING `benchmarks/utils.rs`; the two backend-fetch variants each get their own helper (`fetch_bench_backends` for mod.rs's simple list, `fetch_installed_backend_variants` for the shared spec/mtp one) — do NOT force them into one fn, their semantics genuinely differ. `render_summaries_table` (`mod.rs:31-152`) is used only by mod.rs — leave it. Scope guard: this task touches ONLY the shared boilerplate; each form's preset/apply logic, submit body shape, and results rendering stay per-file.

**Files:**
- Modify: `crates/tama/src/pages/benchmarks/utils.rs`
- Modify: `crates/tama/src/pages/benchmarks/mod.rs`
- Modify: `crates/tama/src/pages/benchmarks/spec_bench.rs`
- Modify: `crates/tama/src/pages/benchmarks/mtp_bench.rs`

**What to implement:**

1. Append to `benchmarks/utils.rs` (imports: `leptos::prelude::*`, `leptos::task::spawn_local`, `crate::utils::{extract_and_store_csrf_token, get_request}`, `super::types::parse_model`):
   ```rust
   /// Parse a comma-separated string of integers into a Vec<u32>.
   /// Zero is meaningful (`-p 0` = pure-TG mode) — only empty/unparsable tokens drop.
   pub fn parse_sizes(s: &str) -> Vec<u32> { /* body from spec_bench.rs:101-108 verbatim */ }

   /// Render "mean ± stddev" with one decimal place, or a single value when stddev rounds to zero.
   pub fn format_mean_stddev(mean: f64, stddev: f64) -> String { /* body from mod.rs:23-29 verbatim */ }

   /// Split a "id:quant" composite (model selector value) into (id, Some(quant)).
   /// No colon → (whole string, None).
   pub fn split_id_quant(raw: &str) -> (String, Option<String>) {
       if let Some(colon) = raw.find(':') {
           (raw[..colon].to_string(), Some(raw[colon + 1..].to_string()))
       } else {
           (raw.to_string(), None)
       }
   }

   /// Split a "name:variant" composite (backend selector value) into (Some(name), Some(variant)).
   /// Empty → (None, None); no colon → (Some(whole), None).
   pub fn split_name_variant(raw: &str) -> (Option<String>, Option<String>) {
       if raw.is_empty() {
           (None, None)
       } else if let Some((name, variant)) = raw.split_once(':') {
           (Some(name.to_string()), Some(variant.to_string()))
       } else {
           (Some(raw.to_string()), None)
       }
   }

   /// Model/backend/job signals shared by all three benchmark forms.
   pub struct BenchmarkFormState {
       pub selected_display_name: RwSignal<String>,
       pub selected_model: RwSignal<String>,
       pub available_models: RwSignal<Vec<(String, String, Vec<String>)>>,
       pub selected_backend: RwSignal<String>,
       pub available_backends: RwSignal<Vec<(String, String)>>,
       pub is_running: RwSignal<bool>,
       pub current_job_id: RwSignal<Option<String>>,
       pub benchmark_results: RwSignal<Option<serde_json::Value>>,
       pub model_refresh: RwSignal<u32>,
   }

   /// Create the shared signals and wire the two universal Effects:
   /// fetch-models-on-refresh and auto-select-first-quant-on-display-name-change.
   pub fn use_benchmark_form_state() -> BenchmarkFormState {
       let state = BenchmarkFormState {
           selected_display_name: RwSignal::new(String::new()),
           selected_model: RwSignal::new(String::new()),
           available_models: RwSignal::new(Vec::new()),
           selected_backend: RwSignal::new(String::new()),
           available_backends: RwSignal::new(Vec::new()),
           is_running: RwSignal::new(false),
           current_job_id: RwSignal::new(None),
           benchmark_results: RwSignal::new(None),
           model_refresh: RwSignal::new(0u32),
       };
       // Fetch-models Effect — body verbatim from mod.rs:206-231
       // (parse_model + flatten + dedup by (name, quant) → available_models),
       // reading state.model_refresh / writing state.available_models.
       // Auto-select-first-quant Effect — body verbatim from mod.rs:418-430,
       // reading state.selected_display_name/available_models, writing state.selected_model.
       state
   }

   /// Fetch backends for the llama-bench form: flat (name, display_name) from
   /// root["backends"], no installed/variant filtering. Body verbatim from mod.rs:232-255.
   pub fn fetch_bench_backends(available_backends: RwSignal<Vec<(String, String)>>) { … }

   /// Fetch backends for the spec/mtp forms: both root["backends"] and root["custom"],
   /// installed-only, value "name:variant", label "display (variant)" ("cpu" → bare display).
   /// Body verbatim from spec_bench.rs:204-252.
   pub fn fetch_installed_backend_variants(available_backends: RwSignal<Vec<(String, String)>>) { … }
   ```
2. `mod.rs`: delete `format_mean_stddev` (keep using it via `use self::utils::format_mean_stddev;`), the fetch-models Effect, the fetch-backends block, the `parse_sizes` closure (call sites use `utils::parse_sizes` — the existing `parse_threads` closure stays), and the auto-select Effect; replace the 9 shared signal declarations with `let state = use_benchmark_form_state();` and either destructure (`let BenchmarkFormState { selected_display_name, … } = state;`) or rename uses — choose destructuring to minimize view!{} edits; replace the submit preamble split with `let (model_id, quant) = split_id_quant(&selected_model.get());`; call `fetch_bench_backends(available_backends);` in place of the deleted block. `render_summaries_table`, presets, apply_bench_type, submit body, results/history rendering: untouched.
3. `spec_bench.rs`: delete `parse_sizes`, `format_mean_stddev` (reimport from `super::utils`), the fetch-models Effect, fetch-backends block, auto-select Effect; use `use_benchmark_form_state()` + `fetch_installed_backend_variants`; submit preamble becomes `let (model_id, quant) = split_id_quant(&raw_model);` (keep the `raw_model.is_empty()` early return BEFORE it) and `let (backend_name, gpu_variant) = split_name_variant(&selected_backend.get());`. `error_msg` signal stays per-form (not shared).
4. `mtp_bench.rs`: same as spec_bench (it has no `format_mean_stddev`; keep its own submit body).
5. Keep every `view!{}` block byte-identical — this is a logic extraction, not a template refactor.

**Steps:**
- [ ] Run `cargo nextest run --package tama` — green baseline
- [ ] Append the helpers + `BenchmarkFormState` + `use_benchmark_form_state` to `benchmarks/utils.rs`; convert `mod.rs`, then `spec_bench.rs`, then `mtp_bench.rs`
- [ ] Run `cargo check --package tama` AND the wasm32 csr check — both compile
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Manually verify the extraction kept semantics: `rg "fn parse_sizes|fn format_mean_stddev" crates/tama/src/pages/benchmarks` — exactly one hit each (in utils.rs)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: extract shared benchmark form state into benchmarks/utils.rs"

**Acceptance criteria:**
- [ ] `benchmarks/utils.rs` exports `parse_sizes`, `format_mean_stddev`, `split_id_quant`, `split_name_variant`, `BenchmarkFormState`, `use_benchmark_form_state`, `fetch_bench_backends`, `fetch_installed_backend_variants`
- [ ] `mod.rs`, `spec_bench.rs`, `mtp_bench.rs` contain zero local copies of the extracted fns/Effects; combined line count of the three files drops by ~150-200 lines
- [ ] The two backend-fetch variants remain separate helpers (no semantic merge)
- [ ] Both feature-set checks compile; `cargo nextest run --package tama` passes; clippy clean
