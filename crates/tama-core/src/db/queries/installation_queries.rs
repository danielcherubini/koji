//! Backend installation database query functions.
//!
//! All functions take a `&PgPool` and are async (plan-190 Task 8).

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};

/// A stored installation record for a backend binary.
#[derive(Debug, Clone)]
pub struct InstallationRecord {
    /// Set to 0 when constructing a record for INSERT (DB assigns the real id).
    pub id: i64,
    pub name: String,
    pub backend_type: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    pub gpu_variant: String,
    pub source: Option<String>,
    pub is_active: bool,
    /// Serialized `DockerConfig` JSON for Docker-based backends.
    pub docker_config: Option<String>,
    /// Stable identity assigned once per logical backend name, preserved across
    /// renames and version upgrades. Empty string on rows not yet assigned.
    pub logical_id: String,
}

/// Decode a `provider_installations` row into an [`InstallationRecord`].
fn decode_installation(row: &sqlx::postgres::PgRow) -> InstallationRecord {
    InstallationRecord {
        id: row.get("id"),
        name: row.get("name"),
        backend_type: row.get("backend_type"),
        version: row.get("version"),
        path: row.get("path"),
        installed_at: row.get("installed_at"),
        gpu_variant: row.get("gpu_variant"),
        source: row.get("source"),
        is_active: row.get("is_active"),
        docker_config: row.get("docker_config"),
        // Defensive: rows are always written with a non-empty logical_id,
        // but treat a stray NULL as unassigned.
        logical_id: row
            .get::<Option<String>, _>("logical_id")
            .unwrap_or_default(),
    }
}

/// Insert or replace a backend installation record, marking it as active.
///
/// In a single transaction:
/// 1. Inserts (or updates) the row with `is_active = TRUE` via
///    `ON CONFLICT (name, gpu_variant, version)`.
/// 2. Sets `is_active = FALSE` for all other rows with the same name AND
///    gpu_variant.
///
/// When a row with the same `(name, gpu_variant, version)` already exists it is
/// updated in place (the row keeps its `id`). All other rows with the same name
/// and gpu_variant are deactivated (different variants are unaffected).
pub async fn insert_installation(pool: &PgPool, record: &InstallationRecord) -> Result<()> {
    // Resolve the stable logical_id: reuse an existing one for this name if any
    // row already has one, otherwise generate a fresh UUID. This preserves the
    // same logical backend identity across renames and version installs.
    let existing_logical: Option<String> = sqlx::query_scalar(
        "SELECT logical_id FROM provider_installations WHERE name = $1
         AND logical_id IS NOT NULL AND logical_id != '' LIMIT 1",
    )
    .bind(&record.name)
    .fetch_optional(pool)
    .await?;
    let logical_id = if !record.logical_id.is_empty() {
        record.logical_id.clone()
    } else if let Some(existing) = existing_logical {
        existing
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO provider_installations
             (name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config, logical_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE, $8, $9)
         ON CONFLICT (name, gpu_variant, version) DO UPDATE SET
             backend_type  = EXCLUDED.backend_type,
             path          = EXCLUDED.path,
             installed_at  = EXCLUDED.installed_at,
             source        = EXCLUDED.source,
             is_active     = TRUE,
             docker_config = EXCLUDED.docker_config,
             logical_id    = EXCLUDED.logical_id",
    )
    .bind(&record.name)
    .bind(&record.backend_type)
    .bind(&record.version)
    .bind(&record.path)
    .bind(record.installed_at)
    .bind(&record.gpu_variant)
    .bind(record.source.as_deref())
    .bind(record.docker_config.as_deref())
    .bind(&logical_id)
    .execute(&mut *tx)
    .await?;
    // Propagate the same logical_id to any other (older) rows of this name so a
    // rename or version rollback keeps them grouped under the same identity.
    sqlx::query(
        "UPDATE provider_installations SET logical_id = $1
         WHERE name = $2 AND (logical_id IS NULL OR logical_id = '')",
    )
    .bind(&logical_id)
    .bind(&record.name)
    .execute(&mut *tx)
    .await?;
    // Stamp matching provider_configs rows with the same stable key so config
    // rows (e.g. created via default-args POST or TOML migration with an empty
    // logical_id) get their logical_id as soon as an installation exists.
    sqlx::query(
        "UPDATE provider_configs SET logical_id = $1
         WHERE name = $2 AND gpu_variant = $3 AND (logical_id IS NULL OR logical_id = '')",
    )
    .bind(&logical_id)
    .bind(&record.name)
    .bind(&record.gpu_variant)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE provider_installations SET is_active = FALSE
         WHERE name = $1 AND gpu_variant = $2 AND version != $3",
    )
    .bind(&record.name)
    .bind(&record.gpu_variant)
    .bind(&record.version)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Get the active backend installation for a given name and gpu_variant.
pub async fn get_active_installation(
    pool: &PgPool,
    name: &str,
    gpu_variant: &str,
) -> Result<Option<InstallationRecord>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(concat!(
        "SELECT ",
        "id, name, backend_type, version, path, \
         installed_at, gpu_variant, source, is_active, docker_config, logical_id",
        " FROM provider_installations WHERE name = $1 AND gpu_variant = $2 AND is_active = TRUE"
    )))
    .bind(name)
    .bind(gpu_variant)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| decode_installation(&r)))
}

/// Return all active backend installations (one per backend name/variant).
pub async fn list_active_installations(pool: &PgPool) -> Result<Vec<InstallationRecord>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(concat!(
        "SELECT ",
        "id, name, backend_type, version, path, \
         installed_at, gpu_variant, source, is_active, docker_config, logical_id",
        " FROM provider_installations WHERE is_active = TRUE"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(decode_installation).collect())
}

/// Return all versions of a backend, ordered by `installed_at DESC` (newest first).
///
/// If `gpu_variant` is `Some`, only returns rows matching that variant.
/// If `None`, returns all variants.
pub async fn list_installation_versions(
    pool: &PgPool,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<Vec<InstallationRecord>> {
    let (sql, variant) = match gpu_variant {
        Some(_) => (
            sqlx::AssertSqlSafe(concat!(
                "SELECT ",
                "id, name, backend_type, version, path, \
                 installed_at, gpu_variant, source, is_active, docker_config, logical_id",
                " FROM provider_installations \
                 WHERE name = $1 AND gpu_variant = $2 ORDER BY installed_at DESC"
            )),
            true,
        ),
        None => (
            sqlx::AssertSqlSafe(concat!(
                "SELECT ",
                "id, name, backend_type, version, path, \
                 installed_at, gpu_variant, source, is_active, docker_config, logical_id",
                " FROM provider_installations \
                 WHERE name = $1 ORDER BY installed_at DESC"
            )),
            false,
        ),
    };
    let mut q = sqlx::query(sql).bind(name);
    if variant {
        q = q.bind(gpu_variant.unwrap_or(""));
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.iter().map(decode_installation).collect())
}

/// Get a specific backend installation by (name, gpu_variant, version).
/// Returns Ok(None) if no row matches.
pub async fn get_installation_by_version(
    pool: &PgPool,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<Option<InstallationRecord>> {
    let row = sqlx::query(sqlx::AssertSqlSafe(concat!(
        "SELECT ",
        "id, name, backend_type, version, path, \
         installed_at, gpu_variant, source, is_active, docker_config, logical_id",
        " FROM provider_installations \
         WHERE name = $1 AND gpu_variant = $2 AND version = $3"
    )))
    .bind(name)
    .bind(gpu_variant)
    .bind(version)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| decode_installation(&r)))
}

/// Delete a specific `(name, gpu_variant, version)` backend installation row.
pub async fn delete_installation(
    pool: &PgPool,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM provider_installations WHERE name = $1 AND gpu_variant = $2 AND version = $3",
    )
    .bind(name)
    .bind(gpu_variant)
    .bind(version)
    .execute(pool)
    .await?;
    Ok(())
}

/// Deactivate all versions for a backend name+variant, then activate the specified version.
///
/// This is an atomic operation executed in a transaction:
/// 1. Check if the target version exists
/// 2. If not, return Ok(false) without any changes
/// 3. SET is_active = FALSE for all rows with the given name AND gpu_variant
/// 4. SET is_active = TRUE for the row matching (name, gpu_variant, version)
///
/// Returns Ok(true) if the version was found and activated, Ok(false) if no matching row exists.
pub async fn activate_installation_version(
    pool: &PgPool,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<bool> {
    let mut tx = pool.begin().await?;

    // Check if the target version exists before making any changes
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM provider_installations WHERE name = $1 AND gpu_variant = $2 AND version = $3",
    )
    .bind(name)
    .bind(gpu_variant)
    .bind(version)
    .fetch_one(&mut *tx)
    .await?;

    if exists == 0 {
        tx.rollback().await?;
        return Ok(false);
    }

    // Deactivate all versions for this backend+variant
    sqlx::query(
        "UPDATE provider_installations SET is_active = FALSE WHERE name = $1 AND gpu_variant = $2",
    )
    .bind(name)
    .bind(gpu_variant)
    .execute(&mut *tx)
    .await?;

    // Activate the requested version
    let res = sqlx::query(
        "UPDATE provider_installations SET is_active = TRUE WHERE name = $1 AND gpu_variant = $2 AND version = $3",
    )
    .bind(name)
    .bind(gpu_variant)
    .bind(version)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

/// Delete all installation rows for a backend name (used by `backend remove`).
///
/// If `gpu_variant` is `Some`, only deletes rows matching that variant.
/// If `None`, deletes all variants.
pub async fn delete_all_installation_versions(
    pool: &PgPool,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<()> {
    if let Some(variant) = gpu_variant {
        sqlx::query("DELETE FROM provider_installations WHERE name = $1 AND gpu_variant = $2")
            .bind(name)
            .bind(variant)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("DELETE FROM provider_installations WHERE name = $1")
            .bind(name)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Update the `source` column on the active backend installation row.
///
/// Fails with an error if no active row matches the given name and gpu_variant.
pub async fn update_installation_source(
    pool: &PgPool,
    name: &str,
    gpu_variant: &str,
    source_json: &str,
) -> Result<()> {
    let res = sqlx::query(
        "UPDATE provider_installations SET source = $1
         WHERE name = $2 AND gpu_variant = $3 AND is_active = TRUE",
    )
    .bind(source_json)
    .bind(name)
    .bind(gpu_variant)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        anyhow::bail!(
            "No active backend '{}' variant '{}' found",
            name,
            gpu_variant
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Backend config queries
// ---------------------------------------------------------------------------

/// A stored config record for a backend.
#[derive(Debug, Clone)]
pub struct InstallationConfigRecord {
    pub id: i64,
    /// Stable logical backend identity (rename/upgrade-safe join key).
    /// `None` on legacy rows not yet backfilled; keyed with `gpu_variant` to
    /// form the stable `(logical_id, gpu_variant)` uniqueness scope.
    pub logical_id: Option<String>,
    pub name: String,
    pub gpu_variant: String,
    /// Parsed from JSON array stored in `default_args` column.
    pub default_args: Vec<String>,
    /// Parsed from JSON array stored in `default_env` column.
    pub default_env: Vec<String>,
    pub health_check_url: Option<String>,
}

/// Raw row struct for provider_configs before JSON parsing.
#[derive(Debug)]
struct RawBackendConfigRow {
    id: i64,
    logical_id: Option<String>,
    name: String,
    gpu_variant: String,
    default_args_raw: Option<String>,
    default_env_raw: Option<String>,
    health_check_url: Option<String>,
}

fn map_raw_installation_config(row: &sqlx::postgres::PgRow) -> RawBackendConfigRow {
    RawBackendConfigRow {
        id: row.get("id"),
        logical_id: row.get("logical_id"),
        name: row.get("name"),
        gpu_variant: row.get("gpu_variant"),
        default_args_raw: row.get("default_args"),
        default_env_raw: row.get("default_env"),
        health_check_url: row.get("health_check_url"),
    }
}

fn raw_to_record(raw: RawBackendConfigRow) -> Result<InstallationConfigRecord> {
    let default_args: Vec<String> = match raw.default_args_raw {
        Some(ref s) if !s.is_empty() => {
            serde_json::from_str(s).context("Failed to parse default_args JSON")?
        }
        _ => Vec::new(),
    };
    let default_env: Vec<String> = match raw.default_env_raw {
        Some(ref s) if !s.is_empty() => {
            serde_json::from_str(s).context("Failed to parse default_env JSON")?
        }
        _ => Vec::new(),
    };

    Ok(InstallationConfigRecord {
        id: raw.id,
        logical_id: raw.logical_id,
        name: raw.name,
        gpu_variant: raw.gpu_variant,
        default_args,
        default_env,
        health_check_url: raw.health_check_url,
    })
}

/// Get the backend config for a backend, matching on the stable `logical_id`
/// first and falling back to the (renameable) `name` for legacy rows.
pub async fn get_installation_config(
    pool: &PgPool,
    key: &str,
    gpu_variant: &str,
) -> Result<Option<InstallationConfigRecord>> {
    let row = sqlx::query(
        "SELECT id, logical_id, name, gpu_variant, default_args, default_env, health_check_url
         FROM provider_configs
         WHERE (logical_id = $1 OR name = $1) AND gpu_variant = $2
         ORDER BY (logical_id = $1) DESC, id ASC LIMIT 1",
    )
    .bind(key)
    .bind(gpu_variant)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(r) => Ok(Some(raw_to_record(map_raw_installation_config(&r))?)),
        None => Ok(None),
    }
}

/// Insert or replace a backend config record keyed by the stable `logical_id`.
/// Returns the row's id.
pub async fn upsert_installation_config(
    pool: &PgPool,
    logical_id: &str,
    name: &str,
    gpu_variant: &str,
    default_args: &[String],
    default_env: &[String],
    health_check_url: Option<&str>,
) -> Result<i64> {
    let default_args_json = if default_args.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(default_args)
                .context("Failed to serialize default_args to JSON")?,
        )
    };
    let default_env_json = if default_env.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(default_env)
                .context("Failed to serialize default_env to JSON")?,
        )
    };

    // First try to update an existing row identified by logical_id (or legacy name).
    let key_logical: Option<&str> = if logical_id.is_empty() {
        None
    } else {
        Some(logical_id)
    };
    let updated = if let Some(lid) = key_logical {
        sqlx::query(
            "UPDATE provider_configs SET
                name = $1,
                logical_id = $6,
                default_args = $2,
                default_env = $3,
                health_check_url = $4
             WHERE gpu_variant = $5 AND (logical_id = $6 OR name = $7)",
        )
        .bind(name)
        .bind(default_args_json.as_deref())
        .bind(default_env_json.as_deref())
        .bind(health_check_url)
        .bind(gpu_variant)
        .bind(lid)
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "UPDATE provider_configs SET
                name = $1,
                default_args = $2,
                default_env = $3,
                health_check_url = $4
             WHERE gpu_variant = $5 AND name = $6",
        )
        .bind(name)
        .bind(default_args_json.as_deref())
        .bind(default_env_json.as_deref())
        .bind(health_check_url)
        .bind(gpu_variant)
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected()
    };

    if updated == 0 {
        sqlx::query(
            "INSERT INTO provider_configs (logical_id, name, gpu_variant, default_args, default_env, health_check_url)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(key_logical)
        .bind(name)
        .bind(gpu_variant)
        .bind(default_args_json.as_deref())
        .bind(default_env_json.as_deref())
        .bind(health_check_url)
        .execute(pool)
        .await?;
    }

    // Fetch the id of the (possibly updated) row
    let id: i64 = if let Some(lid) = key_logical {
        sqlx::query_scalar(
            "SELECT id FROM provider_configs WHERE gpu_variant = $1 AND (logical_id = $2 OR name = $3)",
        )
        .bind(gpu_variant)
        .bind(lid)
        .bind(name)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar("SELECT id FROM provider_configs WHERE gpu_variant = $1 AND name = $2")
            .bind(gpu_variant)
            .bind(name)
            .fetch_one(pool)
            .await?
    };

    Ok(id)
}

/// Return all backend config records.
pub async fn list_installation_configs(pool: &PgPool) -> Result<Vec<InstallationConfigRecord>> {
    let rows = sqlx::query(
        "SELECT id, logical_id, name, gpu_variant, default_args, default_env, health_check_url
         FROM provider_configs",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| raw_to_record(map_raw_installation_config(&row)))
        .collect()
}

/// Resolve the stable `logical_id` for a backend `name`.
/// Returns `Ok(Some(id))` if any installation row (any version/variant) carries one.
pub async fn get_installation_logical_id(pool: &PgPool, name: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT logical_id FROM provider_installations WHERE name = $1
         AND logical_id IS NOT NULL AND logical_id != '' LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?)
}

/// Atomically rename a backend across every table that carries its display name.
///
/// Returns `Ok(true)` if the rename happened, `Ok(false)` if `old_name` had no
/// `provider_installations` row (backend not found).
///
/// The stable `logical_id` join keys are NOT changed, so `provider_configs`
/// (whose uniqueness now scopes on `(logical_id, gpu_variant)`) and any
/// `backend_id` references remain intact across the rename. Fails if the new
/// name would collide with an existing backend (installation or config row)
/// whose `logical_id` differs or is still unassigned.
pub async fn rename_installation(pool: &PgPool, old_name: &str, new_name: &str) -> Result<bool> {
    // Not-found: old_name must have at least one installation row. This check
    // runs before the same-name no-op so a nonexistent backend renamed to
    // its own name reports Ok(false) rather than a false success.
    let old_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM provider_installations WHERE name = $1")
            .bind(old_name)
            .fetch_one(pool)
            .await?;
    if old_exists == 0 {
        return Ok(false);
    }

    if old_name == new_name {
        return Ok(true);
    }

    // Prevent silently merging two distinct logical backends (including the
    // case where the new name already exists as a backend).
    let old_logical = get_installation_logical_id(pool, old_name).await?;
    let new_logical = get_installation_logical_id(pool, new_name).await?;
    let new_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM provider_installations WHERE name = $1")
            .bind(new_name)
            .fetch_one(pool)
            .await?;
    let merges_distinct = match (old_logical.as_deref(), new_logical.as_deref()) {
        (Some(old), Some(new)) => old != new,
        // If we cannot prove both sides are the same logical backend (old has no
        // logical id, or the new name already owns installations), refuse.
        (_, Some(_)) => true,
        (Some(_), None) => new_exists > 0,
        (None, None) => new_exists > 0,
    };
    if merges_distinct {
        anyhow::bail!(
            "A different backend named '{}' already exists; refusing to merge",
            new_name
        );
    }

    // Guard against renaming onto a name that already owns provider_configs rows
    // under a NULL or DIFFERENT logical_id (for any gpu_variant). Such a row
    // would otherwise collide on the `(logical_id, gpu_variant)` uniqueness.
    let config_conflicts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM provider_configs
         WHERE name = $1 AND (logical_id IS NULL OR logical_id = '' OR logical_id != $2)",
    )
    .bind(new_name)
    .bind(old_logical.as_deref().unwrap_or(""))
    .fetch_one(pool)
    .await?;
    if config_conflicts > 0 {
        anyhow::bail!("refusing to merge overlapping backend config/settings");
    }

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE provider_installations SET name = $1 WHERE name = $2")
        .bind(new_name)
        .bind(old_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE provider_configs SET name = $1 WHERE name = $2")
        .bind(new_name)
        .bind(old_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE model_configs SET backend = $1 WHERE backend = $2")
        .bind(new_name)
        .bind(old_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE active_models SET backend = $1 WHERE backend = $2")
        .bind(new_name)
        .bind(old_name)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::postgres::with_schema;

    #[tokio::test]
    async fn test_upsert_installation_config_insert() {
        let guard = with_schema().await;

        let args = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let env = vec!["RADV_PERFTEST=nogttspill".to_string()];
        let id = upsert_installation_config(
            &guard.pool,
            "",
            "llama_cpp",
            "cpu",
            &args,
            &env,
            Some("http://localhost:8080/health"),
        )
        .await
        .unwrap();
        assert_eq!(id, 1);

        let record = get_installation_config(&guard.pool, "llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.id, 1);
        assert_eq!(record.name, "llama_cpp");
        assert_eq!(record.gpu_variant, "cpu");
        assert_eq!(record.default_args, args);
        assert_eq!(record.default_env, env);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:8080/health".to_string())
        );
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_upsert_installation_config_update() {
        let guard = with_schema().await;

        // Insert initial row
        let id1 = upsert_installation_config(
            &guard.pool,
            "",
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string()],
            &[],
            Some("http://localhost:8080/health"),
        )
        .await
        .unwrap();

        // Upsert with different values
        let id2 = upsert_installation_config(
            &guard.pool,
            "",
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string(), "-b 2048".to_string()],
            &["FOO=bar".to_string()],
            Some("http://localhost:9090/health"),
        )
        .await
        .unwrap();

        // ID should be the same (updated, not re-inserted)
        assert_eq!(id1, id2);

        let record = get_installation_config(&guard.pool, "llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.default_args, vec!["-fa 1", "-b 2048"]);
        assert_eq!(record.default_env, vec!["FOO=bar"]);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:9090/health".to_string())
        );
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_installation_config_not_found() {
        let guard = with_schema().await;

        let result = get_installation_config(&guard.pool, "nonexistent", "cpu")
            .await
            .unwrap();
        assert!(result.is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_list_installation_configs() {
        let guard = with_schema().await;

        upsert_installation_config(
            &guard.pool,
            "",
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string()],
            &[],
            Some("http://localhost:8080/health"),
        )
        .await
        .unwrap();
        upsert_installation_config(&guard.pool, "", "llama_cpp", "vulkan", &[], &[], None)
            .await
            .unwrap();
        upsert_installation_config(&guard.pool, "", "ik_llama", "cpu", &[], &[], None)
            .await
            .unwrap();

        let configs = list_installation_configs(&guard.pool).await.unwrap();
        assert_eq!(configs.len(), 3);

        // Verify each config
        let cpu = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "cpu")
            .unwrap();
        assert_eq!(cpu.default_args, vec!["-fa 1"]);

        let vulkan = configs
            .iter()
            .find(|c| c.name == "llama_cpp" && c.gpu_variant == "vulkan")
            .unwrap();
        assert!(vulkan.default_args.is_empty());
        assert!(vulkan.health_check_url.is_none());
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_upsert_installation_config_empty_args() {
        let guard = with_schema().await;

        let id =
            upsert_installation_config(&guard.pool, "", "empty_backend", "cpu", &[], &[], None)
                .await
                .unwrap();
        assert_eq!(id, 1);

        let record = get_installation_config(&guard.pool, "empty_backend", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert!(record.default_args.is_empty());
        assert!(record.default_env.is_empty());
        assert!(record.health_check_url.is_none());
        guard.finish().await;
    }

    fn active_record(name: &str, version: &str, gpu_variant: &str) -> InstallationRecord {
        InstallationRecord {
            id: 0,
            name: name.to_string(),
            backend_type: "llama_cpp".to_string(),
            version: version.to_string(),
            path: "/tmp/test/llama-server".to_string(),
            installed_at: 0,
            gpu_variant: gpu_variant.to_string(),
            source: None,
            is_active: true,
            docker_config: None,
            logical_id: String::new(),
        }
    }

    #[tokio::test]
    async fn test_update_installation_source_success() {
        let guard = with_schema().await;

        // Insert a backend with no source
        insert_installation(&guard.pool, &active_record("llama_cpp", "b8407", "cpu"))
            .await
            .unwrap();

        // Update the source column
        let new_source = r#"{"source":"SourceCode","content":{"version":"b8407","git_url":"https://github.com/ggml-org/llama.cpp.git"}}"#;
        update_installation_source(&guard.pool, "llama_cpp", "cpu", new_source)
            .await
            .unwrap();

        // Verify the source was updated
        let record = get_active_installation(&guard.pool, "llama_cpp", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.source, Some(new_source.to_string()));
        guard.finish().await;
    }

    #[tokio::test]
    async fn test_update_installation_source_not_found() {
        let guard = with_schema().await;

        let result = update_installation_source(
            &guard.pool,
            "nonexistent",
            "cpu",
            r#"{"source":"Prebuilt","content":{"version":"v1"}}"#,
        )
        .await;
        assert!(result.is_err());
        guard.finish().await;
    }

    /// Renaming a backend preserves its default args/env (via logical_id) while
    /// syncing the display name everywhere.
    #[tokio::test]
    async fn test_rename_installation_preserves_config_and_syncs_names() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        // Install a backend; insert assigns a stable logical_id.
        insert_installation(
            pool,
            &InstallationRecord {
                name: "vllm".to_string(),
                backend_type: "docker".to_string(),
                version: "0.5.8".to_string(),
                path: "n/a".to_string(),
                gpu_variant: "rocm".to_string(),
                ..active_record("vllm", "0.5.8", "rocm")
            },
        )
        .await
        .unwrap();
        let lid = get_installation_logical_id(pool, "vllm")
            .await
            .unwrap()
            .unwrap();

        // Add config keyed by the logical id.
        upsert_installation_config(
            pool,
            &lid,
            "vllm",
            "rocm",
            &["-fa 1".into()],
            &["A=1".into()],
            None,
        )
        .await
        .unwrap();

        // A model and an active-model row reference the backend by name.
        sqlx::query("INSERT INTO model_configs (repo_id, backend) VALUES ($1, $2)")
            .bind("m/m")
            .bind("vllm")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO active_models (server_name, model_name, backend, pid, port, backend_url)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind("s1")
        .bind("m")
        .bind("vllm")
        .bind(1i64)
        .bind(8000i64)
        .bind("http://x")
        .execute(pool)
        .await
        .unwrap();

        // Pre-rename, config is found by name.
        assert!(get_installation_config(pool, "vllm", "rocm")
            .await
            .unwrap()
            .is_some());

        assert!(rename_installation(pool, "vllm", "radiance").await.unwrap());

        // logical_id unchanged.
        assert_eq!(
            get_installation_logical_id(pool, "radiance")
                .await
                .unwrap()
                .unwrap(),
            lid
        );

        // Default args/env survive the rename, found by the new name.
        let cfg = get_installation_config(pool, "radiance", "rocm")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cfg.default_args, vec!["-fa 1"]);
        assert_eq!(cfg.default_env, vec!["A=1"]);
        assert_eq!(cfg.name, "radiance");

        // Models / runtime rows now point at the new name.
        let backend: String =
            sqlx::query_scalar("SELECT backend FROM model_configs WHERE repo_id = $1")
                .bind("m/m")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(backend, "radiance");
        let ab: String =
            sqlx::query_scalar("SELECT backend FROM active_models WHERE server_name = $1")
                .bind("s1")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(ab, "radiance");
        guard.finish().await;
    }

    /// Renaming onto an existing different backend is rejected.
    #[tokio::test]
    async fn test_rename_installation_rejects_merging_distinct_backends() {
        let guard = with_schema().await;

        for (i, name) in ["vllm", "other"].iter().enumerate() {
            insert_installation(
                &guard.pool,
                &InstallationRecord {
                    name: name.to_string(),
                    backend_type: "docker".to_string(),
                    version: format!("v{i}"),
                    path: "n/a".to_string(),
                    gpu_variant: "rocm".to_string(),
                    ..active_record(name, "v0", "rocm")
                },
            )
            .await
            .unwrap();
        }

        assert!(rename_installation(&guard.pool, "vllm", "other")
            .await
            .is_err());
        guard.finish().await;
    }

    /// Renaming a backend that has no installation row reports Ok(false).
    #[tokio::test]
    async fn test_rename_installation_not_found_returns_false() {
        let guard = with_schema().await;

        assert!(!rename_installation(&guard.pool, "ghost", "something")
            .await
            .unwrap());
        guard.finish().await;
    }

    /// Renaming a nonexistent backend to its own name still reports Ok(false)
    /// (the existence check must run before the same-name no-op short-circuit).
    #[tokio::test]
    async fn test_rename_installation_not_found_same_name_returns_false() {
        let guard = with_schema().await;

        assert!(!rename_installation(&guard.pool, "ghost", "ghost")
            .await
            .unwrap());
        guard.finish().await;
    }

    /// When two config rows share a (name, gpu_variant) but carry different
    /// logical_ids, the name-based fallback picks the lowest id deterministically.
    #[tokio::test]
    async fn test_get_installation_config_deterministic_tiebreaker() {
        let guard = with_schema().await;

        sqlx::query(
            "INSERT INTO provider_configs (id, logical_id, name, gpu_variant)
             VALUES ($1, 'l1', 'dup', 'cpu')",
        )
        .bind(100i64)
        .execute(&guard.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_configs (id, logical_id, name, gpu_variant)
             VALUES ($1, 'l2', 'dup', 'cpu')",
        )
        .bind(101i64)
        .execute(&guard.pool)
        .await
        .unwrap();

        // Neither row matches by logical_id, so both match by name; the
        // tiebreaker must deterministically return the lowest id.
        let record = get_installation_config(&guard.pool, "dup", "cpu")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.id, 100);
        guard.finish().await;
    }

    /// Renaming onto a name that already owns a legacy (NULL logical_id)
    /// provider_configs row is refused, mirroring the BLOCKING UNIQUE-violation
    /// scenario: a config row created via default-args POST / TOML migration for
    /// name "other" would otherwise collide once backfill stamps it.
    #[tokio::test]
    async fn test_rename_installation_rejects_provider_configs_conflict() {
        let guard = with_schema().await;

        // Legacy config row for "other"/cpu with an empty logical_id.
        upsert_installation_config(&guard.pool, "", "other", "cpu", &["--x".into()], &[], None)
            .await
            .unwrap();

        // Install "vllm"/cpu; it gets a brand-new logical_id.
        insert_installation(&guard.pool, &active_record("vllm", "v1", "cpu"))
            .await
            .unwrap();

        // Renaming vllm -> other would merge onto the NULL-config row: refused.
        assert!(rename_installation(&guard.pool, "vllm", "other")
            .await
            .is_err());
        guard.finish().await;
    }

    /// Renaming onto a name that already has an installation is refused even
    /// when the old backend has no logical id yet (the new_name-exists case).
    #[tokio::test]
    async fn test_rename_installation_rejects_existing_new_name() {
        let guard = with_schema().await;

        insert_installation(&guard.pool, &active_record("radiance", "v1", "rocm"))
            .await
            .unwrap();
        // Strip the logical id so old_name has an installation but no logical id.
        sqlx::query("UPDATE provider_installations SET logical_id = ''")
            .execute(&guard.pool)
            .await
            .unwrap();
        // A distinct backend already exists under the target name.
        insert_installation(&guard.pool, &active_record("vllm", "v2", "cuda"))
            .await
            .unwrap();

        assert!(rename_installation(&guard.pool, "radiance", "vllm")
            .await
            .is_err());
        guard.finish().await;
    }
}
