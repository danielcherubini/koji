/// v25 — Create last_used_model table
pub const MIGRATION: (i32, bool, &str) = (
    25,
    false,
    r#"
        CREATE TABLE IF NOT EXISTS last_used_model (
            id INTEGER PRIMARY KEY,
            server_name TEXT NOT NULL,
            model_name TEXT NOT NULL,
            used_at TEXT NOT NULL
        );
    "#,
);
