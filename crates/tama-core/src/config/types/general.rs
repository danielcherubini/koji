use crate::config::defaults;
use crate::config::types::LogLevel;
use serde::{Deserialize, Serialize};

/// General (non-backend) application settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: LogLevel,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
    /// HuggingFace API token for downloading gated models.
    /// When set, this is exported as HF_TOKEN environment variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    /// How often to check for updates (in hours). Default 12.
    #[serde(default = "defaults::default_update_check_interval")]
    pub update_check_interval: u32,
}

impl Default for General {
    fn default() -> Self {
        Self {
            log_level: LogLevel::default(),
            models_dir: None,
            logs_dir: None,
            hf_token: None,
            update_check_interval: defaults::default_update_check_interval(),
        }
    }
}
