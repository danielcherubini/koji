/// v34 — Rename download_queue table to pull_queue and bytes_downloaded column to bytes_pulled
pub const MIGRATION: (i32, bool, &str) = (
    34,
    false,
    r#"
        -- Rename download_queue to pull_queue to match domain terminology.
        -- "Pull" is the canonical term for downloading models in Tama.
        ALTER TABLE download_queue RENAME TO pull_queue;
        ALTER TABLE pull_queue RENAME COLUMN bytes_downloaded TO bytes_pulled;
    "#,
);
