//! Postgres database module (plan-190 Task 9: SQLite deleted).
//!
//! All query domains live in Postgres. The model registry loaders/savers
//! below are Postgres-based; pool creation/retry in [`pool`], migrations in
//! [`postgres`], and all typed query functions in [`queries`].

pub mod backfill;
pub mod pool;
pub mod postgres;
pub mod queries;

use std::collections::HashMap;

use sqlx::PgPool;

use crate::config::ModelConfig;

/// Load all model_configs rows and return them as a HashMap<config_key, ModelConfig>
/// where config_key is derived via `crate::models::ConfigKey::from_repo_id`.
///
/// NOTE: this is only used internally by the proxy to build its in-memory registry.
/// All external API lookups should use the integer `id` column directly.
///
/// The `model_files` fetch is batched into a single `WHERE model_id = ANY(...)`
/// query (the v2 code did one query per model — an N+1).
pub async fn load_model_configs(pool: &PgPool) -> anyhow::Result<HashMap<String, ModelConfig>> {
    let records = queries::get_all_model_configs(pool).await?;

    // Batch the model_files fetch (single round trip instead of N+1).
    let model_ids: Vec<i64> = records.iter().map(|r| r.id).collect();
    let all_files = queries::get_model_files_by_ids(pool, &model_ids).await?;
    let files_by_model: HashMap<i64, Vec<_>> =
        all_files.into_iter().fold(HashMap::new(), |mut map, file| {
            map.entry(file.model_id).or_default().push(file);
            map
        });

    let mut configs = HashMap::new();
    for record in records {
        let config_key = crate::models::ConfigKey::from_repo_id(&record.repo_id).to_string();
        let mut config = ModelConfig::from_db_record(&record);
        config.db_id = Some(record.id);

        // Populate quants from model_files table to restore them after restart
        if let Some(files) = files_by_model.get(&record.id) {
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
pub async fn save_model_config(
    pool: &PgPool,
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
    queries::upsert_model_config(pool, &record).await
}
