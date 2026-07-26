use crate::gpu::GpuType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,
    /// Optional version pin. When set, resolve_backend_path looks up this
    /// specific version in the DB instead of the currently-active version.
    #[serde(default)]
    pub version: Option<String>,
    /// Optional GPU variant pin (e.g. "cpu", "vulkan", "cuda"). When set,
    /// resolve_backend_path uses this variant to look up the correct backend.
    #[serde(default)]
    pub gpu_variant: Option<GpuType>,
}
