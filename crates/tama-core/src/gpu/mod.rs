//! Shared GPU types + pure helpers (plan-191 Task 10).
//!
//! Host-side GPU sampling — `nvidia-smi`/`rocm-smi` subprocesses, sysinfo
//! GPU enumeration, VRAM queries, `--list-devices` discovery — moved to the
//! tamad crate (`tama::gpu` in `crates/tamad/src/gpu/`). ADR-0010: the proxy
//! never samples local hardware, and the dependency graph now enforces it.
//!
//! What remains here is what both binaries legitimately share:
//! - wire/data types (per-GPU stats, system metrics snapshots, model state);
//! - `GpuVariant` and its (de)serialization (the `gpu_variant` folder name
//!   stored in the central DB);
//! - pure heuristics (context-size suggestions, rocminfo output parsing);
//! - the toolchain probe used by the install-wizard capabilities endpoint
//!   (cmake/git/compiler availability — never backend or GPU sampling).

pub mod detect;
pub mod types;

pub use detect::{
    detect_build_prerequisites, parse_rocminfo_gfx_names, suggest_context_sizes,
    BuildPrerequisites, ContextSuggestion, GpuVariant, DEFAULT_CUDA_VERSION,
};
pub use types::{
    GpuDeviceStats, GpuVendor, MetricBucket, MetricCurrent, MetricSample, MetricsSnapshot,
    ModelState, SystemMetrics, VramInfo,
};
