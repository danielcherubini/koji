//! Backfill `vllm_config` column from `args` for existing safetensors models.
//!
//! Existing models may carry vLLM flags inside their `args` column. This
//! backfill extracts the 8 managed flags into `vllm_config` and strips them
//! from `args`, preventing duplication when the new columns are used.

use anyhow::Result;
use sqlx::PgPool;

/// Backfill `vllm_config` from `args` for models that carry managed vLLM flags.
///
/// Gate: rows whose `args` JSON contains at least one managed vLLM flag.
/// When a row already has a non-empty `vllm_config`, existing column values
/// win per-field; extracted values only fill fields that are `None`/`false`.
///
/// Postgres-based (plan-190 Task 5) — model configs live in Postgres.
pub async fn backfill_vllm_config(pool: &PgPool) -> Result<()> {
    // Find rows whose args contain at least one managed flag.
    // We check for the flag strings in the JSON representation of args.
    const SELECT_SQL: &str = "SELECT id, repo_id, args, vllm_config FROM model_configs \
        WHERE (args LIKE '%--quantization%' OR args LIKE '%--kv-cache-dtype%' \
           OR args LIKE '%--tensor-parallel-size%' OR args LIKE '%--gpu-memory-utilization%' \
           OR args LIKE '%--max-model-len%' OR args LIKE '%--max-num-batched-tokens%' \
           OR args LIKE '%--enable-prefix-caching%' OR args LIKE '%--trust-remote-code%') \
        AND hf_format = 'transformers'";
    // Collect rows into a Vec first — we update the same table during iteration.
    let rows: Vec<(i64, String, Option<String>, Option<String>)> =
        sqlx::query_as(SELECT_SQL).fetch_all(pool).await?;

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
        if let Err(e) = sqlx::query(
            "UPDATE model_configs SET vllm_config = $1, args = $2, updated_at = now() WHERE id = $3",
        )
        .bind(&merged_json)
        .bind(&stripped_args_json)
        .bind(id)
        .execute(pool)
        .await
        {
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
        attention_backend: existing
            .attention_backend
            .clone()
            .or_else(|| extracted.attention_backend.clone()),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{get_model_config, upsert_model_config, ModelConfigRecord};

    fn make_record(
        repo_id: &str,
        args: Option<Vec<String>>,
        vllm_config: Option<crate::config::types::VllmConfig>,
    ) -> ModelConfigRecord {
        let args_json = args.map(|a| serde_json::to_string(&a).unwrap());
        let vllm_json = vllm_config.map(|v| serde_json::to_string(&v).unwrap());

        ModelConfigRecord {
            repo_id: repo_id.to_string(),
            backend: "vllm".to_string(),
            args: args_json,
            hf_format: Some("transformers".to_string()),
            vllm_config: vllm_json,
            ..Default::default()
        }
    }

    async fn get_record(pool: &PgPool, id: i64) -> ModelConfigRecord {
        get_model_config(pool, id).await.unwrap().unwrap()
    }

    /// Row with grouped vLLM args → vllm_config populated, args stripped.
    #[tokio::test]
    async fn test_backfill_grouped_args() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let args = vec![
            "--quantization fp8".to_string(),
            "--kv-cache-dtype fp8".to_string(),
            "--tensor-parallel-size 2".to_string(),
            "--gpu-memory-utilization 0.92".to_string(),
            "--attention-backend ROCM_AITER_UNIFIED_ATTN".to_string(),
            "--max-num-batched-tokens 2560".to_string(),
            "--enable-prefix-caching".to_string(),
        ];
        let id = upsert_model_config(pool, &make_record("test/model1", Some(args), None))
            .await
            .unwrap();

        backfill_vllm_config(pool).await.unwrap();

        let row = get_record(pool, id).await;
        let vllm: crate::config::types::VllmConfig =
            serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
        assert_eq!(vllm.quantization, Some("fp8".to_string()));
        assert_eq!(vllm.kv_cache_dtype, Some("fp8".to_string()));
        assert_eq!(vllm.tensor_parallel_size, Some(2));
        assert_eq!(vllm.gpu_memory_utilization, Some(0.92));
        assert_eq!(vllm.max_num_batched_tokens, Some(2560));
        assert!(vllm.enable_prefix_caching);

        let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
        assert_eq!(args, vec!["--attention-backend ROCM_AITER_UNIFIED_ATTN"]);
        guard.finish().await;
    }

    /// Row without managed vLLM flags → untouched.
    #[tokio::test]
    async fn test_backfill_no_managed_flags_untouched() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let args = vec![
            "--attention-backend ROCM".to_string(),
            "--max-logprobs 100".to_string(),
        ];
        let id = upsert_model_config(pool, &make_record("test/model2", Some(args), None))
            .await
            .unwrap();

        backfill_vllm_config(pool).await.unwrap();

        let row = get_record(pool, id).await;
        assert!(row.vllm_config.is_none());
        let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
        assert_eq!(args, vec!["--attention-backend ROCM", "--max-logprobs 100"]);
        guard.finish().await;
    }

    /// Row with existing vllm_config → existing values win on conflict.
    #[tokio::test]
    async fn test_backfill_existing_vllm_config_wins() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let args = vec![
            "--quantization fp8".to_string(),
            "--tensor-parallel-size 2".to_string(),
        ];
        let existing = crate::config::types::VllmConfig {
            quantization: Some("awq".to_string()),
            kv_cache_dtype: None,
            tensor_parallel_size: Some(4),
            ..Default::default()
        };
        let id = upsert_model_config(
            pool,
            &make_record("test/model3", Some(args), Some(existing)),
        )
        .await
        .unwrap();

        backfill_vllm_config(pool).await.unwrap();

        let row = get_record(pool, id).await;
        let vllm: crate::config::types::VllmConfig =
            serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
        assert_eq!(vllm.quantization, Some("awq".to_string()));
        assert_eq!(vllm.tensor_parallel_size, Some(4));
        guard.finish().await;
    }

    /// Row with existing vllm_config that has None fields → extracted fills gaps.
    #[tokio::test]
    async fn test_backfill_existing_vllm_config_fills_gaps() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let args = vec![
            "--quantization fp8".to_string(),
            "--tensor-parallel-size 2".to_string(),
        ];
        let existing = crate::config::types::VllmConfig {
            quantization: Some("awq".to_string()),
            ..Default::default()
        };
        let id = upsert_model_config(
            pool,
            &make_record("test/model4", Some(args), Some(existing)),
        )
        .await
        .unwrap();

        backfill_vllm_config(pool).await.unwrap();

        let row = get_record(pool, id).await;
        let vllm: crate::config::types::VllmConfig =
            serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
        assert_eq!(vllm.quantization, Some("awq".to_string())); // existing wins
        assert_eq!(vllm.tensor_parallel_size, Some(2)); // filled from args
        guard.finish().await;
    }

    /// Empty DB — no crash.
    #[tokio::test]
    async fn test_backfill_empty_db() {
        let guard = crate::testing::postgres::with_schema().await;
        backfill_vllm_config(&guard.pool).await.unwrap();
        guard.finish().await;
    }

    /// Flattened form in args → correctly extracted.
    #[tokio::test]
    async fn test_backfill_flattened_form() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let args = vec![
            "--quantization".to_string(),
            "fp8".to_string(),
            "--tensor-parallel-size".to_string(),
            "4".to_string(),
        ];
        let id = upsert_model_config(pool, &make_record("test/model5", Some(args), None))
            .await
            .unwrap();

        backfill_vllm_config(pool).await.unwrap();

        let row = get_record(pool, id).await;
        let vllm: crate::config::types::VllmConfig =
            serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
        assert_eq!(vllm.quantization, Some("fp8".to_string()));
        assert_eq!(vllm.tensor_parallel_size, Some(4));

        let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
        assert!(args.is_empty());
        guard.finish().await;
    }

    /// Multiple rows — one with corrupt args JSON — should not abort the batch.
    #[tokio::test]
    async fn test_backfill_corrupt_row_does_not_abort_batch() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let id1 = upsert_model_config(
            pool,
            &make_record(
                "test/good",
                Some(vec!["--quantization fp8".to_string()]),
                None,
            ),
        )
        .await
        .unwrap();
        let id2 = upsert_model_config(pool, &make_record("test/bad", Some(vec![]), None))
            .await
            .unwrap();
        // Overwrite args with invalid JSON that still matches the LIKE query
        sqlx::query("UPDATE model_configs SET args = $1 WHERE id = $2")
            .bind("NOT VALID JSON --quantization")
            .bind(id2)
            .execute(pool)
            .await
            .unwrap();
        let id3 = upsert_model_config(
            pool,
            &make_record(
                "test/good2",
                Some(vec!["--tensor-parallel-size 4".to_string()]),
                None,
            ),
        )
        .await
        .unwrap();

        backfill_vllm_config(pool).await.unwrap();

        // Row 1: migrated
        let row1 = get_record(pool, id1).await;
        let vllm: crate::config::types::VllmConfig =
            serde_json::from_str(row1.vllm_config.as_ref().unwrap()).unwrap();
        assert_eq!(vllm.quantization, Some("fp8".to_string()));

        // Row 2: unchanged (corrupt args skipped)
        let row2 = get_record(pool, id2).await;
        assert_eq!(row2.args.as_deref(), Some("NOT VALID JSON --quantization"));
        assert!(row2.vllm_config.is_none());

        // Row 3: migrated
        let row3 = get_record(pool, id3).await;
        let vllm: crate::config::types::VllmConfig =
            serde_json::from_str(row3.vllm_config.as_ref().unwrap()).unwrap();
        assert_eq!(vllm.tensor_parallel_size, Some(4));
        guard.finish().await;
    }

    /// Row with corrupt existing vllm_config → falls back to extracted config.
    #[tokio::test]
    async fn test_backfill_corrupt_existing_vllm_config_falls_back() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let args = vec![
            "--quantization fp8".to_string(),
            "--tensor-parallel-size 2".to_string(),
        ];
        let id = upsert_model_config(pool, &make_record("test/model", Some(args), None))
            .await
            .unwrap();
        sqlx::query("UPDATE model_configs SET vllm_config = $1 WHERE id = $2")
            .bind("{CORRUPT")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();

        backfill_vllm_config(pool).await.unwrap();

        let row = get_record(pool, id).await;
        let vllm: crate::config::types::VllmConfig =
            serde_json::from_str(row.vllm_config.as_ref().unwrap()).unwrap();
        assert_eq!(vllm.quantization, Some("fp8".to_string()));
        assert_eq!(vllm.tensor_parallel_size, Some(2));

        let args: Vec<String> = serde_json::from_str(row.args.as_ref().unwrap()).unwrap();
        assert!(args.is_empty());
        guard.finish().await;
    }

    /// Row with hf_format != 'transformers' → skipped by backfill.
    #[tokio::test]
    async fn test_backfill_non_transformers_skipped() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = &guard.pool;
        let mut record = make_record(
            "test/model",
            Some(vec!["--quantization fp8".to_string()]),
            None,
        );
        record.hf_format = Some("gguf".to_string());
        let id = upsert_model_config(pool, &record).await.unwrap();

        backfill_vllm_config(pool).await.unwrap();

        let row = get_record(pool, id).await;
        assert!(row.vllm_config.is_none()); // vllm_config still NULL
        let args: Vec<String> = serde_json::from_str(row.args.as_deref().unwrap()).unwrap();
        assert_eq!(args, vec!["--quantization fp8"]); // args unchanged
        guard.finish().await;
    }

    /// Spec-level `attention_backend` in existing config wins over extracted.
    #[test]
    fn test_backfill_merge_spec_attention_backend_existing_wins() {
        let existing = crate::config::types::VllmSpecConfig {
            method: Some("eagle".to_string()),
            attention_backend: Some("FLASH_ATTN".to_string()),
            ..Default::default()
        };
        let extracted = crate::config::types::VllmSpecConfig {
            method: Some("eagle".to_string()),
            attention_backend: Some("ROCM_AITER_UNIFIED_ATTN".to_string()),
            ..Default::default()
        };
        let merged = merge_vllm_spec_decoding(&existing, &extracted);
        assert_eq!(merged.method, Some("eagle".to_string()));
        assert_eq!(merged.attention_backend, Some("FLASH_ATTN".to_string()));
    }

    /// Spec-level `attention_backend` fills from extracted when existing is `None`.
    #[test]
    fn test_backfill_merge_spec_attention_backend_extracted_fills() {
        let existing = crate::config::types::VllmSpecConfig {
            method: Some("eagle".to_string()),
            attention_backend: None,
            ..Default::default()
        };
        let extracted = crate::config::types::VllmSpecConfig {
            method: Some("eagle".to_string()),
            attention_backend: Some("ROCM_AITER_UNIFIED_ATTN".to_string()),
            ..Default::default()
        };
        let merged = merge_vllm_spec_decoding(&existing, &extracted);
        assert_eq!(
            merged.attention_backend,
            Some("ROCM_AITER_UNIFIED_ATTN".to_string())
        );
    }
}
