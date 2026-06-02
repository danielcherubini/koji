/// v6 — Create update_checks table
pub const MIGRATION: (i32, bool, &str) = (
    6,
    false,
    r#"
        CREATE TABLE IF NOT EXISTS update_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            item_type TEXT NOT NULL,           -- 'backend' or 'model'
            item_id TEXT NOT NULL,             -- backend name or model config key
            current_version TEXT,              -- installed version/commit SHA
            latest_version TEXT,               -- remote version/commit SHA
            update_available INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'unknown',
            error_message TEXT,
            details_json TEXT,                 -- JSON blob (per-file changes for models)
            checked_at INTEGER NOT NULL,        -- unix timestamp
            UNIQUE(item_type, item_id)
        );
        CREATE INDEX IF NOT EXISTS idx_update_checks_type ON update_checks(item_type);
    "#,
);
