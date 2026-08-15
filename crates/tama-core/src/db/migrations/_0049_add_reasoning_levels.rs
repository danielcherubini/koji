//! Migration v49: Add reasoning_levels column to model_configs.
//!
//! Stores the JSON array of reasoning effort levels the model accepts
//! (pi vocabulary: off, minimal, low, medium, high, xhigh, max).
//! Nullable so existing rows remain unaffected.

pub const MIGRATION: (i32, bool, &str) = (
    49,
    false, // does not require FKs off
    r#"
ALTER TABLE model_configs ADD COLUMN reasoning_levels TEXT DEFAULT NULL;
"#,
);

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn test_migration_v49_adds_reasoning_levels_column() {
        let conn = Connection::open_in_memory().unwrap();

        // Bring DB up to v48 (pre-v49 schema)
        super::super::run_up_to(&conn, 48).unwrap();

        // Verify reasoning_levels column does NOT exist yet
        let col_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_configs') \
                 WHERE name='reasoning_levels'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            col_before, 0,
            "reasoning_levels should not exist before v49"
        );

        // Apply v49
        super::super::run_up_to(&conn, 49).unwrap();

        // Verify reasoning_levels column now exists
        let col_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_configs') \
                 WHERE name='reasoning_levels'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_after, 1, "reasoning_levels column must exist after v49");

        // Insert a row and verify reasoning_levels defaults to NULL
        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('test/repo', 'llama_cpp')",
            [],
        )
        .unwrap();

        let reasoning_levels: Option<String> = conn
            .query_row(
                "SELECT reasoning_levels FROM model_configs WHERE repo_id='test/repo'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            reasoning_levels.is_none(),
            "reasoning_levels should default to NULL"
        );
    }
}
