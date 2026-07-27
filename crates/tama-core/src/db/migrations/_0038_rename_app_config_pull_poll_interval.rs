//! v38 — Rename download_queue_poll_interval_secs → pull_queue_poll_interval_secs

/// Migration v38: rename app_proxy.download_queue_poll_interval_secs to pull_queue_poll_interval_secs
pub const MIGRATION: (i32, bool, &str) = (
    38,
    false,
    r#"ALTER TABLE app_proxy RENAME COLUMN download_queue_poll_interval_secs TO pull_queue_poll_interval_secs;"#,
);
