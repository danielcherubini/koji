//! Model alias database query functions.

use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection};
use serde::{Deserialize, Serialize};

/// Response type for alias queries that include the resolved model name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasResponse {
    pub id: i64,
    pub name: String,
    pub model_id: i64,
    pub model_name: String, // COALESCE(api_name, repo_id)
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Load all aliases joined with model_configs to get the resolved model name.
/// Used by ProxyState to populate the in-memory cache.
/// Returns (alias_name, resolved_model_name) pairs.
/// resolved_model_name = COALESCE(api_name, repo_id)
pub fn load_aliases_for_cache(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT a.name, COALESCE(m.api_name, m.repo_id)
         FROM model_aliases a
         JOIN model_configs m ON m.id = a.model_id
         WHERE a.enabled = 1
         ORDER BY a.name ASC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Load all aliases with model names for the web API.
pub fn get_all_aliases(conn: &Connection) -> Result<Vec<AliasResponse>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name, a.model_id,
                COALESCE(m.api_name, m.repo_id),
                a.description, a.enabled, a.created_at, a.updated_at
         FROM model_aliases a
         JOIN model_configs m ON m.id = a.model_id
         ORDER BY a.name ASC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(AliasResponse {
            id: row.get(0)?,
            name: row.get(1)?,
            model_id: row.get(2)?,
            model_name: row.get(3)?,
            description: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Get a single alias by integer id.
pub fn get_alias_by_id(conn: &Connection, id: i64) -> Result<Option<AliasResponse>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.name, a.model_id,
                COALESCE(m.api_name, m.repo_id),
                a.description, a.enabled, a.created_at, a.updated_at
         FROM model_aliases a
         JOIN model_configs m ON m.id = a.model_id
         WHERE a.id = ?1",
    )?;

    let mut rows = stmt.query_map([id], |row| {
        Ok(AliasResponse {
            id: row.get(0)?,
            name: row.get(1)?,
            model_id: row.get(2)?,
            model_name: row.get(3)?,
            description: row.get(4)?,
            enabled: row.get::<_, i32>(5)? != 0,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Insert a new alias. Returns the new row's id.
pub fn insert_alias(
    conn: &Connection,
    name: &str,
    model_id: i64,
    description: Option<&str>,
) -> Result<i64> {
    let id: i64 = conn.query_row(
        "INSERT INTO model_aliases (name, model_id, description)
         VALUES (?1, ?2, ?3)
         RETURNING id",
        params![name, model_id, description],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Update an existing alias. Only updates fields that are Some.
pub fn update_alias(
    conn: &Connection,
    id: i64,
    name: Option<&str>,
    model_id: Option<i64>,
    description: Option<Option<&str>>,
    enabled: Option<bool>,
) -> Result<()> {
    let mut sets = Vec::new();
    let mut bind_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(n) = name {
        sets.push("name = ?".to_string());
        bind_params.push(Box::new(n));
    }
    if let Some(m) = model_id {
        sets.push("model_id = ?".to_string());
        bind_params.push(Box::new(m));
    }
    if let Some(desc) = description {
        sets.push("description = ?".to_string());
        bind_params.push(Box::new(desc));
    }
    if let Some(e) = enabled {
        sets.push("enabled = ?".to_string());
        bind_params.push(Box::new(if e { 1i32 } else { 0i32 }));
    }

    if sets.is_empty() {
        return Ok(());
    }

    sets.push("updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')".to_string());

    let sql = format!("UPDATE model_aliases SET {} WHERE id = ?", sets.join(", "));

    let mut params_vec: Vec<&dyn rusqlite::ToSql> =
        bind_params.iter().map(|p| p.as_ref()).collect();
    params_vec.push(&id);

    conn.execute(&sql, params_from_iter(params_vec))?;
    Ok(())
}

/// Delete an alias by id.
pub fn delete_alias(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM model_aliases WHERE id = ?", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Set up an in-memory database with model_aliases and model_configs tables.
    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE model_configs (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                repo_id       TEXT NOT NULL UNIQUE COLLATE NOCASE,
                display_name  TEXT,
                backend       TEXT NOT NULL DEFAULT 'llama_cpp',
                enabled       INTEGER NOT NULL DEFAULT 1,
                api_name      TEXT,
                created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );

            CREATE TABLE model_aliases (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE COLLATE NOCASE,
                model_id INTEGER NOT NULL REFERENCES model_configs(id) ON DELETE CASCADE,
                description TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );
            CREATE INDEX IF NOT EXISTS idx_model_aliases_model_id ON model_aliases(model_id);
            CREATE INDEX IF NOT EXISTS idx_model_aliases_enabled ON model_aliases(enabled);
            "#,
        )
        .unwrap();
        conn
    }

    /// Insert a model config and return its id.
    fn insert_model(conn: &Connection, repo_id: &str, api_name: Option<&str>) -> i64 {
        conn.query_row(
            "INSERT INTO model_configs (repo_id, api_name) VALUES (?1, ?2) RETURNING id",
            params![repo_id, api_name],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// Empty table returns empty vec.
    #[test]
    fn test_load_aliases_for_cache_empty() {
        let conn = setup_test_db();
        let result = load_aliases_for_cache(&conn).unwrap();
        assert!(result.is_empty());
    }

    /// Correct JOIN with COALESCE.
    #[test]
    fn test_load_aliases_for_cache_with_data() {
        let conn = setup_test_db();

        // Model with api_name
        let model1_id = insert_model(&conn, "org/model1", Some("my-api-name"));
        // Model without api_name (falls back to repo_id)
        let model2_id = insert_model(&conn, "org/model2", None);

        insert_alias(&conn, "fast", model1_id, None).unwrap();
        insert_alias(&conn, "slow", model2_id, None).unwrap();
        // Disabled alias
        let disabled_id = insert_model(&conn, "org/model3", Some("disabled-api"));
        insert_alias(&conn, "off", disabled_id, None).unwrap();
        update_alias(&conn, 3, None, None, None, Some(false)).unwrap();

        let result = load_aliases_for_cache(&conn).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("fast".to_string(), "my-api-name".to_string()));
        assert_eq!(result[1], ("slow".to_string(), "org/model2".to_string()));
    }

    /// Round-trip insert + get by id.
    #[test]
    fn test_insert_and_get_alias() {
        let conn = setup_test_db();
        let model_id = insert_model(&conn, "org/test-model", Some("test-api"));

        let alias_id = insert_alias(&conn, "my-alias", model_id, Some("A test alias")).unwrap();
        assert_eq!(alias_id, 1);

        let alias = get_alias_by_id(&conn, alias_id).unwrap().unwrap();
        assert_eq!(alias.name, "my-alias");
        assert_eq!(alias.model_id, model_id);
        assert_eq!(alias.model_name, "test-api");
        assert_eq!(alias.description, Some("A test alias".to_string()));
        assert!(alias.enabled);
    }

    /// Partial update works.
    #[test]
    fn test_update_alias() {
        let conn = setup_test_db();
        let model_id = insert_model(&conn, "org/test-model", Some("test-api"));

        let alias_id = insert_alias(&conn, "original", model_id, Some("Old desc")).unwrap();

        // Update only the name
        update_alias(&conn, alias_id, Some("renamed"), None, None, None).unwrap();

        let alias = get_alias_by_id(&conn, alias_id).unwrap().unwrap();
        assert_eq!(alias.name, "renamed");
        assert_eq!(alias.description, Some("Old desc".to_string())); // unchanged

        // Update description to None
        update_alias(&conn, alias_id, None, None, Some(None), None).unwrap();

        let alias = get_alias_by_id(&conn, alias_id).unwrap().unwrap();
        assert_eq!(alias.description, None);
    }

    /// Delete removes row.
    #[test]
    fn test_delete_alias() {
        let conn = setup_test_db();
        let model_id = insert_model(&conn, "org/test-model", None);

        let alias_id = insert_alias(&conn, "to-delete", model_id, None).unwrap();
        assert!(get_alias_by_id(&conn, alias_id).unwrap().is_some());

        delete_alias(&conn, alias_id).unwrap();
        assert!(get_alias_by_id(&conn, alias_id).unwrap().is_none());
    }

    /// UNIQUE constraint fires (case-insensitive via NOCASE).
    #[test]
    fn test_duplicate_name_rejected() {
        let conn = setup_test_db();
        let model_id = insert_model(&conn, "org/test-model", None);

        insert_alias(&conn, "unique-name", model_id, None).unwrap();

        // Case-variant should also be rejected
        let result = insert_alias(&conn, "Unique-Name", model_id, None);
        assert!(result.is_err());
    }
}
