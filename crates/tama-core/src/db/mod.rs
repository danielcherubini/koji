//! Database module for SQLite
//!
//! Provides connection helpers, automatic migration system, and a Repository
//! layer for domain-level database access.

pub mod backfill;
pub mod migrations;
pub mod postgres;
pub mod queries;
pub mod repository;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
pub(crate) use rusqlite::Connection;

use crate::config::ModelConfig;

/// Result of opening a database connection
pub struct OpenResult {
    pub conn: Connection,
    pub needs_backfill: bool,
}

/// Load all model_configs rows and return them as a HashMap<config_key, ModelConfig>
/// where config_key is derived via `crate::models::ConfigKey::from_repo_id`.
///
/// NOTE: this is only used internally by the proxy to build its in-memory registry.
/// All external API lookups should use the integer `id` column directly.
pub fn load_model_configs(conn: &Connection) -> anyhow::Result<HashMap<String, ModelConfig>> {
    let records = queries::get_all_model_configs(conn)?;
    let mut configs = HashMap::new();

    for record in records {
        let config_key = crate::models::ConfigKey::from_repo_id(&record.repo_id).to_string();
        let mut config = ModelConfig::from_db_record(&record);
        config.db_id = Some(record.id);

        // Populate quants from model_files table to restore them after restart
        let files = queries::get_model_files(conn, record.id)?;
        for file in files {
            let quant_key = file.quant.clone().unwrap_or_else(|| file.filename.clone());
            config.quants.insert(
                quant_key,
                crate::config::QuantEntry {
                    file: file.filename.clone(),
                    kind: crate::config::QuantKind::from_filename(&file.filename),
                    size_bytes: file.size_bytes.map(|s| s as u64),
                    context_length: None,
                },
            );
        }

        configs.insert(config_key, config);
    }

    Ok(configs)
}

/// Persist a single ModelConfig entry.
/// `config_key` is the HashMap key (double-dash, lowercased). The DB's
/// `repo_id` preserves the original HF repo case — taken from `mc.model`
/// when present (carries the exact repo_id the user entered), and only
/// falling back to deriving from `config_key` when `mc.model` is unset.
/// Returns the integer model id from the database.
pub fn save_model_config(
    conn: &Connection,
    config_key: &str,
    mc: &ModelConfig,
) -> anyhow::Result<i64> {
    let repo_id = mc
        .model
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::models::config_key_to_repo_id(config_key));
    let mut record = mc.to_db_record(&repo_id);
    // Default api_name to repo_id at save time so the DB always stores a
    // concrete value. `from_db_record` used to backfill this on load, which
    // meant unsaved rows, JSON exports, and direct DB queries saw NULL even
    // though the in-memory ModelConfig had a value.
    if record.api_name.as_deref().is_none_or(str::is_empty) {
        record.api_name = Some(repo_id.clone());
    }
    queries::upsert_model_config(conn, &record)
}

/// Open (or create) the SQLite database at `config_dir/tama.db`
///
/// Sets up the database with:
/// - WAL mode enabled
/// - Foreign keys enabled
/// - Migrations applied
///
/// Returns a connection and whether backfill is needed (true if DB was freshly created).
pub fn open(config_dir: &Path) -> anyhow::Result<OpenResult> {
    // Ensure the config directory exists before SQLite tries to create the file.
    std::fs::create_dir_all(config_dir)?;
    let db_path = config_dir.join("tama.db");
    let conn = Connection::open(&db_path)?;

    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // Check user_version BEFORE running migrations to detect fresh DB
    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let needs_backfill = current_version == 0;

    migrations::run(&conn)?;
    // Idempotent post-migration backfill: assign stable per-backend logical ids
    // and populate the new stable-key columns (rename-safe backend identity).
    backfill_backend_logical_ids(&conn)?;

    Ok(OpenResult {
        conn,
        needs_backfill,
    })
}

/// Backup the SQLite database at `config_dir/tama.db` to a destination path.
///
/// Uses SQLite's `VACUUM INTO` command to create a clean, consistent copy of
/// the database. This avoids copying WAL/SHM files and guarantees a consistent
/// snapshot even if the database is in use.
///
/// # Arguments
/// * `config_dir` - The tama config directory containing `tama.db`
/// * `dest` - Where to write the backup database file
///
/// # Returns
/// Result<()> indicating success or failure
pub fn backup_db(config_dir: &Path, dest: &Path) -> anyhow::Result<()> {
    // Compute safe parent path - avoid creating directory named after the file
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent).context("Failed to create parent directory for backup")?;

    let db_path = config_dir.join("tama.db");
    let conn = Connection::open(&db_path)?;

    // VACUUM INTO creates a clean copy without WAL/SHM files
    // Convert Path to string for rusqlite parameter binding
    let dest_str = dest.to_string_lossy().to_string();
    conn.execute("VACUUM INTO ?", [&dest_str])
        .context("Failed to vacuum database into destination")?;

    Ok(())
}

/// Open an in-memory SQLite database for testing.
///
/// Applies `PRAGMA foreign_keys=ON` (same as `open()`) and runs migrations.
/// Note: `journal_mode=WAL` is not applied because it is not supported for
/// in-memory databases.
pub fn open_in_memory() -> anyhow::Result<OpenResult> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // In-memory DB starts at version 0, so it needs backfill
    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let needs_backfill = current_version == 0;

    migrations::run(&conn)?;
    backfill_backend_logical_ids(&conn)?;

    Ok(OpenResult {
        conn,
        needs_backfill,
    })
}

/// Assign each logical backend a stable `logical_id` (a UUID preserved across
/// renames and version upgrades) and populates the stable-key columns on
/// `provider_configs`, `model_configs`, and `active_models`.
///
/// Idempotent: rows that already carry a non-empty logical id are left alone,
/// so it is safe to run on every DB open. Existing backends get a fresh UUID;
/// rows sharing a `name` share the same `logical_id` (the name is the logical
/// backend's slug; gpu_variant remains a dimension within it).
pub(crate) fn backfill_backend_logical_ids(conn: &Connection) -> anyhow::Result<()> {
    use std::collections::HashMap;

    // Collect distinct backend names that are missing a logical_id.
    let mut names: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT name FROM provider_installations WHERE logical_id IS NULL OR logical_id = ''",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            names.push(row?);
        }
    }

    // Rust cannot mutate while iterating over prepared statements on the same
    // connection; materialize the name -> logical_id map first, then apply.
    let mut assignments: HashMap<String, String> = HashMap::new();
    for name in &names {
        // Reuse an existing non-empty logical_id for this name if any row has one.
        let existing: Option<String> = conn
            .query_row(
                "SELECT logical_id FROM provider_installations WHERE name = ?1 AND logical_id IS NOT NULL AND logical_id != '' LIMIT 1",
                [name],
                |r| r.get(0),
            )
            .ok();
        let logical_id = existing.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        assignments.insert(name.clone(), logical_id);
    }

    let tx = conn.unchecked_transaction()?;
    for (name, logical_id) in &assignments {
        tx.execute(
            "UPDATE provider_installations SET logical_id = ?1 WHERE name = ?2 AND (logical_id IS NULL OR logical_id = '')",
            (logical_id, name),
        )?;
    }

    // Conflict-tolerant: drop any NULL-logical_id provider_configs row whose
    // computed target (logical_id, gpu_variant) would duplicate an existing row
    // once stamped below. This can happen when a rename merged a stable-key row
    // and a legacy NULL row under the same name; leaving both would violate the
    // UNIQUE(logical_id, gpu_variant) constraint and break db open.
    tx.execute(
        "DELETE FROM provider_configs
         WHERE (logical_id IS NULL OR logical_id = '')
           AND EXISTS (
             SELECT 1 FROM provider_configs bc2
             WHERE bc2.logical_id = (
                 SELECT bi.logical_id FROM provider_installations bi
                 WHERE bi.name = provider_configs.name
                   AND bi.gpu_variant = provider_configs.gpu_variant
                   AND bi.logical_id IS NOT NULL AND bi.logical_id != ''
                 LIMIT 1
             )
               AND bc2.gpu_variant = provider_configs.gpu_variant
               AND bc2.id != provider_configs.id
           )",
        [],
    )?;

    // Backfill provider_configs.logical_id from the matching installation name+variant.
    tx.execute(
        "UPDATE provider_configs
         SET logical_id = (
             SELECT bi.logical_id FROM provider_installations bi
             WHERE bi.name = provider_configs.name AND bi.gpu_variant = provider_configs.gpu_variant
               AND bi.logical_id IS NOT NULL AND bi.logical_id != ''
             LIMIT 1
         )
         WHERE logical_id IS NULL OR logical_id = ''",
        [],
    )?;

    // Backfill model_configs.backend_id from the matching installation name.
    tx.execute(
        "UPDATE model_configs
         SET backend_id = (
             SELECT bi.logical_id FROM provider_installations bi
             WHERE bi.name = model_configs.backend
               AND bi.logical_id IS NOT NULL AND bi.logical_id != ''
             LIMIT 1
         )
         WHERE backend_id IS NULL OR backend_id = ''",
        [],
    )?;

    // Backfill active_models.backend_id from the matching installation name.
    tx.execute(
        "UPDATE active_models
         SET backend_id = (
             SELECT bi.logical_id FROM provider_installations bi
             WHERE bi.name = active_models.backend
               AND bi.logical_id IS NOT NULL AND bi.logical_id != ''
             LIMIT 1
         )
         WHERE backend_id IS NULL OR backend_id = ''",
        [],
    )?;

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Verify tables exist by querying sqlite_master
        let pulls_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='model_pulls'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pulls_count, 1);

        let files_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='model_files'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(files_count, 1);

        let log_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='pull_log'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(log_count, 1);

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='idx_pull_log_repo'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }

    #[test]
    fn test_migrations_idempotent() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Run migrations twice - should not error
        migrations::run(&conn).unwrap();
        migrations::run(&conn).unwrap();
    }

    #[test]
    fn test_user_version_updated() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, migrations::LATEST_VERSION);
    }

    #[test]
    fn test_migration_v3_creates_provider_installations() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='provider_installations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "provider_installations table should exist after migration v3"
        );

        // Verify index was created
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_provider_installations_name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            idx_count, 1,
            "idx_provider_installations_name index should exist after migration v3"
        );
    }

    /// Test that loading model configs from an empty DB returns an empty HashMap.
    #[test]
    fn test_load_model_configs_empty() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let configs = load_model_configs(&conn).unwrap();
        assert!(configs.is_empty());
    }

    /// Backfill assigns a stable, per-name logical_id and populates the stable-key
    /// columns on provider_configs / model_configs / active_models.
    #[test]
    fn test_backfill_backend_logical_ids_assigns_and_propagates() {
        use crate::db::queries::{insert_installation, InstallationRecord};

        let OpenResult { conn, .. } = open_in_memory().unwrap();
        // open_in_memory already ran backfill on an empty DB; insert real data now.
        insert_installation(
            &conn,
            &InstallationRecord {
                id: 0,
                name: "radiance".to_string(),
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

        // Seed a config row referenced by the old (now also current) name.
        queries::upsert_installation_config(
            &conn,
            "",
            "radiance",
            "rocm",
            &["--flag".into()],
            &["A=1".into()],
            None,
        )
        .unwrap();

        // Grant backend_id keys to model_configs and active_models manually so
        // the backfill has something to populate.
        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('qwen/qwen', 'radiance')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO active_models (server_name, model_name, backend, pid, port, backend_url, loaded_at, last_accessed)
             VALUES ('m1', 'qwen', 'radiance', 1, 8000, 'http://x', 't', 't')",
            [],
        )
        .unwrap();

        // Run the idempotent backfill.
        backfill_backend_logical_ids(&conn).unwrap();

        // Every provider_installations row now carries the same logical_id.
        let installs = crate::db::queries::list_active_installations(&conn).unwrap();
        assert_eq!(installs.len(), 1);
        let lid = &installs[0].logical_id;
        assert!(!lid.is_empty(), "logical_id should be assigned");

        // The config row got the same logical_id (rename-safe linkage).
        let cfg = crate::db::queries::get_installation_config(&conn, lid, "rocm")
            .unwrap()
            .expect("backend config should exist");
        assert_eq!(cfg.logical_id.as_deref(), Some(lid.as_str()));

        // model_configs.backend_id and active_models.backend_id are populated.
        let m_backend_id: Option<String> = conn
            .query_row(
                "SELECT backend_id FROM model_configs WHERE repo_id='qwen/qwen'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(m_backend_id.as_deref(), Some(lid.as_str()));
        let a_backend_id: Option<String> = conn
            .query_row(
                "SELECT backend_id FROM active_models WHERE server_name='m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_backend_id.as_deref(), Some(lid.as_str()));

        // Re-running is a no-op (idempotent) and preserves the existing id.
        backfill_backend_logical_ids(&conn).unwrap();
        let installs2 = crate::db::queries::list_active_installations(&conn).unwrap();
        assert_eq!(installs2[0].logical_id, *lid);
    }

    /// Backfill is conflict-tolerant: a legacy NULL-logical_id provider_configs
    /// row whose computed target (logical_id, gpu_variant) duplicates an existing
    /// row is dropped instead of tripping the UNIQUE constraint on db open.
    #[test]
    fn test_backfill_backend_logical_ids_conflict_tolerant() {
        use crate::db::queries::{insert_installation, InstallationRecord};

        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Install a backend; it gets a fresh logical_id.
        insert_installation(
            &conn,
            &InstallationRecord {
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
        let lid = crate::db::queries::get_installation_logical_id(&conn, "vllm")
            .unwrap()
            .unwrap();

        // A stable-key config row already tagged with the logical_id.
        queries::upsert_installation_config(&conn, &lid, "vllm", "cpu", &["--a".into()], &[], None)
            .unwrap();

        // A second, legacy config row for the same name+variant with a NULL
        // logical_id — its target (lid, "cpu") duplicates the stable-key row.
        conn.execute(
            r#"INSERT INTO provider_configs (logical_id, name, gpu_variant, default_args, default_env, health_check_url)
               VALUES (NULL, 'vllm', 'cpu', '["--c"]', NULL, NULL)"#,
            [],
        )
        .unwrap();

        // Backfill must not error / panic on the duplicate computed logical id.
        backfill_backend_logical_ids(&conn).unwrap();

        // The duplicate NULL row is dropped; only the stable-key row remains.
        let cfgs = queries::list_installation_configs(&conn).unwrap();
        assert_eq!(cfgs.len(), 1);
        assert_eq!(cfgs[0].logical_id.as_deref(), Some(lid.as_str()));
        assert_eq!(cfgs[0].default_args, vec!["--a"]);
    }

    /// Test saving and then loading a model config.
    #[test]
    fn test_save_and_load_model_config() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let mc = ModelConfig {
            backend: "llama.cpp".to_string(),
            display_name: Some("Test Model".to_string()),
            ..Default::default()
        };
        let config_key = "owner--repo".to_string();

        save_model_config(&conn, &config_key, &mc).unwrap();

        let configs = load_model_configs(&conn).unwrap();
        assert!(configs.contains_key(&config_key));
        let loaded = configs.get(&config_key).unwrap();
        assert_eq!(loaded.backend, mc.backend);
        assert_eq!(loaded.display_name, mc.display_name);
    }
}
