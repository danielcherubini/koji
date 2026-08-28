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
    /// HuggingFace API token for pulling gated models.
    /// When set, this is exported as HF_TOKEN environment variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    /// How often to check for updates (in hours). Default 12.
    #[serde(default = "defaults::default_update_check_interval")]
    pub update_check_interval: u32,
    /// Target-specific log directives (RUST_LOG syntax, `target=level` pairs
    /// comma-separated). Durable override merged into the runtime filter;
    /// wins over the `RUST_LOG` env var for the same target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_directives: Option<String>,
    /// SQLite log store retention: max entry age in days. Default 7.
    #[serde(default = "defaults::default_log_retention_days")]
    pub log_retention_days: u32,
    /// SQLite log store retention: max row count. Default 50,000.
    #[serde(default = "defaults::default_log_retention_rows")]
    pub log_retention_rows: u64,
    /// SQLite log store retention: max estimated size in MiB. Default 256.
    #[serde(default = "defaults::default_log_retention_max_mb")]
    pub log_retention_max_mb: u64,
}

impl Default for General {
    fn default() -> Self {
        Self {
            log_level: LogLevel::default(),
            models_dir: None,
            logs_dir: None,
            hf_token: None,
            update_check_interval: defaults::default_update_check_interval(),
            log_directives: None,
            log_retention_days: defaults::DEFAULT_LOG_RETENTION_DAYS,
            log_retention_rows: defaults::DEFAULT_LOG_RETENTION_ROWS,
            log_retention_max_mb: defaults::DEFAULT_LOG_RETENTION_MAX_MB,
        }
    }
}
