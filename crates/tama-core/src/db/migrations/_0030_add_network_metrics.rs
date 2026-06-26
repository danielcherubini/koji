/// v30 - Add network metrics columns to system_metrics_history.
/// Stores cumulative network RX/TX bytes for per-tick throughput calculation.
pub const MIGRATION: (i32, bool, &str) = (
    30,
    false,
    r#"
        ALTER TABLE system_metrics_history ADD COLUMN net_rx_bytes BIGINT DEFAULT 0;
        ALTER TABLE system_metrics_history ADD COLUMN net_tx_bytes BIGINT DEFAULT 0;
    "#,
);
