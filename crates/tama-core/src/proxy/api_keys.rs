//! API key management types, generation, hashing, and database operations.
//!
//! Provides deterministic key generation (`tama_` + 32 base62 chars),
//! SHA-256 hashing, and full CRUD operations backed by SQLite.

use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Permissions that an API key may carry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Inference,
    ManagementRead,
    ManagementWrite,
}

/// The subject that was authenticated.
#[derive(Debug, Clone)]
pub enum AuthSubject {
    User { username: String },
    Key { key_id: i64, scopes: Vec<Scope> },
}

/// A loaded API key record returned from the database.
#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyRecord {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,
    pub scopes: Vec<Scope>,
    pub created_by: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub expires_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Key generation / hashing helpers
// ---------------------------------------------------------------------------

/// Generate a new random API key.
///
/// Returns a string of the form `tama_` + 32 alphanumeric characters
/// drawn from the base62 alphabet (`a-zA-Z0-9`).
pub fn generate_key() -> String {
    const PREFIX: &str = "tama_";
    let random_part: String = rand::Rng::sample_iter(&mut rand::rng(), rand::distr::Alphanumeric)
        .take(32)
        .map(|b| b as char)
        .collect();
    format!("{PREFIX}{random_part}")
}

/// Hash a raw API key using SHA-256 and return the hex-encoded digest.
///
/// The hash is computed over the exact bytes of the full key string
/// (including the `tama_` prefix).
pub fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Extract the public prefix portion of an API key.
///
/// Returns `tama_` + the first 8 characters of the random portion
/// (e.g. `tama_aB3dEfGh`).
pub fn extract_prefix(key: &str) -> String {
    let random_part = key.strip_prefix("tama_").unwrap_or(key);
    format!("tama_{}", &random_part[..8.min(random_part.len())])
}

// ---------------------------------------------------------------------------
// Database operations
// ---------------------------------------------------------------------------

/// Validate a raw API key against the database.
///
/// Returns `Some((key_id, scopes))` when the key is valid (not revoked,
/// not expired, and exists).
///
/// Updates `last_used_at` on successful validation.
///
/// Note: Hash lookup via `WHERE key_hash = ?` leaks hash existence through
/// DB query timing. A full constant-time comparison across all stored hashes
/// would be more robust but is impractical for SQLite. The attack surface is
/// mitigated by: (a) the management API is behind auth, (b) keys are 37 chars
/// of base62 (~177 bits of entropy), and (c) rate limiting can be added later.
pub fn validate_key(conn: &Connection, raw_key: &str) -> Result<Option<(i64, Vec<Scope>)>> {
    let key_hash = hash_key(raw_key);
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let row = conn.query_row(
        "SELECT id, scopes, revoked_at, expires_at FROM api_keys WHERE key_hash = ?",
        [&key_hash],
        |row| {
            let id: i64 = row.get(0)?;
            let scopes: String = row.get(1)?;
            let revoked_at: Option<String> = row.get(2)?;
            let expires_at: Option<String> = row.get(3)?;
            Ok((id, scopes, revoked_at, expires_at))
        },
    );

    let (id, scopes_str, revoked_at, expires_at) = match row {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e).context("failed to query api_keys by hash"),
    };

    // Check revocation
    if revoked_at.is_some() {
        return Ok(None);
    }

    // Check expiry
    if let Some(ref expires) = expires_at {
        let expiry = chrono::DateTime::parse_from_rfc3339(expires)
            .context("invalid expires_at format in database")?;
        if chrono::Utc::now() >= expiry {
            return Ok(None);
        }
    }

    // Update last_used_at
    conn.execute(
        "UPDATE api_keys SET last_used_at = ? WHERE id = ?",
        rusqlite::params![now.as_str(), id],
    )?;

    let scopes = parse_scopes(&scopes_str);
    Ok(Some((id, scopes)))
}

/// Create a new API key in the database.
///
/// Validates that scopes are non-empty and contain only known values.
/// Sets `api_keys_enabled = 1` on the `app_proxy` table.
pub fn create_key(
    conn: &Connection,
    name: &str,
    raw_key: &str,
    scopes: &[Scope],
    created_by: &str,
    expires_at: Option<&str>,
) -> Result<i64> {
    validate_scopes(scopes)?;

    let key_hash = hash_key(raw_key);
    let key_prefix = extract_prefix(raw_key);
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let scopes_str = serialize_scopes(scopes);

    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        rusqlite::params![name, key_prefix, key_hash, scopes_str, created_by, now, expires_at],
    )?;

    let id = tx.last_insert_rowid();

    // Enable API keys on app_proxy
    tx.execute("UPDATE app_proxy SET api_keys_enabled = 1 WHERE id = 1", [])?;

    tx.commit()?;
    Ok(id)
}

/// List all API keys (including revoked ones), ordered by creation date.
pub fn list_keys(conn: &Connection) -> Result<Vec<ApiKeyRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, key_prefix, scopes, created_by, created_at, last_used_at, revoked_at, expires_at FROM api_keys ORDER BY created_at DESC, id DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(ApiKeyRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            key_prefix: row.get(2)?,
            scopes: parse_scopes(&row.get::<_, String>(3)?),
            created_by: row.get(4)?,
            created_at: row.get(5)?,
            last_used_at: row.get(6)?,
            revoked_at: row.get(7)?,
            expires_at: row.get(8)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>()
        .context("failed to list api_keys")
}

/// Revoke an API key by setting `revoked_at` to the current time.
///
/// If this was the last active (non-revoked) key, sets
/// `api_keys_enabled = 0` on the `app_proxy` table.
pub fn revoke_key(conn: &Connection, key_id: i64) -> Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let tx = conn.unchecked_transaction()?;

    // Set revoked_at
    tx.execute(
        "UPDATE api_keys SET revoked_at = ? WHERE id = ?",
        rusqlite::params![now.as_str(), key_id],
    )?;

    // Check if any active (non-revoked, non-expired) keys remain.
    // Expired keys are excluded so we don't leave api_keys_enabled = 1
    // with only expired keys — that would lock out the user.
    let active_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        [],
        |row| row.get(0),
    )?;

    if active_count == 0 {
        tx.execute("UPDATE app_proxy SET api_keys_enabled = 0 WHERE id = 1", [])?;
    }

    tx.commit()?;
    Ok(())
}

/// Update the scopes of an existing API key.
///
/// Validates that scopes are non-empty and contain only known values.
/// Returns the updated `ApiKeyRecord` so callers don't need a second query.
pub fn update_key_scopes(conn: &Connection, key_id: i64, scopes: &[Scope]) -> Result<ApiKeyRecord> {
    validate_scopes(scopes)?;

    let tx = conn.unchecked_transaction()?;

    let scopes_str = serialize_scopes(scopes);
    tx.execute(
        "UPDATE api_keys SET scopes = ? WHERE id = ?",
        rusqlite::params![scopes_str, key_id],
    )
    .context("failed to update api_key scopes")?;

    // Return the updated record in the same transaction
    let record = get_key(&tx, key_id)?.ok_or_else(|| anyhow!("key not found after update"))?;

    tx.commit()?;
    Ok(record)
}

/// Look up a single API key by its database ID.
pub fn get_key(conn: &Connection, key_id: i64) -> Result<Option<ApiKeyRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, key_prefix, scopes, created_by, created_at, last_used_at, revoked_at, expires_at FROM api_keys WHERE id = ?",
    )?;

    let row = stmt
        .query_row([key_id], |row| {
            Ok(ApiKeyRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                key_prefix: row.get(2)?,
                scopes: parse_scopes(&row.get::<_, String>(3)?),
                created_by: row.get(4)?,
                created_at: row.get(5)?,
                last_used_at: row.get(6)?,
                revoked_at: row.get(7)?,
                expires_at: row.get(8)?,
            })
        })
        .optional()?;

    Ok(row)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Validate that scopes are non-empty and contain only known values.
fn validate_scopes(scopes: &[Scope]) -> Result<()> {
    if scopes.is_empty() {
        return Err(anyhow!("scopes must not be empty"));
    }
    // All values in the enum are known — scope validation is structural.
    Ok(())
}

/// Serialize a slice of scopes to a JSON string for storage.
fn serialize_scopes(scopes: &[Scope]) -> String {
    serde_json::to_string(scopes).expect("Scope serialization should not fail")
}

/// Parse a JSON string of scopes back into a Vec.
/// Warns if the stored value is malformed (e.g. corruption or unknown scope).
fn parse_scopes(json: &str) -> Vec<Scope> {
    serde_json::from_str(json).unwrap_or_else(|e| {
        tracing::warn!(error = %e, raw = %json, "malformed scopes in api_keys row");
        Vec::new()
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    /// Helper to open an in-memory database with all migrations applied.
    fn test_conn() -> Connection {
        open_in_memory().unwrap().conn
    }

    // -----------------------------------------------------------------------
    // Key generation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_generate_key_format() {
        let key = generate_key();
        assert!(key.starts_with("tama_"), "key must start with tama_");
        assert_eq!(key.len(), 37, "key must be 37 chars (tama_ + 32)");
        let random_part = &key[5..];
        assert!(
            random_part.chars().all(|c| c.is_ascii_alphanumeric()),
            "random part must be base62"
        );
    }

    #[test]
    fn test_generate_key_uniqueness() {
        let keys: Vec<String> = (0..100).map(|_| generate_key()).collect();
        let unique: std::collections::HashSet<&str> = keys.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            keys.len(),
            unique.len(),
            "all generated keys must be unique"
        );
    }

    #[test]
    fn test_hash_key_deterministic() {
        let key = generate_key();
        let h1 = hash_key(&key);
        let h2 = hash_key(&key);
        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(h1.len(), 64, "SHA-256 hex must be 64 chars");
    }

    #[test]
    fn test_hash_key_different_for_different_keys() {
        let key1 = generate_key();
        let key2 = generate_key();
        assert_ne!(key1, key2);
        assert_ne!(hash_key(&key1), hash_key(&key2));
    }

    #[test]
    fn test_extract_prefix() {
        let key = "tama_abcdefghij1234567890abcdef12";
        let prefix = extract_prefix(key);
        assert_eq!(prefix, "tama_abcdefgh");
    }

    #[test]
    fn test_extract_prefix_short_key() {
        let key = "tama_short";
        let prefix = extract_prefix(key);
        assert_eq!(prefix, "tama_short");
    }

    // -----------------------------------------------------------------------
    // Scope tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_scope_serialization() {
        let scopes = vec![Scope::Inference, Scope::ManagementRead];
        let json = serialize_scopes(&scopes);
        let parsed: Vec<Scope> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, scopes);
    }

    #[test]
    fn test_scope_kebab_case() {
        let scope = Scope::ManagementRead;
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(json, "\"management-read\"");
    }

    #[test]
    fn test_scope_validation_rejects_empty() {
        let conn = test_conn();
        let key = generate_key();
        let result = create_key(&conn, "test", &key, &[], "admin", None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("scopes must not be empty"));
    }

    // -----------------------------------------------------------------------
    // CRUD / roundtrip tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_create_and_validate_key_roundtrip() {
        let conn = test_conn();
        let key = generate_key();
        let scopes = vec![Scope::Inference, Scope::ManagementRead];

        let id = create_key(&conn, "test-key", &key, &scopes, "admin", None).unwrap();
        assert!(id > 0);

        let result = validate_key(&conn, &key).unwrap();
        assert!(result.is_some());
        let (validated_id, validated_scopes) = result.unwrap();
        assert_eq!(validated_id, id);
        assert_eq!(validated_scopes, scopes);
    }

    #[test]
    fn test_validate_nonexistent_key_returns_none() {
        let conn = test_conn();
        let result = validate_key(&conn, "tama_nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_revoked_key_returns_none() {
        let conn = test_conn();
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        let id = create_key(&conn, "test", &key, &scopes, "admin", None).unwrap();
        revoke_key(&conn, id).unwrap();

        let result = validate_key(&conn, &key).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_expired_key_returns_none() {
        let conn = test_conn();
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        // Use a past date for expires_at
        let past = "2020-01-01T00:00:00Z";
        let _id = create_key(&conn, "test", &key, &scopes, "admin", Some(past)).unwrap();

        let result = validate_key(&conn, &key).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_validate_future_expiry_succeeds() {
        let conn = test_conn();
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        // Use a future date
        let future = "2099-12-31T23:59:59Z";
        let id = create_key(&conn, "test", &key, &scopes, "admin", Some(future)).unwrap();

        let result = validate_key(&conn, &key).unwrap();
        assert!(result.is_some());
        let (validated_id, _) = result.unwrap();
        assert_eq!(validated_id, id);
    }

    #[test]
    fn test_api_keys_enabled_flag_toggled() {
        let conn = test_conn();

        // Seed the app_proxy row (migrations only add the column, not the row)
        conn.execute("INSERT OR IGNORE INTO app_proxy (id) VALUES (1)", [])
            .unwrap();

        // Initially should be 0
        let enabled: i64 = conn
            .query_row(
                "SELECT api_keys_enabled FROM app_proxy WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 0);

        // Create a key — should toggle to 1
        let key = generate_key();
        create_key(&conn, "test", &key, &[Scope::Inference], "admin", None).unwrap();
        let enabled: i64 = conn
            .query_row(
                "SELECT api_keys_enabled FROM app_proxy WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 1);

        // Revoke the only key — should toggle back to 0
        let id: i64 = conn
            .query_row("SELECT id FROM api_keys", [], |row| row.get(0))
            .unwrap();
        revoke_key(&conn, id).unwrap();
        let enabled: i64 = conn
            .query_row(
                "SELECT api_keys_enabled FROM app_proxy WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 0);
    }

    #[test]
    fn test_list_keys() {
        let conn = test_conn();
        let key1 = generate_key();
        let key2 = generate_key();

        create_key(&conn, "first", &key1, &[Scope::Inference], "admin", None).unwrap();
        create_key(
            &conn,
            "second",
            &key2,
            &[Scope::ManagementWrite],
            "admin",
            None,
        )
        .unwrap();

        let keys = list_keys(&conn).unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].name, "second"); // DESC order
        assert_eq!(keys[1].name, "first");
    }

    #[test]
    fn test_get_key() {
        let conn = test_conn();
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        let id = create_key(&conn, "test", &key, &scopes, "admin", None).unwrap();

        let found = get_key(&conn, id).unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.name, "test");
        assert_eq!(found.scopes, scopes);
    }

    #[test]
    fn test_get_key_not_found() {
        let conn = test_conn();
        let result = get_key(&conn, 9999).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_update_key_scopes() {
        let conn = test_conn();
        let key = generate_key();

        create_key(&conn, "test", &key, &[Scope::Inference], "admin", None).unwrap();

        let id: i64 = conn
            .query_row("SELECT id FROM api_keys", [], |row| row.get(0))
            .unwrap();

        let updated =
            update_key_scopes(&conn, id, &[Scope::ManagementRead, Scope::ManagementWrite]).unwrap();
        assert_eq!(
            updated.scopes,
            vec![Scope::ManagementRead, Scope::ManagementWrite]
        );
    }

    #[test]
    fn test_last_used_at_updated_on_validate() {
        let conn = test_conn();
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        let id = create_key(&conn, "test", &key, &scopes, "admin", None).unwrap();

        // Initially last_used_at should be NULL
        let last_used: Option<String> = conn
            .query_row(
                "SELECT last_used_at FROM api_keys WHERE id = ?",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_used.is_none());

        // Validate the key
        validate_key(&conn, &key).unwrap();

        // Now last_used_at should be set
        let last_used: Option<String> = conn
            .query_row(
                "SELECT last_used_at FROM api_keys WHERE id = ?",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(last_used.is_some());
    }

    #[test]
    fn test_validate_key_wrong_key_returns_none() {
        let conn = test_conn();
        let correct_key = generate_key();
        let wrong_key = generate_key();

        create_key(
            &conn,
            "test",
            &correct_key,
            &[Scope::Inference],
            "admin",
            None,
        )
        .unwrap();

        let result = validate_key(&conn, &wrong_key).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_prefix_matches_key_format() {
        let key = generate_key();
        let prefix = extract_prefix(&key);
        assert!(prefix.starts_with("tama_"));
        assert_eq!(prefix.len(), 13); // "tama_" (5) + 8 chars
    }
}
