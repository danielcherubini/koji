/// v11 — Create download_queue table
pub const MIGRATION: (i32, bool, &str) = (
    11,
    false,
    r#"
        -- Operational download queue table (updated as status changes,
        -- not append-only like download_log).
        CREATE TABLE download_queue (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id        TEXT NOT NULL UNIQUE,
            repo_id       TEXT NOT NULL,
            filename      TEXT NOT NULL,
            display_name  TEXT,
            status        TEXT NOT NULL DEFAULT 'queued',
            bytes_downloaded INTEGER NOT NULL DEFAULT 0,
            total_bytes     INTEGER,
            error_message TEXT,
            started_at     TEXT,
            completed_at   TEXT,
            queued_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            kind           TEXT NOT NULL DEFAULT 'model'
        );
        CREATE INDEX idx_dq_status ON download_queue(status);
    "#,
);
