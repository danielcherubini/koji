//! Model configuration database query functions.

use anyhow::Result;
use rusqlite::{params, Connection};

use super::types::ModelConfigRecord;

/// Insert or update the model configuration.
/// Timestamp updated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now') on conflict.
/// Returns the model id.
pub fn upsert_model_config(conn: &Connection, record: &ModelConfigRecord) -> Result<i64> {
    conn.execute(
        &format!(
            "INSERT INTO model_configs ({}) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
                ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36
            )
             ON CONFLICT(repo_id) DO UPDATE SET
                 display_name = excluded.display_name,
                 backend = excluded.backend,
                 gpu_variant = excluded.gpu_variant,
                 gpu_device = excluded.gpu_device,
                 enabled = excluded.enabled,
                 selected_quant = excluded.selected_quant,
                 selected_mmproj = excluded.selected_mmproj,
                 selected_mtp_model = excluded.selected_mtp_model,
                 context_length = excluded.context_length,
                 num_parallel = excluded.num_parallel,
                 kv_unified = excluded.kv_unified,
                 gpu_layers = excluded.gpu_layers,
                 cache_type_k = excluded.cache_type_k,
                 cache_type_v = excluded.cache_type_v,
                 port = excluded.port,
                 args = excluded.args,
                 sampling = excluded.sampling,
                 modalities = excluded.modalities,
                 profile = excluded.profile,
                 api_name = excluded.api_name,
                 health_check = excluded.health_check,
                 /* HF metadata: use COALESCE to preserve existing values when the
                    upsert record has NULL (e.g. during scan/pull which doesn't fetch HF data) */
                 hf_format = COALESCE(excluded.hf_format, hf_format),
                 hf_base_model = COALESCE(excluded.hf_base_model, hf_base_model),
                 hf_pipeline_tag = COALESCE(excluded.hf_pipeline_tag, hf_pipeline_tag),
                 hf_total_params = COALESCE(excluded.hf_total_params, hf_total_params),
                 hf_active_params = COALESCE(excluded.hf_active_params, hf_active_params),
                 hf_architecture_type = COALESCE(excluded.hf_architecture_type, hf_architecture_type),
                 hf_context_length = COALESCE(excluded.hf_context_length, hf_context_length),
                 hf_num_layers = COALESCE(excluded.hf_num_layers, hf_num_layers),
                 hf_last_modified = COALESCE(excluded.hf_last_modified, hf_last_modified),
                 spec_decoding = excluded.spec_decoding,
                 n_batch = excluded.n_batch,
                 n_ubatch = excluded.n_ubatch,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            ModelConfigRecord::INSERT_COLUMNS,
        ),
        params![
            record.repo_id,
            record.display_name,
            record.backend,
            record.gpu_variant,
            record.gpu_device,
            record.enabled as i32,
            record.selected_quant,
            record.selected_mmproj,
            record.selected_mtp_model,
            record.context_length,
            record.num_parallel,
            record.kv_unified as i32,
            record.gpu_layers,
            record.cache_type_k,
            record.cache_type_v,
            record.port,
            record.args,
            record.sampling,
            record.modalities,
            record.profile,
            record.api_name,
            record.health_check,
            record.hf_format,
            record.hf_base_model,
            record.hf_pipeline_tag,
            record.hf_total_params,
            record.hf_active_params,
            record.hf_architecture_type,
            record.hf_context_length,
            record.hf_num_layers,
            record.hf_last_modified,
            record.spec_decoding,
            record.created_at,
            record.updated_at,
            record.n_batch,
            record.n_ubatch,
        ],
    )?;
    // Return the id (either existing or newly created)
    let id: i64 = conn.query_row(
        "SELECT id FROM model_configs WHERE repo_id = ?1",
        [&record.repo_id],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Get the model configuration by id. Returns None if not found.
pub fn get_model_config(conn: &Connection, id: i64) -> Result<Option<ModelConfigRecord>> {
    let sql = format!(
        "SELECT {} FROM model_configs WHERE id = ?1",
        ModelConfigRecord::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([id], ModelConfigRecord::from_row)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Get the model configuration by repo_id. Returns None if not found.
pub fn get_model_config_by_repo_id(
    conn: &Connection,
    repo_id: &str,
) -> Result<Option<ModelConfigRecord>> {
    let sql = format!(
        "SELECT {} FROM model_configs WHERE repo_id = ?1",
        ModelConfigRecord::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([repo_id], ModelConfigRecord::from_row)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Get all stored model configurations.
pub fn get_all_model_configs(conn: &Connection) -> Result<Vec<ModelConfigRecord>> {
    let sql = format!("SELECT {} FROM model_configs", ModelConfigRecord::COLUMNS);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], ModelConfigRecord::from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete the model configuration by id. CASCADE deletes model_pulls and model_files.
pub fn delete_model_config(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM model_configs WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::db::queries::types::ModelConfigRecord;

    #[test]
    fn test_model_config_columns_match_insert_columns() {
        let select: Vec<&str> = ModelConfigRecord::COLUMNS
            .split(',')
            .map(str::trim)
            .collect();
        let insert: Vec<&str> = ModelConfigRecord::INSERT_COLUMNS
            .split(',')
            .map(str::trim)
            .collect();
        assert_eq!(select.len(), 37);
        assert_eq!(insert.len(), 36);
        assert_eq!(select[0], "id");
        assert_eq!(&select[1..], insert.as_slice());
    }
}
