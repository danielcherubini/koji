//! Model configuration database query functions (Postgres, plan-190 Task 5).
//!
//! All functions are async and take a `&PgPool` — the caller owns the pool.
//!
//! Timestamps: `created_at`/`updated_at` are `TIMESTAMPTZ` in Postgres but the
//! shared [`ModelConfigRecord`] type (still used by the transitional SQLite
//! machinery until Task 9) stores them as `String` in the v2 format
//! `%Y-%m-%dT%H:%M:%fZ`. Reads therefore project the columns with
//! `to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')` so the
//! wire format is byte-identical to v2.
//!
//! Case-insensitive parity: v2 used `COLLATE NOCASE` on `repo_id` (the v1
//! squashed migration intentionally has no case-insensitive index). Lookup
//! and upsert duplicate logic are therefore made explicit with `lower()`.

use anyhow::Result;
use sqlx::{PgPool, Row};

use super::types::ModelConfigRecord;

/// Column list (39 non-`id` columns) in INSERT order.
const COLS: &str = "repo_id, display_name, backend, gpu_variant, gpu_device, enabled, \
     selected_quant, selected_mmproj, selected_mtp_model, context_length, num_parallel, \
     kv_unified, gpu_layers, cache_type_k, cache_type_v, port, args, sampling, modalities, \
     profile, api_name, health_check, hf_format, hf_base_model, hf_pipeline_tag, \
     hf_total_params, hf_active_params, hf_architecture_type, hf_context_length, \
     hf_num_layers, hf_last_modified, spec_decoding, created_at, updated_at, n_batch, \
     n_ubatch, vllm_config, provider_name, reasoning_levels";
const BY_ID_SQL: &str = "SELECT id, repo_id, display_name, backend, gpu_variant, gpu_device, \
     enabled, selected_quant, selected_mmproj, selected_mtp_model, context_length, num_parallel, \
     kv_unified, gpu_layers, cache_type_k, cache_type_v, port, args, sampling, modalities, profile, \
     api_name, health_check, hf_format, hf_base_model, hf_pipeline_tag, hf_total_params, \
     hf_active_params, hf_architecture_type, hf_context_length, hf_num_layers, hf_last_modified, \
     spec_decoding, \
     to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
     to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at, \
     n_batch, n_ubatch, vllm_config, provider_name, reasoning_levels FROM model_configs WHERE id = $1";
const BY_REPO_SQL: &str = "SELECT id, repo_id, display_name, backend, gpu_variant, gpu_device, \
     enabled, selected_quant, selected_mmproj, selected_mtp_model, context_length, num_parallel, \
     kv_unified, gpu_layers, cache_type_k, cache_type_v, port, args, sampling, modalities, profile, \
     api_name, health_check, hf_format, hf_base_model, hf_pipeline_tag, hf_total_params, \
     hf_active_params, hf_architecture_type, hf_context_length, hf_num_layers, hf_last_modified, \
     spec_decoding, \
     to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
     to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at, \
     n_batch, n_ubatch, vllm_config, provider_name, reasoning_levels FROM model_configs WHERE lower(repo_id) = lower($1) LIMIT 1";
const ALL_SQL: &str = "SELECT id, repo_id, display_name, backend, gpu_variant, gpu_device, \
     enabled, selected_quant, selected_mmproj, selected_mtp_model, context_length, num_parallel, \
     kv_unified, gpu_layers, cache_type_k, cache_type_v, port, args, sampling, modalities, profile, \
     api_name, health_check, hf_format, hf_base_model, hf_pipeline_tag, hf_total_params, \
     hf_active_params, hf_architecture_type, hf_context_length, hf_num_layers, hf_last_modified, \
     spec_decoding, \
     to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS created_at, \
     to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS updated_at, \
     n_batch, n_ubatch, vllm_config, provider_name, reasoning_levels FROM model_configs";

/// SET clauses (EXCLUDED form) applied when an existing row is updated on
/// upsert.
///
/// `repo_id` is intentionally absent (v2 parity: a conflicting row keeps its
/// stored case). HF metadata columns use COALESCE to preserve existing values
/// when the upsert record has NULL (e.g. during scan/pull which doesn't fetch
/// HF data). `reasoning_levels` is a plain overwrite: the editor always sends
/// a non-NULL array (`[]` to clear) and scan/pull upserts never populate it,
/// so clearing must work.
const UPSERT_SET: &str = "display_name = EXCLUDED.display_name,
     backend = EXCLUDED.backend,
     gpu_variant = EXCLUDED.gpu_variant,
     gpu_device = EXCLUDED.gpu_device,
     enabled = EXCLUDED.enabled,
     selected_quant = EXCLUDED.selected_quant,
     selected_mmproj = EXCLUDED.selected_mmproj,
     selected_mtp_model = EXCLUDED.selected_mtp_model,
     context_length = EXCLUDED.context_length,
     num_parallel = EXCLUDED.num_parallel,
     kv_unified = EXCLUDED.kv_unified,
     gpu_layers = EXCLUDED.gpu_layers,
     cache_type_k = EXCLUDED.cache_type_k,
     cache_type_v = EXCLUDED.cache_type_v,
     port = EXCLUDED.port,
     args = EXCLUDED.args,
     sampling = EXCLUDED.sampling,
     modalities = EXCLUDED.modalities,
     profile = EXCLUDED.profile,
     api_name = EXCLUDED.api_name,
     health_check = EXCLUDED.health_check,
     hf_format = COALESCE(EXCLUDED.hf_format, model_configs.hf_format),
     hf_base_model = COALESCE(EXCLUDED.hf_base_model, model_configs.hf_base_model),
     hf_pipeline_tag = COALESCE(EXCLUDED.hf_pipeline_tag, model_configs.hf_pipeline_tag),
     hf_total_params = COALESCE(EXCLUDED.hf_total_params, model_configs.hf_total_params),
     hf_active_params = COALESCE(EXCLUDED.hf_active_params, model_configs.hf_active_params),
     hf_architecture_type = COALESCE(EXCLUDED.hf_architecture_type, model_configs.hf_architecture_type),
     hf_context_length = COALESCE(EXCLUDED.hf_context_length, model_configs.hf_context_length),
     hf_num_layers = COALESCE(EXCLUDED.hf_num_layers, model_configs.hf_num_layers),
     hf_last_modified = COALESCE(EXCLUDED.hf_last_modified, model_configs.hf_last_modified),
     spec_decoding = EXCLUDED.spec_decoding,
     n_batch = EXCLUDED.n_batch,
     n_ubatch = EXCLUDED.n_ubatch,
     vllm_config = EXCLUDED.vllm_config,
     provider_name = EXCLUDED.provider_name,
     reasoning_levels = EXCLUDED.reasoning_levels,
     updated_at = now()";

/// Push the 39 bound record values in [`COLS`] order onto the builder.
fn push_record_values(qb: &mut sqlx::QueryBuilder<sqlx::Postgres>, record: &ModelConfigRecord) {
    qb.push_bind(&record.repo_id);
    qb.push(", ").push_bind(&record.display_name);
    qb.push(", ").push_bind(&record.backend);
    qb.push(", ").push_bind(&record.gpu_variant);
    qb.push(", ").push_bind(&record.gpu_device);
    qb.push(", ").push_bind(record.enabled);
    qb.push(", ").push_bind(&record.selected_quant);
    qb.push(", ").push_bind(&record.selected_mmproj);
    qb.push(", ").push_bind(&record.selected_mtp_model);
    qb.push(", ")
        .push_bind(record.context_length.map(|v| v as i64));
    qb.push(", ")
        .push_bind(record.num_parallel.map(|v| v as i64));
    qb.push(", ").push_bind(record.kv_unified);
    qb.push(", ").push_bind(record.gpu_layers.map(|v| v as i64));
    qb.push(", ").push_bind(&record.cache_type_k);
    qb.push(", ").push_bind(&record.cache_type_v);
    qb.push(", ").push_bind(record.port.map(|v| v as i64));
    qb.push(", ").push_bind(&record.args);
    qb.push(", ").push_bind(&record.sampling);
    qb.push(", ").push_bind(&record.modalities);
    qb.push(", ").push_bind(&record.profile);
    qb.push(", ").push_bind(&record.api_name);
    qb.push(", ").push_bind(&record.health_check);
    qb.push(", ").push_bind(&record.hf_format);
    qb.push(", ").push_bind(&record.hf_base_model);
    qb.push(", ").push_bind(&record.hf_pipeline_tag);
    qb.push(", ").push_bind(&record.hf_total_params);
    qb.push(", ").push_bind(&record.hf_active_params);
    qb.push(", ").push_bind(&record.hf_architecture_type);
    qb.push(", ")
        .push_bind(record.hf_context_length.map(|v| v as i64));
    qb.push(", ")
        .push_bind(record.hf_num_layers.map(|v| v as i64));
    qb.push(", ").push_bind(&record.hf_last_modified);
    qb.push(", ").push_bind(&record.spec_decoding);
    qb.push(", COALESCE(NULLIF(")
        .push_bind(&record.created_at)
        .push(", '')::timestamptz, now())");
    qb.push(", COALESCE(NULLIF(")
        .push_bind(&record.updated_at)
        .push(", '')::timestamptz, now())");
    qb.push(", ").push_bind(record.n_batch.map(|v| v as i64));
    qb.push(", ").push_bind(record.n_ubatch.map(|v| v as i64));
    qb.push(", ").push_bind(&record.vllm_config);
    qb.push(", ").push_bind(&record.provider_name);
    qb.push(", ").push_bind(&record.reasoning_levels);
}

/// Build `INSERT ... VALUES (record) ON CONFLICT (repo_id) DO UPDATE SET ...`.
///
/// The `id` column is omitted so the identity sequence assigns it for new
/// rows (explicit-id inserts do not advance sequences — v2 parity).
fn push_repo_upsert(qb: &mut sqlx::QueryBuilder<sqlx::Postgres>, record: &ModelConfigRecord) {
    qb.push("INSERT INTO model_configs (")
        .push(COLS)
        .push(") VALUES (");
    push_record_values(qb, record);
    qb.push(") ON CONFLICT (repo_id) DO UPDATE SET ")
        .push(UPSERT_SET);
}

/// Build `UPDATE model_configs SET <all 39 record columns> WHERE id = ...`.
///
/// Used for the case-insensitive branch: updates the pre-fetched row in
/// place, keeping its stored `repo_id` case (v2 parity — the ON CONFLICT
/// DO UPDATE list never rewrote repo_id).
fn push_id_update(
    qb: &mut sqlx::QueryBuilder<sqlx::Postgres>,
    record: &ModelConfigRecord,
    id: i64,
) {
    qb.push("UPDATE model_configs SET repo_id = ")
        .push_bind(&record.repo_id);
    qb.push(", display_name = ").push_bind(&record.display_name);
    qb.push(", backend = ").push_bind(&record.backend);
    qb.push(", gpu_variant = ").push_bind(&record.gpu_variant);
    qb.push(", gpu_device = ").push_bind(&record.gpu_device);
    qb.push(", enabled = ").push_bind(record.enabled);
    qb.push(", selected_quant = ")
        .push_bind(&record.selected_quant);
    qb.push(", selected_mmproj = ")
        .push_bind(&record.selected_mmproj);
    qb.push(", selected_mtp_model = ")
        .push_bind(&record.selected_mtp_model);
    qb.push(", context_length = ")
        .push_bind(record.context_length.map(|v| v as i64));
    qb.push(", num_parallel = ")
        .push_bind(record.num_parallel.map(|v| v as i64));
    qb.push(", kv_unified = ").push_bind(record.kv_unified);
    qb.push(", gpu_layers = ")
        .push_bind(record.gpu_layers.map(|v| v as i64));
    qb.push(", cache_type_k = ").push_bind(&record.cache_type_k);
    qb.push(", cache_type_v = ").push_bind(&record.cache_type_v);
    qb.push(", port = ")
        .push_bind(record.port.map(|v| v as i64));
    qb.push(", args = ").push_bind(&record.args);
    qb.push(", sampling = ").push_bind(&record.sampling);
    qb.push(", modalities = ").push_bind(&record.modalities);
    qb.push(", profile = ").push_bind(&record.profile);
    qb.push(", api_name = ").push_bind(&record.api_name);
    qb.push(", health_check = ").push_bind(&record.health_check);
    qb.push(", hf_format = COALESCE(")
        .push_bind(&record.hf_format)
        .push(", hf_format)");
    qb.push(", hf_base_model = COALESCE(")
        .push_bind(&record.hf_base_model)
        .push(", hf_base_model)");
    qb.push(", hf_pipeline_tag = COALESCE(")
        .push_bind(&record.hf_pipeline_tag)
        .push(", hf_pipeline_tag)");
    qb.push(", hf_total_params = COALESCE(")
        .push_bind(&record.hf_total_params)
        .push(", hf_total_params)");
    qb.push(", hf_active_params = COALESCE(")
        .push_bind(&record.hf_active_params)
        .push(", hf_active_params)");
    qb.push(", hf_architecture_type = COALESCE(")
        .push_bind(&record.hf_architecture_type)
        .push(", hf_architecture_type)");
    qb.push(", hf_context_length = COALESCE(")
        .push_bind(record.hf_context_length.map(|v| v as i64))
        .push(", hf_context_length)");
    qb.push(", hf_num_layers = COALESCE(")
        .push_bind(record.hf_num_layers.map(|v| v as i64))
        .push(", hf_num_layers)");
    qb.push(", hf_last_modified = COALESCE(")
        .push_bind(&record.hf_last_modified)
        .push(", hf_last_modified)");
    qb.push(", spec_decoding = ")
        .push_bind(&record.spec_decoding);
    qb.push(", n_batch = ")
        .push_bind(record.n_batch.map(|v| v as i64));
    qb.push(", n_ubatch = ")
        .push_bind(record.n_ubatch.map(|v| v as i64));
    qb.push(", vllm_config = ").push_bind(&record.vllm_config);
    qb.push(", provider_name = ")
        .push_bind(&record.provider_name);
    qb.push(", reasoning_levels = ")
        .push_bind(&record.reasoning_levels);
    // NOTE: repo_id above keeps the incoming casing — v2's NOCASE upsert
    // actually preserved the STORED casing. Preserve it too: the caller
    // passes the stored repo_id in `record` for this path.
    qb.push(", updated_at = now() WHERE id = ").push_bind(id);
}

/// Insert or update the model configuration.
///
/// v2 parity notes:
/// - `repo_id` was `COLLATE NOCASE` in SQLite. The Postgres unique index is
///   case-sensitive, so a row whose `repo_id` differs only in case is found
///   with an explicit `lower()` pre-check and updated in place (keeping the
///   stored case), instead of creating a duplicate row.
/// - `updated_at` is refreshed to `now()` on conflict.
///
/// Returns the model id (either existing or newly created).
pub async fn upsert_model_config(pool: &PgPool, record: &ModelConfigRecord) -> Result<i64> {
    // Case-insensitive pre-check (v2 `COLLATE NOCASE` upsert parity).
    let existing: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, repo_id FROM model_configs WHERE lower(repo_id) = lower($1) LIMIT 1",
    )
    .bind(&record.repo_id)
    .fetch_optional(pool)
    .await?;

    let id = match existing {
        Some((existing_id, existing_repo_id)) if existing_repo_id != record.repo_id => {
            // Case differs: update the existing row by id, preserving its
            // stored repo_id casing (v2 parity).
            let mut stored = record.clone();
            stored.repo_id = existing_repo_id;
            let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new("");
            push_id_update(&mut qb, &stored, existing_id);
            qb.build().execute(pool).await?;
            existing_id
        }
        _ => {
            // Exact match or no row: standard upsert on repo_id (the
            // ON CONFLICT arm covers concurrent inserts too).
            let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new("");
            push_repo_upsert(&mut qb, record);
            qb.build().execute(pool).await?;
            let row = sqlx::query("SELECT id FROM model_configs WHERE repo_id = $1")
                .bind(&record.repo_id)
                .fetch_one(pool)
                .await?;
            row.get("id")
        }
    };
    Ok(id)
}

/// Get the model configuration by id. Returns None if not found.
pub async fn get_model_config(pool: &PgPool, id: i64) -> Result<Option<ModelConfigRecord>> {
    let row = sqlx::query(BY_ID_SQL).bind(id).fetch_optional(pool).await?;
    Ok(row.map(|r| decode_model_config(&r)))
}

/// Get the model configuration by repo_id. Returns None if not found.
///
/// Case-insensitive (v2 `COLLATE NOCASE` parity): repo_id lookups matched
/// regardless of case in SQLite.
pub async fn get_model_config_by_repo_id(
    pool: &PgPool,
    repo_id: &str,
) -> Result<Option<ModelConfigRecord>> {
    let row = sqlx::query(BY_REPO_SQL)
        .bind(repo_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| decode_model_config(&r)))
}

/// Get all stored model configurations.
pub async fn get_all_model_configs(pool: &PgPool) -> Result<Vec<ModelConfigRecord>> {
    let rows = sqlx::query(ALL_SQL).fetch_all(pool).await?;
    Ok(rows.iter().map(decode_model_config).collect())
}

/// Delete the model configuration by id. CASCADE deletes model_pulls and model_files.
pub async fn delete_model_config(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM model_configs WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Decode a row selected in `SELECT_LIST` order into a record.
fn decode_model_config(row: &sqlx::postgres::PgRow) -> ModelConfigRecord {
    ModelConfigRecord {
        id: row.get("id"),
        repo_id: row.get("repo_id"),
        display_name: row.get("display_name"),
        backend: row.get("backend"),
        gpu_variant: row.get("gpu_variant"),
        gpu_device: row.get("gpu_device"),
        enabled: row.get("enabled"),
        selected_quant: row.get("selected_quant"),
        selected_mmproj: row.get("selected_mmproj"),
        selected_mtp_model: row.get("selected_mtp_model"),
        context_length: row
            .get::<Option<i64>, _>("context_length")
            .map(|v| v as u32),
        num_parallel: row.get::<Option<i64>, _>("num_parallel").map(|v| v as u32),
        kv_unified: row.get("kv_unified"),
        gpu_layers: row.get::<Option<i64>, _>("gpu_layers").map(|v| v as u32),
        cache_type_k: row.get("cache_type_k"),
        cache_type_v: row.get("cache_type_v"),
        port: row.get::<Option<i64>, _>("port").map(|v| v as u16),
        args: row.get("args"),
        sampling: row.get("sampling"),
        modalities: row.get("modalities"),
        profile: row.get("profile"),
        api_name: row.get("api_name"),
        health_check: row.get("health_check"),
        hf_format: row.get("hf_format"),
        hf_base_model: row.get("hf_base_model"),
        hf_pipeline_tag: row.get("hf_pipeline_tag"),
        hf_total_params: row.get("hf_total_params"),
        hf_active_params: row.get("hf_active_params"),
        hf_architecture_type: row.get("hf_architecture_type"),
        hf_context_length: row
            .get::<Option<i64>, _>("hf_context_length")
            .map(|v| v as u32),
        hf_num_layers: row.get::<Option<i64>, _>("hf_num_layers").map(|v| v as u32),
        hf_last_modified: row.get("hf_last_modified"),
        spec_decoding: row.get("spec_decoding"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        n_batch: row.get::<Option<i64>, _>("n_batch").map(|v| v as i32),
        n_ubatch: row.get::<Option<i64>, _>("n_ubatch").map(|v| v as i32),
        vllm_config: row.get("vllm_config"),
        provider_name: row.get("provider_name"),
        reasoning_levels: row.get("reasoning_levels"),
    }
}
