//! v39 — Rename downloaded_at → pulled_at in model_files

/// Migration v39: rename model_files.downloaded_at to pulled_at
pub const MIGRATION: (i32, bool, &str) = (
    39,
    false,
    r#"ALTER TABLE model_files RENAME COLUMN downloaded_at TO pulled_at;"#,
);
