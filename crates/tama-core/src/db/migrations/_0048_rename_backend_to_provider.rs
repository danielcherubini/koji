//! Migration 48: Rename `backend_installations` → `provider_installations` and `backend_configs` → `provider_configs`.

pub const MIGRATION: (i32, bool, &str) = (
    48,
    false, // no table referenced by FK is dropped
    "
        -- Rename tables
        ALTER TABLE backend_installations RENAME TO provider_installations;
        ALTER TABLE backend_configs RENAME TO provider_configs;

        -- SQLite RENAME TABLE preserves indexes (keeps old names).
        -- Drop old indexes and recreate with new names for consistency.
        DROP INDEX IF EXISTS idx_backend_installations_name;
        CREATE INDEX idx_provider_installations_name ON provider_installations(name);

        DROP INDEX IF EXISTS idx_backend_installations_name_variant;
        CREATE INDEX idx_provider_installations_name_variant ON provider_installations(name, gpu_variant);

        DROP INDEX IF EXISTS idx_backend_configs_name_variant;
        CREATE INDEX idx_provider_configs_name_variant ON provider_configs(name, gpu_variant);

        DROP INDEX IF EXISTS idx_backend_configs_logical_variant;
        CREATE INDEX idx_provider_configs_logical_variant ON provider_configs(logical_id, gpu_variant);
    ",
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations::{run_up_to, FkGuard};
    use rusqlite::Connection;

    fn has_table(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{}'",
                name
            ),
            [],
            |row| row.get::<_, i32>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false)
    }

    fn has_index(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='{}'",
                name
            ),
            [],
            |row| row.get::<_, i32>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false)
    }

    #[test]
    fn test_migration_v48_renames_tables() {
        let conn = Connection::open_in_memory().unwrap();
        // Run up to v47 to build the schema
        run_up_to(&conn, 47).unwrap();

        // Old tables should exist at v47
        assert!(has_table(&conn, "backend_installations"));
        assert!(has_table(&conn, "backend_configs"));
        assert!(!has_table(&conn, "provider_installations"));
        assert!(!has_table(&conn, "provider_configs"));

        // Run v48
        let _fk_guard = FkGuard::disable(&conn).unwrap();
        conn.execute_batch(MIGRATION.2).unwrap();

        // New tables should exist, old ones should not
        assert!(has_table(&conn, "provider_installations"));
        assert!(has_table(&conn, "provider_configs"));
        assert!(!has_table(&conn, "backend_installations"));
        assert!(!has_table(&conn, "backend_configs"));
    }

    #[test]
    fn test_migration_v48_renames_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        run_up_to(&conn, 47).unwrap();

        // Old indexes should exist at v47
        assert!(has_index(&conn, "idx_backend_installations_name"));
        assert!(has_index(&conn, "idx_backend_installations_name_variant"));
        assert!(has_index(&conn, "idx_backend_configs_name_variant"));
        assert!(has_index(&conn, "idx_backend_configs_logical_variant"));

        // Run v48
        let _fk_guard = FkGuard::disable(&conn).unwrap();
        conn.execute_batch(MIGRATION.2).unwrap();

        // New indexes should exist, old ones should not
        assert!(has_index(&conn, "idx_provider_installations_name"));
        assert!(has_index(&conn, "idx_provider_installations_name_variant"));
        assert!(has_index(&conn, "idx_provider_configs_name_variant"));
        assert!(has_index(&conn, "idx_provider_configs_logical_variant"));
        assert!(!has_index(&conn, "idx_backend_installations_name"));
        assert!(!has_index(&conn, "idx_backend_installations_name_variant"));
        assert!(!has_index(&conn, "idx_backend_configs_name_variant"));
        assert!(!has_index(&conn, "idx_backend_configs_logical_variant"));
    }

    #[test]
    fn test_migration_v48_preserves_data() {
        let conn = Connection::open_in_memory().unwrap();
        run_up_to(&conn, 47).unwrap();

        // Insert test data into old tables (v47 schema uses backend_*)
        conn.execute(
            "INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_variant, is_active)
             VALUES ('llama_cpp', 'llama_cpp', 'b8407', '/tmp/llama', 1234567890, 'cpu', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO backend_configs (name, gpu_variant, default_args)
             VALUES ('llama_cpp', 'cpu', '[\"--ctx-size 4096\"]')",
            [],
        )
        .unwrap();

        // Run v48
        let _fk_guard = FkGuard::disable(&conn).unwrap();
        conn.execute_batch(MIGRATION.2).unwrap();

        // Data should survive the rename
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_installations WHERE name='llama_cpp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM provider_configs WHERE name='llama_cpp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
