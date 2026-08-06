//! Config and database merge logic for backup/restore.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;

/// RAII guard to ensure database is always detached.
struct DetachGuard<'a> {
    conn: &'a rusqlite::Connection,
    attached: bool,
}

impl<'a> DetachGuard<'a> {
    fn new(conn: &'a rusqlite::Connection) -> Self {
        Self {
            conn,
            attached: false,
        }
    }
    fn attach(&mut self, path: &str) -> Result<()> {
        self.conn
            .execute("ATTACH DATABASE ? AS backup_db", [path])
            .context("Failed to attach backup database")?;
        self.attached = true;
        Ok(())
    }
}

impl Drop for DetachGuard<'_> {
    fn drop(&mut self) {
        if self.attached {
            // Best effort detach - ignore errors since we're in Drop
            let _ = self.conn.execute("DETACH DATABASE backup_db", []);
        }
    }
}

/// Statistics from merging config.
#[derive(Debug, Default)]
pub struct MergeStats {
    pub new_backends: Vec<String>,
    pub new_sampling_templates: Vec<String>,
    pub skipped_backends: Vec<String>,
}

/// Merge a backup config into a local config.
///
/// - New backends/models are added
/// - Existing local values are preserved (local wins)
/// - Sampling templates are merged (local wins)
///
/// Returns statistics about what was added vs skipped.
pub fn merge_config(local: &mut Config, backup: &Config) -> MergeStats {
    let mut stats = MergeStats::default();

    // Merge backends
    for (name, backend) in &backup.backends {
        if local.backends.contains_key(name) {
            stats.skipped_backends.push(name.clone());
        } else {
            local.backends.insert(name.clone(), backend.clone());
            stats.new_backends.push(name.clone());
        }
    }

    // Merge sampling templates (local wins)
    for (name, template) in &backup.sampling_templates {
        if !local.sampling_templates.contains_key(name) {
            local
                .sampling_templates
                .insert(name.clone(), template.clone());
            stats.new_sampling_templates.push(name.clone());
        }
    }

    stats
}

/// Merge model card TOML files from backup to local.
///
/// Copies any card that doesn't exist locally.
pub fn merge_model_cards(
    local_configs_dir: &Path,
    backup_configs_dir: &Path,
) -> Result<Vec<String>> {
    let mut copied = Vec::new();

    if !backup_configs_dir.exists() {
        return Ok(copied);
    }

    // Ensure local directory exists
    std::fs::create_dir_all(local_configs_dir).with_context(|| {
        format!(
            "Failed to create local configs directory: {}",
            local_configs_dir.display()
        )
    })?;

    for entry in std::fs::read_dir(backup_configs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let local_path = local_configs_dir.join(&filename);
            if !local_path.exists() {
                std::fs::copy(&path, &local_path)
                    .with_context(|| format!("Failed to copy card: {}", filename))?;
                copied.push(filename);
            }
        }
    }

    Ok(copied)
}

/// Statistics from merging database.
#[derive(Debug, Default)]
pub struct DbMergeStats {
    pub new_model_pulls: u32,
    pub new_model_files: u32,
    pub new_backend_installations: u32,
}

/// Merge database records from backup to local.
///
/// Uses `INSERT OR IGNORE` to skip existing records.
/// Only merges essential tables (model_pulls, model_files, backend_installations).
/// Ephemeral tables (active_models, pull_log, system_metrics_history) are skipped.
pub fn merge_database(
    local_db: &rusqlite::Connection,
    backup_db_path: &Path,
) -> Result<DbMergeStats> {
    let mut stats = DbMergeStats::default();

    // Attach backup database with RAII guard to ensure cleanup
    let mut guard = DetachGuard::new(local_db);
    let backup_db_path_str = backup_db_path.to_string_lossy().to_string();
    guard.attach(&backup_db_path_str)?;

    // Merge model_configs first — new repos from the backup become available
    // for model_pulls / model_files resolution below.
    //
    // NOTE: This was changed from a partial two-column merge because the old code
    // silently failed to preserve model_pulls/model_files after migration _0008
    // added the `id`/model_id FK column — without a full copy, INSERT OR IGNORE
    // on model_pulls could not resolve the FK. Copying all columns (except id,
    // created_at, updated_at) ensures defaults apply and data is preserved.
    //
    // The column list is computed as the intersection of the hard-coded list
    // (which includes all columns known to the current schema, including
    // vllm_config added by migration _0044) and the backup DB's actual columns.
    // This ensures pre-44 backups (lacking vllm_config) still restore correctly
    // while post-44 backups preserve the new column.
    //
    // NOTE: created_at and updated_at are excluded — restored rows get fresh
    // timestamps from the DEFAULT expression rather than carrying stale values
    // from the backup.
    let model_configs_columns: &[&str] = &[
        "repo_id",
        "display_name",
        "backend",
        "gpu_variant",
        "gpu_device",
        "enabled",
        "selected_quant",
        "selected_mmproj",
        "selected_mtp_model",
        "context_length",
        "num_parallel",
        "kv_unified",
        "gpu_layers",
        "cache_type_k",
        "cache_type_v",
        "port",
        "args",
        "sampling",
        "modalities",
        "profile",
        "api_name",
        "health_check",
        "hf_format",
        "hf_base_model",
        "hf_pipeline_tag",
        "hf_total_params",
        "hf_active_params",
        "hf_architecture_type",
        "hf_context_length",
        "hf_num_layers",
        "hf_last_modified",
        "spec_decoding",
        "n_batch",
        "n_ubatch",
        "vllm_config",
    ];

    // Get the backup DB's column list via PRAGMA table_info on the attached DB.
    // This is more robust than parsing CREATE TABLE text (which can false-positive
    // on substring matches like "args" appearing inside other identifiers).
    let backup_columns: Vec<String> = local_db
        .prepare("PRAGMA backup_db.table_info(model_configs)")
        .context("Failed to prepare PRAGMA table_info on backup_db")?
        .query_map([], |row| row.get::<_, String>(1)) // name column (index 1)
        .context("Failed to query PRAGMA table_info")?
        .collect::<Result<_, _>>()
        .context("Failed to read backup column names")?;

    // Filter to columns that exist in the backup's model_configs table
    let common_columns: Vec<&str> = model_configs_columns
        .iter()
        .filter(|&&col| backup_columns.iter().any(|c| c == col))
        .copied()
        .collect();

    let cols = common_columns.join(", ");
    let merge_sql = format!(
        "INSERT OR IGNORE INTO model_configs ({cols}) SELECT {cols} FROM backup_db.model_configs"
    );
    local_db
        .execute_batch(&merge_sql)
        .context("Failed to merge model_configs")?;

    // Merge model_pulls — resolve local model_id via repo_id LOWER join.
    let before = count_model_pulls(local_db)?;
    local_db
        .execute_batch(
            "INSERT OR IGNORE INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
         SELECT mc.id, bp.repo_id, bp.commit_sha, bp.pulled_at \
         FROM backup_db.model_pulls bp \
         JOIN model_configs mc ON LOWER(mc.repo_id) = LOWER(bp.repo_id)",
        )
        .context("Failed to merge model_pulls")?;
    let after = count_model_pulls(local_db)?;
    stats.new_model_pulls = after.saturating_sub(before);

    // Merge model_files — resolve local model_id via repo_id LOWER join.
    // The live table uses `pulled_at` (migration v39). Old backups (pre-v39)
    // use `downloaded_at`. Detect which column the backup DB uses.
    let backup_pulled_col = if local_db
        .query_row(
            "SELECT name FROM backup_db.pragma_table_info('model_files') WHERE name = 'pulled_at'",
            [],
            |row| row.get::<_, String>(0),
        )
        .is_ok()
    {
        "bf.pulled_at"
    } else {
        "bf.downloaded_at"
    };

    let before = count_model_files(local_db)?;
    local_db
        .execute_batch(&format!(
            "INSERT OR IGNORE INTO model_files (model_id, repo_id, filename, quant, \
             lfs_oid, size_bytes, pulled_at, last_verified_at, verified_ok, verify_error) \
         SELECT mc.id, bf.repo_id, bf.filename, bf.quant, bf.lfs_oid, bf.size_bytes, \
                {backup_pulled_col}, bf.last_verified_at, bf.verified_ok, bf.verify_error \
         FROM backup_db.model_files bf \
         JOIN model_configs mc ON LOWER(mc.repo_id) = LOWER(bf.repo_id)",
        ))
        .context("Failed to merge model_files")?;
    let after = count_model_files(local_db)?;
    stats.new_model_files = after.saturating_sub(before);

    // Merge backend_installations (explicit column list, no id).
    // The live table has docker_config (migration v43). Old backups (pre-v43)
    // don't have it. Detect whether the backup DB has the column.
    let has_docker_config = local_db
        .query_row(
            "SELECT name FROM backup_db.pragma_table_info('backend_installations') WHERE name = 'docker_config'",
            [],
            |row| row.get::<_, String>(0),
        )
        .is_ok();

    let dc_select = if has_docker_config {
        "bf.docker_config"
    } else {
        "NULL"
    };

    let before = count_backend_installations(local_db)?;
    local_db
        .execute_batch(&format!(
            "INSERT OR IGNORE INTO backend_installations \
         (name, backend_type, version, path, installed_at, gpu_variant, source, is_active, docker_config) \
         SELECT name, backend_type, version, path, installed_at, gpu_variant, source, is_active, {dc_select} \
         FROM backup_db.backend_installations bf",
        ))
        .context("Failed to merge backend_installations")?;
    let after = count_backend_installations(local_db)?;
    stats.new_backend_installations = after.saturating_sub(before);

    // Guard will detach on drop
    Ok(stats)
}

fn count_model_pulls(conn: &rusqlite::Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM model_pulls", [], |row| row.get(0))?)
}

fn count_model_files(conn: &rusqlite::Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM model_files", [], |row| row.get(0))?)
}

fn count_backend_installations(conn: &rusqlite::Connection) -> Result<u32> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM backend_installations", [], |row| {
            row.get(0)
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    // Full model_configs column list (as of migration _0044, includes vllm_config).
    // Used for creating local DB schema in tests.
    const MODEL_CONFIGS_FULL_SCHEMA: &str = "
        CREATE TABLE model_configs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id TEXT NOT NULL,
            display_name TEXT,
            backend TEXT NOT NULL,
            gpu_variant TEXT NOT NULL DEFAULT 'cpu',
            gpu_device TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            selected_quant TEXT,
            selected_mmproj TEXT,
            selected_mtp_model TEXT,
            context_length INTEGER,
            num_parallel INTEGER,
            kv_unified INTEGER,
            gpu_layers INTEGER,
            cache_type_k TEXT,
            cache_type_v TEXT,
            port INTEGER,
            args TEXT,
            sampling TEXT,
            modalities TEXT,
            profile TEXT,
            api_name TEXT,
            health_check INTEGER NOT NULL DEFAULT 0,
            hf_format TEXT,
            hf_base_model TEXT,
            hf_pipeline_tag TEXT,
            hf_total_params TEXT,
            hf_active_params TEXT,
            hf_architecture_type TEXT,
            hf_context_length INTEGER,
            hf_num_layers INTEGER,
            hf_last_modified TEXT,
            spec_decoding TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            n_batch INTEGER,
            n_ubatch INTEGER,
            vllm_config TEXT
        )
    ";

    // Pre-44 model_configs schema (without vllm_config, n_batch, n_ubatch).
    const MODEL_CONFIGS_PRE44_SCHEMA: &str = "
        CREATE TABLE model_configs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            repo_id TEXT NOT NULL,
            display_name TEXT,
            backend TEXT NOT NULL,
            gpu_variant TEXT NOT NULL DEFAULT 'cpu',
            gpu_device TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            selected_quant TEXT,
            selected_mmproj TEXT,
            selected_mtp_model TEXT,
            context_length INTEGER,
            num_parallel INTEGER,
            kv_unified INTEGER,
            gpu_layers INTEGER,
            cache_type_k TEXT,
            cache_type_v TEXT,
            port INTEGER,
            args TEXT,
            sampling TEXT,
            modalities TEXT,
            profile TEXT,
            api_name TEXT,
            health_check INTEGER NOT NULL DEFAULT 0,
            hf_format TEXT,
            hf_base_model TEXT,
            hf_pipeline_tag TEXT,
            hf_total_params TEXT,
            hf_active_params TEXT,
            hf_architecture_type TEXT,
            hf_context_length INTEGER,
            hf_num_layers INTEGER,
            hf_last_modified TEXT,
            spec_decoding TEXT,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        )
    ";

    // Minimal tables needed for merge_database to succeed.
    const MINIMAL_EXTRA_TABLES: &str = "
        CREATE TABLE model_pulls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            model_id INTEGER,
            repo_id TEXT NOT NULL,
            commit_sha TEXT NOT NULL,
            pulled_at TEXT NOT NULL
        );
        CREATE TABLE model_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            model_id INTEGER,
            repo_id TEXT NOT NULL,
            filename TEXT NOT NULL,
            quant TEXT,
            lfs_oid TEXT,
            size_bytes INTEGER NOT NULL,
            pulled_at TEXT NOT NULL,
            last_verified_at TEXT,
            verified_ok INTEGER,
            verify_error TEXT
        );
        CREATE TABLE backend_installations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            backend_type TEXT NOT NULL,
            version TEXT NOT NULL,
            path TEXT NOT NULL,
            installed_at INTEGER NOT NULL,
            gpu_variant TEXT NOT NULL DEFAULT 'cpu',
            source TEXT,
            is_active INTEGER NOT NULL DEFAULT 0,
            docker_config TEXT DEFAULT NULL,
            UNIQUE(name, gpu_variant, version)
        )
    ";

    #[test]
    fn test_merge_config_adds_new_backends() {
        let mut local = Config::default();
        let mut backup = Config::default();

        // Add a new backend to backup
        backup.backends.insert(
            "new_backend".to_string(),
            crate::config::BackendConfig {
                path: None,
                version: None,
                gpu_variant: None,
            },
        );

        let stats = merge_config(&mut local, &backup);

        assert_eq!(stats.new_backends.len(), 1);
        assert_eq!(stats.new_backends[0], "new_backend");
        assert!(local.backends.contains_key("new_backend"));
    }

    #[test]
    fn test_merge_config_preserves_local() {
        let mut local = Config::default();
        let mut backup = Config::default();

        // Clear defaults to make test predictable
        local.backends.clear();
        backup.backends.clear();
        // Models are now stored in DB, so we don't clear them from Config
        // local.models.clear();
        // backup.models.clear();

        // Add a backend to local
        local.backends.insert(
            "existing".to_string(),
            crate::config::BackendConfig {
                path: Some("/local/path".to_string()),
                version: None,
                gpu_variant: None,
            },
        );

        // Try to add same backend to backup with different value
        backup.backends.insert(
            "existing".to_string(),
            crate::config::BackendConfig {
                path: Some("/backup/path".to_string()),
                version: None,
                gpu_variant: None,
            },
        );

        let stats = merge_config(&mut local, &backup);

        assert_eq!(stats.skipped_backends.len(), 1);
        assert!(stats.skipped_backends.contains(&"existing".to_string()));
        // Local value should be preserved
        assert_eq!(
            local.backends["existing"].path,
            Some("/local/path".to_string())
        );
    }

    #[test]
    fn test_merge_config_empty_backup() {
        let mut local = Config::default();
        local.backends.clear(); // Clear defaults for predictable test
        let mut backup = Config::default();
        backup.backends.clear(); // Also clear backup defaults

        let stats = merge_config(&mut local, &backup);

        assert!(stats.new_backends.is_empty());
        assert!(stats.skipped_backends.is_empty());
    }

    #[test]
    fn test_merge_config_empty_local() {
        let _local = Config::default();
        let mut backup = Config::default();
        backup.backends.insert(
            "new".to_string(),
            crate::config::BackendConfig {
                path: None,
                version: None,
                gpu_variant: None,
            },
        );

        // This should work — local gets the backup's backends
        let mut local_mut = Config::default();
        let stats = merge_config(&mut local_mut, &backup);
        assert_eq!(stats.new_backends.len(), 1);
    }

    #[test]
    fn test_merge_config_multiple_new_backends() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.backends.clear();
        backup.backends.clear();

        for i in 1..=5 {
            backup.backends.insert(
                format!("backend{}", i),
                crate::config::BackendConfig {
                    path: None,
                    version: None,
                    gpu_variant: None,
                },
            );
        }

        let stats = merge_config(&mut local, &backup);
        assert_eq!(stats.new_backends.len(), 5);
        assert_eq!(local.backends.len(), 5);
    }

    #[test]
    fn test_merge_config_mixed_new_and_existing() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.backends.clear();
        backup.backends.clear();

        // Add some to local
        for i in 1..=3 {
            local.backends.insert(
                format!("local{}", i),
                crate::config::BackendConfig {
                    path: None,
                    version: None,
                    gpu_variant: None,
                },
            );
        }
        // Add some to backup (some overlapping with local)
        for i in 1..=5 {
            backup.backends.insert(
                format!("backend{}", i),
                crate::config::BackendConfig {
                    path: None,
                    version: None,
                    gpu_variant: None,
                },
            );
        }
        // Overlap: local1, local2 are in both (backup overrides)
        backup.backends.insert(
            "local1".to_string(),
            crate::config::BackendConfig {
                path: Some("/backup/path".to_string()),
                version: None,
                gpu_variant: None,
            },
        );
        backup.backends.insert(
            "local2".to_string(),
            crate::config::BackendConfig {
                path: Some("/backup/path".to_string()),
                version: None,
                gpu_variant: None,
            },
        );

        let stats = merge_config(&mut local, &backup);
        // New: backend1, backend2, backend3, backend4, backend5 (5 new)
        // Skipped: local1, local2 (2 skipped)
        assert_eq!(stats.new_backends.len(), 5);
        assert_eq!(stats.skipped_backends.len(), 2);
    }

    #[test]
    fn test_merge_config_sampling_templates() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.sampling_templates.clear();
        backup.sampling_templates.clear();

        let params = crate::profiles::SamplingParams {
            temperature: Some(0.7),
            top_k: Some(50),
            top_p: None,
            min_p: None,
            presence_penalty: None,
            frequency_penalty: None,
            repeat_penalty: None,
        };
        backup
            .sampling_templates
            .insert("coding".to_string(), params);

        let stats = merge_config(&mut local, &backup);
        assert_eq!(stats.new_sampling_templates.len(), 1);
        assert!(local.sampling_templates.contains_key("coding"));
    }

    #[test]
    fn test_merge_config_local_wins_for_sampling_templates() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.sampling_templates.clear();
        backup.sampling_templates.clear();

        // Local has a template with temperature 0.5
        local.sampling_templates.insert(
            "coding".to_string(),
            crate::profiles::SamplingParams {
                temperature: Some(0.5),
                top_k: None,
                top_p: None,
                min_p: None,
                presence_penalty: None,
                frequency_penalty: None,
                repeat_penalty: None,
            },
        );

        // Backup has a different template with same name (temperature 0.9)
        backup.sampling_templates.insert(
            "coding".to_string(),
            crate::profiles::SamplingParams {
                temperature: Some(0.9),
                top_k: None,
                top_p: None,
                min_p: None,
                presence_penalty: None,
                frequency_penalty: None,
                repeat_penalty: None,
            },
        );

        let stats = merge_config(&mut local, &backup);
        // Local should win — no new templates added
        assert!(stats.new_sampling_templates.is_empty());
        assert_eq!(local.sampling_templates["coding"].temperature, Some(0.5));
    }

    #[test]
    fn test_merge_stats_default() {
        let stats = MergeStats::default();
        assert!(stats.new_backends.is_empty());
        assert!(stats.new_sampling_templates.is_empty());
        assert!(stats.skipped_backends.is_empty());
    }

    #[test]
    fn test_merge_stats_debug() {
        let stats = MergeStats {
            new_backends: vec!["a".to_string()],
            new_sampling_templates: vec!["b".to_string()],
            skipped_backends: vec!["c".to_string()],
        };
        let debug_str = format!("{:?}", stats);
        assert!(debug_str.contains("new_backends"));
        assert!(debug_str.contains("skipped_backends"));
    }

    /// Restore from a pre-44 backup (no vllm_config column) succeeds.
    /// The vllm_config column in the local DB should default to NULL.
    #[test]
    fn test_merge_database_pre44_backup_without_vllm_config() {
        let temp_dir = tempfile::tempdir().expect("tempdir");

        // Create local DB with full schema (includes vllm_config)
        let local_path = temp_dir.path().join("local.db");
        let local_conn = Connection::open(&local_path).expect("open local db");
        local_conn
            .execute_batch(MODEL_CONFIGS_FULL_SCHEMA)
            .expect("create local tables");
        local_conn
            .execute_batch(MINIMAL_EXTRA_TABLES)
            .expect("create extra tables");

        // Create backup DB with pre-44 schema (no vllm_config)
        let backup_path = temp_dir.path().join("backup.db");
        let backup_conn = Connection::open(&backup_path).expect("open backup db");
        backup_conn
            .execute_batch(MODEL_CONFIGS_PRE44_SCHEMA)
            .expect("create backup tables");
        backup_conn
            .execute_batch(MINIMAL_EXTRA_TABLES)
            .expect("create extra tables");

        // Insert a model config into backup
        backup_conn
            .execute(
                "INSERT INTO model_configs (repo_id, display_name, backend, spec_decoding) 
             VALUES ('test/repo', 'Test Model', 'llama_cpp', '{\"model\": \"test\"}')",
                [],
            )
            .expect("insert backup model");

        // Verify backup DB schema before merge
        let backup_cols: Vec<String> = backup_conn
            .prepare("SELECT name FROM pragma_table_info('model_configs')")
            .expect("prepare pragma")
            .query_map([], |row| row.get(0))
            .expect("query pragma")
            .collect::<Result<_, _>>()
            .expect("read column names");
        assert!(
            !backup_cols.contains(&"vllm_config".to_string()),
            "backup should not have vllm_config"
        );
        assert!(
            !backup_cols.contains(&"n_batch".to_string()),
            "backup should not have n_batch"
        );

        // Merge should succeed despite missing vllm_config in backup
        let result = merge_database(&local_conn, &backup_path);
        assert!(
            result.is_ok(),
            "merge_database should succeed with pre-44 backup: {:?}",
            result.err()
        );

        // Verify the model was merged and vllm_config is NULL
        let vllm: Option<String> = local_conn
            .query_row(
                "SELECT vllm_config FROM model_configs WHERE repo_id = 'test/repo'",
                [],
                |row| row.get(0),
            )
            .expect("query vllm_config");
        assert!(
            vllm.is_none(),
            "vllm_config should be NULL for pre-44 backup restore, got: {:?}",
            vllm
        );

        // Verify other columns were preserved
        let spec: Option<String> = local_conn
            .query_row(
                "SELECT spec_decoding FROM model_configs WHERE repo_id = 'test/repo'",
                [],
                |row| row.get(0),
            )
            .expect("query spec_decoding");
        assert_eq!(spec, Some("{\"model\": \"test\"}".to_string()));
    }

    /// Restore from a post-44 backup (with vllm_config) preserves the value.
    #[test]
    fn test_merge_database_post44_backup_with_vllm_config() {
        let temp_dir = tempfile::tempdir().expect("tempdir");

        // Create local DB with full schema (includes vllm_config)
        let local_path = temp_dir.path().join("local.db");
        let local_conn = Connection::open(&local_path).expect("open local db");
        local_conn
            .execute_batch(MODEL_CONFIGS_FULL_SCHEMA)
            .expect("create local tables");
        local_conn
            .execute_batch(MINIMAL_EXTRA_TABLES)
            .expect("create extra tables");

        // Create backup DB with full schema (includes vllm_config)
        let backup_path = temp_dir.path().join("backup.db");
        let backup_conn = Connection::open(&backup_path).expect("open backup db");
        backup_conn
            .execute_batch(MODEL_CONFIGS_FULL_SCHEMA)
            .expect("create backup tables");
        backup_conn
            .execute_batch(MINIMAL_EXTRA_TABLES)
            .expect("create extra tables");

        // Insert a model config with vllm_config into backup
        let vllm_value = r#"{"quantization":"fp8","tensor_parallel_size":2}"#;
        backup_conn.execute(
            "INSERT INTO model_configs (repo_id, display_name, backend, spec_decoding, vllm_config) 
             VALUES ('test/repo', 'Test Model', 'vllm', '{\"model\": \"spec\"}', ?1)",
            [vllm_value],
        ).expect("insert backup model");

        // Merge should succeed and preserve vllm_config
        let result = merge_database(&local_conn, &backup_path);
        assert!(
            result.is_ok(),
            "merge_database should succeed with post-44 backup: {:?}",
            result.err()
        );

        // Verify vllm_config was preserved
        let vllm: Option<String> = local_conn
            .query_row(
                "SELECT vllm_config FROM model_configs WHERE repo_id = 'test/repo'",
                [],
                |row| row.get(0),
            )
            .expect("query vllm_config");
        assert_eq!(
            vllm,
            Some(vllm_value.to_string()),
            "vllm_config should be preserved from post-44 backup"
        );

        // Verify other columns were also preserved
        let spec: Option<String> = local_conn
            .query_row(
                "SELECT spec_decoding FROM model_configs WHERE repo_id = 'test/repo'",
                [],
                |row| row.get(0),
            )
            .expect("query spec_decoding");
        assert_eq!(spec, Some("{\"model\": \"spec\"}".to_string()));
    }
}
