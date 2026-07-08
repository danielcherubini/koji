//! General configuration section (WASM mirror).

use serde::{Deserialize, Serialize};

use tama_core::config::LogLevel as CoreLogLevel;

/// General configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct General {
    pub log_level: CoreLogLevel,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
    /// HuggingFace API token for downloading gated models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    /// How often to check for updates (in hours). Default 12.
    #[serde(default = "default_update_check_interval")]
    pub update_check_interval: u32,
}

fn default_update_check_interval() -> u32 {
    12
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_general_serialization() {
        let general = General {
            log_level: CoreLogLevel::Info,
            models_dir: None,
            logs_dir: None,
            hf_token: None,
            update_check_interval: 24,
        };

        let json = serde_json::to_string(&general).unwrap();
        let deserialized: General = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.update_check_interval, 24);
        assert_eq!(deserialized.log_level, CoreLogLevel::Info);
    }
}
