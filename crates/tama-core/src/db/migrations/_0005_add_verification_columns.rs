/// v5 — Add SHA-256 verification columns to model_files
pub const MIGRATION: (i32, bool, &str) = (
    5,
    false,
    r#"
        -- Local SHA-256 verification tracking for previously downloaded quants.
        -- last_verified_at is ISO 8601 of the most recent verification attempt.
        -- verified_ok is nullable: NULL = never verified or no upstream hash available.
        -- verify_error holds a short message on mismatch or verification failure.
        ALTER TABLE model_files ADD COLUMN last_verified_at TEXT;
        ALTER TABLE model_files ADD COLUMN verified_ok INTEGER;
        ALTER TABLE model_files ADD COLUMN verify_error TEXT;
    "#,
);
