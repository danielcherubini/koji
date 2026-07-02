# GPU isolation via environment variables (UUID)

## Context and Problem Statement

When multiple backends run on different GPUs, Tama needs to isolate each backend to its assigned GPU. The original approach used `--device` flags injected into backend CLI args, but this was fragile — backends interpreted device IDs differently (ordinal vs. UUID vs. PCI bus), and AMD/NVIDIA use different flag conventions.

## Decision Drivers

* Works consistently across CUDA and ROCm backends
* GPU-agnostic — does not depend on backend-specific CLI flags
* Stable identifiers — UUIDs persist across reboots, unlike ordinals
* Supports multi-GPU setups with mixed vendors

## Considered Options

* Environment variables (`CUDA_VISIBLE_DEVICES`, `ROCR_VISIBLE_DEVICES`) with UUIDs
* CLI `--device` flag injection (status quo)
* Positional GPU indexes (GPU0, GPU1, …)

## Decision Outcome

Chosen option: "Environment variables with UUIDs", because `CUDA_VISIBLE_DEVICES` and `ROCR_VISIBLE_DEVICES` accept UUIDs and are respected by all major backends (llama.cpp, ik_llama). UUIDs are captured during GPU enumeration via `nvidia-smi` (NVIDIA) and `rocm-smi` (AMD), normalized to the `GPU-xxxxxxxx-xxxxxxxx-xxxxxxxx` format, and injected into the backend's environment at spawn time.

The approach evolved from UUID-based isolation to positional indexes keyed off `gpu_variant` (commit `1d9e91e6`), which maps model configs to GPU positions (0, 1, 2, …) rather than raw UUIDs. This simplifies the UI — users select a GPU by position, and the proxy resolves the correct env var at spawn time.

### Consequences

* Good, because env vars work with all backends — no CLI flag parsing needed
* Good, because UUIDs are stable across reboots and driver updates
* Good, because `CUDA_VISIBLE_DEVICES`/`ROCR_VISIBLE_DEVICES` are standard
* Bad, because requires `nvidia-smi`/`rocm-smi` to be installed for UUID capture
* Bad, because AMD UUID format normalization (0x prefix → GPU-) adds complexity

### Confirmation

GPU UUIDs are captured during device enumeration (`gpu::system::collect_system_metrics`). The `inject_gpu_env` helper sets `CUDA_VISIBLE_DEVICES` or `ROCR_VISIBLE_DEVICES` based on the backend's `gpu_variant`. The `ProcessSupervisor` passes the resolved env to spawned backends. Tests verify UUID normalization, env injection, and fallback to legacy device IDs.

## Pros and Cons of the Options

### Environment variables with UUIDs

Set `CUDA_VISIBLE_DEVICES` / `ROCR_VISIBLE_DEVICES` at spawn time.

* Good, because universal — all backends respect these env vars
* Good, because UUIDs are stable identifiers
* Good, because no CLI flag parsing or backend-specific logic
* Bad, because requires SMI tools for initial UUID capture

### CLI --device flag injection

Pass `--device N` or `--device UUID` to backend args.

* Good, because explicit — visible in process args
* Bad, because backends use different flag names and formats
* Bad, because fragile — backend updates may change flag behavior
* Bad, because requires knowing the backend's flag convention

### Positional GPU indexes

Use GPU0, GPU1, … and map to devices at runtime.

* Good, because simple — users select by position
* Good, because the proxy handles the mapping to UUIDs/env vars
* Bad, because positions can change if GPUs are added/removed
* Bad, because less intuitive than named devices

## More Information

* PR #130: [GPU isolation via env-var (UUID)](https://github.com/danielcherubini/tama/pull/130)
* Implementation plan: `docs/plans/2026-07-01-gpu-env-var-isolation.md`
