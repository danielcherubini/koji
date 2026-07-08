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

/// Convert from CoreBackendConfig to mirror type.
impl From<tama_core::config::BackendConfig> for BackendConfig {
    fn from(b: tama_core::config::BackendConfig) -> Self {
        Self {
            path: b.path,
            version: b.version,
            gpu_variant: b.gpu_variant,
        }
    }
}

/// Convert from mirror BackendConfig to CoreBackendConfig.
impl From<BackendConfig> for tama_core::config::BackendConfig {
    fn from(b: BackendConfig) -> Self {
        Self {
            path: b.path,
            version: b.version,
            gpu_variant: b.gpu_variant,
        }
    }
}
