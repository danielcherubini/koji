//! Provider registry database query functions.

use anyhow::Result;
use rusqlite::params;
use std::str::FromStr;

use crate::providers::{Engine, Provider, ProviderType};
use rusqlite::Connection;

// ─── Queries ────────────────────────────────────────────────────────────────

/// Insert a new provider record. Returns the auto-assigned row id.
pub fn insert_provider(
    conn: &Connection,
    name: &str,
    provider_type: &str,
    engine: &str,
    tamad_id: Option<&str>,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<i64> {
    let id: i64 = conn.query_row(
        "INSERT INTO provider_registry (name, provider_type, engine, tamad_id, base_url, api_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         RETURNING id",
        params![name, provider_type, engine, tamad_id, base_url, api_key],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Internal: raw provider row as (id, name, provider_type, engine, tamad_id, base_url, api_key, created_at).
type RawProviderRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);

/// Get a provider by name.
pub fn get_provider(conn: &Connection, name: &str) -> Result<Option<Provider>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_type, engine, tamad_id, base_url, api_key, created_at
         FROM provider_registry
         WHERE name = ?1",
    )?;

    let mut rows = stmt.query_map([name], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ))
    })?;

    match rows.next() {
        Some(Ok(row)) => Ok(Some(raw_to_provider(row)?)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Get a provider by id.
pub fn get_provider_by_id(conn: &Connection, id: i64) -> Result<Option<Provider>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_type, engine, tamad_id, base_url, api_key, created_at
         FROM provider_registry
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
            row.get(6)?,
            row.get(7)?,
        ))
    })?;

    match rows.next() {
        Some(Ok(row)) => Ok(Some(raw_to_provider(row)?)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// List all providers ordered by name.
pub fn list_providers(conn: &Connection) -> Result<Vec<Provider>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, provider_type, engine, tamad_id, base_url, api_key, created_at
         FROM provider_registry
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
            row.get(6)?,
            row.get(7)?,
        ))
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(raw_to_provider)
        .collect()
}

/// Convert a raw row tuple to a typed Provider.
fn raw_to_provider(row: RawProviderRow) -> Result<Provider> {
    let (id, name, provider_type, engine, tamad_id, base_url, api_key, created_at) = row;
    Ok(Provider {
        id,
        name,
        provider_type: ProviderType::from_str(&provider_type)
            .map_err(|e| anyhow::anyhow!("invalid provider_type '{}': {}", provider_type, e))?,
        engine: Engine::from_str(&engine)
            .map_err(|e| anyhow::anyhow!("invalid engine '{}': {}", engine, e))?,
        tamad_id,
        base_url,
        api_key,
        created_at,
    })
}

/// Update a provider's base_url and/or api_key.
pub fn update_provider(
    conn: &Connection,
    name: &str,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE provider_registry
         SET base_url = COALESCE(?2, base_url),
             api_key = COALESCE(?3, api_key)
         WHERE name = ?1",
        params![name, base_url, api_key],
    )?;
    Ok(())
}

/// Delete a provider by name. Returns true if a row was deleted.
pub fn delete_provider(conn: &Connection, name: &str) -> Result<bool> {
    let changed = conn.execute(
        "DELETE FROM provider_registry WHERE name = ?1",
        params![name],
    )?;
    Ok(changed > 0)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Set up an in-memory database with the provider_registry table.
    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS provider_registry (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                provider_type TEXT NOT NULL CHECK(provider_type IN ('local', 'remote')),
                engine TEXT NOT NULL,
                tamad_id TEXT,
                base_url TEXT,
                api_key TEXT,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
            );
            "#,
        )
        .unwrap();
        conn
    }

    // ── insert_provider ──

    #[test]
    fn test_insert_provider_returns_id() {
        let conn = setup_test_db();
        let id = insert_provider(
            &conn,
            "my-provider",
            "local",
            "llama_cpp",
            Some("tamad-uuid"),
            None,
            None,
        )
        .unwrap();
        assert!(id > 0);
    }

    #[test]
    fn test_insert_remote_provider() {
        let conn = setup_test_db();
        let id = insert_provider(
            &conn,
            "openai-proxy",
            "remote",
            "openai",
            None,
            Some("https://api.openai.com/v1"),
            Some("sk-xxx"),
        )
        .unwrap();
        assert!(id > 0);

        let provider = get_provider(&conn, "openai-proxy").unwrap().unwrap();
        assert_eq!(provider.name, "openai-proxy");
        assert!(provider.provider_type.is_remote());
        assert!(provider.engine.is_open_ai());
        assert_eq!(
            provider.base_url,
            Some("https://api.openai.com/v1".to_string())
        );
        assert_eq!(provider.api_key, Some("sk-xxx".to_string()));
        assert!(provider.tamad_id.is_none());
    }

    #[test]
    fn test_insert_duplicate_name_fails() {
        let conn = setup_test_db();
        insert_provider(&conn, "my-provider", "local", "llama_cpp", None, None, None).unwrap();

        let result = insert_provider(&conn, "my-provider", "local", "llama_cpp", None, None, None);
        assert!(result.is_err());
    }

    // ── get_provider ──

    #[test]
    fn test_get_provider_not_found() {
        let conn = setup_test_db();
        let result = get_provider(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_get_provider_round_trip() {
        let conn = setup_test_db();
        insert_provider(
            &conn,
            "local-llama",
            "local",
            "llama_cpp",
            Some("uuid-123"),
            None,
            None,
        )
        .unwrap();

        let provider = get_provider(&conn, "local-llama").unwrap().unwrap();
        assert_eq!(provider.name, "local-llama");
        assert!(provider.provider_type.is_local());
        assert!(provider.engine.is_llama_cpp());
        assert_eq!(provider.tamad_id, Some("uuid-123".to_string()));
    }

    // ── list_providers ──

    #[test]
    fn test_list_providers_empty() {
        let conn = setup_test_db();
        let providers = list_providers(&conn).unwrap();
        assert!(providers.is_empty());
    }

    #[test]
    fn test_list_providers_ordered() {
        let conn = setup_test_db();
        insert_provider(&conn, "zebra", "local", "llama_cpp", None, None, None).unwrap();
        insert_provider(
            &conn,
            "alpha",
            "remote",
            "openai",
            None,
            Some("https://alpha.api/v1"),
            None,
        )
        .unwrap();

        let providers = list_providers(&conn).unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].name, "alpha");
        assert_eq!(providers[1].name, "zebra");
    }

    // ── update_provider ──

    #[test]
    fn test_update_provider_base_url() {
        let conn = setup_test_db();
        insert_provider(
            &conn,
            "remote-api",
            "remote",
            "openai",
            None,
            Some("https://old.api/v1"),
            Some("sk-old"),
        )
        .unwrap();

        update_provider(&conn, "remote-api", Some("https://new.api/v1"), None).unwrap();

        let provider = get_provider(&conn, "remote-api").unwrap().unwrap();
        assert_eq!(provider.base_url, Some("https://new.api/v1".to_string()));
        // api_key unchanged
        assert_eq!(provider.api_key, Some("sk-old".to_string()));
    }

    #[test]
    fn test_update_provider_nonexistent_no_error() {
        let conn = setup_test_db();
        update_provider(&conn, "nonexistent", Some("https://x.com"), None).unwrap();
    }

    // ── delete_provider ──

    #[test]
    fn test_delete_provider() {
        let conn = setup_test_db();
        insert_provider(&conn, "to-delete", "local", "llama_cpp", None, None, None).unwrap();

        let deleted = delete_provider(&conn, "to-delete").unwrap();
        assert!(deleted);
        assert!(get_provider(&conn, "to-delete").unwrap().is_none());
    }

    #[test]
    fn test_delete_provider_not_found() {
        let conn = setup_test_db();
        let deleted = delete_provider(&conn, "nonexistent").unwrap();
        assert!(!deleted);
    }
}
