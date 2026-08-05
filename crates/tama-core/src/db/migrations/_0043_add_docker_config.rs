//! Migration v43: Add docker_config column to backend_installations.
//!
//! Stores the serialized `DockerConfig` JSON for Docker-based backends.
//! Nullable so existing rows remain unaffected.

pub const MIGRATION: (i32, bool, &str) = (
    43,
    false, // does not require FKs off
    r#"
ALTER TABLE backend_installations ADD COLUMN docker_config TEXT DEFAULT NULL;
"#,
);

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn test_migration_v43_adds_docker_config_column() {
        let conn = Connection::open_in_memory().unwrap();

        // Bring DB up to v42 (pre-v43 schema)
        super::super::run_up_to(&conn, 42).unwrap();

        // Verify docker_config column does NOT exist yet
        let col_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('backend_installations') WHERE name='docker_config'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_before, 0, "docker_config should not exist before v43");

        // Apply v43
        super::super::run_up_to(&conn, 43).unwrap();

        // Verify docker_config column now exists
        let col_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('backend_installations') WHERE name='docker_config'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_after, 1, "docker_config column must exist after v43");

        // Insert a row and verify docker_config defaults to NULL
        conn.execute(
            "INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_variant, is_active) \
             VALUES ('test_backend', 'llama_cpp', 'v1.0', '/tmp/test', 1234567890, 'cpu', 1)",
            [],
        )
        .unwrap();

        let docker_config: Option<String> = conn
            .query_row(
                "SELECT docker_config FROM backend_installations WHERE name='test_backend'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            docker_config.is_none(),
            "docker_config should default to NULL"
        );
    }
}
