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
pub use tama_core::types::enums::{CompactionDevice, LogLevel, RestartPolicy};
#[cfg(feature = "ssr")]
pub use tama_core::types::gpu::{GpuVendor, ModelState};
#[cfg(feature = "ssr")]
pub use tama_core::types::quant::{infer_quant_from_filename, QuantEntry, QuantKind};

#[cfg(not(feature = "ssr"))]
#[path = "../../tama-core/src/types/enums.rs"]
mod enums;
#[cfg(not(feature = "ssr"))]
#[path = "../../tama-core/src/types/gpu.rs"]
mod gpu;
#[cfg(not(feature = "ssr"))]
#[path = "../../tama-core/src/types/quant.rs"]
mod quant;

#[cfg(not(feature = "ssr"))]
pub use enums::{CompactionDevice, LogLevel, RestartPolicy};
#[cfg(not(feature = "ssr"))]
pub use gpu::{GpuVendor, ModelState};
#[cfg(not(feature = "ssr"))]
pub use quant::{infer_quant_from_filename, QuantEntry, QuantKind};
