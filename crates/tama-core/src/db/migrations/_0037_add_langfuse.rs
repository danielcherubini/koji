/// v37 — Add Langfuse configuration table for observability and telemetry.
pub const MIGRATION: (i32, bool, &str) = (
    37,
    false, // not reversible
    r#"CREATE TABLE IF NOT EXISTS app_langfuse (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        enabled INTEGER NOT NULL DEFAULT 0,
        public_key TEXT NOT NULL DEFAULT '',
        secret_key TEXT NOT NULL DEFAULT '',
        host TEXT NOT NULL DEFAULT 'https://cloud.langfuse.com',
        environment TEXT NOT NULL DEFAULT 'default',
        capture_input INTEGER NOT NULL DEFAULT 1,
        capture_output INTEGER NOT NULL DEFAULT 1,
        capture_streaming INTEGER NOT NULL DEFAULT 1,
        telemetry_max_bytes INTEGER NOT NULL DEFAULT 1048576,
        electricity_price_per_kwh REAL NOT NULL DEFAULT 0.0
    )"#,
);
