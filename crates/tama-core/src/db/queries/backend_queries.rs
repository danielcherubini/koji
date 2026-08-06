//! Backend installation database query functions.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// A stored installation record for a backend binary.
#[derive(Debug, Clone)]
pub struct BackendInstallationRecord {
    /// Set to 0 when constructing a record for INSERT (DB assigns the real id via AUTOINCREMENT).
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

/// Shared row-mapping closure for BackendInstallationRecord queries.
///
/// Extracted to a function so it can be reused across multiple query_map
/// calls without hitting Rust's "each closure has a unique type" issue.
fn map_backend_record(row: &rusqlite::Row) -> rusqlite::Result<BackendInstallationRecord> {
    Ok(BackendInstallationRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        backend_type: row.get(2)?,
        version: row.get(3)?,
        path: row.get(4)?,
        installed_at: row.get(5)?,
        gpu_variant: row.get(6)?,
        source: row.get(7)?,
        is_active: row.get::<_, i64>(8)? != 0,
        docker_config: row.get(9)?,
        logical_id: row.get(10)?,
    })
}

/// Insert or replace a backend installation record, marking it as active.
///
/// In a single transaction:
/// 1. Inserts (or replaces) the row with `is_active = 1`.
/// 2. Sets `is_active = 0` for all other rows with the same name AND gpu_variant.
///
/// When a row with the same `(name, gpu_variant, version)` already exists, SQLite's `REPLACE`
/// semantics delete the old row and re-insert (the row gets a new `id`). All other rows with
/// the same name and gpu_variant are deactivated (different variants are unaffected).
pub fn insert_backend_installation(
    conn: &Connection,
    record: &BackendInstallationRecord,
) -> Result<()> {
    // Resolve the stable logical_id: reuse an existing one for this name if any
    // row already has one, otherwise generate a fresh UUID. This preserves the
    // same logical backend identity across renames and version installs.
    let existing_logical: Option<String> = conn
        .query_row(
            "SELECT logical_id FROM backend_installations WHERE name = ?1
             AND logical_id IS NOT NULL AND logical_id != '' LIMIT 1",
            [&record.name],
            |r| r.get(0),
        )
        .ok();
    let logical_id = if !record.logical_id.is_empty() {
        record.logical_id.clone()
    } else if let Some(existing) = existing_logical {
        existing
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO backend_installations
             (name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config, logical_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9)",
        (
            &record.name,
            &record.backend_type,
            &record.version,
            &record.path,
            record.installed_at,
            &record.gpu_variant,
            record.source.as_deref(),
            record.docker_config.as_deref(),
            &logical_id,
        ),
    )?;
    // Propagate the same logical_id to any other (older) rows of this name so a
    // rename or version rollback keeps them grouped under the same identity.
    tx.execute(
        "UPDATE backend_installations SET logical_id = ?1 WHERE name = ?2 AND (logical_id IS NULL OR logical_id = '')",
        (&logical_id, &record.name),
    )?;
    // Stamp matching backend_configs rows with the same stable key so config
    // rows (e.g. created via default-args POST or TOML migration with an empty
    // logical_id) get their logical_id as soon as an installation exists.
    tx.execute(
        "UPDATE backend_configs SET logical_id = ?1 WHERE name = ?2 AND gpu_variant = ?3 AND (logical_id IS NULL OR logical_id = '')",
        (&logical_id, &record.name, &record.gpu_variant),
    )?;
    tx.execute(
        "UPDATE backend_installations SET is_active = 0 WHERE name = ?1 AND gpu_variant = ?2 AND version != ?3",
        (&record.name, &record.gpu_variant, &record.version),
    )?;
    tx.commit()?;
    Ok(())
}

/// Get the active backend installation for a given name and gpu_variant.
pub fn get_active_backend(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
) -> Result<Option<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config, logical_id
         FROM backend_installations
         WHERE name = ?1 AND gpu_variant = ?2 AND is_active = 1",
    )?;
    let mut rows = stmt.query_map((name, gpu_variant), map_backend_record)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Return all active backend installations (one per backend name/variant).
pub fn list_active_backends(conn: &Connection) -> Result<Vec<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config, logical_id
         FROM backend_installations
         WHERE is_active = 1",
    )?;
    let rows = stmt.query_map([], map_backend_record)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Return all versions of a backend, ordered by `installed_at DESC` (newest first).
///
/// If `gpu_variant` is `Some`, only returns rows matching that variant.
/// If `None`, returns all variants.
pub fn list_backend_versions(
    conn: &Connection,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<Vec<BackendInstallationRecord>> {
    let sql = if let Some(_variant) = gpu_variant {
        "SELECT id, name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config, logical_id
         FROM backend_installations
         WHERE name = ?1 AND gpu_variant = ?2
         ORDER BY installed_at DESC"
    } else {
        "SELECT id, name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config, logical_id
         FROM backend_installations
         WHERE name = ?1
         ORDER BY installed_at DESC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(variant) = gpu_variant {
        stmt.query_map((name, variant), map_backend_record)?
    } else {
        stmt.query_map([name], map_backend_record)?
    };
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Get a specific backend installation by (name, gpu_variant, version).
/// Returns Ok(None) if no row matches.
pub fn get_backend_by_version(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<Option<BackendInstallationRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config, logical_id
         FROM backend_installations
         WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
    )?;
    let mut rows = stmt.query_map((name, gpu_variant, version), map_backend_record)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Delete a specific `(name, gpu_variant, version)` backend installation row.
pub fn delete_backend_installation(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM backend_installations WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
        (name, gpu_variant, version),
    )?;
    Ok(())
}

/// Deactivate all versions for a backend name+variant, then activate the specified version.
///
/// This is an atomic operation executed in a transaction:
/// 1. Check if the target version exists
/// 2. If not, return Ok(false) without any changes
/// 3. SET is_active = 0 for all rows with the given name AND gpu_variant
/// 4. SET is_active = 1 for the row matching (name, gpu_variant, version)
///
/// Returns Ok(true) if the version was found and activated, Ok(false) if no matching row exists.
pub fn activate_backend_version(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;

    // Check if the target version exists before making any changes
    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM backend_installations WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
        (name, gpu_variant, version),
        |row| row.get(0),
    )?;

    if exists == 0 {
        tx.commit()?;
        return Ok(false);
    }

    // Deactivate all versions for this backend+variant
    tx.execute(
        "UPDATE backend_installations SET is_active = 0 WHERE name = ?1 AND gpu_variant = ?2",
        (name, gpu_variant),
    )?;

    // Activate the requested version
    let changes = tx.execute(
        "UPDATE backend_installations SET is_active = 1 WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3",
        (name, gpu_variant, version),
    )?;

    tx.commit()?;
    Ok(changes > 0)
}

/// Delete all installation rows for a backend name (used by `backend remove`).
///
/// If `gpu_variant` is `Some`, only deletes rows matching that variant.
/// If `None`, deletes all variants.
pub fn delete_all_backend_versions(
    conn: &Connection,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<()> {
    if let Some(variant) = gpu_variant {
        conn.execute(
            "DELETE FROM backend_installations WHERE name = ?1 AND gpu_variant = ?2",
            (name, variant),
        )?;
    } else {
        conn.execute("DELETE FROM backend_installations WHERE name = ?1", [name])?;
    }
    Ok(())
}

/// Update the `source` column on the active backend installation row.
///
/// Fails with an error if no active row matches the given name and gpu_variant.
pub fn update_backend_source(
    conn: &Connection,
    name: &str,
    gpu_variant: &str,
    source_json: &str,
) -> Result<()> {
    let rows = conn.execute(
        "UPDATE backend_installations SET source = ?1 WHERE name = ?2 AND gpu_variant = ?3 AND is_active = 1",
        (source_json, name, gpu_variant),
    )?;
    if rows == 0 {
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
pub struct BackendConfigRecord {
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

/// Raw row struct for backend_configs before JSON parsing.
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

fn map_raw_backend_config(row: &rusqlite::Row) -> rusqlite::Result<RawBackendConfigRow> {
    Ok(RawBackendConfigRow {
        id: row.get(0)?,
        logical_id: row.get(1)?,
        name: row.get(2)?,
        gpu_variant: row.get(3)?,
        default_args_raw: row.get(4)?,
        default_env_raw: row.get(5)?,
        health_check_url: row.get(6)?,
    })
}

fn raw_to_record(raw: RawBackendConfigRow) -> Result<BackendConfigRecord> {
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

    Ok(BackendConfigRecord {
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
pub fn get_backend_config(
    conn: &Connection,
    key: &str,
    gpu_variant: &str,
) -> Result<Option<BackendConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, logical_id, name, gpu_variant, default_args, default_env, health_check_url
         FROM backend_configs
         WHERE (logical_id = ?1 OR name = ?1) AND gpu_variant = ?2
         ORDER BY (logical_id = ?1) DESC, id ASC LIMIT 1",
    )?;
    let mut rows = stmt.query_map((key, gpu_variant), map_raw_backend_config)?;
    match rows.next() {
        Some(row) => {
            let raw = row?;
            Ok(Some(raw_to_record(raw)?))
        }
        None => Ok(None),
    }
}

/// Insert or replace a backend config record keyed by the stable `logical_id`.
/// Returns the row's id.
pub fn upsert_backend_config(
    conn: &Connection,
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
        conn.execute(
            "UPDATE backend_configs SET
                name = ?1,
                logical_id = ?6,
                default_args = ?2,
                default_env = ?3,
                health_check_url = ?4
             WHERE gpu_variant = ?5 AND (logical_id = ?6 OR name = ?7)",
            (
                name,
                default_args_json.as_deref(),
                default_env_json.as_deref(),
                health_check_url,
                gpu_variant,
                lid,
                name,
            ),
        )?
    } else {
        conn.execute(
            "UPDATE backend_configs SET
                name = ?1,
                default_args = ?2,
                default_env = ?3,
                health_check_url = ?4
             WHERE gpu_variant = ?5 AND name = ?6",
            (
                name,
                default_args_json.as_deref(),
                default_env_json.as_deref(),
                health_check_url,
                gpu_variant,
                name,
            ),
        )?
    };

    if updated == 0 {
        conn.execute(
            "INSERT INTO backend_configs (logical_id, name, gpu_variant, default_args, default_env, health_check_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                key_logical,
                name,
                gpu_variant,
                default_args_json.as_deref(),
                default_env_json.as_deref(),
                health_check_url,
            ),
        )?;
    }

    // Fetch the id of the (possibly updated) row
    let id: i64 = if let Some(lid) = key_logical {
        conn.query_row(
            "SELECT id FROM backend_configs WHERE gpu_variant = ?1 AND (logical_id = ?2 OR name = ?3)",
            (gpu_variant, lid, name),
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT id FROM backend_configs WHERE gpu_variant = ?1 AND name = ?2",
            (gpu_variant, name),
            |row| row.get(0),
        )?
    };

    Ok(id)
}

/// Return all backend config records.
pub fn list_backend_configs(conn: &Connection) -> Result<Vec<BackendConfigRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, logical_id, name, gpu_variant, default_args, default_env, health_check_url
         FROM backend_configs",
    )?;
    let raw_rows = stmt.query_map([], map_raw_backend_config)?;
    let records: Vec<BackendConfigRecord> = raw_rows
        .map(|row| raw_to_record(row?))
        .collect::<Result<Vec<_>>>()?;
    Ok(records)
}

/// Resolve the stable `logical_id` for a backend `name`.
/// Returns `Ok(Some(id))` if any installation row (any version/variant) carries one.
pub fn get_backend_logical_id(conn: &Connection, name: &str) -> Result<Option<String>> {
    let logical_id: Option<String> = conn
        .query_row(
            "SELECT logical_id FROM backend_installations WHERE name = ?1
             AND logical_id IS NOT NULL AND logical_id != '' LIMIT 1",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(logical_id)
}

/// Atomically rename a backend across every table that carries its display name.
///
/// Returns `Ok(true)` if the rename happened, `Ok(false)` if `old_name` had no
/// `backend_installations` row (backend not found).
///
/// The stable `logical_id` join keys are NOT changed, so `backend_configs`
/// (whose uniqueness now scopes on `(logical_id, gpu_variant)`) and any
/// `backend_id` references remain intact across the rename. Fails if the new
/// name would collide with an existing backend (installation or config row)
/// whose `logical_id` differs or is still unassigned.
pub fn rename_backend(conn: &Connection, old_name: &str, new_name: &str) -> Result<bool> {
    // Not-found: old_name must have at least one installation row. This check
    // runs before the same-name no-op so a nonexistent backend renamed to
    // its own name reports Ok(false) rather than a false success.
    let old_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM backend_installations WHERE name = ?1",
        [old_name],
        |r| r.get(0),
    )?;
    if old_exists == 0 {
        return Ok(false);
    }

    if old_name == new_name {
        return Ok(true);
    }

    // Prevent silently merging two distinct logical backends (including the
    // case where the new name already exists as a backend).
    let old_logical: Option<String> = get_backend_logical_id(conn, old_name)?;
    let new_logical: Option<String> = get_backend_logical_id(conn, new_name)?;
    let new_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM backend_installations WHERE name = ?1",
        [new_name],
        |r| r.get(0),
    )?;
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

    // Guard against renaming onto a name that already owns backend_configs rows
    // under a NULL or DIFFERENT logical_id (for any gpu_variant). Such a row
    // would otherwise collide on the `(logical_id, gpu_variant)` uniqueness.
    let config_conflicts: i64 = conn.query_row(
        "SELECT COUNT(*) FROM backend_configs
         WHERE name = ?1 AND (logical_id IS NULL OR logical_id = '' OR logical_id != ?2)",
        (new_name, old_logical.as_deref().unwrap_or("")),
        |r| r.get(0),
    )?;
    if config_conflicts > 0 {
        anyhow::bail!("refusing to merge overlapping backend config/settings");
    }

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE backend_installations SET name = ?1 WHERE name = ?2",
        (new_name, old_name),
    )?;
    tx.execute(
        "UPDATE backend_configs SET name = ?1 WHERE name = ?2",
        (new_name, old_name),
    )?;
    tx.execute(
        "UPDATE model_configs SET backend = ?1 WHERE backend = ?2",
        (new_name, old_name),
    )?;
    tx.execute(
        "UPDATE active_models SET backend = ?1 WHERE backend = ?2",
        (new_name, old_name),
    )?;
    tx.commit()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_in_memory, OpenResult};

    #[test]
    fn test_upsert_backend_config_insert() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let args = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let env = vec!["RADV_PERFTEST=nogttspill".to_string()];
        let id = upsert_backend_config(
            &conn,
            "",
            "llama_cpp",
            "cpu",
            &args,
            &env,
            Some("http://localhost:8080/health"),
        )
        .unwrap();
        assert_eq!(id, 1);

        let record = get_backend_config(&conn, "llama_cpp", "cpu")
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
    }

    #[test]
    fn test_upsert_backend_config_update() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Insert initial row
        let id1 = upsert_backend_config(
            &conn,
            "",
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string()],
            &[],
            Some("http://localhost:8080/health"),
        )
        .unwrap();

        // Upsert with different values
        let id2 = upsert_backend_config(
            &conn,
            "",
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string(), "-b 2048".to_string()],
            &["FOO=bar".to_string()],
            Some("http://localhost:9090/health"),
        )
        .unwrap();

        // ID should be the same (updated, not re-inserted)
        assert_eq!(id1, id2);

        let record = get_backend_config(&conn, "llama_cpp", "cpu")
            .unwrap()
            .unwrap();
        assert_eq!(record.default_args, vec!["-fa 1", "-b 2048"]);
        assert_eq!(record.default_env, vec!["FOO=bar"]);
        assert_eq!(
            record.health_check_url,
            Some("http://localhost:9090/health".to_string())
        );
    }

    #[test]
    fn test_get_backend_config_not_found() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let result = get_backend_config(&conn, "nonexistent", "cpu").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_backend_configs() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        upsert_backend_config(
            &conn,
            "",
            "llama_cpp",
            "cpu",
            &["-fa 1".to_string()],
            &[],
            Some("http://localhost:8080/health"),
        )
        .unwrap();
        upsert_backend_config(&conn, "", "llama_cpp", "vulkan", &[], &[], None).unwrap();
        upsert_backend_config(&conn, "", "ik_llama", "cpu", &[], &[], None).unwrap();

        let configs = list_backend_configs(&conn).unwrap();
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
    }

    #[test]
    fn test_upsert_backend_config_empty_args() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let id = upsert_backend_config(&conn, "", "empty_backend", "cpu", &[], &[], None).unwrap();
        assert_eq!(id, 1);

        let record = get_backend_config(&conn, "empty_backend", "cpu")
            .unwrap()
            .unwrap();
        assert!(record.default_args.is_empty());
        assert!(record.default_env.is_empty());
        assert!(record.health_check_url.is_none());
    }

    #[test]
    fn test_update_backend_source_success() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Insert a backend with no source
        insert_backend_installation(
            &conn,
            &BackendInstallationRecord {
                id: 0,
                name: "llama_cpp".to_string(),
                backend_type: "llama_cpp".to_string(),
                version: "b8407".to_string(),
                path: "/tmp/test/llama-server".to_string(),
                installed_at: 0,
                gpu_variant: "cpu".to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .unwrap();

        // Update the source column
        let new_source = r#"{"source":"SourceCode","content":{"version":"b8407","git_url":"https://github.com/ggml-org/llama.cpp.git"}}"#;
        update_backend_source(&conn, "llama_cpp", "cpu", new_source).unwrap();

        // Verify the source was updated
        let record = get_active_backend(&conn, "llama_cpp", "cpu")
            .unwrap()
            .unwrap();
        assert_eq!(record.source, Some(new_source.to_string()));
    }

    #[test]
    fn test_update_backend_source_not_found() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let result = update_backend_source(
            &conn,
            "nonexistent",
            "cpu",
            r#"{"source":"Prebuilt","content":{"version":"v1"}}"#,
        );
        assert!(result.is_err());
    }

    /// Renaming a backend preserves its default args/env (via logical_id) while
    /// syncing the display name everywhere.
    #[test]
    fn test_rename_backend_preserves_config_and_syncs_names() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Install a backend; insert assigns a stable logical_id.
        insert_backend_installation(
            &conn,
            &BackendInstallationRecord {
                id: 0,
                name: "vllm".to_string(),
                backend_type: "docker".to_string(),
                version: "0.5.8".to_string(),
                path: "n/a".to_string(),
                installed_at: 0,
                gpu_variant: "rocm".to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .unwrap();
        let lid = get_backend_logical_id(&conn, "vllm").unwrap().unwrap();

        // Add config keyed by the logical id.
        upsert_backend_config(
            &conn,
            &lid,
            "vllm",
            "rocm",
            &["-fa 1".into()],
            &["A=1".into()],
            None,
        )
        .unwrap();

        // A model and an active-model row reference the backend by name.
        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('m/m', 'vllm')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO active_models (server_name, model_name, backend, pid, port, backend_url, loaded_at, last_accessed)
             VALUES ('s1', 'm', 'vllm', 1, 8000, 'http://x', 't', 't')",
            [],
        )
        .unwrap();

        // Pre-rename, config is found by name.
        assert!(get_backend_config(&conn, "vllm", "rocm").unwrap().is_some());

        assert!(rename_backend(&conn, "vllm", "radiance").unwrap());

        // logical_id unchanged.
        assert_eq!(
            get_backend_logical_id(&conn, "radiance").unwrap().unwrap(),
            lid
        );

        // Default args/env survive the rename, found by the new name.
        let cfg = get_backend_config(&conn, "radiance", "rocm")
            .unwrap()
            .unwrap();
        assert_eq!(cfg.default_args, vec!["-fa 1"]);
        assert_eq!(cfg.default_env, vec!["A=1"]);
        assert_eq!(cfg.name, "radiance");

        // Models / runtime rows now point at the new name.
        let backend: String = conn
            .query_row(
                "SELECT backend FROM model_configs WHERE repo_id='m/m'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(backend, "radiance");
        let ab: String = conn
            .query_row(
                "SELECT backend FROM active_models WHERE server_name='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ab, "radiance");
    }

    /// Renaming onto an existing different backend is rejected.
    #[test]
    fn test_rename_backend_rejects_merging_distinct_backends() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        for (i, name) in ["vllm", "other"].iter().enumerate() {
            insert_backend_installation(
                &conn,
                &BackendInstallationRecord {
                    id: 0,
                    name: name.to_string(),
                    backend_type: "docker".to_string(),
                    version: format!("v{i}"),
                    path: "n/a".to_string(),
                    installed_at: 0,
                    gpu_variant: "rocm".to_string(),
                    source: None,
                    is_active: true,
                    docker_config: None,
                    logical_id: String::new(),
                },
            )
            .unwrap();
        }

        assert!(rename_backend(&conn, "vllm", "other").is_err());
    }

    /// Renaming a backend that has no installation row reports Ok(false).
    #[test]
    fn test_rename_backend_not_found_returns_false() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        assert!(!rename_backend(&conn, "ghost", "something").unwrap());
    }

    /// Renaming a nonexistent backend to its own name still reports Ok(false)
    /// (the existence check must run before the same-name no-op short-circuit).
    #[test]
    fn test_rename_backend_not_found_same_name_returns_false() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        assert!(!rename_backend(&conn, "ghost", "ghost").unwrap());
    }

    /// When two config rows share a (name, gpu_variant) but carry different
    /// logical_ids, the name-based fallback picks the lowest id deterministically.
    #[test]
    fn test_get_backend_config_deterministic_tiebreaker() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        conn.execute(
            "INSERT INTO backend_configs (id, logical_id, name, gpu_variant)
             VALUES (100, 'l1', 'dup', 'cpu')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO backend_configs (id, logical_id, name, gpu_variant)
             VALUES (101, 'l2', 'dup', 'cpu')",
            [],
        )
        .unwrap();

        // Neither row matches by logical_id, so both match by name; the
        // tiebreaker must deterministically return the lowest id.
        let record = get_backend_config(&conn, "dup", "cpu").unwrap().unwrap();
        assert_eq!(record.id, 100);
    }

    /// Renaming onto a name that already owns a legacy (NULL logical_id)
    /// backend_configs row is refused, mirroring the BLOCKING UNIQUE-violation
    /// scenario: a config row created via default-args POST / TOML migration for
    /// name "other" would otherwise collide once backfill stamps it.
    #[test]
    fn test_rename_backend_rejects_backend_configs_conflict() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Legacy config row for "other"/cpu with an empty logical_id.
        upsert_backend_config(&conn, "", "other", "cpu", &["--x".into()], &[], None).unwrap();

        // Install "vllm"/cpu; it gets a brand-new logical_id.
        insert_backend_installation(
            &conn,
            &BackendInstallationRecord {
                id: 0,
                name: "vllm".to_string(),
                backend_type: "docker".to_string(),
                version: "v1".to_string(),
                path: "n/a".to_string(),
                installed_at: 0,
                gpu_variant: "cpu".to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .unwrap();

        // Renaming vllm -> other would merge onto the NULL-config row: refused.
        assert!(rename_backend(&conn, "vllm", "other").is_err());
    }

    /// Renaming onto a name that already has an installation is refused even
    /// when the old backend has no logical id yet (the new_name-exists case).
    #[test]
    fn test_rename_backend_rejects_existing_new_name() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        insert_backend_installation(
            &conn,
            &BackendInstallationRecord {
                id: 0,
                name: "radiance".to_string(),
                backend_type: "docker".to_string(),
                version: "v1".to_string(),
                path: "n/a".to_string(),
                installed_at: 0,
                gpu_variant: "rocm".to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .unwrap();
        // Strip the logical id so old_name has an installation but no logical id.
        conn.execute("UPDATE backend_installations SET logical_id = ''", [])
            .unwrap();
        // A distinct backend already exists under the target name.
        insert_backend_installation(
            &conn,
            &BackendInstallationRecord {
                id: 0,
                name: "vllm".to_string(),
                backend_type: "docker".to_string(),
                version: "v2".to_string(),
                path: "n/a".to_string(),
                installed_at: 0,
                gpu_variant: "cuda".to_string(),
                source: None,
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .unwrap();

        assert!(rename_backend(&conn, "radiance", "vllm").is_err());
    }
}
