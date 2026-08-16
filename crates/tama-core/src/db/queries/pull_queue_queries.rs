//! Query functions for the `pull_queue` table.
//!
//! All functions take a `&PgPool` and are async (plan-190 Task 7).

use anyhow::Result;
use sqlx::types::time::OffsetDateTime;
use sqlx::{PgPool, Row};

/// `SELECT {ITEM_COLUMNS}` for a single queued row (FIFO).
const GET_QUEUED_SQL: &str = concat!(
    "SELECT ",
    "id, job_id, repo_id, filename, display_name, status, \
     bytes_pulled, total_bytes, error_message, started_at, \
     completed_at, queued_at, kind, quant, context_length",
    " FROM pull_queue WHERE status = 'queued' ORDER BY queued_at ASC LIMIT 1"
);

/// `SELECT {ITEM_COLUMNS}` for a single row by job_id.
const GET_BY_JOB_ID_SQL: &str = concat!(
    "SELECT ",
    "id, job_id, repo_id, filename, display_name, status, \
     bytes_pulled, total_bytes, error_message, started_at, \
     completed_at, queued_at, kind, quant, context_length",
    " FROM pull_queue WHERE job_id = $1 LIMIT 1"
);

/// `SELECT {ITEM_COLUMNS}` for all active rows.
const GET_ACTIVE_SQL: &str = concat!(
    "SELECT ",
    "id, job_id, repo_id, filename, display_name, status, \
     bytes_pulled, total_bytes, error_message, started_at, \
     completed_at, queued_at, kind, quant, context_length",
    " FROM pull_queue WHERE status IN ('queued', 'running', 'verifying') \
     ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'verifying' THEN 1 ELSE 2 END, \
              queued_at ASC"
);

/// `SELECT {ITEM_COLUMNS}` for terminal rows, newest first.
const GET_HISTORY_SQL: &str = concat!(
    "SELECT ",
    "id, job_id, repo_id, filename, display_name, status, \
     bytes_pulled, total_bytes, error_message, started_at, \
     completed_at, queued_at, kind, quant, context_length",
    " FROM pull_queue WHERE status IN ('completed', 'failed', 'cancelled') \
     ORDER BY completed_at DESC LIMIT $1 OFFSET $2"
);

/// `SELECT {ITEM_COLUMNS}` for the active row of a (repo_id, filename) pair.
const GET_ACTIVE_BY_REPO_FILENAME_SQL: &str = concat!(
    "SELECT ",
    "id, job_id, repo_id, filename, display_name, status, \
     bytes_pulled, total_bytes, error_message, started_at, \
     completed_at, queued_at, kind, quant, context_length",
    " FROM pull_queue WHERE repo_id = $1 AND filename = $2 \
     AND status IN ('queued', 'running', 'verifying') LIMIT 1"
);

/// A row from the pull_queue table.
#[derive(Debug, Clone)]
pub struct PullQueueItem {
    pub id: i64,
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub display_name: Option<String>,
    pub status: String, // "queued" | "running" | "verifying" | "completed" | "failed" | "cancelled"
    pub bytes_pulled: i64,
    pub total_bytes: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: Option<OffsetDateTime>,
    pub completed_at: Option<OffsetDateTime>,
    pub queued_at: OffsetDateTime,
    pub kind: String, // "model" | "backend"
    pub quant: Option<String>,
    pub context_length: Option<u32>,
}

/// Decode a `pull_queue` row into a `PullQueueItem`.
fn decode_item(row: &sqlx::postgres::PgRow) -> anyhow::Result<PullQueueItem> {
    let context_length: Option<i64> = row.get("context_length");
    Ok(PullQueueItem {
        id: row.get("id"),
        job_id: row.get("job_id"),
        repo_id: row.get("repo_id"),
        filename: row.get("filename"),
        display_name: row.get("display_name"),
        status: row.get("status"),
        bytes_pulled: row.get("bytes_pulled"),
        total_bytes: row.get("total_bytes"),
        error_message: row.get("error_message"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        queued_at: row.get("queued_at"),
        kind: row.get("kind"),
        quant: row.get("quant"),
        context_length: context_length.and_then(|v| u32::try_from(v).ok()),
    })
}

/// Insert a new item into the pull queue.
/// Returns the new row id.
#[allow(clippy::too_many_arguments)]
pub async fn insert_queue_item(
    pool: &PgPool,
    job_id: &str,
    repo_id: &str,
    filename: &str,
    display_name: Option<&str>,
    kind: &str,
    quant: Option<&str>,
    context_length: Option<u32>,
) -> Result<i64> {
    let row = sqlx::query(
        "INSERT INTO pull_queue \
         (job_id, repo_id, filename, display_name, kind, quant, context_length) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id",
    )
    .bind(job_id)
    .bind(repo_id)
    .bind(filename)
    .bind(display_name)
    .bind(kind)
    .bind(quant)
    .bind(context_length.map(i64::from))
    .fetch_one(pool)
    .await?;
    Ok(row.get("id"))
}

/// Retrieve the oldest queued item (FIFO).
pub async fn get_queued_item(pool: &PgPool) -> Result<Option<PullQueueItem>> {
    let row = sqlx::query(GET_QUEUED_SQL).fetch_optional(pool).await?;
    row.map(|r| decode_item(&r)).transpose()
}

/// Update a queue item's status and related fields.
///
/// - `started_at` is set only if it's currently NULL (first time going to running).
/// - `completed_at` is set only when transitioning to a terminal state
///   (completed, failed, cancelled).
pub async fn update_queue_status(
    pool: &PgPool,
    job_id: &str,
    new_status: &str,
    bytes_pulled: i64,
    total_bytes: Option<i64>,
    error_message: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE pull_queue SET \
         status = $1, \
         bytes_pulled = $2, \
         total_bytes = $3, \
         error_message = $4, \
         started_at = COALESCE(started_at, now()), \
         completed_at = CASE WHEN $5 IN ('completed','failed','cancelled') \
             THEN now() ELSE completed_at END \
         WHERE job_id = $6",
    )
    .bind(new_status)
    .bind(bytes_pulled)
    .bind(total_bytes)
    .bind(error_message)
    .bind(new_status)
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update only the progress fields (bytes_pulled, total_bytes) without
/// changing the status. Used for real-time progress streaming via SSE.
pub async fn update_progress_only(
    pool: &PgPool,
    job_id: &str,
    bytes_pulled: i64,
    total_bytes: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE pull_queue SET \
         bytes_pulled = $1, \
         total_bytes = $2 \
         WHERE job_id = $3",
    )
    .bind(bytes_pulled)
    .bind(total_bytes)
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Atomically claim a queued item as running.
///
/// Returns `true` if a row was affected (item was queued, now running),
/// `false` if no row matched (item already started by someone else).
/// This is the atomic CAS guard that prevents double-starting pulls.
pub async fn try_mark_running(pool: &PgPool, job_id: &str) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE pull_queue SET \
         status = 'running', \
         started_at = COALESCE(started_at, now()) \
         WHERE job_id = $1 AND status = 'queued'",
    )
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Retrieve a queue item by its job_id.
pub async fn get_item_by_job_id(pool: &PgPool, job_id: &str) -> Result<Option<PullQueueItem>> {
    let row = sqlx::query(GET_BY_JOB_ID_SQL)
        .bind(job_id)
        .fetch_optional(pool)
        .await?;
    row.map(|r| decode_item(&r)).transpose()
}

/// Get all active items (queued, running, verifying), ordered by status priority then queued_at.
pub async fn get_active_items(pool: &PgPool) -> Result<Vec<PullQueueItem>> {
    let rows = sqlx::query(GET_ACTIVE_SQL).fetch_all(pool).await?;
    rows.iter().map(decode_item).collect::<Result<Vec<_>>>()
}

/// Get history items (completed, failed, cancelled), sorted newest first.
pub async fn get_history_items(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<PullQueueItem>> {
    let rows = sqlx::query(GET_HISTORY_SQL)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    rows.iter().map(decode_item).collect::<Result<Vec<_>>>()
}

/// Count total history items (completed, failed, cancelled).
pub async fn count_history_items(pool: &PgPool) -> Result<i64> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS n FROM pull_queue \
         WHERE status IN ('completed', 'failed', 'cancelled')",
    )
    .fetch_one(pool)
    .await?;
    Ok(row.get("n"))
}

/// Cancel a queue item if it hasn't reached a terminal state.
pub async fn cancel_queue_item(pool: &PgPool, job_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE pull_queue SET \
         status = 'cancelled', \
         completed_at = now() \
         WHERE job_id = $1 AND status IN ('queued', 'running', 'verifying')",
    )
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark stale running items as failed (process died without completing).
pub async fn mark_stale_running_as_failed(pool: &PgPool) -> Result<()> {
    sqlx::query(
        "UPDATE pull_queue SET \
         status = 'failed', \
         error_message = 'Download was interrupted (process restart)', \
         completed_at = now() \
         WHERE status IN ('running', 'verifying') AND completed_at IS NULL",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Check if there's an active pull (queued/running/verifying) for this repo_id + filename.
pub async fn get_active_item_by_repo_filename(
    pool: &PgPool,
    repo_id: &str,
    filename: &str,
) -> Result<Option<PullQueueItem>> {
    let row = sqlx::query(GET_ACTIVE_BY_REPO_FILENAME_SQL)
        .bind(repo_id)
        .bind(filename)
        .fetch_optional(pool)
        .await?;
    row.map(|r| decode_item(&r)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::postgres::with_schema;
    use sqlx::types::time::OffsetDateTime;

    async fn insert(
        pool: &PgPool,
        job_id: &str,
        repo_id: &str,
        filename: &str,
        display_name: Option<&str>,
        quant: Option<&str>,
        context_length: Option<u32>,
    ) -> i64 {
        insert_queue_item(
            pool,
            job_id,
            repo_id,
            filename,
            display_name,
            "model",
            quant,
            context_length,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_insert_and_get_queued() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        let id = insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            Some(4096),
        )
        .await;
        assert!(id > 0);

        let item = get_queued_item(pool).await.unwrap().unwrap();
        assert_eq!(item.job_id, "pull-abc123");
        assert_eq!(item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
        assert_eq!(item.filename, "Qwen3.6-35B-A3B-Q4_K_M.gguf");
        assert_eq!(item.display_name, Some("Qwen3.6 35B".to_string()));
        assert_eq!(item.status, "queued");
        assert_eq!(item.kind, "model");
        assert_eq!(item.quant, Some("Q4_K_M".to_string()));
        assert_eq!(item.context_length, Some(4096));

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_update_status_sets_timestamps() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            None,
        )
        .await;

        // Update to running — started_at should be set
        update_queue_status(pool, "pull-abc123", "running", 0, None, None)
            .await
            .unwrap();
        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "running");
        assert!(
            item.started_at.is_some(),
            "started_at should be set when going to running"
        );
        assert!(
            item.completed_at.is_none(),
            "completed_at should not be set when going to running"
        );

        // Update to completed — completed_at should be set
        update_queue_status(pool, "pull-abc123", "completed", 1000, Some(2000), None)
            .await
            .unwrap();
        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "completed");
        assert!(
            item.completed_at.is_some(),
            "completed_at should be set when going to completed"
        );

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_active_items_ordering() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(pool, "pull-1", "repo/1", "file1.gguf", None, None, None).await;

        insert(pool, "pull-2", "repo/2", "file2.gguf", None, None, None).await;
        update_queue_status(pool, "pull-2", "running", 500, Some(1000), None)
            .await
            .unwrap();

        insert(pool, "pull-3", "repo/3", "file3.gguf", None, None, None).await;
        update_queue_status(pool, "pull-3", "verifying", 1000, Some(1000), None)
            .await
            .unwrap();

        let items = get_active_items(pool).await.unwrap();
        assert_eq!(items.len(), 3);
        // Running should come first, then verifying, then queued
        assert_eq!(items[0].status, "running");
        assert_eq!(items[1].status, "verifying");
        assert_eq!(items[2].status, "queued");

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_cancel_queue_item() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            None,
        )
        .await;

        cancel_queue_item(pool, "pull-abc123").await.unwrap();

        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "cancelled");
        assert!(
            item.completed_at.is_some(),
            "completed_at should be set on cancel"
        );

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_cancel_does_not_affect_completed() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            None,
        )
        .await;

        // Mark as completed first
        update_queue_status(pool, "pull-abc123", "completed", 1000, Some(2000), None)
            .await
            .unwrap();

        // Try to cancel — should have no effect
        cancel_queue_item(pool, "pull-abc123").await.unwrap();

        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            item.status, "completed",
            "completed items should not be cancelled"
        );

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_history_items() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        // Insert completed item first
        insert(pool, "pull-1", "repo/1", "file1.gguf", None, None, None).await;
        update_queue_status(pool, "pull-1", "completed", 1000, Some(2000), None)
            .await
            .unwrap();

        // Small delay to ensure different completed_at timestamps
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Insert failed item second
        insert(pool, "pull-2", "repo/2", "file2.gguf", None, None, None).await;
        update_queue_status(
            pool,
            "pull-2",
            "failed",
            500,
            Some(1000),
            Some("connection error"),
        )
        .await
        .unwrap();

        let items = get_history_items(pool, 10, 0).await.unwrap();
        assert_eq!(items.len(), 2);
        // Should be sorted by completed_at DESC (newest first)
        assert_eq!(items[0].status, "failed");
        assert_eq!(items[1].status, "completed");

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_count_history_items() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(pool, "pull-1", "repo/1", "file1.gguf", None, None, None).await;
        update_queue_status(pool, "pull-1", "completed", 1000, Some(2000), None)
            .await
            .unwrap();

        insert(pool, "pull-2", "repo/2", "file2.gguf", None, None, None).await;
        update_queue_status(pool, "pull-2", "failed", 500, Some(1000), Some("error"))
            .await
            .unwrap();

        insert(pool, "pull-3", "repo/3", "file3.gguf", None, None, None).await;
        update_queue_status(pool, "pull-3", "cancelled", 0, None, None)
            .await
            .unwrap();

        // Insert a non-terminal item — should not be counted
        insert(pool, "pull-4", "repo/4", "file4.gguf", None, None, None).await;

        let count = count_history_items(pool).await.unwrap();
        assert_eq!(count, 3);

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_mark_stale_running_as_failed() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            None,
        )
        .await;

        // Manually set to running without completed_at (simulates process crash)
        sqlx::query(
            "UPDATE pull_queue SET status = 'running', started_at = now() \
             WHERE job_id = 'pull-abc123'",
        )
        .execute(pool)
        .await
        .unwrap();

        mark_stale_running_as_failed(pool).await.unwrap();

        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "failed");
        assert!(item.completed_at.is_some());
        assert_eq!(
            item.error_message.as_deref(),
            Some("Download was interrupted (process restart)")
        );

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_try_mark_running_succeeds() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            None,
        )
        .await;

        let claimed = try_mark_running(pool, "pull-abc123").await.unwrap();
        assert!(claimed, "should return true when claiming a queued item");

        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "running");

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_try_mark_running_fails_if_already_started() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            None,
        )
        .await;

        // Manually set to running so it's not queued anymore
        sqlx::query(
            "UPDATE pull_queue SET status = 'running', started_at = now() \
             WHERE job_id = 'pull-abc123'",
        )
        .execute(pool)
        .await
        .unwrap();

        let claimed = try_mark_running(pool, "pull-abc123").await.unwrap();
        assert!(!claimed, "should return false when item is already running");

        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.status, "running", "status should remain unchanged");

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_item_by_job_id() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(
            pool,
            "pull-abc123",
            "unsloth/Qwen3.6-35B-A3B-GGUF",
            "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            Some("Qwen3.6 35B"),
            Some("Q4_K_M"),
            None,
        )
        .await;

        let item = get_item_by_job_id(pool, "pull-abc123")
            .await
            .unwrap()
            .unwrap();
        assert!(item.id > 0);
        assert_eq!(item.job_id, "pull-abc123");
        assert_eq!(item.repo_id, "unsloth/Qwen3.6-35B-A3B-GGUF");
        assert_eq!(item.filename, "Qwen3.6-35B-A3B-Q4_K_M.gguf");
        assert_eq!(item.display_name, Some("Qwen3.6 35B".to_string()));
        assert_eq!(item.status, "queued");
        assert_eq!(item.kind, "model");
        assert!(item.queued_at <= OffsetDateTime::now_utc());

        // Non-existent job_id should return None
        let none_item = get_item_by_job_id(pool, "pull-nonexistent").await.unwrap();
        assert!(none_item.is_none());

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_get_active_item_by_repo_filename() {
        let guard = with_schema().await;
        let pool = &guard.pool;

        insert(pool, "pull-dup", "repo/dup", "dup.gguf", None, None, None).await;

        let item = get_active_item_by_repo_filename(pool, "repo/dup", "dup.gguf")
            .await
            .unwrap()
            .expect("active item should be found");
        assert_eq!(item.job_id, "pull-dup");

        // Terminal items are not returned
        cancel_queue_item(pool, "pull-dup").await.unwrap();
        let item = get_active_item_by_repo_filename(pool, "repo/dup", "dup.gguf")
            .await
            .unwrap();
        assert!(item.is_none());

        guard.finish().await;
    }
}
