/// v27 — Replace last_used_model with model_aliases
pub const MIGRATION: (i32, bool, &str) = (
    27,
    false,
    r#"
        CREATE TABLE IF NOT EXISTS model_aliases (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE COLLATE NOCASE,
            model_id INTEGER NOT NULL REFERENCES model_configs(id) ON DELETE CASCADE,
            description TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );

        -- Migrate last_used_model into model_aliases before dropping it.
        -- server_name in last_used_model maps to repo_id in model_configs.
        INSERT OR IGNORE INTO model_aliases (name, model_id, description, enabled)
        SELECT
            lum.server_name,
            mc.id,
            'Migrated from last_used_model',
            1
        FROM last_used_model lum
        JOIN model_configs mc ON mc.repo_id = lum.server_name;

        DROP TABLE IF EXISTS last_used_model;

        CREATE INDEX IF NOT EXISTS idx_model_aliases_model_id ON model_aliases(model_id);
        CREATE INDEX IF NOT EXISTS idx_model_aliases_enabled ON model_aliases(enabled);

        -- Seed default alias for backward compatibility (only if enabled models exist)
        INSERT OR IGNORE INTO model_aliases (name, model_id, description, enabled)
        SELECT 'whatevers-hot-n-fresh', id, 'Default alias — routes to this model', 1
        FROM model_configs
        WHERE enabled = 1
        ORDER BY id ASC
        LIMIT 1;
    "#,
);
