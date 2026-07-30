use anyhow::Result;

/// Backfill HF metadata columns for existing models that have NULL values.
///
/// After migration v19 runs, existing model_configs rows have NULL for all 9
/// new columns. This function fetches metadata from the HuggingFace API for
/// each affected model and populates the columns.
///
/// Designed to run once on startup after migration, then be a no-op on
/// subsequent startups (no rows match `hf_format IS NULL`).
///
/// Failures for individual models are logged as warnings — the backfill
/// continues for remaining models even if some fail. A 200ms delay between
/// API calls avoids rate limiting.
///
/// Takes a `db_dir` path (not a `&Connection`) so it can be called from a
/// `tokio::spawn` task. Opens its own connection internally.
pub async fn backfill_hf_metadata(db_dir: &std::path::Path) -> Result<()> {
    // Open DB and read models needing backfill via spawn_blocking (keeps future Send)
    let db_dir_clone = db_dir.to_path_buf();
    let models: Vec<(i64, String)> =
        tokio::task::spawn_blocking(move || -> Result<Vec<(i64, String)>> {
            let open_result = crate::db::open(&db_dir_clone)?;
            let conn = &open_result.conn;
            let models: Vec<(i64, String)> = conn
                .prepare("SELECT id, repo_id FROM model_configs WHERE hf_format IS NULL")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            // Connection dropped at end of closure
            Ok(models)
        })
        .await??;

    if models.is_empty() {
        tracing::debug!("No models need HF metadata backfill");
        return Ok(());
    }

    let total = models.len();
    tracing::info!("Backfilling HF metadata for {} model(s)", total);

    // ── Phase 1: Fetch metadata for all models (async) ──────────────────────
    let mut updates: Vec<(i64, String, crate::models::pull::HfModelMetadata)> = Vec::new();
    for (i, (model_id, repo_id)) in models.iter().enumerate() {
        tracing::info!(
            "[{}/{}] Fetching HF metadata for {}...",
            i + 1,
            total,
            repo_id
        );

        let meta = match crate::models::pull::lookup_hf_metadata(repo_id).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Failed to fetch HF metadata for '{}': {}", repo_id, e);
                continue;
            }
        };

        updates.push((*model_id, repo_id.clone(), meta));

        // Small delay between API calls to avoid rate limiting
        if i + 1 < total {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    // ── Phase 2: Write all updates in a single connection (sync) ─────────────
    if !updates.is_empty() {
        let db_dir_clone = db_dir.to_path_buf();
        let update_result = tokio::task::spawn_blocking(move || -> Result<()> {
            let open_result = crate::db::open(&db_dir_clone)?;
            for (mid, repo_id, meta) in updates {
                if let Err(e) = crate::models::update::update_model_config_hf_metadata(
                    &open_result.conn,
                    mid,
                    &meta,
                ) {
                    tracing::warn!(
                        "Failed to update HF metadata for '{}' (id={}): {}",
                        repo_id,
                        mid,
                        e
                    );
                }
            }
            Ok(())
        })
        .await?;

        if let Err(e) = update_result {
            tracing::warn!("HF metadata backfill DB write failed: {}", e);
        }
    }

    tracing::info!("HF metadata backfill complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::OpenResult;

    /// Test that backfill_hf_metadata runs without crashing when there are models
    /// with NULL hf_format. In tests, the HF API calls will fail (no network),
    /// but the function should handle failures gracefully and return Ok.
    #[tokio::test]
    async fn test_backfill_hf_metadata_no_crash_with_null_rows() {
        use crate::db::queries::{upsert_model_config, ModelConfigRecord};

        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();
        let now = "2026-05-03T00:00:00Z".to_string();

        // Insert a model_config row with NULL hf_format (simulating post-migration state)
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let record = ModelConfigRecord {
                id: 0,
                repo_id: "test/repo".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
                gpu_variant: None,
                gpu_device: None,
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                selected_mtp_model: None,
                context_length: None,
                num_parallel: Some(1),
                kv_unified: false,
                gpu_layers: None,
                cache_type_k: None,
                cache_type_v: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                hf_format: None,
                hf_base_model: None,
                hf_pipeline_tag: None,
                hf_total_params: None,
                hf_active_params: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                hf_last_modified: None,
                spec_decoding: None,
                created_at: now.clone(),
                updated_at: now,
                n_batch: None,
                n_ubatch: None,
            };
            upsert_model_config(&conn, &record).unwrap();

            // Verify the row exists with NULL hf_format
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM model_configs WHERE hf_format IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }

        // Run backfill — HF API calls will fail (no network in tests),
        // but the function should handle failures gracefully and return Ok
        let result = backfill_hf_metadata(&db_dir).await;
        assert!(
            result.is_ok(),
            "backfill should not crash even when HF API fails"
        );

        // hf_format will still be NULL since the fetch failed (expected in tests)
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM model_configs WHERE hf_format IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "hf_format should still be NULL after failed fetch"
            );
        }
    }

    /// Test that backfill_hf_metadata is a no-op when all models already have hf_format.
    #[tokio::test]
    async fn test_backfill_hf_metadata_noop_when_all_populated() {
        use crate::db::queries::{upsert_model_config, ModelConfigRecord};

        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();
        let now = "2026-05-03T00:00:00Z".to_string();

        // Insert a model_config row with hf_format already set
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let record = ModelConfigRecord {
                id: 0,
                repo_id: "test/repo".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
                gpu_variant: None,
                gpu_device: None,
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                selected_mtp_model: None,
                context_length: None,
                num_parallel: Some(1),
                kv_unified: false,
                gpu_layers: None,
                cache_type_k: None,
                cache_type_v: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                hf_format: Some("gguf".to_string()),
                hf_base_model: None,
                hf_pipeline_tag: None,
                hf_total_params: None,
                hf_active_params: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                hf_last_modified: None,
                spec_decoding: None,
                created_at: now.clone(),
                updated_at: now,
                n_batch: None,
                n_ubatch: None,
            };
            upsert_model_config(&conn, &record).unwrap();
        }

        // Run backfill — should be a no-op (no models need backfill)
        let result = backfill_hf_metadata(&db_dir).await;
        assert!(result.is_ok());
    }

    /// Test that backfill_hf_metadata returns Ok with an empty DB.
    #[tokio::test]
    async fn test_backfill_hf_metadata_empty_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        // Create the DB (empty, just migrations)
        let OpenResult { .. } = crate::db::open(&db_dir).unwrap();

        let result = backfill_hf_metadata(&db_dir).await;
        assert!(result.is_ok());
    }
}
