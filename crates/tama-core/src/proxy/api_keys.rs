//! API key management types, generation, hashing, and database operations.
//!
//! Provides deterministic key generation (`tama_` + 32 base62 chars),
//! SHA-256 hashing, and full CRUD operations backed by Postgres.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::types::time::OffsetDateTime;
use sqlx::{PgPool, Row, Transaction};
use std::sync::Arc;

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

/// Database access for API keys.
///
/// Wraps a Postgres pool handle; all methods are async.
///
/// This struct replaces the previous public free functions that took a
/// raw `&Connection` parameter, encapsulating the pool within a small
/// handle so callers don't pass raw pools around.
pub struct ApiKeyStore {
    pool: Arc<PgPool>,
}

impl ApiKeyStore {
    /// Create a new `ApiKeyStore` from a clone of the shared pool.
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    /// Validate a raw API key against the database.
    ///
    /// Returns `Some((key_id, scopes))` when the key is valid (not revoked,
    /// not expired, and exists).
    ///
    /// Updates `last_used_at` on successful validation.
    ///
    /// Note: Hash lookup via `WHERE key_hash = $1` leaks hash existence
    /// through DB query timing. A full constant-time comparison across all
    /// stored hashes would be more robust but is impractical at scale. The
    /// attack surface is mitigated by: (a) the management API is behind auth,
    /// (b) keys are 37 chars of base62 (~177 bits of entropy), and (c) rate
    /// limiting can be added later.
    pub async fn validate_key(&self, raw_key: &str) -> Result<Option<(i64, Vec<Scope>)>> {
        let key_hash = hash_key(raw_key);

        let row = sqlx::query(
            "SELECT id, scopes, revoked_at, expires_at FROM api_keys WHERE key_hash = $1",
        )
        .bind(&key_hash)
        .fetch_optional(&*self.pool)
        .await
        .context("failed to query api_keys by hash")?;

        let Some((id, scopes_str, revoked_at, expires_at)) = row.map(|r| {
            (
                r.get::<i64, _>("id"),
                r.get::<String, _>("scopes"),
                r.get::<Option<OffsetDateTime>, _>("revoked_at"),
                r.get::<Option<OffsetDateTime>, _>("expires_at"),
            )
        }) else {
            return Ok(None);
        };

        // Check revocation
        if revoked_at.is_some() {
            return Ok(None);
        }

        // Check expiry
        if let Some(expires) = expires_at {
            if OffsetDateTime::now_utc() >= expires {
                return Ok(None);
            }
        }

        // Update last_used_at
        sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(&*self.pool)
            .await
            .context("failed to update api_keys.last_used_at")?;

        let scopes = parse_scopes(&scopes_str);
        Ok(Some((id, scopes)))
    }

    /// Create a new API key in the database.
    ///
    /// Validates that scopes are non-empty and contain only known values.
    /// Sets `api_keys_enabled = TRUE` on the `app_proxy` table.
    pub async fn create_key(
        &self,
        name: &str,
        raw_key: &str,
        scopes: &[Scope],
        created_by: &str,
        expires_at: Option<&str>,
    ) -> Result<i64> {
        validate_scopes(scopes)?;

        let expires_at = expires_at
            .map(|s| {
                let dt = chrono::DateTime::parse_from_rfc3339(s)
                    .with_context(|| format!("invalid expires_at: {s}"))?;
                OffsetDateTime::from_unix_timestamp(dt.timestamp())
                    .with_context(|| format!("invalid expires_at: {s}"))
            })
            .transpose()?;

        let key_hash = hash_key(raw_key);
        let key_prefix = extract_prefix(raw_key);
        let scopes_str = serialize_scopes(scopes);

        let mut tx: Transaction<'_, sqlx::Postgres> = self.pool.begin().await?;

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(name)
        .bind(&key_prefix)
        .bind(&key_hash)
        .bind(&scopes_str)
        .bind(created_by)
        .bind(expires_at)
        .fetch_one(&mut *tx)
        .await?;

        // Enable API keys on app_proxy
        sqlx::query("UPDATE app_proxy SET api_keys_enabled = TRUE WHERE id = 1")
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(id)
    }

    /// List all API keys (including revoked ones), ordered by creation date.
    pub async fn list_keys(&self) -> Result<Vec<ApiKeyRecord>> {
        let rows = sqlx::query(
            "SELECT id, name, key_prefix, scopes, created_by, created_at,
                    last_used_at, revoked_at, expires_at
             FROM api_keys ORDER BY created_at DESC, id DESC",
        )
        .fetch_all(&*self.pool)
        .await
        .context("failed to list api_keys")?;

        rows.into_iter()
            .map(|row| {
                Ok(ApiKeyRecord {
                    id: row.get("id"),
                    name: row.get("name"),
                    key_prefix: row.get("key_prefix"),
                    scopes: parse_scopes(&row.get::<String, _>("scopes")),
                    created_by: row.get("created_by"),
                    created_at: row.get::<OffsetDateTime, _>("created_at").to_string(),
                    last_used_at: row
                        .get::<Option<OffsetDateTime>, _>("last_used_at")
                        .map(|d| d.to_string()),
                    revoked_at: row
                        .get::<Option<OffsetDateTime>, _>("revoked_at")
                        .map(|d| d.to_string()),
                    expires_at: row
                        .get::<Option<OffsetDateTime>, _>("expires_at")
                        .map(|d| d.to_string()),
                })
            })
            .collect()
    }

    /// Revoke an API key by setting `revoked_at` to the current time.
    ///
    /// If this was the last active (non-revoked, non-expired) key, sets
    /// `api_keys_enabled = FALSE` on the `app_proxy` table.
    ///
    /// Returns the new value of `api_keys_enabled` (false if this was the
    /// last active key, true otherwise). The caller must sync the in-memory
    /// config with this value.
    pub async fn revoke_key(&self, key_id: i64) -> Result<bool> {
        let mut tx: Transaction<'_, sqlx::Postgres> = self.pool.begin().await?;

        // Set revoked_at
        sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1")
            .bind(key_id)
            .execute(&mut *tx)
            .await?;

        // Check if any active (non-revoked, non-expired) keys remain.
        // Expired keys are excluded so we don't leave api_keys_enabled = TRUE
        // with only expired keys — that would lock out the user.
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM api_keys
             WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())",
        )
        .fetch_one(&mut *tx)
        .await?;

        let enabled = active_count > 0;
        sqlx::query("UPDATE app_proxy SET api_keys_enabled = $1 WHERE id = 1")
            .bind(enabled)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(enabled)
    }

    /// Update the scopes of an existing API key.
    ///
    /// Validates that scopes are non-empty and contain only known values.
    /// Returns the updated `ApiKeyRecord` so callers don't need a second query.
    pub async fn update_key_scopes(&self, key_id: i64, scopes: &[Scope]) -> Result<ApiKeyRecord> {
        validate_scopes(scopes)?;

        let mut tx: Transaction<'_, sqlx::Postgres> = self.pool.begin().await?;

        let scopes_str = serialize_scopes(scopes);
        sqlx::query("UPDATE api_keys SET scopes = $1 WHERE id = $2")
            .bind(&scopes_str)
            .bind(key_id)
            .execute(&mut *tx)
            .await
            .context("failed to update api_key scopes")?;

        // Return the updated record in the same transaction
        let row = sqlx::query(
            "SELECT id, name, key_prefix, scopes, created_by, created_at,
                    last_used_at, revoked_at, expires_at
             FROM api_keys WHERE id = $1",
        )
        .bind(key_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow!("key not found after update"))?;

        let record = ApiKeyRecord {
            id: row.get("id"),
            name: row.get("name"),
            key_prefix: row.get("key_prefix"),
            scopes: parse_scopes(&row.get::<String, _>("scopes")),
            created_by: row.get("created_by"),
            created_at: row.get::<OffsetDateTime, _>("created_at").to_string(),
            last_used_at: row
                .get::<Option<OffsetDateTime>, _>("last_used_at")
                .map(|d| d.to_string()),
            revoked_at: row
                .get::<Option<OffsetDateTime>, _>("revoked_at")
                .map(|d| d.to_string()),
            expires_at: row
                .get::<Option<OffsetDateTime>, _>("expires_at")
                .map(|d| d.to_string()),
        };

        tx.commit().await?;
        Ok(record)
    }

    /// Look up a single API key by its database ID.
    pub async fn get_key(&self, key_id: i64) -> Result<Option<ApiKeyRecord>> {
        let row = sqlx::query(
            "SELECT id, name, key_prefix, scopes, created_by, created_at,
                    last_used_at, revoked_at, expires_at
             FROM api_keys WHERE id = $1",
        )
        .bind(key_id)
        .fetch_optional(&*self.pool)
        .await
        .context("failed to get api_key")?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(ApiKeyRecord {
            id: row.get("id"),
            name: row.get("name"),
            key_prefix: row.get("key_prefix"),
            scopes: parse_scopes(&row.get::<String, _>("scopes")),
            created_by: row.get("created_by"),
            created_at: row.get::<OffsetDateTime, _>("created_at").to_string(),
            last_used_at: row
                .get::<Option<OffsetDateTime>, _>("last_used_at")
                .map(|d| d.to_string()),
            revoked_at: row
                .get::<Option<OffsetDateTime>, _>("revoked_at")
                .map(|d| d.to_string()),
            expires_at: row
                .get::<Option<OffsetDateTime>, _>("expires_at")
                .map(|d| d.to_string()),
        }))
    }

    /// Look up the name of an API key by its database ID.
    pub async fn get_key_name(&self, key_id: i64) -> Result<Option<String>> {
        let name = sqlx::query_scalar("SELECT name FROM api_keys WHERE id = $1")
            .bind(key_id)
            .fetch_optional(&*self.pool)
            .await
            .context("failed to get api_key name")?;
        Ok(name)
    }
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

    /// Fixed key used to pin the v2 (SHA-256 hex) hash format across the
    /// SQLite → Postgres migration. If `hash_key` ever changes, this test
    /// fails and previously issued keys stop validating.
    const FIXED_KEY: &str = "tama_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
    const FIXED_KEY_HASH: &str = "52ef7a9982e57c9a273161c12928327b28eba515738e0c4347c518b0b5e17cf2";

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

    /// The v2 hash format is pinned: a fixed key must hash to a fixed
    /// literal. v2 keys issued under SQLite must keep validating after the
    /// Postgres migration.
    #[test]
    fn test_hash_key_fixed_v2_hash() {
        assert_eq!(hash_key(FIXED_KEY), FIXED_KEY_HASH);
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

    #[test]
    fn test_extract_prefix_matches_key_format() {
        let key = generate_key();
        let prefix = extract_prefix(&key);
        assert!(prefix.starts_with("tama_"));
        assert_eq!(prefix.len(), 13); // "tama_" (5) + 8 chars
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

    // -----------------------------------------------------------------------
    // CRUD / roundtrip tests (Postgres harness)
    // -----------------------------------------------------------------------

    async fn test_store() -> (ApiKeyStore, crate::testing::postgres::SchemaGuard) {
        let guard = crate::testing::postgres::with_schema().await;
        let store = ApiKeyStore::new(Arc::new(guard.pool.clone()));
        (store, guard)
    }

    #[tokio::test]
    async fn test_scope_validation_rejects_empty() {
        let (store, _guard) = test_store().await;
        let key = generate_key();
        let result = store.create_key("test", &key, &[], "admin", None).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("scopes must not be empty"));
    }

    #[tokio::test]
    async fn test_create_and_validate_key_roundtrip() {
        let (store, _guard) = test_store().await;
        let key = generate_key();
        let scopes = vec![Scope::Inference, Scope::ManagementRead];

        let id = store
            .create_key("test-key", &key, &scopes, "admin", None)
            .await
            .unwrap();
        assert!(id > 0);

        let result = store.validate_key(&key).await.unwrap();
        assert!(result.is_some());
        let (validated_id, validated_scopes) = result.unwrap();
        assert_eq!(validated_id, id);
        assert_eq!(validated_scopes, scopes);
    }

    #[tokio::test]
    async fn test_validate_nonexistent_key_returns_none() {
        let (store, _guard) = test_store().await;
        let result = store.validate_key("tama_nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_validate_revoked_key_returns_none() {
        let (store, _guard) = test_store().await;
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        let id = store
            .create_key("test", &key, &scopes, "admin", None)
            .await
            .unwrap();
        store.revoke_key(id).await.unwrap();

        let result = store.validate_key(&key).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_validate_expired_key_returns_none() {
        let (store, _guard) = test_store().await;
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        // Use a past date for expires_at
        let past = "2020-01-01T00:00:00Z";
        let _id = store
            .create_key("test", &key, &scopes, "admin", Some(past))
            .await
            .unwrap();

        let result = store.validate_key(&key).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_validate_future_expiry_succeeds() {
        let (store, _guard) = test_store().await;
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        // Use a future date
        let future = "2099-12-31T23:59:59Z";
        let id = store
            .create_key("test", &key, &scopes, "admin", Some(future))
            .await
            .unwrap();

        let result = store.validate_key(&key).await.unwrap();
        assert!(result.is_some());
        let (validated_id, _) = result.unwrap();
        assert_eq!(validated_id, id);
    }

    #[tokio::test]
    async fn test_validate_key_wrong_key_returns_none() {
        let (store, _guard) = test_store().await;
        let correct_key = generate_key();
        let wrong_key = generate_key();

        store
            .create_key("test", &correct_key, &[Scope::Inference], "admin", None)
            .await
            .unwrap();

        let result = store.validate_key(&wrong_key).await.unwrap();
        assert!(result.is_none());
    }

    /// A key created via the store must be stored with the pinned v2
    /// (SHA-256 hex) hash, so keys issued under SQLite keep validating.
    #[tokio::test]
    async fn test_created_key_stores_fixed_v2_hash() {
        let (store, _guard) = test_store().await;
        store
            .create_key("hash-key", FIXED_KEY, &[Scope::Inference], "admin", None)
            .await
            .unwrap();

        let stored_hash: String =
            sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE name = 'hash-key'")
                .fetch_one(&*store.pool)
                .await
                .unwrap();
        assert_eq!(stored_hash, FIXED_KEY_HASH);

        // And the fixed key validates through the store.
        let result = store.validate_key(FIXED_KEY).await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_api_keys_enabled_flag_toggled() {
        let guard = crate::testing::postgres::with_schema().await;
        let store = ApiKeyStore::new(Arc::new(guard.pool.clone()));
        // Seed the app_proxy row (migrations only create the table, not the row)
        crate::db::queries::seed_defaults(&guard.pool)
            .await
            .unwrap();

        // Initially should be FALSE
        let enabled: bool =
            sqlx::query_scalar("SELECT api_keys_enabled FROM app_proxy WHERE id = 1")
                .fetch_one(&guard.pool)
                .await
                .unwrap();
        assert!(!enabled);

        // Create a key — should toggle to TRUE
        let key = generate_key();
        store
            .create_key("test", &key, &[Scope::Inference], "admin", None)
            .await
            .unwrap();
        let enabled: bool =
            sqlx::query_scalar("SELECT api_keys_enabled FROM app_proxy WHERE id = 1")
                .fetch_one(&guard.pool)
                .await
                .unwrap();
        assert!(enabled);

        // Revoke the only key — should toggle back to FALSE
        let id: i64 = sqlx::query_scalar("SELECT id FROM api_keys")
            .fetch_one(&guard.pool)
            .await
            .unwrap();
        store.revoke_key(id).await.unwrap();
        let enabled: bool =
            sqlx::query_scalar("SELECT api_keys_enabled FROM app_proxy WHERE id = 1")
                .fetch_one(&guard.pool)
                .await
                .unwrap();
        assert!(!enabled);
    }

    #[tokio::test]
    async fn test_list_keys() {
        let (store, _guard) = test_store().await;
        let key1 = generate_key();
        let key2 = generate_key();

        store
            .create_key("first", &key1, &[Scope::Inference], "admin", None)
            .await
            .unwrap();
        store
            .create_key("second", &key2, &[Scope::ManagementWrite], "admin", None)
            .await
            .unwrap();

        let keys = store.list_keys().await.unwrap();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].name, "second"); // DESC order
        assert_eq!(keys[1].name, "first");
    }

    #[tokio::test]
    async fn test_get_key() {
        let (store, _guard) = test_store().await;
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        let id = store
            .create_key("test", &key, &scopes, "admin", None)
            .await
            .unwrap();

        let found = store.get_key(id).await.unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.name, "test");
        assert_eq!(found.scopes, scopes);
    }

    #[tokio::test]
    async fn test_get_key_not_found() {
        let (store, _guard) = test_store().await;
        let result = store.get_key(9999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_update_key_scopes() {
        let (store, guard) = test_store().await;
        let key = generate_key();

        store
            .create_key("test", &key, &[Scope::Inference], "admin", None)
            .await
            .unwrap();

        let id: i64 = sqlx::query_scalar("SELECT id FROM api_keys")
            .fetch_one(&guard.pool)
            .await
            .unwrap();

        let updated = store
            .update_key_scopes(id, &[Scope::ManagementRead, Scope::ManagementWrite])
            .await
            .unwrap();
        assert_eq!(
            updated.scopes,
            vec![Scope::ManagementRead, Scope::ManagementWrite]
        );
    }

    #[tokio::test]
    async fn test_last_used_at_updated_on_validate() {
        let (store, guard) = test_store().await;
        let key = generate_key();
        let scopes = vec![Scope::Inference];

        let id = store
            .create_key("test", &key, &scopes, "admin", None)
            .await
            .unwrap();

        // Initially last_used_at should be NULL
        let last_used: Option<String> =
            sqlx::query_scalar("SELECT last_used_at::text FROM api_keys WHERE id = $1")
                .bind(id)
                .fetch_one(&guard.pool)
                .await
                .unwrap();
        assert!(last_used.is_none());

        // Validate the key
        store.validate_key(&key).await.unwrap();

        // Now last_used_at should be set
        let last_used: Option<String> =
            sqlx::query_scalar("SELECT last_used_at::text FROM api_keys WHERE id = $1")
                .bind(id)
                .fetch_one(&guard.pool)
                .await
                .unwrap();
        assert!(last_used.is_some());
    }

    #[tokio::test]
    async fn test_get_key_name() {
        let (store, guard) = test_store().await;
        let key = generate_key();
        store
            .create_key("my-service-key", &key, &[Scope::Inference], "admin", None)
            .await
            .unwrap();

        let id: i64 = sqlx::query_scalar("SELECT id FROM api_keys")
            .fetch_one(&guard.pool)
            .await
            .unwrap();

        let name = store.get_key_name(id).await.unwrap().unwrap();
        assert_eq!(name, "my-service-key");
    }

    #[tokio::test]
    async fn test_get_key_name_not_found() {
        let (store, _guard) = test_store().await;
        let result = store.get_key_name(9999).await.unwrap();
        assert!(result.is_none());
    }

    /// Smoke test: ApiKeyStore exposes the full CRUD surface on the pool.
    #[tokio::test]
    async fn test_api_key_store_full_round_trip() {
        let (store, _guard) = test_store().await;

        let raw_key = generate_key();
        let scopes = vec![Scope::Inference, Scope::ManagementRead];
        let id = store
            .create_key("store-test", &raw_key, &scopes, "admin", None)
            .await
            .unwrap();
        assert!(id > 0);

        // validate
        let validated = store.validate_key(&raw_key).await.unwrap();
        assert_eq!(validated.unwrap().0, id);

        // get_key
        let record = store.get_key(id).await.unwrap().unwrap();
        assert_eq!(record.name, "store-test");

        // list_keys
        let keys = store.list_keys().await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "store-test");

        // get_key_name
        let name = store.get_key_name(id).await.unwrap().unwrap();
        assert_eq!(name, "store-test");

        // update_key_scopes
        let updated = store
            .update_key_scopes(id, &[Scope::ManagementRead, Scope::ManagementWrite])
            .await
            .unwrap();
        assert_eq!(
            updated.scopes,
            vec![Scope::ManagementRead, Scope::ManagementWrite]
        );

        // revoke_key
        let still_active = store.revoke_key(id).await.unwrap();
        assert!(!still_active, "last active key revoked should return false");

        // validate after revoke returns None
        assert!(store.validate_key(&raw_key).await.unwrap().is_none());
    }
}
