use serde::{Deserialize, Serialize};

/// Configuration for the LLMLingua-2 compaction service.
/// When absent from the configuration, compaction is disabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Whether compaction is enabled. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the Python entrypoint (main.py). If omitted, uses embedded default.
    #[serde(default)]
    pub server_path: Option<String>,
    /// Compute device: "cpu", "cuda", "cuda:0", "mps". Default: "cpu".
    #[serde(default = "default_compaction_device")]
    pub device: String,
    /// Fixed port for the compaction server. If omitted, auto-assigned via TcpListener.
    #[serde(default)]
    pub port: Option<u16>,
    /// Request timeout in milliseconds. Default: 30000 (30s).
    #[serde(default = "default_compaction_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server_path: None,
            device: default_compaction_device(),
            port: None,
            request_timeout_ms: default_compaction_request_timeout_ms(),
        }
    }
}

fn default_compaction_device() -> String {
    "cpu".to_string()
}

fn default_compaction_request_timeout_ms() -> u64 {
    30_000
}
