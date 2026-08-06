//! Migration v44: Add vllm_config column to model_configs.
//!
//! Stores the serialized `VllmConfig` JSON for vLLM-based backends.
//! Nullable so existing rows remain unaffected.

pub const MIGRATION: (i32, bool, &str) = (
    44,
    false, // does not require FKs off
    r#"
ALTER TABLE model_configs ADD COLUMN vllm_config TEXT DEFAULT NULL;
"#,
);

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn test_migration_v44_adds_vllm_config_column() {
        let conn = Connection::open_in_memory().unwrap();

        // Bring DB up to v43 (pre-v44 schema)
        super::super::run_up_to(&conn, 43).unwrap();

        // Verify vllm_config column does NOT exist yet
        let col_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='vllm_config'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_before, 0, "vllm_config should not exist before v44");

        // Apply v44
        super::super::run_up_to(&conn, 44).unwrap();

        // Verify vllm_config column now exists
        let col_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='vllm_config'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_after, 1, "vllm_config column must exist after v44");

        // Insert a row and verify vllm_config defaults to NULL
        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('test/repo', 'vllm')",
            [],
        )
        .unwrap();

        let vllm_config: Option<String> = conn
            .query_row(
                "SELECT vllm_config FROM model_configs WHERE repo_id='test/repo'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(vllm_config.is_none(), "vllm_config should default to NULL");
    }
}
