/// v36 — Create API keys table and enable flag on app_proxy
pub const MIGRATION: (i32, bool, &str) = (
    36,
    false,
    r#"
        CREATE TABLE api_keys (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL DEFAULT '',
            key_prefix TEXT NOT NULL,
            key_hash TEXT NOT NULL UNIQUE,
            scopes TEXT NOT NULL,
            created_by TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
            last_used_at TEXT,
            revoked_at TEXT,
            expires_at TEXT
        );
        CREATE INDEX idx_api_keys_active_created ON api_keys (revoked_at, created_at DESC);
        ALTER TABLE app_proxy ADD COLUMN api_keys_enabled INTEGER NOT NULL DEFAULT 0;
    "#,
);
