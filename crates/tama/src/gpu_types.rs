//! Mirror types for GPU enums from tama-core that can be used from WASM.
//!
//! These match the serde serialization of tama_core::gpu types so the frontend
//! can deserialize SSE payloads without depending on tama-core (which is
//! unavailable in WASM builds).

use serde::{Deserialize, Serialize};

/// Mirror of `tama_core::gpu::GpuVendor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
}

/// Mirror of `tama_core::gpu::ModelState`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelState {
    #[default]
    Idle,
    Loading,
    Ready,
    Unloading,
    Failed,
}

impl ModelState {
    /// Convert the state to its string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Unloading => "unloading",
            Self::Failed => "failed",
        }
    }
}
