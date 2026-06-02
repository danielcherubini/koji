/// v10 — Add unique index on model_pulls.model_id
pub const MIGRATION: (i32, bool, &str) = (
    10,
    false,
    r#"
        -- Deduplicate historical rows first (keep row with highest id
        -- per model_id). Without this, CREATE UNIQUE INDEX would fail
        -- on upgraded databases that have duplicate model_pulls rows.
        DELETE FROM model_pulls
        WHERE id NOT IN (
            SELECT MAX(id) FROM model_pulls GROUP BY model_id
        );

        -- Add UNIQUE index on model_pulls.model_id so that
        -- upsert_model_pull's ON CONFLICT(model_id) has a matching
        -- constraint. Without it, refresh_metadata (which calls
        -- upsert_model_pull before upserting files) fails entirely,
        -- leaving all file hashes unbaked.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_model_pulls_model_id
            ON model_pulls(model_id);
    "#,
);
