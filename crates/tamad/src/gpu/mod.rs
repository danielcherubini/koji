//! Host-side GPU sampling (plan-191 Task 10).
//!
//! Moved here from `tama_core::gpu` — ADR-0010: the proxy never samples
//! local hardware, and the dependency graph now enforces it. The shared
//! wire types (`GpuDeviceStats`, `SystemMetrics`, `VramInfo`,
//! `GpuVariant`, ...) stay in `tama_core::gpu`.

pub mod amd;
pub mod detect;
pub mod env;
pub mod nvidia;
pub mod system;
#[cfg(test)]
mod tests;
pub mod vram;
