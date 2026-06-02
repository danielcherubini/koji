/// v1 — Initial schema: model_pulls, model_files, download_log
pub const MIGRATION: (i32, bool, &str) = (
    1,
    false,
    r#"
        -- Tracks HuggingFace repo state at time of pull
        CREATE TABLE IF NOT EXISTS model_pulls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id TEXT NOT NULL,           -- e.g. "bartowski/OmniCoder-8B-GGUF"
            commit_sha TEXT NOT NULL,        -- HF repo HEAD commit hash
            pulled_at TEXT NOT NULL,         -- ISO 8601 timestamp
            UNIQUE(repo_id)                 -- one row per repo, updated on re-pull
        );

        -- Tracks per-file metadata for downloaded GGUFs
        CREATE TABLE IF NOT EXISTS model_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id TEXT NOT NULL,           -- FK-like reference to model_pulls.repo_id
            filename TEXT NOT NULL,          -- e.g. "OmniCoder-8B-Q4_K_M.gguf"
            quant TEXT,                      -- e.g. "Q4_K_M"
            lfs_oid TEXT,                    -- LFS SHA256 content hash
            size_bytes INTEGER,              -- file size (i64 in Rust)
            downloaded_at TEXT NOT NULL,     -- ISO 8601 timestamp
            UNIQUE(repo_id, filename)        -- one row per file per repo
        );

        -- Download event log (append-only)
        CREATE TABLE IF NOT EXISTS download_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id TEXT NOT NULL,
            filename TEXT NOT NULL,
            started_at TEXT NOT NULL,
            completed_at TEXT,
            size_bytes INTEGER,              -- i64 in Rust
            duration_ms INTEGER,             -- i64 in Rust
            success INTEGER NOT NULL DEFAULT 0,
            error_message TEXT
        );

        -- Index for querying download history by repo
        CREATE INDEX IF NOT EXISTS idx_download_log_repo ON download_log(repo_id);
    "#,
);
