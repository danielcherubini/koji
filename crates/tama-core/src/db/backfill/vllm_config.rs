//! Backfill `vllm_config` column from `args` for existing safetensors models.
//!
//! Existing models may carry vLLM flags inside their `args` column. This
//! backfill extracts the 8 managed flags into `vllm_config` and strips them
//! from `args`, preventing duplication when the new columns are used.

use anyhow::Result;

/// Backfill `vllm_config` from `args` for models that carry managed vLLM flags.
///
/// Gate: rows whose `args` JSON contains at least one managed vLLM flag.
/// When a row already has a non-empty `vllm_config`, existing column values
/// win per-field; extracted values only fill fields that are `None`/`false`.
///
/// Takes a `db_dir` path (not a `&Connection`) so it can be called from a
/// `tokio::task::spawn_blocking` task. Opens its own connection internally.
pub fn backfill_vllm_config(db_dir: &std::path::Path) -> Result<()> {
    let conn = crate::db::open(db_dir)?.conn;

    // Find rows whose args contain at least one managed flag.
    // We check for the flag strings in the JSON representation of args.
    let managed_flags = [
        "--quantization",
        "--kv-cache-dtype",
        "--tensor-parallel-size",
        "--gpu-memory-utilization",
        "--max-model-len",
        "--max-num-batched-tokens",
        "--enable-prefix-caching",
        "--trust-remote-code",
    ];

    // Build a query that matches any managed flag in the args column.
    let conditions: Vec<String> = managed_flags
        .iter()
        .map(|f| format!("args LIKE '%{}%'", f))
        .collect();
    let where_clause = conditions.join(" OR ");

    let sql = format!(
        "SELECT id, repo_id, args, vllm_config FROM model_configs WHERE ({}) AND hf_format = 'transformers'",
        where_clause
    );

    let mut stmt = conn.prepare(&sql)?;
    // Collect rows into a Vec first — we update the same table during iteration.
    let rows: Vec<_> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut migrated = 0;
    let mut failed = 0;
    for (id, repo_id, args_json, existing_vllm_json) in rows {
        // Parse args JSON
        let args: Vec<String> = match args_json.as_deref() {
            Some(json) => match serde_json::from_str(json) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!(
                        model_id = id,
                        repo_id = %repo_id,
                        "Failed to parse args JSON during vLLM config backfill: {}", e
                    );
                    failed += 1;
                    continue;
                }
            },
            None => continue,
        };

        // Extract vLLM flags
        let (extracted, stripped_args) = crate::config::extract_vllm_args(&args);

        // Skip if nothing was extracted
        if extracted.is_empty() {
            continue;
        }

        // Merge with existing vllm_config (existing non-default values win)
        let merged = match existing_vllm_json.as_deref() {
            Some(json) => match serde_json::from_str::<crate::config::types::VllmConfig>(json) {
                Ok(existing) => merge_vllm_config(&existing, &extracted),
                Err(e) => {
                    tracing::warn!(
                        model_id = id,
                        repo_id = %repo_id,
                        "Failed to parse existing vllm_config JSON during backfill: {}. Using extracted config instead.", e
                    );
                    extracted
                }
            },
            None => extracted,
        };

        // Serialize the merged config and stripped args
        let merged_json = match serde_json::to_string(&merged) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                    model_id = id,
                    repo_id = %repo_id,
                    "Failed to serialize merged vllm_config during backfill: {}", e
                );
                failed += 1;
                continue;
            }
        };
        let stripped_args_json = match serde_json::to_string(&stripped_args) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                    model_id = id,
                    repo_id = %repo_id,
                    "Failed to serialize stripped args during backfill: {}", e
                );
                failed += 1;
                continue;
            }
        };

        // Update the row
        if let Err(e) = conn.execute(
            "UPDATE model_configs SET vllm_config = ?1, args = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?3",
            rusqlite::params![merged_json, stripped_args_json, id],
        ) {
            tracing::warn!(
                model_id = id,
                repo_id = %repo_id,
                "Failed to update row during vLLM config backfill: {}", e
            );
            failed += 1;
            continue;
        }

        migrated += 1;
        tracing::info!(
            model_id = id,
            repo_id = %repo_id,
            "Migrated vLLM flags from args to vllm_config"
        );
    }

    if migrated > 0 || failed > 0 {
        tracing::info!(
            migrated = migrated,
            failed = failed,
            "vLLM config backfill complete"
        );
    } else {
        tracing::debug!("No models need vLLM config backfill");
    }

    Ok(())
}

/// Merge two VllmConfigs: `existing` non-default values win per-field.
fn merge_vllm_config(
    existing: &crate::config::types::VllmConfig,
    extracted: &crate::config::types::VllmConfig,
) -> crate::config::types::VllmConfig {
    use crate::config::types::VllmConfig;

    VllmConfig {
        quantization: existing
            .quantization
            .clone()
            .or_else(|| extracted.quantization.clone()),
        kv_cache_dtype: existing
            .kv_cache_dtype
            .clone()
            .or_else(|| extracted.kv_cache_dtype.clone()),
        tensor_parallel_size: existing
            .tensor_parallel_size
            .or(extracted.tensor_parallel_size),
        gpu_memory_utilization: existing
            .gpu_memory_utilization
            .or(extracted.gpu_memory_utilization),
        max_model_len: existing.max_model_len.or(extracted.max_model_len),
        max_num_batched_tokens: existing
            .max_num_batched_tokens
            .or(extracted.max_num_batched_tokens),
        enable_prefix_caching: existing.enable_prefix_caching || extracted.enable_prefix_caching,
        trust_remote_code: existing.trust_remote_code || extracted.trust_remote_code,
        attention_backend: existing
            .attention_backend
            .clone()
            .or_else(|| extracted.attention_backend.clone()),
        spec_decoding: merge_vllm_spec_decoding(&existing.spec_decoding, &extracted.spec_decoding),
    }
}

/// Merge two VllmSpecConfigs: `existing` non-default values win per-field.
fn merge_vllm_spec_decoding(
    existing: &crate::config::types::VllmSpecConfig,
    extracted: &crate::config::types::VllmSpecConfig,
) -> crate::config::types::VllmSpecConfig {
    use crate::config::types::VllmSpecConfig;

    VllmSpecConfig {
        method: existing.method.clone().or_else(|| extracted.method.clone()),
        model: existing.model.clone().or_else(|| extracted.model.clone()),
        num_speculative_tokens: existing
            .num_speculative_tokens
            .or(extracted.num_speculative_tokens),
        rejection_sample_method: existing
            .rejection_sample_method
            .clone()
            .or_else(|| extracted.rejection_sample_method.clone()),
        draft_tensor_parallel_size: existing
            .draft_tensor_parallel_size
            .or(extracted.draft_tensor_parallel_size),
        draft_sample_method: existing
            .draft_sample_method
            .clone()
            .or_else(|| extracted.draft_sample_method.clone()),
        disable_padded_drafter_batch: existing
            .disable_padded_drafter_batch
            .or(extracted.disable_padded_drafter_batch),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{upsert_model_config, ModelConfigRecord};
    use crate::db::OpenResult;

    fn make_record(
        id: i64,
        repo_id: &str,
        args: Option<Vec<String>>,
        vllm_config: Option<crate::config::types::VllmConfig>,
    ) -> ModelConfigRecord {
        let now = "2026-05-03T00:00:00Z".to_string();
        let args_json = args.map(|a| serde_json::to_string(&a).unwrap());
        let vllm_json = vllm_config.map(|v| serde_json::to_string(&v).unwrap());

        ModelConfigRecord {
            id,
            repo_id: repo_id.to_string(),
            display_name: None,
            backend: "vllm".to_string(),
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
            args: args_json,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            hf_format: Some("transformers".to_string()),
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
            vllm_config: vllm_json,
        }
    }

    /// Row with grouped vLLM args → vllm_config populated, args stripped.
    #[test]
    fn test_backfill_grouped_args() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let args = vec![
                "--quantization fp8".to_string(),
                "--kv-cache-dtype fp8".to_string(),
                "--tensor-parallel-size 2".to_string(),
                "--gpu-memory-utilization 0.92".to_string(),
                "--attention-backend ROCM_AITER_UNIFIED_ATTN".to_string(),
                "--max-num-batched-tokens 2560".to_string(),
                "--enable-prefix-caching".to_string(),
            ];
            let record = make_record(1, "test/model1", Some(args), None);
            upsert_model_config(&conn, &record).unwrap();
        }

        // Run backfill
        backfill_vllm_config(&db_dir).unwrap();

        // Verify
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let row: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 1",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();

            // vllm_config should have the extracted values
            let vllm: crate::config::types::VllmConfig =
                serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
            assert_eq!(vllm.quantization, Some("fp8".to_string()));
            assert_eq!(vllm.kv_cache_dtype, Some("fp8".to_string()));
            assert_eq!(vllm.tensor_parallel_size, Some(2));
            assert_eq!(vllm.gpu_memory_utilization, Some(0.92));
            assert_eq!(vllm.max_num_batched_tokens, Some(2560));
            assert!(vllm.enable_prefix_caching);

            // args should be stripped to only unmanaged flags
            let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
            assert_eq!(args, vec!["--attention-backend ROCM_AITER_UNIFIED_ATTN"]);
        }
    }

    /// Row without managed vLLM flags → untouched.
    #[test]
    fn test_backfill_no_managed_flags_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let args = vec![
                "--attention-backend ROCM".to_string(),
                "--max-logprobs 100".to_string(),
            ];
            let record = make_record(1, "test/model2", Some(args), None);
            upsert_model_config(&conn, &record).unwrap();
        }

        // Run backfill
        backfill_vllm_config(&db_dir).unwrap();

        // Verify — args unchanged, vllm_config still NULL
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let row: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 1",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();

            assert!(row.vllm_config.is_none());
            let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
            assert_eq!(args, vec!["--attention-backend ROCM", "--max-logprobs 100"]);
        }
    }

    /// Row with existing vllm_config → existing values win on conflict.
    #[test]
    fn test_backfill_existing_vllm_config_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let args = vec![
                "--quantization fp8".to_string(),
                "--tensor-parallel-size 2".to_string(),
            ];
            // Existing vllm_config has quantization=awq and tensor_parallel_size=4
            // These should win over the extracted values
            let existing = crate::config::types::VllmConfig {
                quantization: Some("awq".to_string()),
                kv_cache_dtype: None,
                tensor_parallel_size: Some(4),
                gpu_memory_utilization: None,
                max_model_len: None,
                max_num_batched_tokens: None,
                enable_prefix_caching: false,
                trust_remote_code: false,
                ..Default::default()
            };
            let record = make_record(1, "test/model3", Some(args), Some(existing));
            upsert_model_config(&conn, &record).unwrap();
        }

        // Run backfill
        backfill_vllm_config(&db_dir).unwrap();

        // Verify — existing values win
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let row: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 1",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();

            let vllm: crate::config::types::VllmConfig =
                serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
            // Existing values win
            assert_eq!(vllm.quantization, Some("awq".to_string()));
            assert_eq!(vllm.tensor_parallel_size, Some(4));
        }
    }

    /// Row with existing vllm_config that has None fields → extracted fills gaps.
    #[test]
    fn test_backfill_existing_vllm_config_fills_gaps() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let args = vec![
                "--quantization fp8".to_string(),
                "--tensor-parallel-size 2".to_string(),
            ];
            // Existing vllm_config has quantization but not tensor_parallel_size
            let existing = crate::config::types::VllmConfig {
                quantization: Some("awq".to_string()),
                kv_cache_dtype: None,
                tensor_parallel_size: None, // This should be filled from args
                gpu_memory_utilization: None,
                max_model_len: None,
                max_num_batched_tokens: None,
                enable_prefix_caching: false,
                trust_remote_code: false,
                ..Default::default()
            };
            let record = make_record(1, "test/model4", Some(args), Some(existing));
            upsert_model_config(&conn, &record).unwrap();
        }

        // Run backfill
        backfill_vllm_config(&db_dir).unwrap();

        // Verify — existing quantization wins, tensor_parallel_size filled from args
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let row: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 1",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();

            let vllm: crate::config::types::VllmConfig =
                serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
            assert_eq!(vllm.quantization, Some("awq".to_string())); // existing wins
            assert_eq!(vllm.tensor_parallel_size, Some(2)); // filled from args
        }
    }

    /// Empty DB — no crash.
    #[test]
    fn test_backfill_empty_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        // Create the DB (empty, just migrations)
        let _ = crate::db::open(&db_dir).unwrap();

        let result = backfill_vllm_config(&db_dir);
        assert!(result.is_ok());
    }

    /// Flattened form in args → correctly extracted.
    #[test]
    fn test_backfill_flattened_form() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let args = vec![
                "--quantization".to_string(),
                "fp8".to_string(),
                "--tensor-parallel-size".to_string(),
                "4".to_string(),
            ];
            let record = make_record(1, "test/model5", Some(args), None);
            upsert_model_config(&conn, &record).unwrap();
        }

        // Run backfill
        backfill_vllm_config(&db_dir).unwrap();

        // Verify
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let row: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 1",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();

            let vllm: crate::config::types::VllmConfig =
                serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
            assert_eq!(vllm.quantization, Some("fp8".to_string()));
            assert_eq!(vllm.tensor_parallel_size, Some(4));

            // args should be empty (all entries were managed)
            let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
            assert!(args.is_empty());
        }
    }

    /// Multiple rows — one with corrupt args JSON — should not abort the batch.
    /// The good row still migrates; the corrupt row is skipped with a warning.
    #[test]
    fn test_backfill_corrupt_row_does_not_abort_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            // Row 1: valid args with managed vLLM flags
            let args1 = vec!["--quantization fp8".to_string()];
            let record1 = make_record(1, "test/good", Some(args1), None);
            upsert_model_config(&conn, &record1).unwrap();

            // Row 2: corrupt args JSON that contains a managed flag substring
            let record2 = make_record(2, "test/bad", Some(vec![]), None);
            upsert_model_config(&conn, &record2).unwrap();
            // Overwrite args with invalid JSON that still matches the LIKE query
            conn.execute(
                "UPDATE model_configs SET args = 'NOT VALID JSON --quantization' WHERE id = 2",
                [],
            )
            .unwrap();

            // Row 3: another valid row
            let args3 = vec!["--tensor-parallel-size 4".to_string()];
            let record3 = make_record(3, "test/good2", Some(args3), None);
            upsert_model_config(&conn, &record3).unwrap();
        }

        // Backfill must succeed (not abort on the corrupt row)
        backfill_vllm_config(&db_dir).unwrap();

        // Verify — good rows migrated, corrupt row left as-is
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();

            // Row 1: migrated
            let row1: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 1",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();
            let vllm: crate::config::types::VllmConfig =
                serde_json::from_str(row1.vllm_config.as_ref().unwrap()).unwrap();
            assert_eq!(vllm.quantization, Some("fp8".to_string()));

            // Row 2: unchanged (corrupt args skipped)
            let row2: (String, Option<String>) = conn
                .query_row(
                    "SELECT args, vllm_config FROM model_configs WHERE id = 2",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(row2.0, "NOT VALID JSON --quantization");
            assert!(row2.1.is_none());

            // Row 3: migrated
            let row3: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 3",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();
            let vllm: crate::config::types::VllmConfig =
                serde_json::from_str(row3.vllm_config.as_ref().unwrap()).unwrap();
            assert_eq!(vllm.tensor_parallel_size, Some(4));
        }
    }

    /// Row with corrupt existing vllm_config → falls back to extracted config.
    /// The corrupt value is overwritten (safe — it was unusable anyway).
    #[test]
    fn test_backfill_corrupt_existing_vllm_config_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let args = vec![
                "--quantization fp8".to_string(),
                "--tensor-parallel-size 2".to_string(),
            ];
            let record = make_record(1, "test/model", Some(args), None);
            upsert_model_config(&conn, &record).unwrap();
            // Overwrite vllm_config with corrupt JSON
            conn.execute(
                "UPDATE model_configs SET vllm_config = '{CORRUPT' WHERE id = 1",
                [],
            )
            .unwrap();
        }

        // Backfill must succeed — corrupt vllm_config falls back to extracted
        backfill_vllm_config(&db_dir).unwrap();

        // Verify — extracted values are used, corrupt value overwritten
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let row: ModelConfigRecord = conn
                .query_row(
                    &format!(
                        "SELECT {} FROM model_configs WHERE id = 1",
                        ModelConfigRecord::COLUMNS
                    ),
                    [],
                    ModelConfigRecord::from_row,
                )
                .unwrap();

            let vllm: crate::config::types::VllmConfig =
                serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
            assert_eq!(vllm.quantization, Some("fp8".to_string()));
            assert_eq!(vllm.tensor_parallel_size, Some(2));

            // args should be stripped
            let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
            assert!(args.is_empty());
        }
    }

    /// Row with hf_format != 'transformers' → skipped by backfill.
    #[test]
    fn test_backfill_non_transformers_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let args = vec!["--quantization fp8".to_string()];
            let mut record = make_record(1, "test/model", Some(args), None);
            record.hf_format = Some("gguf".to_string());
            upsert_model_config(&conn, &record).unwrap();
        }

        // Backfill runs
        backfill_vllm_config(&db_dir).unwrap();

        // Verify — row untouched because hf_format is not 'transformers'
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let row: (String, Option<String>) = conn
                .query_row(
                    "SELECT args, vllm_config FROM model_configs WHERE id = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();

            assert!(row.1.is_none()); // vllm_config still NULL
            let args: Vec<String> = serde_json::from_str(&row.0).unwrap();
            assert_eq!(args, vec!["--quantization fp8"]); // args unchanged
        }
    }
}
