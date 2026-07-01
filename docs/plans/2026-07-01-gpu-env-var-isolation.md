# GPU Isolation via Env-Var (UUID) + MTP Model API Fix Plan

**Goal:** Replace the `--device` CLI-flag approach to GPU selection with driver-level env-var isolation using hardware UUIDs, so a backend process (main model + MTP draft) is pinned to exactly one GPU card. Also fix the unrelated `mtp_model` API response bug.

**Architecture:** When `gpu_device` is set, tama resolves the selected `GPU<N>` to its hardware UUID (captured during GPU enumeration) and sets the vendor visibility env var (`ROCR_VISIBLE_DEVICES` for AMD, `CUDA_VISIBLE_DEVICES` for NVIDIA) on the spawned backend process. This hides all other GPUs, forcing both the main model and the MTP draft model onto the single visible card (auto-placement — no `--device-draft` needed). The `--device` CLI flag injection is removed entirely.

**Tech Stack:** Rust, sysfs / nvidia-smi / rocm-smi enumeration, llama.cpp backend processes

**Key research findings (authoritative):**
- `--device` is a *restrictor* (picks from visible GPUs), not a *hider*. Only `*_VISIBLE_DEVICES` env vars hide GPUs. (llama.cpp docs/multi-gpu.md)
- When `HIP_VISIBLE_DEVICES=N` is set, the GPU is remapped to index 0, so `--device ROCm1` would *fail* — the two approaches are mutually exclusive. (llama.cpp issue #23152)
- For single-GPU MTP, `--device-draft` is unnecessary — the draft auto-places on the only visible GPU. (llama.cpp discussions #23751, #24927)
- AMD recommends UUIDs via `ROCR_VISIBLE_DEVICES` (supports UUIDs; `HIP_VISIBLE_DEVICES` is indices-only). (AMD ROCm gpu-isolation docs)
- Tama's `gpu/system.rs` already correlates AMD devices by PCI bus via rocm-smi — reusable for UUID lookup.

---

### Task 1: Capture UUIDs during GPU enumeration

**Context:** Tama's `gpu/system.rs` enumerates all physical GPUs and assigns position-based IDs `GPU0`, `GPU1`, … (vendor-sorted). Today it captures `device_id`, `vendor`, `name`, and stats — but no UUID. To set `ROCR_VISIBLE_DEVICES`/`CUDA_VISIBLE_DEVICES` by UUID, each enumerated GPU must carry its hardware UUID. AMD correlation already happens by PCI bus (the `AMD_DEVICE_NAMES` static uses `rocm-smi --showbus --showproductname --json` → PCI bus → name); we extend this to also fetch UUIDs. NVIDIA's `nvidia-smi --query-gpu` supports a `uuid` field directly, and its index already equals the CUDA runtime index.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs`

**What to implement:**

1. Add two fields to `GpuDeviceStats`:
   ```rust
   /// PCI bus address (e.g. "0000:03:00.0") used for vendor-tool correlation. None for NVIDIA (uses index directly) or when unavailable.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub pci_bus: Option<String>,
   /// Hardware UUID for env-var GPU isolation (e.g. "GPU-4b2c1a9f-..."). None when unavailable (Vulkan/Metal/no tooling).
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub uuid: Option<String>,
   ```
   Update every `GpuDeviceStats { ... }` construction site to include the new fields (set `None` where not yet populated; they'll be filled in below).

2. **AMD UUID capture:** Add a second `OnceLock<HashMap<String, String>>` static `AMD_DEVICE_UUIDS` (PCI bus → UUID), populated by a `query_amd_device_uuids()` function modelled exactly on the existing `query_amd_device_names()`. Extend the `rocm-smi` invocation to also pass `--showuniqueid` (alongside `--showbus` and `--showproductname`) and parse the `"Unique ID"` field from each card's JSON object, keyed by `"PCI Bus"`. In `query_amd_devices()`, where `pci_bus` is already read from `uevent` `PCI_SLOT_NAME`, look up the UUID the same way the name is looked up:
   ```rust
   let uuid = pci_bus.as_ref().and_then(|pci| query_amd_device_uuids().get(pci).cloned());
   ```
   Populate `pci_bus` and `uuid` on the AMD `GpuDeviceStats`.

3. **NVIDIA UUID capture:** In `query_nvidia_devices()`, add `uuid` to the `--query-gpu=` field list (e.g. `index,name,uuid,utilization.gpu,...`). Update `parse_nvidia_smi_csv_line` to expect 9 fields (insert `uuid` at index 2) and populate the `uuid` field. NVIDIA has no PCI-bus correlation need, so `pci_bus` stays `None` for NVIDIA.

4. **Extract `detect_gpu_devices()`:** Refactor the GPU-detection portion of `collect_system_metrics_with()` (the `query_nvidia_devices()` + `query_amd_devices()` + sort + assign-`GPU<N>` block, lines ~315-323) into a standalone `pub fn detect_gpu_devices() -> Vec<GpuDeviceStats>` that returns the full list WITH `GPU<N>` IDs and UUIDs assigned. Have `collect_system_metrics_with()` call it. This shared function is what env-var resolution (Task 2) will call so position IDs stay consistent between metrics and launch-time resolution.

**Do NOT change:**
- The sort order or `GPU<N>` assignment logic (must stay identical so existing behavior is preserved).
- The `--list-devices` probe in `gpu/discover.rs` (still used for backend device listing/VRAM display).

**Steps:**
- [ ] Add `pci_bus` and `uuid` fields to `GpuDeviceStats`; update all construction sites.
- [ ] Add `AMD_DEVICE_UUIDS` OnceLock + `query_amd_device_uuids()` mirroring `query_amd_device_names()`; extend the rocm-smi args with `--showuniqueid`; parse `"Unique ID"` keyed by `"PCI Bus"`.
- [ ] Populate `uuid` (and `pci_bus`) in `query_amd_devices()`.
- [ ] Add `uuid` to the nvidia-smi `--query-gpu` field list; update `parse_nvidia_smi_csv_line` to 9 fields.
- [ ] Extract `detect_gpu_devices()` and call it from `collect_system_metrics_with()`.
- [ ] Write a unit test `test_detect_gpu_devices_assigns_position_ids` using a mock-friendly approach: since `detect_gpu_devices` shells out, test the *sort + assign* logic by extracting a pure helper `assign_position_ids(mut Vec<GpuDeviceStats>) -> Vec<GpuDeviceStats>` and asserting it sorts by `(vendor, device_id)` then assigns `GPU0`/`GPU1`/… (no subprocess needed).
- [ ] Run `cargo test --package tama-core -- gpu::system`
- [ ] Run `cargo fmt --all` and `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit: "feat: capture GPU UUIDs during enumeration (AMD via rocm-smi, NVIDIA via nvidia-smi)"

**Acceptance criteria:**
- [ ] `GpuDeviceStats` carries `uuid` and `pci_bus`
- [ ] AMD devices get UUID via PCI-bus correlation from `rocm-smi --showuniqueid`
- [ ] NVIDIA devices get UUID via the `uuid` nvidia-smi field
- [ ] `detect_gpu_devices()` is a reusable public function returning `GPU<N>`-assigned devices with UUIDs
- [ ] Existing metrics tests still pass

---

### Task 2: Add `inject_gpu_env` helper

**Context:** With UUIDs now available per GPU (Task 1), we need a helper that resolves a selected `gpu_device` string (`GPU<N>`) to its UUID + vendor, maps the vendor to the correct env var name, and sets it on a `Command`. This helper is the single source of truth for env-var GPU isolation and is called from both the proxy spawn site and `tama run`. It must NOT be applied to the `--list-devices` probe (which must keep seeing all devices).

**Files:**
- Create: `crates/tama-core/src/gpu/env.rs`
- Modify: `crates/tama-core/src/gpu/mod.rs` (add `pub mod env;`)

**What to implement:**

1. A resolution function:
   ```rust
   /// Resolve a `gpu_device` string (e.g. "GPU1") to (env_var_name, value) for
   /// driver-level GPU isolation. Returns None if the device is not found, has
   /// no UUID, or the vendor has no env-var mechanism.
   pub fn resolve_gpu_device_env(gpu_device: &str) -> Option<(String, String)> {
       let device = gpu_device.trim();
       if device.is_empty() { return None; }
       let gpus = super::system::detect_gpu_devices();
       let gpu = gpus.into_iter().find(|g| g.device_id == device)?;
       match gpu.vendor.as_str() {
           "amd" => gpu.uuid.as_ref().map(|u| ("ROCR_VISIBLE_DEVICES".to_string(), u.clone())),
           "nvidia" => gpu.uuid.as_ref().map(|u| ("CUDA_VISIBLE_DEVICES".to_string(), u.clone())),
           // Vulkan: no UUID env var; GGML_VK_VISIBLE_DEVICES uses indices — degraded, out of scope for UUID path.
           _ => None,
       }
   }
   ```
   (For AMD use `ROCR_VISIBLE_DEVICES` per AMD's recommendation — supports UUIDs. Do NOT also set `HIP_VISIBLE_DEVICES` to avoid the conflict documented in vLLM/PyTorch.)

2. An injection helper that takes a generic command (reuse the `BackendCommand` trait from `crate::process` so it works for both `tokio::process::Command` and `std::process::Command`):
   ```rust
   pub fn inject_gpu_env(cmd: &mut impl crate::process::BackendCommand, gpu_device: &Option<String>) {
       if let Some(device) = gpu_device {
           if let Some((name, value)) = resolve_gpu_device_env(device) {
               cmd.env(&name, &value);
           }
       }
   }
   ```
   Check whether `BackendCommand` exposes `env`; if not, extend the trait with an `env(&mut self, key: &str, value: &str)` method (both impls already delegate to the underlying `Command::env`).

3. A pure helper `vendor_env_var(vendor: &str, uuid: &str) -> Option<(String, String)>` extracted from the match above, so it can be unit-tested without subprocesses.

**Steps:**
- [ ] Create `crates/tama-core/src/gpu/env.rs` with `resolve_gpu_device_env`, `inject_gpu_env`, and `vendor_env_var`.
- [ ] Add `pub mod env;` to `crates/tama-core/src/gpu/mod.rs`.
- [ ] If needed, extend `BackendCommand` trait (`crates/tama-core/src/process.rs`) with `fn env(&mut self, key: &str, value: &str);` and implement for both `tokio` and `std` `Command`.
- [ ] Write unit tests in `gpu/env.rs`:
  - `test_vendor_env_var_amd` → `("ROCR_VISIBLE_DEVICES", uuid)`
  - `test_vendor_env_var_nvidia` → `("CUDA_VISIBLE_DEVICES", uuid)`
  - `test_vendor_env_var_unknown` → `None`
  - `test_resolve_gpu_device_env_empty_and_none` → `None` for `""` and missing device (use a detect mock or assert None when device_id not found — since `detect_gpu_devices` shells out, prefer testing `vendor_env_var` and a resolution helper that takes a `&[GpuDeviceStats]` slice. Refactor `resolve_gpu_device_env` to delegate to `resolve_gpu_device_env_from(gpu_device, &detect_gpu_devices())` and test the `_from` variant with a hand-built slice.)
- [ ] Run `cargo test --package tama-core -- gpu::env`
- [ ] Run `cargo fmt --all` and `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit: "feat: add inject_gpu_env helper for UUID-based GPU isolation"

**Acceptance criteria:**
- [ ] `resolve_gpu_device_env` maps `GPU<N>` → vendor env var + UUID
- [ ] AMD → `ROCR_VISIBLE_DEVICES=<uuid>`, NVIDIA → `CUDA_VISIBLE_DEVICES=<uuid>`, Vulkan/unknown → None
- [ ] `inject_gpu_env` sets the env var on a `Command` and is a no-op when `gpu_device` is None or unresolvable
- [ ] Helper works for both `tokio` and `std` `Command` via `BackendCommand`

---

### Task 3: Drop `--device` injection, inject env var at proxy spawn site

**Context:** The proxy's `load_model` currently injects `--device` via `build_full_args` and then overrides it with the llama.cpp-mapped name via `resolve_gpu_device_to_backend_name`. Per the research, this is the wrong mechanism — env vars hide GPUs entirely and are mutually exclusive with `--device` (the env var remaps to index 0, breaking `--device ROCm1`). This task removes the `--device` path from the proxy and calls `inject_gpu_env` at the spawn site instead.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`
- Modify: `crates/tama-core/src/config/resolve/tests/gpu_device.rs`

**What to implement:**

1. **`config/resolve/mod.rs`** — Remove the entire `--device` injection block (the `if is_llama_cpp_backend { if let Some(ref device) = server.gpu_device { ... grouped.push("--device ...") } }` block, around lines 513-525). `gpu_device` no longer produces a CLI flag. Leave the `is_llama_cpp_backend` variable in place if still used by the `--alias` block above; otherwise remove if now unused (let clippy guide).

2. **`proxy/lifecycle/mod.rs`**:
   - Remove the `resolve_gpu_device_to_backend_name` call and the `override_arg(&mut args, "--device", &mapped_device)` block (around lines 156-166). The `resolve_gpu_device_to_backend_name` function itself (around 682-707) may now be unused — check all callers; if unused, remove it (and its `--list-devices` dependency is then only the discovery probe, which stays).
   - At the spawn site (around line 182, where `configure_backend_command` and `.env("MODEL_NAME", ...)` are called), add `crate::gpu::env::inject_gpu_env(&mut child, &server_config.gpu_device);` before `.args(&args)`. Only inject when the backend is a GPU backend — gate on `gpu_variant` being one of `cuda`/`rocm`/`vulkan` (i.e. not `cpu`); a `gpu_device` set on a `cpu` variant should be ignored.

3. **Tests in `gpu_device.rs`:** The existing tests assert `--device` is injected via `build_full_args`. Since `--device` is no longer injected, these tests must be rewritten:
   - `test_gpu_device_injected_for_rocm` → rename to `test_gpu_device_not_injected_as_cli_arg` and assert `--device` is NOT in args (the env var is set at spawn, not in `build_full_args`).
   - `test_gpu_device_none_no_injection`, `test_gpu_device_no_duplicate_when_already_set`, `test_gpu_device_not_injected_for_non_llama_cpp`, `test_gpu_device_empty_string_no_injection` → update to assert no `--device` appears in args regardless of `gpu_device` (the `build_full_args` no longer touches `gpu_device`).
   - Keep them as `build_full_args`-level tests (they verify the args vector). Env-var injection itself is tested in Task 2.

**Do NOT change:**
- The `--list-devices` probe in `gpu/discover.rs` (still used for device listing).
- The `gpu_device` field on `ModelConfig` (still the config input; now consumed by `inject_gpu_env` instead of `build_full_args`).

**Steps:**
- [ ] Remove the `--device` injection block from `build_full_args` in `resolve/mod.rs`.
- [ ] Remove the `resolve_gpu_device_to_backend_name` + `override_arg(--device)` block from `load_model`; remove the now-unused `resolve_gpu_device_to_backend_name` function if no other callers.
- [ ] Add `inject_gpu_env(&mut child, &server_config.gpu_device)` at the spawn site, gated on non-cpu `gpu_variant`.
- [ ] Rewrite the `gpu_device.rs` tests to assert `--device` is absent from args.
- [ ] Run `cargo test --package tama-core -- gpu_device`
- [ ] Run `cargo test --package tama-core` (catch any regressions from removing the function)
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`
- [ ] Commit: "refactor: drop --device injection, use env-var GPU isolation at proxy spawn"

**Acceptance criteria:**
- [ ] `build_full_args` no longer injects `--device`
- [ ] `load_model` sets the vendor env var on the spawned process via `inject_gpu_env`
- [ ] `resolve_gpu_device_to_backend_name` removed if unused
- [ ] `gpu_device.rs` tests updated and passing
- [ ] No regressions in `tama-core` test suite

---

### Task 4: Wire env-var GPU isolation into `tama run`

**Context:** `tama run` (the standalone CLI foreground runner) currently passes `gpu_device` to `build_full_args`, which injects `--device GPU1` as a *literal string* without ever resolving it via `resolve_gpu_device_to_backend_name` — so it's already half-broken for GPU selection. After Task 3 removes `--device` injection from `build_full_args`, `tama run` would lose GPU selection entirely unless we wire `inject_gpu_env` into its spawn path. This task adds env-var injection to `ProcessSupervisor` so `tama run` gets the same isolation as the proxy (and fixes the pre-existing gap).

**Files:**
- Modify: `crates/tama-core/src/process.rs` (`ProcessSupervisor`)
- Modify: `crates/tama-cli/src/handlers/run.rs`

**What to implement:**

1. **`ProcessSupervisor`** — add an optional env-var field:
   ```rust
   pub struct ProcessSupervisor {
       // ... existing fields ...
       /// Optional (env_var_name, value) pairs for driver-level GPU isolation,
       /// resolved from the model's `gpu_device` before constructing the supervisor.
       gpu_env: Option<(String, String)>,
   }
   ```
   Add a builder method `pub fn with_gpu_env(mut self, env: Option<(String, String)>) -> Self`. In `run()`, at the spawn site (around line 137, after `configure_backend_command(&mut cmd, exe)`), apply it:
   ```rust
   if let Some((k, v)) = &self.gpu_env {
       cmd.env(k, v);
   }
   ```
   (Use `std::process::Command`'s native `env` — `ProcessSupervisor` uses `std::process::Command` per `run()`'s `Command::new(exe)`.)

2. **`handlers/run.rs`** — In `cmd_run`, after resolving `server.gpu_device`, resolve the env var and pass it to the supervisor:
   ```rust
   let gpu_env = server.gpu_device.as_deref().and_then(crate::gpu_env_placeholder...);
   ```
   Concretely: call `tama_core::gpu::env::resolve_gpu_device_env(gpu_device)` (the helper from Task 2) and pass the result via `.with_gpu_env(...)`. Gate on non-cpu `gpu_variant` (skip resolution when `gpu_variant == "cpu"`). Construct the supervisor with the env:
   ```rust
   let supervisor = ProcessSupervisor::new(...)
       .with_gpu_env(gpu_env);
   ```

**Do NOT change:**
- `ProcessSupervisor::new`'s existing signature (add via builder, don't break callers).
- The bench harness or discovery probe (they don't get GPU env).

**Steps:**
- [ ] Add `gpu_env: Option<(String, String)>` field + `with_gpu_env` builder to `ProcessSupervisor`.
- [ ] Apply `gpu_env` in `run()` at spawn.
- [ ] In `cmd_run`, resolve env via `tama_core::gpu::env::resolve_gpu_device_env` and pass via `.with_gpu_env(...)`.
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --package tama-core -- process`
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`
- [ ] Commit: "feat: wire env-var GPU isolation into tama run (ProcessSupervisor)"

**Acceptance criteria:**
- [ ] `ProcessSupervisor` applies the GPU env var at spawn when set
- [ ] `tama run` resolves `gpu_device` → env var and passes it to the supervisor
- [ ] Existing `ProcessSupervisor` callers unaffected (builder is optional)
- [ ] `cargo build --workspace` succeeds

---

### Task 5: Fix `mtp_model` missing from API response

**Context:** The `model_entry_json` function in `info.rs` builds the JSON for `GET /tama/v1/models/:id` and `GET /tama/v1/models`. The DB stores `selected_mtp_model`, and `ModelConfig::from_db_record` rehydrates it into `m.mtp_model`, but the JSON builder omits `mtp_model` entirely — so the frontend always receives `null` and the model editor's "MTP Draft Model" dropdown shows "(none)". This is an unrelated bug fix included in this branch.

**Files:**
- Modify: `crates/tama-web/src/api/models/info.rs`

**What to implement:**
In `model_entry_json`, add `"mtp_model": m.mtp_model,` to the `serde_json::json!` macro, placed right after the existing `"mmproj": m.mmproj,` line. Use `m.mtp_model` (the `ModelConfig` field), consistent with how `mmproj`, `quant`, and `model` are sourced.

Add a `#[cfg(test)] mod tests` at the bottom of `info.rs` with a test `test_model_entry_json_includes_mtp_model`:
- Build a `ModelConfigRecord` with `selected_mtp_model: Some("mtp-test.gguf".to_string())` and minimal defaults for all other fields.
- Build a `ModelConfig` with `mtp_model: Some("mtp-test.gguf".to_string())` and minimal defaults.
- Call `model_entry_json(1, &record, &config, &std::path::Path::new("."), None)`.
- Assert `result.get("mtp_model").and_then(|v| v.as_str()) == Some("mtp-test.gguf")`.
- Test the `None` case: `config.mtp_model = None`; assert `result["mtp_model"].is_null()`.

**CRITICAL — test compilation:** `info.rs` is gated behind `#[cfg(feature = "ssr")]`. The test only compiles/runs with `--features ssr`. Run with:
```bash
cargo test --package tama-web --lib --features ssr -- mtp_model
```
(The prior attempt's test silently didn't compile/run without `--features ssr`, masking a missing fix.)

**Steps:**
- [ ] Add `"mtp_model": m.mtp_model,` after `"mmproj": m.mmproj,` in `model_entry_json`.
- [ ] Add the `#[cfg(test)] mod tests` with `test_model_entry_json_includes_mtp_model` (both Some and None cases).
- [ ] Run `cargo test --package tama-web --lib --features ssr -- mtp_model` — verify the test COMPILES and PASSES.
- [ ] Run `cargo test --package tama-core -- mtp_model` (existing round-trip tests still pass).
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace --features ssr -- -D warnings` (if the web clippy needs ssr)
- [ ] Commit: "fix: include mtp_model in API model response JSON"

**Acceptance criteria:**
- [ ] `GET /tama/v1/models/:id` returns `mtp_model` in the JSON
- [ ] `GET /tama/v1/models` includes `mtp_model` per model
- [ ] The new unit test compiles AND passes with `--features ssr`
- [ ] Existing `mtp_model` DB round-trip tests still pass
