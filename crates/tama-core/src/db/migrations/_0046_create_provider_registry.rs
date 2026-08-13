//! Migration v46: create provider_registry and tamad_registry tables.
//!
//! These tables support the provider abstraction layer, allowing users to
//! register both local (tamad-managed) and remote (HTTP API) providers.
//! The `provider_registry` tracks named providers with their engine type,
//! while `tamad_registry` tracks tamad daemon connections.

pub const MIGRATION: (i32, bool, &str) = (
    46,
    false, // fk_off
    "CREATE TABLE IF NOT EXISTS provider_registry (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL UNIQUE,
        provider_type TEXT NOT NULL CHECK(provider_type IN ('local', 'remote')),
        engine TEXT NOT NULL,
        tamad_id TEXT,
        base_url TEXT,
        api_key TEXT,
        created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
    );
    CREATE TABLE IF NOT EXISTS tamad_registry (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL UNIQUE,
        url TEXT NOT NULL,
        protocol TEXT NOT NULL CHECK(protocol IN ('grpc', 'http')),
        token TEXT,
        status TEXT NOT NULL DEFAULT 'unknown' CHECK(status IN ('online', 'offline', 'unknown'))
    );",
);

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    fn has_table(conn: &Connection, name: &str) -> bool {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        n > 0
    }

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
    fn test_migration_v46_creates_provider_registry() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run_up_to(&conn, 45).unwrap();

        assert!(!has_table(&conn, "provider_registry"));

        super::super::run_up_to(&conn, 46).unwrap();

        assert!(has_table(&conn, "provider_registry"));
        for col in [
            "id",
            "name",
            "provider_type",
            "engine",
            "tamad_id",
            "base_url",
            "api_key",
            "created_at",
        ] {
            assert!(
                has_column(&conn, "provider_registry", col),
                "column '{}' must exist",
                col
            );
        }
    }

    #[test]
    fn test_migration_v46_creates_tamad_registry() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run_up_to(&conn, 45).unwrap();

        assert!(!has_table(&conn, "tamad_registry"));

        super::super::run_up_to(&conn, 46).unwrap();

        assert!(has_table(&conn, "tamad_registry"));
        for col in ["id", "name", "url", "protocol", "token", "status"] {
            assert!(
                has_column(&conn, "tamad_registry", col),
                "column '{}' must exist",
                col
            );
        }
    }

    #[test]
    fn test_migration_v46_provider_check_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run(&conn).unwrap();

        // Valid insert
        conn.execute(
            "INSERT INTO provider_registry (name, provider_type, engine) VALUES ('test', 'local', 'llama_cpp')",
            [],
        )
        .unwrap();

        // Invalid provider_type must fail
        let err = conn.execute(
            "INSERT INTO provider_registry (name, provider_type, engine) VALUES ('bad', 'invalid', 'llama_cpp')",
            [],
        );
        assert!(
            err.is_err(),
            "invalid provider_type must fail CHECK constraint"
        );
    }

    #[test]
    fn test_migration_v46_tamad_check_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run(&conn).unwrap();

        // Valid insert
        conn.execute(
            "INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('1', 'test', 'grpc://localhost:50051', 'grpc')",
            [],
        )
        .unwrap();

        // Invalid protocol must fail
        let err = conn.execute(
            "INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('2', 'bad', 'http://x', 'websocket')",
            [],
        );
        assert!(err.is_err(), "invalid protocol must fail CHECK constraint");

        // Invalid status must fail
        let err = conn.execute(
            "UPDATE tamad_registry SET status = 'disconnected' WHERE id = '1'",
            [],
        );
        assert!(err.is_err(), "invalid status must fail CHECK constraint");
    }

    #[test]
    fn test_migration_v46_tamad_default_status() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::run(&conn).unwrap();

        conn.execute(
            "INSERT INTO tamad_registry (id, name, url, protocol) VALUES ('1', 'test', 'grpc://localhost:50051', 'grpc')",
            [],
        )
        .unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tamad_registry WHERE id = '1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "unknown", "default status must be 'unknown'");
    }
}
