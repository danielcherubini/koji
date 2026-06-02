/// v3 — Create backend_installations table
pub const MIGRATION: (i32, bool, &str) = (
    3,
    false,
    r#"
        CREATE TABLE IF NOT EXISTS backend_installations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,             -- backend key, e.g. "llama_cpp", "ik_llama"
            backend_type TEXT NOT NULL,     -- serialized enum, e.g. "llama_cpp", "ik_llama", "custom"
            version TEXT NOT NULL,          -- version string, e.g. "b8407", "main@abc12345"
            path TEXT NOT NULL,             -- absolute path to installed binary
            installed_at INTEGER NOT NULL,  -- unix timestamp (i64)
            gpu_type TEXT,                  -- JSON string (nullable, serialized GpuType)
            source TEXT,                    -- JSON string (nullable, serialized BackendSource)
            is_active INTEGER NOT NULL DEFAULT 0, -- 1 = current active version for this name
            UNIQUE(name, version)
        );
        CREATE INDEX IF NOT EXISTS idx_backend_installations_name ON backend_installations(name);
    "#,
);
