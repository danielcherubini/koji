//! General configuration section (WASM mirror).

use serde::{Deserialize, Serialize};

use crate::core_shared::LogLevel as CoreLogLevel;

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
    /// Target-specific log directives (RUST_LOG syntax, `target=level` pairs
    /// comma-separated). Durable override merged into the runtime filter;
    /// wins over the `RUST_LOG` env var for the same target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_directives: Option<String>,
    /// SQLite log store retention: max entry age in days. Default 7.
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// SQLite log store retention: max row count. Default 50,000.
    #[serde(default = "default_log_retention_rows")]
    pub log_retention_rows: u64,
    /// SQLite log store retention: max estimated size in MiB. Default 256.
    #[serde(default = "default_log_retention_max_mb")]
    pub log_retention_max_mb: u64,
}

fn default_update_check_interval() -> u32 {
    12
}

fn default_log_retention_days() -> u32 {
    7
}

fn default_log_retention_rows() -> u64 {
    50_000
}

fn default_log_retention_max_mb() -> u64 {
    256
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
            log_directives: Some("probe_target=debug".to_string()),
            log_retention_days: 14,
            log_retention_rows: 25_000,
            log_retention_max_mb: 128,
        };

        let json = serde_json::to_string(&general).unwrap();
        let deserialized: General = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.update_check_interval, 24);
        assert_eq!(deserialized.log_level, CoreLogLevel::Info);
        assert_eq!(
            deserialized.log_directives,
            Some("probe_target=debug".to_string())
        );
        assert_eq!(deserialized.log_retention_days, 14);
        assert_eq!(deserialized.log_retention_rows, 25_000);
        assert_eq!(deserialized.log_retention_max_mb, 128);

        // A payload missing the new fields deserializes to the defaults
        // (serde(default) on every field — stale clients never break).
        let legacy: General =
            serde_json::from_str(r#"{"log_level":"warn","update_check_interval":6}"#).unwrap();
        assert!(legacy.log_directives.is_none());
        assert_eq!(legacy.log_retention_days, 7);
        assert_eq!(legacy.log_retention_rows, 50_000);
        assert_eq!(legacy.log_retention_max_mb, 256);
    }
}
