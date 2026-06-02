/// v12 — Add quant and context_length to download_queue
pub const MIGRATION: (i32, bool, &str) = (
    12,
    false,
    r#"
        -- Add quant and context_length to download_queue so the queue
        -- processor can reconstruct a QuantDownloadSpec from the DB row.
        ALTER TABLE download_queue ADD COLUMN quant TEXT;
        ALTER TABLE download_queue ADD COLUMN context_length INTEGER;
    "#,
);
