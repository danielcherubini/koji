# Backend GPU variant restructure

Backends are organized by `type/variant/version` folder layout (e.g. `llama.cpp/cuda/v0.3.`, `llama.cpp/rocm/v0.3.`, `ik_llama/cuda/v1.0`). The `gpu_variant` field on a backend identifies its compilation target: `cuda`, `rocm`, `vulkan`, `metal`, or `cpu`. A single backend name can have multiple gpu_variants installed simultaneously, and models reference a specific `(name, gpu_variant)` pair.

This replaced the flat backend layout where each backend had a single binary regardless of GPU target. The restructure was needed because users commonly run mixed-GPU setups (e.g. NVIDIA + AMD) and need different backend binaries for each vendor. The DB schema uses a unique index on `(name, gpu_variant, version)` with `INSERT OR REPLACE` to manage multiple active variants.

The `gpu_variant` also determines which GPU isolation env var the proxy sets at spawn time (ADR-0007): a Vulkan backend on an AMD card needs `GGML_VK_VISIBLE_DEVICES`, not `ROCR_VISIBLE_DEVICES`. The variant is a property of the backend binary, not the physical GPU.

**Status:** accepted

**Considered Options:**

- **Flat backend layout** (status quo) — one binary per backend name; impossible to run CUDA and ROCm variants simultaneously
- **type/variant/version folders** (chosen) — explicit variant separation; supports mixed-GPU setups and per-variant updates
- **Single multi-variant binary** — backend detects GPU at runtime; not feasible for `llama.cpp` (compile-time CUDA/ROCm flags)
- **Naming convention** (e.g. `llama.cpp-cuda`, `llama.cpp-rocm` as separate backend names) — works but duplicates config and loses the semantic grouping

**Consequences:**

- Good, because mixed-GPU setups work naturally — CUDA backends on NVIDIA cards, ROCm on AMD
- Good, because per-variant updates — update CUDA variant without touching ROCm
- Good, because `gpu_variant` drives GPU isolation env var selection at spawn time
- Bad, because DB queries must include `gpu_variant` (more complex joins)
- Bad, because backend discovery must scan variant subdirectories
