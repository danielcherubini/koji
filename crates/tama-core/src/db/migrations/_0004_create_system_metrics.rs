/// v4 — Create system_metrics_history table
pub const MIGRATION: (i32, bool, &str) = (
    4,
    false,
    r#"
        CREATE TABLE IF NOT EXISTS system_metrics_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_unix_ms          INTEGER NOT NULL,
            cpu_usage_pct       REAL    NOT NULL,
            ram_used_mib        INTEGER NOT NULL,
            ram_total_mib       INTEGER NOT NULL,
            gpu_utilization_pct INTEGER,
            vram_used_mib       INTEGER,
            vram_total_mib      INTEGER,
            models_loaded       INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_system_metrics_ts
            ON system_metrics_history(ts_unix_ms);
    "#,
);
