//! Tamad registry database query functions.

use anyhow::Result;
use rusqlite::params;
use std::str::FromStr;

use crate::providers::{Protocol, TamadConnection, TamadStatus};
use rusqlite::Connection;

// ─── Raw row type ───────────────────────────────────────────────────────────

/// Internal: raw tamad row as (id, name, url, protocol, token, status).
type RawTamadRow = (String, String, String, String, Option<String>, String);

/// Convert a raw row tuple to a typed TamadConnection.
fn raw_to_tamad(row: RawTamadRow) -> Result<TamadConnection> {
    let (id, name, url, protocol, token, status) = row;
    Ok(TamadConnection {
        id,
        name,
        url,
        protocol: Protocol::from_str(&protocol)
            .map_err(|e| anyhow::anyhow!("invalid protocol '{}': {}", protocol, e))?,
        token,
        status: TamadStatus::from_str(&status)
            .map_err(|e| anyhow::anyhow!("invalid status '{}': {}", status, e))?,
    })
}

// ─── Queries ────────────────────────────────────────────────────────────────

/// Insert a new tamad connection record.
pub fn insert_tamad(
    conn: &Connection,
    id: &str,
    name: &str,
    url: &str,
    protocol: &str,
    token: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO tamad_registry (id, name, url, protocol, token)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, name, url, protocol, token],
    )?;
    Ok(())
}

/// Get a tamad connection by id.
pub fn get_tamad(conn: &Connection, id: &str) -> Result<Option<TamadConnection>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, url, protocol, token, status
         FROM tamad_registry
         WHERE id = ?1",
    )?;

    let mut rows = stmt.query_map([id], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    })?;

    match rows.next() {
        Some(Ok(row)) => Ok(Some(raw_to_tamad(row)?)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// List all tamad connections ordered by name.
pub fn list_tamads(conn: &Connection) -> Result<Vec<TamadConnection>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, url, protocol, token, status
         FROM tamad_registry
         ORDER BY name ASC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(raw_to_tamad)
        .collect()
}

/// Update a tamad connection's url and/or token.
pub fn update_tamad(conn: &Connection, id: &str, url: &str, token: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE tamad_registry
         SET url = ?2,
             token = COALESCE(?3, token)
         WHERE id = ?1",
        params![id, url, token],
    )?;
    Ok(())
}

/// Delete a tamad connection by id. Returns true if a row was deleted.
pub fn delete_tamad(conn: &Connection, id: &str) -> Result<bool> {
    let changed = conn.execute("DELETE FROM tamad_registry WHERE id = ?1", params![id])?;
    Ok(changed > 0)
}

/// Update only the status field of a tamad connection.
pub fn update_tamad_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE tamad_registry SET status = ?2 WHERE id = ?1",
        params![id, status],
    )?;
    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Set up an in-memory database with the tamad_registry table.
    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tamad_registry (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                url TEXT NOT NULL,
                protocol TEXT NOT NULL CHECK(protocol IN ('grpc', 'http')),
                token TEXT,
                status TEXT NOT NULL DEFAULT 'unknown' CHECK(status IN ('online', 'offline', 'unknown'))
            );
            "#,
        )
        .unwrap();
        conn
    }

    // ── insert_tamad ──

    #[test]
    fn test_insert_tamad() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-001",
            "local-tamad",
            "grpc://localhost:50051",
            "grpc",
            Some("secret-token"),
        )
        .unwrap();

        let tamad = get_tamad(&conn, "uuid-001").unwrap().unwrap();
        assert_eq!(tamad.id, "uuid-001");
        assert_eq!(tamad.name, "local-tamad");
        assert_eq!(tamad.url, "grpc://localhost:50051");
        assert!(tamad.protocol.is_grpc());
        assert_eq!(tamad.token, Some("secret-token".to_string()));
        assert!(tamad.status.is_unknown()); // default
    }

    #[test]
    fn test_insert_tamad_http() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-002",
            "remote-tamad",
            "http://192.168.1.100:8080",
            "http",
            None,
        )
        .unwrap();

        let tamad = get_tamad(&conn, "uuid-002").unwrap().unwrap();
        assert!(tamad.protocol.is_http());
        assert!(tamad.token.is_none());
    }

    #[test]
    fn test_insert_duplicate_id_fails() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-001",
            "tamad-one",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .unwrap();

        let result = insert_tamad(
            &conn,
            "uuid-001",
            "tamad-two",
            "grpc://localhost:50052",
            "grpc",
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_insert_duplicate_name_fails() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .unwrap();

        let result = insert_tamad(
            &conn,
            "uuid-002",
            "my-tamad",
            "grpc://localhost:50052",
            "grpc",
            None,
        );
        assert!(result.is_err());
    }

    // ── get_tamad ──

    #[test]
    fn test_get_tamad_not_found() {
        let conn = setup_test_db();
        let result = get_tamad(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    // ── list_tamads ──

    #[test]
    fn test_list_tamads_empty() {
        let conn = setup_test_db();
        let result = list_tamads(&conn).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_list_tamads_ordered() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-z",
            "z-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .unwrap();
        insert_tamad(
            &conn,
            "uuid-a",
            "a-tamad",
            "http://localhost:8080",
            "http",
            None,
        )
        .unwrap();

        let result = list_tamads(&conn).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "a-tamad");
        assert_eq!(result[1].name, "z-tamad");
    }

    // ── update_tamad ──

    #[test]
    fn test_update_tamad_url() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            Some("old-token"),
        )
        .unwrap();

        update_tamad(&conn, "uuid-001", "grpc://localhost:50099", None).unwrap();

        let tamad = get_tamad(&conn, "uuid-001").unwrap().unwrap();
        assert_eq!(tamad.url, "grpc://localhost:50099");
        // token unchanged when None passed
        assert_eq!(tamad.token, Some("old-token".to_string()));
    }

    #[test]
    fn test_update_tamad_token() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            Some("old-token"),
        )
        .unwrap();

        update_tamad(
            &conn,
            "uuid-001",
            "grpc://localhost:50051",
            Some("new-token"),
        )
        .unwrap();

        let tamad = get_tamad(&conn, "uuid-001").unwrap().unwrap();
        assert_eq!(tamad.token, Some("new-token".to_string()));
    }

    // ── delete_tamad ──

    #[test]
    fn test_delete_tamad() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .unwrap();

        let deleted = delete_tamad(&conn, "uuid-001").unwrap();
        assert!(deleted);
        assert!(get_tamad(&conn, "uuid-001").unwrap().is_none());
    }

    #[test]
    fn test_delete_tamad_not_found() {
        let conn = setup_test_db();
        let deleted = delete_tamad(&conn, "nonexistent").unwrap();
        assert!(!deleted);
    }

    // ── update_tamad_status ──

    #[test]
    fn test_update_tamad_status() {
        let conn = setup_test_db();
        insert_tamad(
            &conn,
            "uuid-001",
            "my-tamad",
            "grpc://localhost:50051",
            "grpc",
            None,
        )
        .unwrap();

        // Default status is unknown
        let tamad = get_tamad(&conn, "uuid-001").unwrap().unwrap();
        assert!(tamad.status.is_unknown());

        update_tamad_status(&conn, "uuid-001", "online").unwrap();

        let tamad = get_tamad(&conn, "uuid-001").unwrap().unwrap();
        assert!(tamad.status.is_online());

        update_tamad_status(&conn, "uuid-001", "offline").unwrap();

        let tamad = get_tamad(&conn, "uuid-001").unwrap().unwrap();
        assert!(tamad.status.is_offline());
    }
}
