# Newtypes Plan

**Goal:** Replace three stringly-typed domains with real types: the `gpu_variant` string (re-validated with literals in ≥3 places despite an existing enum), open-coded HuggingFace endpoint/URL construction at 6 sites (with `search.rs` ignoring `HF_ENDPOINT` entirely), and three divergent `repo_id` validators (one of which lets `../x` through).

**Architecture:** F16 extends the existing `tama_core::gpu::GpuType` enum (`crates/tama-core/src/gpu/detect.rs:6-13`) with `FromStr`/`Display`/string-form serde and adopts it in `BackendConfig.gpu_variant`/`ModelConfig.gpu_variant`, `gpu::env::resolve_gpu_env`, and the install/compaction request DTOs — the enum keeps its current name (plan-173 owns the `GpuType`→`GpuVariant` rename; do not rename here). F17 adds free helper fns (`hf_endpoint`, `hf_*_url`, `hf_auth_headers`) in `models/pull/mod.rs` next to the `hf_api()` OnceCell — free fns, not a struct, because the env var must be re-read per call (existing tests toggle `HF_ENDPOINT` per-test). F18 adds one `is_valid_repo_id` in `tama_core::models` implementing the strictest union of the three current validators. The DB layer (`ModelConfigRecord`, `ModelConfigDto`, `BackendInfo`, `backend_configs` table) stays `String`-based — the type boundary is the config structs; parse happens at the DB↔config edge.

**Tech Stack:** Rust, Axum, SQLite (rusqlite), serde, reqwest

---

### Task 1: Extend `GpuType` with `FromStr`, `Display`, and string-form serde (F16 core)

**Context:**
`GpuType` (`Cuda { version }`, `Vulkan`, `Metal`, `RocM { version }`, `CpuOnly`, `Custom`) exists with `variant_folder()` mapping to the canonical strings `"cpu","cuda","vulkan","rocm","metal","custom"`, but has zero production constructors and zero `variant_folder()` callers outside its own test — the taxonomy is re-declared with string literals at `api/backends/install.rs:65`, `gpu/env.rs:45-48,76-79`, and ad-hoc `to_lowercase() == "cuda"` compares. This task is the additive foundation: give the enum the traits every adoption site needs, without changing any behavior yet. Two decisions: (a) serde is **manual** (serialize via `variant_folder()`, deserialize via `FromStr`) — `#[serde(rename_all = "lowercase")]` cannot work (`CpuOnly` would become `"cpuonly"`, and the struct variants `Cuda{version}`/`RocM{version}` would serialize as maps, not strings); the version payloads are detection artifacts that intentionally do **not** round-trip (`"cuda"` parses to `Cuda { version: String::new() }`). (b) `FromStr` is **case-insensitive** because the install endpoint currently accepts `"CUDA"` (it lowercases only for the check at install.rs:66). One collateral fix in the same commit (the serde shape change forces it): `db/backfill/mod.rs:31`'s legacy `gpu_type: Option<GpuType>` field deserializes from the old externally-tagged TOML form (`{ Cuda = { version = "12.4" } }`) during one-time migration (`initial_backfill.rs:125`) — with string-form serde that legacy data would fail to parse and abort the whole migration; the field is dead (`#[allow(dead_code)]`), so retype it to `Option<toml::Value>`.

**Files:**
- Modify: `crates/tama-core/src/gpu/detect.rs`
- Modify: `crates/tama-core/src/db/backfill/mod.rs`

**What to implement:**

1. **`detect.rs`** —
   - Change the derive on `GpuType` from `#[derive(Debug, Clone, Serialize, Deserialize)]` to `#[derive(Debug, Clone, PartialEq, Eq)]` (drop the serde derives; `PartialEq`/`Eq` are needed by tests in later tasks — `Cuda{version: String}`/`RocM{version: String}` are `Eq`-compatible).
   - Add (doc comments noting the string form is the canonical `gpu_variant` representation and that `version` payloads do not survive a string round-trip):
     ```rust
     impl std::fmt::Display for GpuType {
         fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
             f.write_str(self.variant_folder())
         }
     }

     impl std::str::FromStr for GpuType {
         type Err = anyhow::Error;

         /// Parse a gpu_variant string, case-insensitively
         /// ("cpu", "cuda", "vulkan", "rocm", "metal", "custom").
         fn from_str(s: &str) -> Result<Self, Self::Err> {
             match s.to_ascii_lowercase().as_str() {
                 "cpu" => Ok(GpuType::CpuOnly),
                 "cuda" => Ok(GpuType::Cuda { version: String::new() }),
                 "vulkan" => Ok(GpuType::Vulkan),
                 "metal" => Ok(GpuType::Metal),
                 "rocm" => Ok(GpuType::RocM { version: String::new() }),
                 "custom" => Ok(GpuType::Custom),
                 other => Err(anyhow::anyhow!(
                     "unknown gpu_variant '{}'; expected one of: cpu, cuda, vulkan, rocm, metal, custom",
                     other
                 )),
             }
         }
     }

     impl serde::Serialize for GpuType {
         fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
             serializer.serialize_str(self.variant_folder())
         }
     }

     impl<'de> serde::Deserialize<'de> for GpuType {
         fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
             let s = String::deserialize(deserializer)?;
             <GpuType as std::str::FromStr>::from_str(&s).map_err(serde::de::Error::custom)
         }
     }
     ```
     (`use serde::Deserialize as _;` is already at `detect.rs:1` — keep it; the manual impls reference fully-qualified `serde::` paths so no import churn. Add `use std::str::FromStr as _;` only if needed by the impl body — the fully-qualified call avoids it.)
   - Do **not** rename the enum (plan-173) and do not change `variant_folder()` (line 40-49) or any detection code.

2. **`backfill/mod.rs:31`** — retype the dead legacy field:
   ```rust
   #[serde(default)]
   #[allow(dead_code)]
   gpu_type: Option<toml::Value>,
   ```
   (`toml` is already a tama-core dependency — `crates/tama-core/Cargo.toml:10`.)

3. **Tests** — append to the existing `mod tests` in `detect.rs` (around line 455):
   - `test_from_str_all_variants`: the 6 lowercase strings parse to the expected variants (assert with `==`, using `Cuda { version: String::new() }`/`RocM { version: String::new() }`).
   - `test_from_str_case_insensitive`: `"CUDA"` → `Cuda`, `"Rocm"`/`"ROCM"` → `RocM`, `"Cpu"` → `CpuOnly`.
   - `test_from_str_unknown_rejected`: `"tpu"`, `""`, `"gpu"` → `Err`.
   - `test_display_matches_variant_folder`: for all 6 canonical constructions, `format!("{}", v) == v.variant_folder()`.
   - `test_string_roundtrip_canonical`: for each canonical variant `v`, `v.to_string().parse::<GpuType>().unwrap() == v` (construct with empty versions).
   - `test_serde_string_form`: `serde_json::to_string(&GpuType::Cuda { version: "12.4".into() }).unwrap() == "\"cuda\""`; `serde_json::from_str::<GpuType>("\"vulkan\"").unwrap() == GpuType::Vulkan`; `serde_json::from_str::<GpuType>("\"tpu\"").is_err()`.
   - Keep the existing `test_variant_folder_all_variants` unchanged.

**Steps:**
- [ ] Write the six failing tests in `crates/tama-core/src/gpu/detect.rs`
- [ ] Run `cargo nextest run --package tama-core -- gpu::detect` — verify they fail (missing impls)
- [ ] Implement the trait impls in `detect.rs` and the `backfill/mod.rs` field retype
- [ ] Run `cargo nextest run --package tama-core` — all pass (catches any other consumer of the old derived serde)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "feat: FromStr/Display/string serde for GpuType"

**Acceptance criteria:**
- [ ] `GpuType` has `FromStr` (case-insensitive, `anyhow::Error`), `Display` (= `variant_folder()`), `PartialEq`, `Eq`, and string-form serde; no call-site behavior changes in this commit
- [ ] `backfill/mod.rs` no longer deserializes the legacy enum form (field is `Option<toml::Value>`)
- [ ] `cargo nextest run --package tama-core` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 2: Adopt `GpuType` in config types, `gpu::env`, and all consumers (F16 adoption)

**Context:**
`BackendConfig.gpu_variant` (`config/types/backend.rs:14`) and `ModelConfig.gpu_variant` (`config/types/model.rs:73`) are `Option<String>`; every consumer re-validates or blindly passes the string. This task flips both fields to `Option<GpuType>` and fixes every consumer in one compile-atomic commit. Key decisions: (a) the DB boundary is **lenient with a warning** — `from_db_record`/`from_db_record_for_repo` are infallible (return `Self`) and unknown DB strings (only possible via hand edits; legacy uppercase like `"CUDA"` parses fine via case-insensitive `FromStr`) map to `GpuType::Custom` with a `tracing::warn!`, which preserves today's graceful "variant not found → try all variants" fallback in `resolve_backend_path` instead of inventing a new hard failure; (b) `resolve_backend_path` takes `Option<&GpuType>` — its internals stay string-based because `BackendManager::get_active`/`get_by_version` (`backends/manager.rs:147,183`) are DB-string APIs (out of scope); (c) the wire DTOs `ModelBody`/`ModelPatchBody` in `crates/tama/src/api/models/crud/mod.rs:31,78` flip to `Option<GpuType>` too — this is required for compilation (the merge at crud/mod.rs:111,226 is `body.gpu_variant.or(existing…)`), and it means create/update/patch now reject invalid variant strings at the serde edge with **422** instead of persisting garbage (intended F16 edge validation — document in the commit body); (d) `resolve_gpu_env`/`resolve_gpu_env_from` take `&GpuType`, deleting the match arms at `env.rs:45-48,76-79`. Out of scope: `components/install_modal.rs:93` (WASM string signal — typing the mirror types is F29/plan-173), `bench/llama_bench/discovery.rs:64`'s unrelated `detect_gpu_type() -> String` (F27), `InstallOptions.gpu_variant: String` (task 3 converts at construction).

**Files:**
- Modify: `crates/tama-core/src/config/types/backend.rs`
- Modify: `crates/tama-core/src/config/types/model.rs`
- Modify: `crates/tama-core/src/config/types/mod.rs`
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/gpu/env.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`
- Modify: `crates/tama-core/src/bench/runner.rs`
- Modify: `crates/tama-core/src/bench/llama_bench/mod.rs`
- Modify: `crates/tama-core/src/db/backfill/migrate_toml_to_db.rs`
- Modify: `crates/tama/src/api/models/crud/mod.rs`
- Modify: `crates/tama/src/api/models/crud/tests.rs`
- Modify: `crates/tama/src/api/benchmarks/mtp.rs`
- Modify: `crates/tama/src/api/benchmarks/spec.rs`

**What to implement:**

1. **`config/types/backend.rs:14`** — `pub gpu_variant: Option<GpuType>,` with `use crate::gpu::GpuType;` (doc comment unchanged: the TOML/JSON form is still `"cuda"` etc. thanks to task 1's serde).

2. **`config/types/model.rs`** —
   - Line 73: `pub gpu_variant: Option<GpuType>,` (`use crate::gpu::GpuType;` + `use std::str::FromStr;` — neither is imported today).
   - `to_db_record` (line 196): `gpu_variant: self.gpu_variant.as_ref().map(|v| v.variant_folder().to_string()),`
   - `from_db_record` (line 244) and `from_db_record_for_repo` (line 309), both:
     ```rust
     gpu_variant: record.gpu_variant.as_deref().map(|s| {
         GpuType::from_str(s).unwrap_or_else(|_| {
             tracing::warn!(
                 "unknown gpu_variant '{}' in model_configs row; treating as custom",
                 s
             );
             GpuType::Custom
         })
     }),
     ```

3. **`config/types/mod.rs:211`** — the `BackendConfig` construction from the `backend_configs` row (`record.gpu_variant` is `String`, NOT NULL):
   ```rust
   gpu_variant: Some(
       crate::gpu::GpuType::from_str(&record.gpu_variant).unwrap_or_else(|_| {
           tracing::warn!(
               "unknown gpu_variant '{}' in backend_configs row; treating as custom",
               record.gpu_variant
           );
           crate::gpu::GpuType::Custom
       }),
   ),
   ```
   (`use std::str::FromStr;` if not already imported in this file.)

4. **`config/resolve/mod.rs`** —
   - `EMPTY_BACKEND_CONFIG` (lines 7-10): `gpu_variant: None` — compiles unchanged; verify, don't touch.
   - `resolve_backend_path` (line 611): signature becomes
     ```rust
     pub fn resolve_backend_path(
         &self,
         name: &str,
         model_variant: Option<&crate::gpu::GpuType>,
         manager: &crate::backends::BackendManager,
     ) -> Result<std::path::PathBuf>
     ```
     and the resolution block (lines 618-626) becomes:
     ```rust
     let gpu_variant: &str = model_variant
         .map(|v| v.variant_folder())
         .or_else(|| {
             self.backends
                 .get(name)
                 .and_then(|b| b.gpu_variant.as_ref())
                 .map(|v| v.variant_folder())
         })
         .unwrap_or("cpu");
     ```
     Downstream uses drop one reference level: `manager.get_by_version(name, gpu_variant, pinned_version)?` and `manager.get_active(name, gpu_variant)?` (both take `&str`). The `bail!` message and the all-variants fallback (including `manager.get_active(name, &v.gpu_variant)` at line 665 — `v.gpu_variant` is the DB record's `String`, unchanged) are untouched. Also update the doc comment's priority list wording if it references types (it references semantics — leave it).

5. **`gpu/env.rs`** — both fns take the enum:
   ```rust
   pub fn resolve_gpu_env(gpu_device: &str, gpu_variant: &GpuType) -> Option<(String, String)> {
       let device = gpu_device.trim();
       if device.is_empty() || matches!(gpu_variant, GpuType::CpuOnly) {
           return None;
       }
       // … unchanged index math …
       let env_name = match gpu_variant {
           GpuType::RocM { .. } => "ROCR_VISIBLE_DEVICES",
           GpuType::Cuda { .. } => "CUDA_VISIBLE_DEVICES",
           GpuType::Vulkan => "GGML_VK_VISIBLE_DEVICES",
           _ => return None,
       };
       Some((env_name.to_string(), per_vendor_index.to_string()))
   }
   ```
   Same transformation for `resolve_gpu_env_from` (add `&GpuType` param). Import: `use super::detect::GpuType;` (or `super::GpuType` — `gpu/mod.rs:16` re-exports it). Tests in `env.rs`'s `mod tests`: replace every string literal argument with `&GpuType::from_str("rocm").unwrap()` / `"cuda"` / `"vulkan"` / `"cpu"` as applicable (also exercises task 1's parser); `test_resolve_unknown_variant_returns_none` becomes `test_resolve_variant_without_env_mechanism_returns_none` using `&GpuType::Metal` (metal has no env-var mechanism — the "unknown string" case is now unrepresentable by construction, which is the point).

6. **`proxy/lifecycle/mod.rs`** — add `use crate::gpu::GpuType;` (no gpu import exists today). Changes:
   - Line 128: `server_config.gpu_variant.as_deref()` → `server_config.gpu_variant.as_ref()` (matches the new `Option<&GpuType>` param).
   - Line 158: `let gpu_variant = server_config.gpu_variant.clone().unwrap_or(GpuType::CpuOnly);`
   - Line 159: `manager.get_default_args(&server_config.backend, gpu_variant.variant_folder())`
   - Line 188: `if !matches!(gpu_variant, GpuType::CpuOnly) {`
   - Line 190: `crate::gpu::env::resolve_gpu_env(device, &gpu_variant)` — the log lines at :194/:202 keep `gpu_variant` as the `{}` argument (now via `Display`).
   - Line 211: `manager.get_default_env(&server_config.backend, gpu_variant.variant_folder())`

7. **`bench/runner.rs:123`** and **`bench/llama_bench/mod.rs:108`** — `server_config.gpu_variant.as_deref()` → `server_config.gpu_variant.as_ref()`.

8. **`db/backfill/migrate_toml_to_db.rs:139-142`** —
   ```rust
   let gpu_variant = backend_config
       .gpu_variant
       .as_ref()
       .map(|v| v.variant_folder().to_string())
       .unwrap_or_else(|| "cpu".to_string());
   ```

9. **`crates/tama/src/api/models/crud/mod.rs`** — `ModelBody.gpu_variant` (line 31) and `ModelPatchBody.gpu_variant` (line 78) become `Option<tama_core::gpu::GpuType>` (keep `#[serde(default)]`). The merges at :111 and :226 (`body.gpu_variant.or(existing.gpu_variant.clone())` / `.or(base.gpu_variant)`) compile unchanged. **Behavior change:** invalid variant strings in create/update/patch bodies now 422 at the JSON extractor instead of being stored.

10. **`crates/tama/src/api/models/crud/tests.rs`** — the only two `Some` literal sites: line 1261 `gpu_variant: Some("cuda".into()),` → `gpu_variant: Some(tama_core::gpu::GpuType::Cuda { version: String::new() }),`; the test at ~1854 (`test_apply_model_patch_gpu_variant_override`): `body.gpu_variant = Some("rocm".to_string());` → `body.gpu_variant = Some(tama_core::gpu::GpuType::RocM { version: String::new() });` and `assert_eq!(result.gpu_variant, Some("rocm".into()));` → compare against the same typed construction. All `gpu_variant: None` literals compile unchanged.

11. **`crates/tama/src/api/benchmarks/mtp.rs`** and **`spec.rs`** — the request DTOs (`MtpBenchmarkRunRequest.gpu_variant`, `SpecBenchmarkRunRequest.gpu_variant`) stay `Option<String>` (wire); parse at the boundary inside the inner fns (both return `anyhow::Result`): replace `let gpu_variant = req.gpu_variant.clone();` with
    ```rust
    let gpu_variant: Option<tama_core::gpu::GpuType> = req
        .gpu_variant
        .as_deref()
        .map(tama_core::gpu::GpuType::from_str)
        .transpose()?;
    ```
    and the `resolve_backend_path` calls (mtp.rs:198, spec.rs:205) pass `gpu_variant.as_ref()`. **Behavior change:** an invalid benchmark-request variant now fails the job with a clear parse error instead of silently falling through variant discovery. (Note: plan-169 task 4 wraps these lines in `spawn_blocking` — if it has landed, apply the same change inside the closure; the parse is cheap and stays in async context either way.)

12. Sweep for leftovers: `rg "\.gpu_variant" crates/` — every remaining read must be on a DB record/DTO type (`BackendInfo`, `ModelConfigDto`, `UpdateCheckDto`, query-param structs, `BackendOption`, WASM mirrors in `crates/tama/src/gpu_types.rs`, `types/config/`, `pages/`, `components/`), which all stay `String`. Fix any stragglers the compiler flags by the same patterns above; do not change DB record types or mirror types.

**Steps:**
- [ ] Run `cargo nextest run --workspace` — record the green baseline
- [ ] Apply items 1–4 (config types + resolve); run `cargo check --package tama-core` and fix the flagged consumers per items 5–8
- [ ] Apply items 5–8 (env.rs incl. test updates, lifecycle, bench, migrate); run `cargo nextest run --package tama-core -- gpu::env` and `-- config::resolve` — pass
- [ ] Apply items 9–11 (crud DTOs + tests, benchmark DTOs); run `cargo check --package tama`
- [ ] Run `cargo nextest run --package tama-core` and `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: type gpu_variant as GpuType across config and consumers"

**Acceptance criteria:**
- [ ] `BackendConfig.gpu_variant` and `ModelConfig.gpu_variant` are `Option<GpuType>`; `resolve_gpu_env(_from)` and `resolve_backend_path` take the enum; `rg 'valid_variants|to_lowercase\(\) == "cuda"' crates/` finds no gpu_variant string validation outside WASM mirrors
- [ ] TOML/JSON wire forms unchanged (`"gpu_variant": "cuda"`), verified by existing config/crud tests passing unmodified except the two typed literals
- [ ] Unknown DB strings → `GpuType::Custom` + warning (grep for the two `tracing::warn!` sites); invalid API bodies → 422
- [ ] `cargo nextest run --workspace` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 3: Type `InstallRequest.gpu_variant` as `GpuType`; delete the literal array (F16 DTO)

**Context:**
`InstallRequest.gpu_variant` (`crates/tama/src/api/backends/types.rs:214`) is a required `String` validated by hand at `crates/tama/src/api/backends/install.rs:64-71` against `let valid_variants = ["cpu","cuda","vulkan","rocm","metal","custom"];` — the third copy of the taxonomy. With task 1's serde, the field deserializes directly as `GpuType` and the manual validation block is deleted: invalid variants become axum `Json` data-error rejections (**422**, replacing the hand-rolled 400 — accepted and consistent with task 2's edge-validation change; a missing field was already a 422). Two downstream adjustments: `install.rs:255`'s `req.gpu_variant.to_lowercase() == "cuda"` becomes a `matches!`, and the raw request string currently flows into `get_backend_install_path`/`InstallOptions.gpu_variant`/`reg_gpu_variant` (install.rs:379,408,422) — meaning `"CUDA"` today creates a literally-named `CUDA` install directory; after this change the canonical `Display` form (`"cuda"`) is used — a deliberate bug fix, documented in the commit body. `components/install_modal.rs:93` (`gpu_variant.get() == "cuda"`, a WASM string signal) is **not** changed — typing the mirror is F29/plan-173 scope.

**Files:**
- Modify: `crates/tama/src/api/backends/types.rs`
- Modify: `crates/tama/src/api/backends/install.rs`

**What to implement:**

1. **`types.rs:210-218`** — `pub gpu_variant: tama_core::gpu::GpuType,` (keep the doc comment; it already lists the valid strings, which remain the wire form).

2. **`install.rs`** —
   - Delete the validation block at lines 64-71 (`// Validate gpu_variant: …` through the closing `}`).
   - Line 255: `let is_cuda = matches!(req.gpu_variant, tama_core::gpu::GpuType::Cuda { .. });`
   - Line 379: `let gpu_variant = req.gpu_variant.to_string();` (canonical lowercase via `Display`; everything downstream — `get_backend_install_path`, `InstallOptions.gpu_variant`, `reg_gpu_variant` — stays `String` and compiles unchanged).

3. **Tests** — `install.rs` has no route tests (F24 scope), but the DTO is serde-testable. Add `#[cfg(test)] mod tests` to `crates/tama/src/api/backends/types.rs`:
   - `test_install_request_accepts_known_variant`: `serde_json::from_str::<InstallRequest>(r#"{"backend_type":"llama_cpp","version":null,"gpu_variant":"cuda","build_from_source":false,"force":false}"#)` → `Ok`, `matches!(req.gpu_variant, GpuType::Cuda { .. })`.
   - `test_install_request_rejects_unknown_variant`: same body with `"gpu_variant":"tpu"` → `Err`.
   - `test_install_request_variant_case_insensitive`: `"CUDA"` → `Ok(Cuda)`.

**Steps:**
- [ ] Write the three failing tests in `crates/tama/src/api/backends/types.rs`
- [ ] Run `cargo nextest run --package tama -- api::backends::types` — verify failure
- [ ] Apply the DTO change and the `install.rs` edits
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: type InstallRequest.gpu_variant as GpuType"

**Acceptance criteria:**
- [ ] The `valid_variants` literal array no longer exists (`rg "valid_variants" crates/` → 0 hits)
- [ ] Invalid variants → 422 from the extractor (no handler code runs); `"CUDA"` installs into the canonical `cuda` path
- [ ] `cargo nextest run --package tama` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 4: Deserialize `CompactionToggleRequest.device` as `CompactionDevice` (F16 tail)

**Context:**
`crates/tama/src/api/backends/compaction.rs:13` declares `pub device: Option<String>` and the handler (lines 33-36) lossy-parses it: `CompactionDevice::from_str(device).unwrap_or(config.compaction.device.clone())` — an invalid device string **silently keeps the old value**, so a typo'd request returns success while doing nothing. `CompactionDevice` (`crates/tama-core/src/config/types/enums.rs:115-125`) already has custom string-form serde (`"cpu"`, `"cuda"`, `"cuda:N"`, `"mps"` — `enums.rs:127-149`) that rejects invalid values with a serde error, so typing the DTO field directly gives a 422 rejection for free.

**Files:**
- Modify: `crates/tama/src/api/backends/compaction.rs`

**What to implement:**

1. Line 13: `pub device: Option<CompactionDevice>,` (`CompactionDevice` is already imported at line 8).
2. Lines 33-36 become:
   ```rust
   if let Some(device) = &req.device {
       config.compaction.device = device.clone();
   }
   ```
   (delete the `from_str`/`unwrap_or`; the inherent `CompactionDevice::from_str` and the `FromStr` impl in `enums.rs` stay — other callers use them).
3. **Tests** — add `#[cfg(test)] mod tests` at the bottom of `compaction.rs`:
   - `test_compaction_request_deserializes_device`: `serde_json::from_str::<CompactionToggleRequest>(r#"{"enabled":true,"device":"cuda:1","port":null,"request_timeout_ms":null}"#)` → `Ok`, `matches!(req.device, Some(CompactionDevice::CudaDevice(1)))`.
   - `test_compaction_request_rejects_invalid_device`: `"device":"tpu"` → `Err` (serde rejects → axum would 422).
   - `test_compaction_request_device_optional`: no `device` key → `Ok`, `req.device.is_none()`.

**Steps:**
- [ ] Write the three failing tests in `crates/tama/src/api/backends/compaction.rs`
- [ ] Run `cargo nextest run --package tama -- api::backends::compaction` — verify failure
- [ ] Apply the DTO + handler change
- [ ] Run `cargo nextest run --package tama` — all pass
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: reject invalid compaction device with 422 instead of silently keeping old value"

**Acceptance criteria:**
- [ ] `CompactionToggleRequest.device` is `Option<CompactionDevice>`; no `unwrap_or` fallback remains in the handler
- [ ] Invalid device values are rejected at deserialization (422); valid values (`cpu`, `cuda`, `cuda:N`, `mps`) behave exactly as before
- [ ] `cargo nextest run --package tama` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 5: Shared HuggingFace endpoint/URL/auth helpers (F17)

**Context:**
`std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string())` + `format!` URL building is open-coded at 6 sites: `crates/tama-core/src/models/pull/download.rs:27-30` (resolve URL) and `:37-45` (auth headers, fail-hard on parse), `models/pull/api.rs:82-84` (`fetch_blob_metadata`, `?blobs=true`), `:115-118` (`fetch_hf_metadata`, model API + README raw URLs at :167-168), `:210-212` (`fetch_model_pipeline_tag`), and `proxy/tama_handlers/pull/download.rs:174-186` (resolve URL + auth headers, lenient parse). Meanwhile `models/search.rs:4` has `const HF_API_BASE: &str = "https://huggingface.co/api/models"` which **ignores `HF_ENDPOINT`** — mirror-endpoint support is inconsistent by construction. Decision: free `pub(crate)` fns in `models/pull/mod.rs` next to `hf_api()` (line 452), reading the env var per call (no caching — the existing env-toggling tests keep working, and endpoint changes don't require process restart). One accepted behavior change: `models/pull/download.rs`'s auth-header construction currently aborts the pull on an unparseable token (`?` with context); the shared `hf_auth_headers()` adopts the lenient skip-on-parse-failure already used at `tama_handlers/pull/download.rs:181-186` (a trimmed HF token is virtually always a valid header value; the proxy-side behavior wins for uniformity).

**Files:**
- Modify: `crates/tama-core/src/models/pull/mod.rs`
- Modify: `crates/tama-core/src/models/pull/download.rs`
- Modify: `crates/tama-core/src/models/pull/api.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs`
- Modify: `crates/tama-core/src/models/search.rs`

**What to implement:**

1. **`pull/mod.rs`** — add directly above `hf_api()` (`HeaderMap` is already imported at line 19):
   ```rust
   /// Base URL for HuggingFace, honoring the `HF_ENDPOINT` env var (mirror support).
   pub(crate) fn hf_endpoint() -> String {
       std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string())
   }

   /// `{endpoint}/api/models` — model-list/search API base.
   pub(crate) fn hf_api_models_url() -> String {
       format!("{}/api/models", hf_endpoint())
   }

   /// `{endpoint}/api/models/{repo_id}`
   pub(crate) fn hf_api_model_url(repo_id: &str) -> String {
       format!("{}/{}", hf_api_models_url(), repo_id)
   }

   /// `{endpoint}/api/models/{repo_id}?blobs=true`
   pub(crate) fn hf_api_model_blobs_url(repo_id: &str) -> String {
       format!("{}?blobs=true", hf_api_model_url(repo_id))
   }

   /// `{endpoint}/{repo_id}/resolve/main/{filename}`
   pub(crate) fn hf_resolve_url(repo_id: &str, filename: &str) -> String {
       format!("{}/{}/resolve/main/{}", hf_endpoint(), repo_id, filename)
   }

   /// `{endpoint}/{repo_id}/raw/{branch}/{path}`
   pub(crate) fn hf_raw_url(repo_id: &str, branch: &str, path: &str) -> String {
       format!("{}/{}/raw/{}/{}", hf_endpoint(), repo_id, branch, path)
   }

   /// Authorization headers for HF requests; empty when no token is configured.
   /// An unparseable token is skipped (never aborts the request).
   pub(crate) fn hf_auth_headers() -> HeaderMap {
       let mut headers = HeaderMap::new();
       if let Some(token) = get_hf_token() {
           if let Ok(value) = format!("Bearer {}", token).parse::<reqwest::header::HeaderValue>() {
               headers.insert(reqwest::header::AUTHORIZATION, value);
           }
       }
       headers
   }
   ```

2. **`models/pull/download.rs`** — in `pull_gguf_with_progress`: lines 27-30 become `let url = super::hf_resolve_url(repo_id, filename);`; the header block at :37-45 (including the `format!(...).parse().context(...)?` fail-hard) becomes `let headers = super::hf_auth_headers();`.

3. **`models/pull/api.rs`** — `fetch_blob_metadata` (:82-84): `let url = super::hf_api_model_blobs_url(repo_id);`. `fetch_hf_metadata` (:115-118): `let url = super::hf_api_model_url(repo_id);`, and the README URLs (:167-168) become `let readme_url = super::hf_raw_url(repo_id, "main", "README.md");` / `let readme_fallback = super::hf_raw_url(repo_id, "master", "README.md");`. `fetch_model_pipeline_tag` (:210-212): `let url = super::hf_api_model_url(repo_id);`. Delete each now-unused `let endpoint = …;` local.

4. **`proxy/tama_handlers/pull/download.rs:174-186`** — becomes:
   ```rust
   // Resolve URL and auth headers (shared by HEAD + pull)
   let resolve_url = crate::models::pull::hf_resolve_url(&repo_id_clone, &filename_clone);
   let headers = crate::models::pull::hf_auth_headers();
   ```
   (Same lenient-parse semantics as the current code — no behavior change here.)

5. **`models/search.rs`** — delete `const HF_API_BASE` (line 4); the search URL (lines 56-62) becomes:
   ```rust
   let url = format!(
       "{}?search={}&library=gguf&sort={}&direction=-1&limit={}",
       crate::models::pull::hf_api_models_url(),
       urlencoding(query),
       sort.as_str(),
       limit,
   );
   ```
   **Behavior change (the fix):** search now honors `HF_ENDPOINT`.

6. **Tests** —
   - In `pull/mod.rs`'s existing `mod tests` (line 521, has `ENV_GUARD`): add `test_hf_resolve_url_default_endpoint` (`remove_var("HF_ENDPOINT")` → `"https://huggingface.co/org/model/resolve/main/model.gguf"`), `test_hf_resolve_url_custom_endpoint` (`set_var` to `https://hf.mirror.example.com` → mirror URL, then `remove_var`), `test_hf_api_model_blobs_url` (default endpoint → `"https://huggingface.co/api/models/org/repo?blobs=true"`), `test_hf_auth_headers_empty_token_omits_header` (`HF_TOKEN=""` → `headers.get(AUTHORIZATION).is_none()`), `test_hf_auth_headers_valid_token` (`HF_TOKEN="hf_test_token_123"` → `Some("Bearer hf_test_token_123")`). All take `_guard = ENV_GUARD.lock().unwrap()` per the existing pattern.
   - In `models/pull/download.rs`'s `mod tests`: **delete** `test_pull_gguf_url_construction_default_endpoint` and `test_pull_gguf_url_construction_custom_endpoint` (:83-117 — they assert on inline `format!` strings, not on any function; superseded by the real `hf_resolve_url` tests above). Rewrite `test_valid_token_produces_bearer_header`'s tail to assert on `super::super::hf_auth_headers()` instead of hand-building the header; keep `test_empty_token_no_auth_header`/`test_whitespace_token_no_auth_header` as-is (they test `get_hf_token`, which is unchanged).

**Steps:**
- [ ] Write the five new failing tests in `crates/tama-core/src/models/pull/mod.rs`; delete/rewrite the download.rs tests per above
- [ ] Run `cargo nextest run --package tama-core -- models::pull` — verify the new tests fail (missing helpers)
- [ ] Implement the helpers in `pull/mod.rs`; migrate the 6 sites per above
- [ ] Run `cargo nextest run --package tama-core -- models::pull` and `cargo nextest run --package tama-core -- models::search` and `cargo nextest run --package tama-core -- proxy::tama_handlers` — pass
- [ ] Run `cargo nextest run --package tama-core` — whole crate passes
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "refactor: shared HF endpoint/URL/auth helpers; search honors HF_ENDPOINT"

**Acceptance criteria:**
- [ ] `rg "HF_ENDPOINT" crates/tama-core/src/` → hits only in `pull/mod.rs` (helper + tests); `rg "HF_API_BASE" crates/` → 0 hits
- [ ] `rg "huggingface.co" crates/tama-core/src/ --type rust | grep -v test | grep -v pull/mod.rs` → 0 hits outside the helper and doc comments
- [ ] `search_models` builds its URL from `hf_api_models_url()`; all 6 former sites call the shared helpers
- [ ] `cargo nextest run --package tama-core` passes; `cargo clippy --workspace -- -D warnings` clean

---

### Task 6: One `is_valid_repo_id` validator (F18)

**Context:**
Three validators disagree on what a legal repo_id is (security-relevant): `crates/tama/src/api/models/crud/mod.rs:302-313` is a charset whitelist `[a-zA-Z0-9._\-/]` that allows `.` — so `../x` **passes** create/rename; `crates/tama-core/src/proxy/tama_handlers/types.rs:129-131` (`is_safe_path_component`) blacklists `..` (containment), `/`, `\`, NUL per component; `crates/tama/src/api/hf.rs:27-29` is an inline per-component closure that checks `!= ".."` (exact) and NUL but **omits the backslash check**. Decision — the strictest union, as one rule: split on `/`; every component must be non-empty, must not **contain** `..` (containment, the tama_handlers phrasing — rejects `..`, `../x`, and `foo..bar`), and may contain only ASCII alphanumerics, `.`, `_`, `-` (the crud charset — dots *inside* names are legitimate, e.g. `model.v2`; the whitelist inherently rejects `\`, NUL, and whitespace). Placement: `tama_core::models` (`crates/tama-core/src/models/mod.rs`, next to `config_key_to_repo_id`/`repo_path`) as a `pub fn`. `is_safe_path_component` **stays** — it validates single filenames at `tama_handlers/pull/download.rs:39` (a filename is not a repo_id); only its two repo_id uses migrate. Behavior tightenings to document in the commit body: crud create/rename now reject `../x`, `a//b`, `/abs`, `a/`; the tama_handlers sites (`system.rs:55`, `pull/download.rs:58`) additionally reject spaces/unicode in components; hf.rs now rejects `\`, `foo..bar`, and spaces/unicode.

**Files:**
- Modify: `crates/tama-core/src/models/mod.rs`
- Modify: `crates/tama/src/api/models/crud/mod.rs`
- Modify: `crates/tama/src/api/models/crud/create.rs`
- Modify: `crates/tama/src/api/models/crud/rename.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/system.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs`
- Modify: `crates/tama/src/api/hf.rs`

**What to implement:**

1. **`crates/tama-core/src/models/mod.rs`** — add after `repo_path`:
   ```rust
   /// Validate a HuggingFace-style repo_id (e.g. `"unsloth/gemma-4-26b-it-GGUF"`).
   ///
   /// Rules: split on `/`; every component must be non-empty (rejects `a//b`,
   /// leading/trailing slashes), must not contain `..` (rejects `..`, `../x`,
   /// `foo..bar`), and may contain only ASCII alphanumerics, `.`, `_`, `-`
   /// (dots inside names are legitimate: `model.v2`). The charset whitelist
   /// inherently rejects backslashes, NUL bytes, and whitespace.
   pub fn is_valid_repo_id(repo_id: &str) -> bool {
       if repo_id.is_empty() {
           return false;
       }
       repo_id.split('/').all(|component| {
           !component.is_empty()
               && !component.contains("..")
               && component
                   .chars()
                   .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
       })
   }
   ```

2. **`crud/mod.rs`** — delete the local `is_valid_repo_id` (lines 300-313, including its doc comment and the `// ── Validation helpers ──` header only if nothing else remains under it — `validate_model_body` follows, so keep the header). **`create.rs:6`**: remove `is_valid_repo_id` from the `use super::{…}` list, add `use tama_core::models::is_valid_repo_id;`. **`rename.rs:11`**: `use super::is_valid_repo_id;` → `use tama_core::models::is_valid_repo_id;`. Call sites unchanged (`is_valid_repo_id(&repo_id)` / `(&new_repo_id)`).

3. **`tama_handlers/system.rs:55`** — `if !repo_id.split('/').all(is_safe_path_component) {` → `if !crate::models::is_valid_repo_id(&repo_id) {` (400 body unchanged). Remove `is_safe_path_component` from the import at :13 **only if** it's now unused in this file (grep first).

4. **`tama_handlers/pull/download.rs:58`** — `if !repo_id_clone.split('/').all(is_safe_path_component) {` → `if !crate::models::is_valid_repo_id(&repo_id_clone) {` ("Invalid repo_id" failure path unchanged). **Keep** the `is_safe_path_component` import at :5 — the filename check at :39 still uses it.

5. **`crates/tama/src/api/hf.rs:27-29`** — replace the inline closure check with `if !tama_core::models::is_valid_repo_id(&repo_id) {` (the 400 `{"error": "Invalid repo_id"}` body is unchanged).

6. **Tests** — add `#[cfg(test)] mod tests` at the bottom of `crates/tama-core/src/models/mod.rs` (the file already declares `mod manager_tests;` at :16 — a second inline test module named `tests` is fine):
   - `test_is_valid_repo_id_accepts_legitimate`: `"unsloth/gemma-4-26b-it-GGUF"`, `"model.v2"`, `"a"`, `"Org_Name/Repo-Name.1"`, `"a/b/c"` → `true`.
   - `test_is_valid_repo_id_rejects_traversal`: `".."`, `"../x"`, `"a/../b"`, `"foo..bar"` → `false`.
   - `test_is_valid_repo_id_rejects_empty_components`: `""`, `"a//b"`, `"/a"`, `"a/"` → `false`.
   - `test_is_valid_repo_id_rejects_backslash_nul_whitespace`: `"a\\b"`, `"a\0b"`, `"a b"`, `"owner/repo name"` → `false`.
   - The existing `is_safe_path_component` tests (`types.rs:177-190`) stay untouched.

**Steps:**
- [ ] Write the four failing tests in `crates/tama-core/src/models/mod.rs`
- [ ] Run `cargo nextest run --package tama-core -- models::` — verify failure (missing fn)
- [ ] Implement `is_valid_repo_id`; migrate the five call sites (items 2–5)
- [ ] Run `cargo nextest run --package tama-core -- models` and `cargo nextest run --package tama -- api::models` and `cargo nextest run --package tama -- api::hf` — pass
- [ ] Run `cargo nextest run --workspace` — full suite green (validator tightens behavior in two crates)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` — clean
- [ ] Commit with message: "fix: unify repo_id validation; reject traversal at all three entry points"

**Acceptance criteria:**
- [ ] `rg "is_valid_repo_id" crates/` → one definition (`tama_core::models`) + 5 call sites; `rg "is_safe_path_component" crates/` → only the filename check (`pull/download.rs:39`), its definition, and its tests
- [ ] `../x` is rejected at create, rename, HF-metadata, HF-quant-list, and pull entry points; `model.v2` is accepted everywhere
- [ ] `cargo nextest run --workspace` passes; `cargo clippy --workspace -- -D warnings` clean
