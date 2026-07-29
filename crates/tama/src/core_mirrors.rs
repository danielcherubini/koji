//! Mirror types from tama-core that can be used from WASM.
//!
//! These are re-exports of `crate::core_shared` (which bridges to
//! `tama_core::types` on ssr and includes the same source files on csr).
//! Kept as a stable module name so existing `crate::core_mirrors::*` imports
//! keep working.
pub use crate::core_shared::{CompactionDevice, GpuVendor, LogLevel, ModelState, RestartPolicy};
