//! Centralized model data access.
//!
//! `ModelManager` is a facade over the Postgres pool for all model-domain
//! data (configs, files, pulls, pull queue).

use std::path::Path;

use anyhow::{anyhow, Result};
use sqlx::{PgPool, Row};
use std::sync::Arc;

use crate::config::ModelConfig;
use crate::db::queries::{
    ModelConfigRecord, ModelFileRecord, ModelPullRecord, PullLogEntry, PullQueueItem,
};

/// Centralized model data access. Each caller opens its own instance.
pub struct ModelManager {
    /// Postgres pool for all model-domain data.
    pub(crate) pool: Arc<PgPool>,
}

impl ModelManager {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    /// Returns the Postgres pool.
    pub fn pool(&self) -> &PgPool {
        self.pool.as_ref()
    }

    // ── Config CRUD ────────────────────────────────────────────

    /// Get the model configuration by id. Returns None if not found.
    pub async fn get_config(&self, id: i64) -> Result<Option<ModelConfigRecord>> {
        crate::db::queries::get_model_config(self.pool.as_ref(), id).await
    }

    /// Get the model configuration by repo_id. Returns None if not found.
    pub async fn get_config_by_repo_id(&self, repo_id: &str) -> Result<Option<ModelConfigRecord>> {
        crate::db::queries::get_model_config_by_repo_id(self.pool.as_ref(), repo_id).await
    }

    /// Get all stored model configurations.
    pub async fn get_all_configs(&self) -> Result<Vec<ModelConfigRecord>> {
        crate::db::queries::get_all_model_configs(self.pool.as_ref()).await
    }

    /// Insert or update the model configuration. Returns the model id.
    pub async fn upsert_config(&self, record: &ModelConfigRecord) -> Result<i64> {
        crate::db::queries::upsert_model_config(self.pool.as_ref(), record).await
    }

    /// Delete the model configuration by id. CASCADE deletes model_pulls and model_files.
    pub async fn delete_config(&self, id: i64) -> Result<()> {
        crate::db::queries::delete_model_config(self.pool.as_ref(), id).await
    }

    /// Rename a config by updating its repo_id.
    ///
    /// Uses a direct UPDATE to avoid triggering CASCADE deletes on
    /// model_files. Case-insensitive collision with another row is rejected
    /// (v2 `COLLATE NOCASE` parity) — the DB unique index is case-sensitive,
    /// so the check is explicit.
    pub async fn rename_config(&self, id: i64, new_repo_id: &str) -> Result<()> {
        // Verify the record exists
        let _exists = self
            .get_config(id)
            .await?
            .ok_or_else(|| anyhow!("Model config with id {} not found", id))?;

        let collision: Option<i64> = sqlx::query(
            "SELECT id FROM model_configs WHERE lower(repo_id) = lower($1) AND id <> $2",
        )
        .bind(new_repo_id)
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?
        .map(|r| r.get("id"));
        if collision.is_some() {
            return Err(anyhow!(
                "a model config with repo_id '{}' already exists (case-insensitive)",
                new_repo_id
            ));
        }

        sqlx::query("UPDATE model_configs SET repo_id = $1, updated_at = now() WHERE id = $2")
            .bind(new_repo_id)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Enable a model by config_key.
    pub async fn enable_model(&self, config_key: &str) -> Result<()> {
        let repo_id = crate::models::config_key_to_repo_id(config_key);
        sqlx::query(
            "UPDATE model_configs SET enabled = TRUE, updated_at = now() \
                     WHERE lower(repo_id) = lower($1)",
        )
        .bind(&repo_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Disable a model by config_key.
    pub async fn disable_model(&self, config_key: &str) -> Result<()> {
        let repo_id = crate::models::config_key_to_repo_id(config_key);
        sqlx::query(
            "UPDATE model_configs SET enabled = FALSE, updated_at = now() \
                     WHERE lower(repo_id) = lower($1)",
        )
        .bind(&repo_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Convenience method to save a ModelConfig as a DB record.
    ///
    /// Converts config_key to repo_id, converts ModelConfig → ModelConfigRecord,
    /// sets api_name default, and calls upsert_config.
    pub async fn save_model_config(&self, config_key: &str, mc: &ModelConfig) -> Result<i64> {
        let repo_id = crate::models::config_key_to_repo_id(config_key);
        let mut record = mc.to_db_record(&repo_id);
        if record.api_name.as_deref().is_none_or(str::is_empty) {
            record.api_name = Some(repo_id.clone());
        }
        self.upsert_config(&record).await
    }

    // ── File tracking ──────────────────────────────────────────

    /// Get all stored file records for a model.
    pub async fn get_files(&self, model_id: i64) -> Result<Vec<ModelFileRecord>> {
        crate::db::queries::get_model_files(self.pool.as_ref(), model_id).await
    }

    /// Get all stored file records across all models.
    pub async fn get_all_files(&self) -> Result<Vec<ModelFileRecord>> {
        crate::db::queries::get_all_model_files(self.pool.as_ref()).await
    }

    /// Insert or update a file record for a pulled GGUF.
    pub async fn upsert_file(
        &self,
        model_id: i64,
        repo_id: &str,
        filename: &str,
        quant: Option<&str>,
        lfs_oid: Option<&str>,
        size_bytes: Option<i64>,
    ) -> Result<()> {
        crate::db::queries::upsert_model_file(
            self.pool.as_ref(),
            model_id,
            repo_id,
            filename,
            quant,
            lfs_oid,
            size_bytes,
        )
        .await
    }

    /// Delete a single model file record by (model_id, filename).
    pub async fn delete_file(&self, model_id: i64, filename: &str) -> Result<()> {
        crate::db::queries::delete_model_file(self.pool.as_ref(), model_id, filename).await
    }

    /// Update the `kind` column on a model file (model / mmproj / mtp).
    pub async fn set_file_kind(&self, model_id: i64, filename: &str, kind: &str) -> Result<()> {
        crate::db::queries::update_model_file_kind(self.pool.as_ref(), model_id, filename, kind)
            .await
    }

    /// Update the verification columns for a single file.
    pub async fn update_verification(
        &self,
        model_id: i64,
        filename: &str,
        verified_ok: Option<bool>,
        verify_error: Option<&str>,
    ) -> Result<()> {
        crate::db::queries::update_verification(
            self.pool.as_ref(),
            model_id,
            filename,
            verified_ok,
            verify_error,
        )
        .await
    }

    // ── Pull tracking ──────────────────────────────────────────

    /// Insert or update the pull record for a model.
    pub async fn upsert_pull(&self, model_id: i64, repo_id: &str, commit_sha: &str) -> Result<()> {
        crate::db::queries::upsert_model_pull(self.pool.as_ref(), model_id, repo_id, commit_sha)
            .await
    }

    /// Get the stored pull record for a model. Returns None if never pulled.
    pub async fn get_pull(&self, model_id: i64) -> Result<Option<ModelPullRecord>> {
        crate::db::queries::get_model_pull(self.pool.as_ref(), model_id).await
    }

    /// Log a pull event (append-only).
    pub async fn log_pull(&self, entry: &PullLogEntry) -> Result<()> {
        crate::db::queries::log_pull(self.pool.as_ref(), entry).await
    }

    // ── Pull queue ────────────────────────────────────────────

    /// Insert a new item into the pull queue. Returns the new row id.
    #[allow(clippy::too_many_arguments)]
    pub async fn queue_insert(
        &self,
        job_id: &str,
        repo_id: &str,
        filename: &str,
        display_name: Option<&str>,
        kind: &str,
        quant: Option<&str>,
        context_length: Option<u32>,
    ) -> Result<i64> {
        crate::db::queries::insert_queue_item(
            self.pool.as_ref(),
            job_id,
            repo_id,
            filename,
            display_name,
            kind,
            quant,
            context_length,
        )
        .await
    }

    /// Retrieve the oldest queued item (FIFO).
    pub async fn queue_get_queued(&self) -> Result<Option<PullQueueItem>> {
        crate::db::queries::get_queued_item(self.pool.as_ref()).await
    }

    /// Get all active items (queued, running, verifying), ordered by status priority then queued_at.
    pub async fn queue_get_active(&self) -> Result<Vec<PullQueueItem>> {
        crate::db::queries::get_active_items(self.pool.as_ref()).await
    }

    /// Get history items (completed, failed, cancelled), sorted newest first.
    pub async fn queue_get_history(&self, limit: i64, offset: i64) -> Result<Vec<PullQueueItem>> {
        crate::db::queries::get_history_items(self.pool.as_ref(), limit, offset).await
    }

    /// Update a queue item's status and related fields.
    pub async fn queue_update_status(
        &self,
        job_id: &str,
        new_status: &str,
        bytes_pulled: i64,
        total_bytes: Option<i64>,
        error_message: Option<&str>,
    ) -> Result<()> {
        crate::db::queries::update_queue_status(
            self.pool.as_ref(),
            job_id,
            new_status,
            bytes_pulled,
            total_bytes,
            error_message,
        )
        .await
    }

    /// Update only progress fields (bytes_pulled, total_bytes) without
    /// changing the status. Used for real-time progress streaming via SSE.
    pub async fn queue_update_progress(
        &self,
        job_id: &str,
        bytes_pulled: i64,
        total_bytes: Option<i64>,
    ) -> Result<()> {
        crate::db::queries::update_progress_only(
            self.pool.as_ref(),
            job_id,
            bytes_pulled,
            total_bytes,
        )
        .await
    }

    /// Cancel a queue item if it hasn't reached a terminal state.
    pub async fn queue_cancel(&self, job_id: &str) -> Result<()> {
        crate::db::queries::cancel_queue_item(self.pool.as_ref(), job_id).await
    }

    /// Retrieve a queue item by its job_id.
    pub async fn queue_get_by_job_id(&self, job_id: &str) -> Result<Option<PullQueueItem>> {
        crate::db::queries::get_item_by_job_id(self.pool.as_ref(), job_id).await
    }

    // ── Async wrappers ────────────────────────────────────────

    /// Check for HuggingFace updates for a model. Async wrapper around
    /// `crate::models::update::check_for_updates`.
    pub async fn check_for_updates(
        &self,
        repo_id: &str,
    ) -> Result<crate::models::update::UpdateCheckResult> {
        crate::models::update::check_for_updates(self.pool.as_ref(), repo_id).await
    }

    /// Refresh HuggingFace metadata for a model. Async wrapper around
    /// `crate::models::update::refresh_metadata`.
    pub async fn refresh_metadata(&self, models_dir: &Path, repo_id: &str) -> Result<()> {
        crate::models::update::refresh_metadata(self.pool.as_ref(), models_dir, repo_id).await
    }
}
