//! Database migrations for SQLite
//!
//! Uses SQLite's `PRAGMA user_version` to track schema version.
//! Each migration lives in its own file under `migrations/` and runs in its own transaction.

mod _0001_create_initial_tables;
mod _0002_create_active_models;
mod _0003_create_backend_installations;
mod _0004_create_system_metrics;
mod _0005_add_verification_columns;
mod _0006_create_update_checks;
mod _0007_create_model_configs;
mod _0008_rebuild_model_tables;
mod _0009_rebuild_model_configs_nocase;
mod _0010_add_model_pulls_unique_index;
mod _0011_create_download_queue;
mod _0012_add_queue_quant_context;
mod _0013_create_benchmarks;
mod _0014_add_num_parallel;
mod _0015_create_tts_configs;
mod _0016_add_benchmark_type;
mod _0017_add_kv_unified;
mod _0018_add_cache_types;
mod _0019_add_hf_metadata;
mod _0020_rebuild_backend_installations;
mod _0021_add_model_gpu_variant;
mod _0022_add_inference_metrics;
mod _0023_create_backend_configs;
mod _0024_add_spec_decoding;
mod _0025_create_last_used_model;
mod _0026_rebuild_model_configs_auto_parallel;
mod _0027_create_model_aliases;
mod _0028_add_selected_mtp_model;

#[cfg(test)]
mod migrations_tests;

use rusqlite::Connection;

/// RAII guard that re-enables SQLite foreign keys on drop.
///
/// Used around migrations that temporarily disable FK enforcement
/// (e.g., migration v9 which rebuilds `model_configs` via DROP + RENAME).
/// Ensures `PRAGMA foreign_keys=ON` runs even if the migration panics or
/// returns an error, preventing permanent FK disabling.
pub struct FkGuard<'conn> {
    conn: &'conn Connection,
}

impl<'conn> FkGuard<'conn> {
    /// Disable foreign keys and return a guard that re-enables them on Drop.
    pub fn disable(conn: &'conn Connection) -> anyhow::Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
        Ok(Self { conn })
    }
}

impl Drop for FkGuard<'_> {
    fn drop(&mut self) {
        // Ignore errors — best effort to restore FK state.
        let _ = self.conn.execute_batch("PRAGMA foreign_keys=ON;");
    }
}

/// A single migration: (version, requires_fk_off, sql)
type Migration = (i32, bool, &'static str);

/// Version number for the latest migration
pub const LATEST_VERSION: i32 = 28;

/// All migrations collected from individual files, sorted by version.
const MIGRATIONS: &[Migration] = &[
    _0001_create_initial_tables::MIGRATION,
    _0002_create_active_models::MIGRATION,
    _0003_create_backend_installations::MIGRATION,
    _0004_create_system_metrics::MIGRATION,
    _0005_add_verification_columns::MIGRATION,
    _0006_create_update_checks::MIGRATION,
    _0007_create_model_configs::MIGRATION,
    _0008_rebuild_model_tables::MIGRATION,
    _0009_rebuild_model_configs_nocase::MIGRATION,
    _0010_add_model_pulls_unique_index::MIGRATION,
    _0011_create_download_queue::MIGRATION,
    _0012_add_queue_quant_context::MIGRATION,
    _0013_create_benchmarks::MIGRATION,
    _0014_add_num_parallel::MIGRATION,
    _0015_create_tts_configs::MIGRATION,
    _0016_add_benchmark_type::MIGRATION,
    _0017_add_kv_unified::MIGRATION,
    _0018_add_cache_types::MIGRATION,
    _0019_add_hf_metadata::MIGRATION,
    _0020_rebuild_backend_installations::MIGRATION,
    _0021_add_model_gpu_variant::MIGRATION,
    _0022_add_inference_metrics::MIGRATION,
    _0023_create_backend_configs::MIGRATION,
    _0024_add_spec_decoding::MIGRATION,
    _0025_create_last_used_model::MIGRATION,
    _0026_rebuild_model_configs_auto_parallel::MIGRATION,
    _0027_create_model_aliases::MIGRATION,
    _0028_add_selected_mtp_model::MIGRATION,
];

/// Run all applicable migrations on the database
///
/// Reads current `user_version`, applies any migrations with a higher version number.
/// Each individual migration runs in its own transaction. After each successful
/// migration, updates `user_version` to that migration's version.
pub fn run(conn: &Connection) -> anyhow::Result<()> {
    run_up_to(conn, i32::MAX)
}

/// Run migrations only up to (and including) `target_version`. Primarily for
/// tests that need to simulate a pre-release schema (e.g. insert rows against
/// the v8 schema before running v9 to verify FK cascade behaviour).
pub(crate) fn run_up_to(conn: &Connection, target_version: i32) -> anyhow::Result<()> {
    let current_version: i32 =
        conn.pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))?;

    for (version, fk_off, sql) in MIGRATIONS {
        if *version > current_version && *version <= target_version {
            // PRAGMA foreign_keys must be toggled outside any transaction —
            // it is a no-op inside one. For rebuild-style migrations we need
            // FKs off so DROP TABLE on the parent does not cascade-delete
            // rows in referencing tables.
            let _fk_guard = if *fk_off {
                Some(FkGuard::disable(conn)?)
            } else {
                None
            };
            // Run each migration in its own transaction so a crash mid-migration
            // leaves the DB in a consistent state (user_version only updates on commit).
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {version};"))?;
            tx.commit()?;
            tracing::debug!("Applied migration to version {}", version);
        }
    }

    Ok(())
}
