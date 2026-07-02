# GPU isolation via positional indexes and environment variables

## Context and Problem Statement

When multiple backends run on different GPUs, Tama needs to isolate each backend to its assigned GPU. The original approach used `--device` flags injected into backend CLI args, but this was fragile — backends interpreted device IDs differently (ordinal vs. UUID vs. PCI bus), and AMD/NVIDIA use different flag conventions.

## Decision Drivers

* Works consistently across CUDA, ROCm, and Vulkan backends
* GPU-agnostic — does not depend on backend-specific CLI flags
* Simple UI — users select GPUs by position (GPU0, GPU1, …)
* Supports multi-GPU setups with mixed vendors

## Considered Options

* Environment variables with positional indexes (`CUDA_VISIBLE_DEVICES`, `ROCR_VISIBLE_DEVICES`, `GGML_VK_VISIBLE_DEVICES`)
* Environment variables with UUIDs
* CLI `--device` flag injection

## Decision Outcome

Chosen option: "Environment variables with positional indexes", because `CUDA_VISIBLE_DEVICES`, `ROCR_VISIBLE_DEVICES`, and `GGML_VK_VISIBLE_DEVICES` accept integer indexes and are respected by all major backends. Users select a GPU by position (GPU0, GPU1, …) in the UI, and the proxy resolves the correct per-vendor index and env var at spawn time based on the backend's `gpu_variant`.

The approach evolved from UUID-based isolation (PR #130) to positional indexes (commit `1d9e91e6`). UUIDs were initially chosen for stability across reboots, but positional indexes proved simpler for the UI and sufficient for Tama's use case — the proxy re-enumerates GPUs on startup anyway.

### Consequences

* Good, because env vars work with all backends — no CLI flag parsing needed
* Good, because positional indexes are simple — users select GPU0, GPU1, …
* Good, because `CUDA_VISIBLE_DEVICES`/`ROCR_VISIBLE_DEVICES`/`GGML_VK_VISIBLE_DEVICES` are standard
* Good, because the env var is chosen by the backend's `gpu_variant` (what the binary was compiled for), not the GPU's physical vendor — e.g. an AMD card running a Vulkan backend needs `GGML_VK_VISIBLE_DEVICES`, not `ROCR_VISIBLE_DEVICES`
* Bad, because positions can change if GPUs are added/removed between reboots
* Bad, because requires `nvidia-smi`/`rocm-smi`/sysfs for initial device enumeration

### Confirmation

GPU devices are enumerated via `gpu::system::detect_gpu_devices` (blocks on `nvidia-smi`, `rocm-smi`, sysfs). The `resolve_gpu_env` function in `gpu::env` maps a `gpu_device` string (e.g. "GPU1") + `gpu_variant` to `(env_var_name, per_vendor_index)`. The `ProcessSupervisor` passes the resolved env to spawned backends. Tests verify index resolution, mixed-vendor setups, and edge cases.

## Pros and Cons of the Options

### Environment variables with positional indexes

Set `CUDA_VISIBLE_DEVICES` / `ROCR_VISIBLE_DEVICES` / `GGML_VK_VISIBLE_DEVICES` to per-vendor integer indexes at spawn time.

* Good, because universal — all backends respect these env vars
* Good, because simple — users select by position, no UUID complexity
* Good, because the proxy handles the mapping from global position to per-vendor index
* Bad, because positions can change if GPUs are added/removed

### Environment variables with UUIDs

Set `CUDA_VISIBLE_DEVICES` / `ROCR_VISIBLE_DEVICES` to UUIDs at spawn time.

* Good, because UUIDs are stable identifiers across reboots
* Good, because universal — all backends respect these env vars
* Bad, because AMD UUID format normalization (0x prefix → GPU-) adds complexity
* Bad, because requires SMI tools for initial UUID capture
* Bad, because more complex UI — users must understand UUIDs

### CLI --device flag injection

Pass `--device N` or `--device UUID` to backend args.

* Good, because explicit — visible in process args
* Bad, because backends use different flag names and formats
* Bad, because fragile — backend updates may change flag behavior
* Bad, because requires knowing the backend's flag convention

## More Information

* PR #130: [GPU isolation via env-var (UUID)](https://github.com/danielcherubini/tama/pull/130)
* Implementation plan: `docs/plans/2026-07-01-gpu-env-var-isolation.md`
* [ADR-0006](./0006-openai-compatible-api-proxy-pattern.md) — The proxy uses GPU env vars at spawn time
* [ADR-0004](./0004-linux-only-drop-windows-support.md) — Linux-only scope simplifies GPU enumeration (no Windows WSL edge cases)
* Module: `crates/tama-core/src/gpu/env.rs` — `resolve_gpu_env` and `resolve_gpu_env_from`

