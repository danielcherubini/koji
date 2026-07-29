//! Compaction configuration (WASM mirror).

use serde::{Deserialize, Serialize};

use crate::core_shared::CompactionDevice as CoreCompactionDevice;

/// Configuration for the LLMLingua-2 compaction service.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompactionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server_path: Option<String>,
    #[serde(default = "default_compaction_device")]
    pub device: CoreCompactionDevice,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_compaction_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

fn default_compaction_device() -> CoreCompactionDevice {
    CoreCompactionDevice::Cpu
}

fn default_compaction_request_timeout_ms() -> u64 {
    30_000
}
