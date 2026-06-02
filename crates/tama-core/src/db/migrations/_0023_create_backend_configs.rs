/// v23 — Create backend_configs table
pub const MIGRATION: (i32, bool, &str) = (
    23,
    false,
    r#"
        CREATE TABLE backend_configs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            gpu_variant TEXT NOT NULL DEFAULT 'cpu',
            default_args TEXT,
            health_check_url TEXT,
            UNIQUE(name, gpu_variant)
        );
    "#,
);
