//! Compaction configuration (WASM mirror).

use serde::{Deserialize, Serialize};

use tama_core::config::CompactionDevice as CoreCompactionDevice;

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

/// Convert from mirror CompactionConfig to tama_core::config::CompactionConfig.
impl From<CompactionConfig> for tama_core::config::CompactionConfig {
    fn from(c: CompactionConfig) -> Self {
        Self {
            enabled: c.enabled,
            server_path: c.server_path,
            device: c.device,
            port: c.port,
            request_timeout_ms: c.request_timeout_ms,
        }
    }
}

/// Convert from tama_core::config::CompactionConfig to mirror CompactionConfig.
impl From<tama_core::config::CompactionConfig> for CompactionConfig {
    fn from(c: tama_core::config::CompactionConfig) -> Self {
        Self {
            enabled: c.enabled,
            server_path: c.server_path,
            device: c.device,
            port: c.port,
            request_timeout_ms: c.request_timeout_ms,
        }
    }
}
