// Profile resolution is now handled via Config.sampling_templates
// and ModelToml.sampling directly. See resolve.rs.

pub fn default_update_check_interval() -> u32 {
    12
}

/// Default retention for the SQLite structured log store: days.
pub const DEFAULT_LOG_RETENTION_DAYS: u32 = 7;

/// Default retention for the SQLite structured log store: row cap.
pub const DEFAULT_LOG_RETENTION_ROWS: u64 = 50_000;

/// Default retention for the SQLite structured log store: size cap (MiB).
pub const DEFAULT_LOG_RETENTION_MAX_MB: u64 = 256;

/// Serde default wrapper for [`DEFAULT_LOG_RETENTION_DAYS`].
pub fn default_log_retention_days() -> u32 {
    DEFAULT_LOG_RETENTION_DAYS
}

/// Serde default wrapper for [`DEFAULT_LOG_RETENTION_ROWS`].
pub fn default_log_retention_rows() -> u64 {
    DEFAULT_LOG_RETENTION_ROWS
}

/// Serde default wrapper for [`DEFAULT_LOG_RETENTION_MAX_MB`].
pub fn default_log_retention_max_mb() -> u64 {
    DEFAULT_LOG_RETENTION_MAX_MB
}
