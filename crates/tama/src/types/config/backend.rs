//! Backend configuration (WASM mirror).

use serde::{Deserialize, Serialize};

/// Backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,
    /// Optional version pin.
    #[serde(default)]
    pub version: Option<String>,
    /// Optional GPU variant pin (e.g. "cpu", "vulkan", "cuda").
    #[serde(default)]
    pub gpu_variant: Option<String>,
}
