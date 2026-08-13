//! Migration v47: add provider_name column to model_configs.
//!
//! When set, `provider_name` overrides the `backend` field for routing.
//! A model with `provider_name` resolves to a remote provider and uses
//! `RemoteForwarder` instead of local backend lifecycle.

pub const MIGRATION: (i32, bool, &str) = (
    47,
    false,
    "ALTER TABLE model_configs ADD COLUMN provider_name TEXT;",
);

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn has_column(conn: &Connection, table: &str, col: &str) -> bool {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                (table, col),
                |r| r.get(0),
            )
            .unwrap();
        n > 0
    }

    #[test]
    fn test_migration_v47_adds_provider_name() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run_up_to(&conn, 46).unwrap();

        assert!(
            !has_column(&conn, "model_configs", "provider_name"),
            "provider_name should not exist before v47"
        );

        super::super::run_up_to(&conn, 47).unwrap();

        assert!(
            has_column(&conn, "model_configs", "provider_name"),
            "provider_name must exist after v47"
        );
    }

    #[test]
    fn test_migration_v47_provider_name_defaults_null() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run(&conn).unwrap();

        // Insert a model without provider_name
        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('test/repo', 'llama_cpp')",
            [],
        )
        .unwrap();

        let provider_name: Option<String> = conn
            .query_row(
                "SELECT provider_name FROM model_configs WHERE repo_id = 'test/repo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            provider_name.is_none(),
            "provider_name must default to NULL"
        );
    }
}
