//! Unified TOML → SQLite migration.
//!
//! Migrates ALL sections from `config.toml` into the SQLite database in a single pass:
//! - `[backends]` → `backend_configs` table
//! - Global config (general, proxy, supervisor, compaction, sampling_templates) → `app_*` tables
//! - `[models]` → `model_configs` table (if present in TOML)
//!
//! After successful migration, `config.toml` is renamed to `config.toml.migrated`.
//!
//! This is **idempotent**: if `app_general` already has a row with `id = 1`, the migration
//! is skipped entirely.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing;

use crate::config::Config;
use crate::db::queries;

/// Result of the TOML → DB migration, with counts per section.
#[derive(Debug, Default)]
pub struct MigrationResult {
    /// Number of backend configs migrated from `[backends]` section.
    pub backends_migrated: usize,
    /// Number of models migrated from `[models]` section.
    pub models_migrated: usize,
    /// Whether global config (general, proxy, supervisor, compaction, sampling_templates) was migrated.
    pub global_migrated: bool,
    /// Whether the migration was skipped because the DB was already populated.
    pub already_migrated: bool,
}

/// Run the unified TOML → SQLite migration.
///
/// If `config.toml` exists in `config_dir`, all its sections are migrated into the SQLite DB
/// at `db_path`. The TOML file is then renamed to `config.toml.migrated` as a backup.
///
/// This function is **idempotent**: if `app_general` already has a row with `id = 1`, it returns
/// immediately with `MigrationResult { already_migrated: true, ..Default::default() }`.
///
/// # Arguments
/// * `config_dir` - Directory containing `config.toml`
/// * `db_path` - Path to the SQLite database file
///
/// # Returns
/// A `MigrationResult` with counts of migrated items per section.
pub fn migrate_toml_to_db(config_dir: &Path, db_path: &Path) -> Result<MigrationResult> {
    // ── Step 1: Open DB once, run migrations, idempotency check ────────
    // Use a single connection for the entire migration to avoid races.
    // Check idempotency FIRST — if the DB is already populated, skip
    // regardless of whether config.toml still exists (it may have been
    // renamed by a prior call).
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open DB at {}", db_path.display()))?;
    crate::db::migrations::run(&conn)?;

    let has_data: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM app_general WHERE id = 1",
        [],
        |row| row.get(0),
    )?;

    if has_data {
        tracing::info!("DB already populated — skipping TOML migration");
        return Ok(MigrationResult {
            already_migrated: true,
            ..Default::default()
        });
    }

    // ── Step 2: Read and parse config.toml ─────────────────────────────
    let config_path = config_dir.join("config.toml");
    if !config_path.exists() {
        tracing::info!("No config.toml found — nothing to migrate");
        return Ok(MigrationResult::default());
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;

    // Parse as Config for global + backends sections
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", config_path.display()))?;

    // Also parse as raw TOML value to extract [models] section
    let raw_value: toml::Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse TOML value from {}", config_path.display()))?;

    // ── Step 3: Seed defaults ──────────────────────────────────────────
    queries::seed_defaults(&conn)?;

    // ── Step 4: Migrate [backends] section → backend_configs ────────────
    let backends_migrated = migrate_backends_section(&conn, &config.backends)?;

    // ── Step 5: Migrate [models] section → model_configs ───────────────
    let models_migrated = migrate_models_section(&conn, &raw_value)?;

    // ── Step 6: Migrate global config → app_* tables ──────────────────
    migrate_global_config(&conn, &config)?;

    // ── Step 7: Rename config.toml → config.toml.migrated ─────────────
    let migrated_path = config_dir.join("config.toml.migrated");
    std::fs::rename(&config_path, &migrated_path).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            config_path.display(),
            migrated_path.display()
        )
    })?;

    tracing::info!(
        backends = backends_migrated,
        models = models_migrated,
        "TOML → DB migration complete"
    );

    Ok(MigrationResult {
        backends_migrated,
        models_migrated,
        global_migrated: true,
        already_migrated: false,
    })
}

/// Migrate the `[backends]` section from parsed Config into `backend_configs`.
fn migrate_backends_section(
    conn: &Connection,
    backends: &std::collections::HashMap<String, crate::config::BackendConfig>,
) -> Result<usize> {
    if backends.is_empty() {
        return Ok(0);
    }

    let mut count = 0usize;

    for (name, backend_config) in backends {
        let gpu_variant = backend_config
            .gpu_variant
            .clone()
            .unwrap_or_else(|| "cpu".to_string());

        queries::upsert_backend_config(conn, name, &gpu_variant, &[], &[], None)
            .with_context(|| format!("Failed to migrate backend config '{}'", name))?;
        count += 1;
    }

    tracing::info!("Migrated {} backend config(s) from [backends]", count);
    Ok(count)
}

/// Migrate the `[models]` section from raw TOML into `model_configs`.
///
/// Reads the raw TOML value to extract the `[models]` table, then attempts to
/// deserialize each entry as a `ModelConfig` and save it to the DB.
fn migrate_models_section(conn: &Connection, raw_value: &toml::Value) -> Result<usize> {
    let models_table = match raw_value.get("models").and_then(|v| v.as_table()) {
        Some(table) => table,
        None => {
            tracing::debug!("No [models] section in config.toml");
            return Ok(0);
        }
    };

    if models_table.is_empty() {
        return Ok(0);
    }

    // Collect all valid model configs first, then save — prevents partial migration.
    let mut all_configs: Vec<(String, crate::config::ModelConfig)> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();

    for (key, val) in models_table {
        match val.clone().try_into() {
            Ok(mc) => all_configs.push((key.to_string(), mc)),
            Err(e) => failed.push((key.to_string(), e.to_string())),
        }
    }

    if !failed.is_empty() {
        let errors: Vec<String> = failed
            .iter()
            .map(|(key, err)| format!("  {}: {}", key, err))
            .collect();
        anyhow::bail!(
            "Failed to migrate {} model(s) from [models]:\n{}",
            failed.len(),
            errors.join("\n")
        );
    }

    let migrated_count = all_configs.len();

    for (key, mc) in all_configs {
        crate::db::save_model_config(conn, &key, &mc)?;
    }

    if migrated_count > 0 {
        tracing::info!("Migrated {} model(s) from [models]", migrated_count);
    }

    Ok(migrated_count)
}

/// Migrate global config sections (general, proxy, supervisor, compaction, sampling_templates)
/// into their corresponding `app_*` tables.
fn migrate_global_config(conn: &Connection, config: &Config) -> Result<()> {
    // General
    queries::upsert_general(
        conn,
        &config.general.log_level,
        config.general.models_dir.as_deref(),
        config.general.logs_dir.as_deref(),
        config.general.hf_token.as_deref(),
        config.general.update_check_interval,
    )?;

    // Proxy
    queries::upsert_proxy(
        conn,
        &config.proxy.host,
        config.proxy.port,
        config.proxy.auto_unload,
        config.proxy.idle_timeout_secs,
        config.proxy.startup_timeout_secs,
        config.proxy.circuit_breaker_threshold,
        config.proxy.circuit_breaker_cooldown_seconds,
        config.proxy.metrics_retention_secs,
        config.proxy.download_queue_poll_interval_secs,
        config.proxy.max_loaded_models,
        config.proxy.authenticator_url.as_deref(),
        &config.proxy.authenticator_skip_paths,
    )?;

    // Supervisor
    queries::upsert_supervisor(
        conn,
        &config.supervisor.restart_policy,
        config.supervisor.max_restarts,
        config.supervisor.restart_delay_ms,
        config.supervisor.health_check_interval_ms,
        config.supervisor.health_check_timeout_ms,
        config.supervisor.health_check_retries,
    )?;

    // Compaction
    queries::upsert_compaction(
        conn,
        config.compaction.enabled,
        config.compaction.server_path.as_deref(),
        &config.compaction.device,
        config.compaction.port,
        config.compaction.request_timeout_ms,
    )?;

    // Sampling templates — use INSERT OR IGNORE to preserve any user-added
    // templates that might already exist in the DB (e.g. from seed_defaults).
    // Templates from TOML take precedence via upsert.
    for (name, params) in &config.sampling_templates {
        queries::upsert_sampling_template(
            conn,
            name,
            params.temperature,
            params.top_k,
            params.top_p,
            params.min_p,
            params.presence_penalty,
            params.frequency_penalty,
            params.repeat_penalty,
        )?;
    }

    tracing::info!(
        "Migrated global config (general, proxy, supervisor, compaction, sampling_templates)"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::OpenResult;
    use tempfile::tempdir;

    /// Helper: create a temp dir with a config.toml containing backends and global config.
    fn create_test_config_toml(
        dir: &Path,
        log_level: &str,
        backends: &[&str],
    ) -> std::path::PathBuf {
        let mut toml_content = format!("[general]\nlog_level = \"{log_level}\"\n");

        for backend_name in backends {
            toml_content.push_str(&format!(
                r#"
[backends.{backend_name}]
gpu_variant = "cpu"
"#
            ));
        }

        let config_path = dir.join("config.toml");
        std::fs::write(&config_path, &toml_content).unwrap();
        config_path
    }

    /// Test that migrate_toml_to_db migrates backends and global config from config.toml.
    #[test]
    fn test_migrate_toml_to_db_basic() {
        let tmp = tempdir().unwrap();
        let config_dir = tmp.path();
        let db_path = config_dir.join("tama.db");

        create_test_config_toml(config_dir, "debug", &["llama_cpp", "ik_llama"]);

        let result = migrate_toml_to_db(config_dir, &db_path).unwrap();

        // Verify migration succeeded
        assert!(!result.already_migrated);
        assert!(result.global_migrated);
        assert_eq!(result.backends_migrated, 2);
        assert_eq!(result.models_migrated, 0);

        // Verify DB has backend configs by opening the file-based DB
        let OpenResult { conn, .. } = crate::db::open(config_dir).unwrap();
        let llama = queries::get_backend_config(&conn, "llama_cpp", "cpu")
            .unwrap()
            .expect("llama_cpp should exist in DB after migration");
        assert_eq!(llama.name, "llama_cpp");
        let ik = queries::get_backend_config(&conn, "ik_llama", "cpu")
            .unwrap()
            .expect("ik_llama should exist in DB after migration");
        assert_eq!(ik.name, "ik_llama");

        // Verify config.toml was renamed
        assert!(!config_dir.join("config.toml").exists());
        assert!(config_dir.join("config.toml.migrated").exists());
    }

    /// Test that migrate_toml_to_db is idempotent — calling twice doesn't error.
    #[test]
    fn test_migrate_toml_to_db_idempotent() {
        let tmp = tempdir().unwrap();
        let config_dir = tmp.path();
        let db_path = config_dir.join("tama.db");

        create_test_config_toml(config_dir, "info", &["llama_cpp"]);

        // First call migrates
        let result1 = migrate_toml_to_db(config_dir, &db_path).unwrap();
        assert!(!result1.already_migrated);
        assert_eq!(result1.backends_migrated, 1);

        // Second call should be a no-op (already_migrated = true)
        let result2 = migrate_toml_to_db(config_dir, &db_path).unwrap();
        assert!(result2.already_migrated);
        assert_eq!(result2.backends_migrated, 0);
    }

    /// Test that migrate_toml_to_db returns Ok with no-op when config.toml doesn't exist.
    #[test]
    fn test_migrate_toml_to_db_no_config_file() {
        let tmp = tempdir().unwrap();
        let config_dir = tmp.path();
        let db_path = config_dir.join("tama.db");

        let result = migrate_toml_to_db(config_dir, &db_path).unwrap();

        assert!(!result.already_migrated);
        assert_eq!(result.backends_migrated, 0);
        assert_eq!(result.models_migrated, 0);
        assert!(!result.global_migrated);
    }

    /// Test that migrate_toml_to_db migrates [models] section from raw TOML.
    #[test]
    fn test_migrate_toml_to_db_with_models() {
        let tmp = tempdir().unwrap();
        let config_dir = tmp.path();
        let db_path = config_dir.join("tama.db");

        // Write a config.toml with [models] section
        let config_path = config_dir.join("config.toml");
        let toml_content = r#"
[general]
log_level = "info"

[models]
model1 = { backend = "llama_cpp", enabled = true }
model2 = { backend = "llama_cpp", enabled = false }
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = migrate_toml_to_db(config_dir, &db_path).unwrap();

        assert_eq!(result.models_migrated, 2);
        assert_eq!(result.backends_migrated, 0);

        // Verify model_configs in DB
        let OpenResult { conn, .. } = crate::db::open(config_dir).unwrap();
        let models = queries::get_all_model_configs(&conn).unwrap();
        assert_eq!(models.len(), 2);
    }

    /// Test that migrate_toml_to_db migrates all global config sections.
    #[test]
    fn test_migrate_toml_to_db_global_config() {
        let tmp = tempdir().unwrap();
        let config_dir = tmp.path();
        let db_path = config_dir.join("tama.db");

        let config_path = config_dir.join("config.toml");
        let toml_content = r#"
[general]
log_level = "debug"
models_dir = "/data/models"
update_check_interval = 24

[proxy]
host = "127.0.0.1"
port = 8080
auto_unload = true
idle_timeout_secs = 600

[supervisor]
restart_policy = "on-failure"
max_restarts = 5

[compaction]
enabled = true
device = "cuda"
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = migrate_toml_to_db(config_dir, &db_path).unwrap();

        assert!(result.global_migrated);

        // Verify general was migrated correctly
        let OpenResult { conn, .. } = crate::db::open(config_dir).unwrap();
        let general = queries::get_general(&conn).unwrap().unwrap();
        assert_eq!(general.log_level, "debug");
        assert_eq!(general.models_dir, Some("/data/models".to_string()));
        assert_eq!(general.update_check_interval, 24);

        // Verify proxy was migrated
        let proxy = queries::get_proxy(&conn).unwrap().unwrap();
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 8080);
        assert!(proxy.auto_unload);
        assert_eq!(proxy.idle_timeout_secs, 600);

        // Verify supervisor was migrated
        let supervisor = queries::get_supervisor(&conn).unwrap().unwrap();
        assert_eq!(supervisor.restart_policy, "on-failure");
        assert_eq!(supervisor.max_restarts, 5);

        // Verify compaction was migrated
        let compaction = queries::get_compaction(&conn).unwrap().unwrap();
        assert!(compaction.enabled);
        assert_eq!(compaction.device, "cuda");

        // Verify config.toml was renamed
        assert!(!config_dir.join("config.toml").exists());
        assert!(config_dir.join("config.toml.migrated").exists());
    }

    /// Test that migrate_toml_to_db with an empty [backends] section is a no-op for backends.
    #[test]
    fn test_migrate_toml_to_db_empty_backends() {
        let tmp = tempdir().unwrap();
        let config_dir = tmp.path();
        let db_path = config_dir.join("tama.db");

        let config_path = config_dir.join("config.toml");
        let toml_content = r#"
[general]
log_level = "info"

[backends]
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let result = migrate_toml_to_db(config_dir, &db_path).unwrap();

        assert_eq!(result.backends_migrated, 0);
        assert!(result.global_migrated);
    }
}
