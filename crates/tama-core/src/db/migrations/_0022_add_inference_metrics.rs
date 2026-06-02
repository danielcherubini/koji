/// v22 — Add inference metrics to system_metrics_history
pub const MIGRATION: (i32, bool, &str) = (
    22,
    false,
    r#"
        ALTER TABLE system_metrics_history ADD COLUMN tps REAL;
        ALTER TABLE system_metrics_history ADD COLUMN prompt_tps REAL;
        ALTER TABLE system_metrics_history ADD COLUMN cache_hit_pct REAL;
        ALTER TABLE system_metrics_history ADD COLUMN spec_accept_pct REAL;
    "#,
);
