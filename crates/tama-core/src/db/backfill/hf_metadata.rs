use anyhow::Result;
use sqlx::{PgPool, Row};

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
/// Postgres-based (plan-190 Task 5) — model configs live in Postgres.
pub async fn backfill_hf_metadata(pool: &PgPool) -> Result<()> {
    let models: Vec<(i64, String)> =
        sqlx::query("SELECT id, repo_id FROM model_configs WHERE hf_format IS NULL")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| (r.get("id"), r.get("repo_id")))
            .collect();

    if models.is_empty() {
        tracing::debug!("No models need HF metadata backfill");
        return Ok(());
    }

    let total = models.len();
    tracing::info!("Backfilling HF metadata for {} model(s)", total);

    // ── Fetch metadata for each model and write it back ──
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

        if let Err(e) =
            crate::models::update::update_model_config_hf_metadata(pool, *model_id, &meta).await
        {
            tracing::warn!(
                "Failed to update HF metadata for '{}' (id={}): {}",
                repo_id,
                model_id,
                e
            );
        }

        // Small delay between API calls to avoid rate limiting
        if i + 1 < total {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    tracing::info!("HF metadata backfill complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::{get_model_config, upsert_model_config, ModelConfigRecord};

    /// Build a minimal model config record with NULL hf_format.
    fn null_hf_record(repo_id: &str) -> ModelConfigRecord {
        ModelConfigRecord {
            repo_id: repo_id.to_string(),
            backend: "llama_cpp".to_string(),
            ..Default::default()
        }
    }

    /// Test that backfill_hf_metadata runs without crashing when there are models
    /// with NULL hf_format. In tests, the HF API calls will fail (no network),
    /// but the function should handle failures gracefully and return Ok.
    #[tokio::test]
    async fn test_backfill_hf_metadata_no_crash_with_null_rows() {
        let guard = crate::testing::postgres::with_schema().await;

        // Insert a model_config row with NULL hf_format (simulating post-migration state)
        upsert_model_config(&guard.pool, &null_hf_record("test/repo"))
            .await
            .unwrap();

        // Run backfill — HF API calls will fail (no network in tests),
        // but the function should handle failures gracefully and return Ok
        let result = backfill_hf_metadata(&guard.pool).await;
        assert!(
            result.is_ok(),
            "backfill should not crash even when HF API fails"
        );

        guard.finish().await;
    }

    /// Test that backfill_hf_metadata is a no-op when all models already have hf_format.
    #[tokio::test]
    async fn test_backfill_hf_metadata_noop_when_all_populated() {
        let guard = crate::testing::postgres::with_schema().await;

        // Insert a model_config row WITH hf_format set
        let mut record = null_hf_record("test/repo");
        record.hf_format = Some("gguf".to_string());
        upsert_model_config(&guard.pool, &record).await.unwrap();

        // Run backfill — no rows match, should return Ok immediately
        let result = backfill_hf_metadata(&guard.pool).await;
        assert!(result.is_ok());

        // hf_format should be unchanged
        let row = get_model_config(&guard.pool, 1).await.unwrap().unwrap();
        assert_eq!(row.hf_format.as_deref(), Some("gguf"));

        guard.finish().await;
    }

    /// Test that backfill_hf_metadata returns Ok with an empty DB.
    #[tokio::test]
    async fn test_backfill_hf_metadata_empty_db() {
        let guard = crate::testing::postgres::with_schema().await;

        let result = backfill_hf_metadata(&guard.pool).await;
        assert!(result.is_ok());

        guard.finish().await;
    }
}
