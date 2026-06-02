/// v2 — Create active_models table for tracking running backend processes
pub const MIGRATION: (i32, bool, &str) = (
    2,
    false,
    r#"
        -- Tracks running backend processes
        CREATE TABLE IF NOT EXISTS active_models (
            server_name TEXT PRIMARY KEY,   -- config key, e.g. "my-coding-model"
            model_name TEXT NOT NULL,       -- model identifier used for loading
            backend TEXT NOT NULL,          -- backend key, e.g. "llama-server"
            pid INTEGER NOT NULL,           -- backend process PID (i64 in Rust)
            port INTEGER NOT NULL,          -- backend port (i64 in Rust)
            backend_url TEXT NOT NULL,      -- full URL, e.g. "http://127.0.0.1:54321"
            loaded_at TEXT NOT NULL,        -- ISO 8601 timestamp
            last_accessed TEXT NOT NULL     -- ISO 8601 timestamp, updated periodically
        );
    "#,
);
