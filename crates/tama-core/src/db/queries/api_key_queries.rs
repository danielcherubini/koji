//! Queries for the `api_keys` table.

use anyhow::Result;
use sqlx::{PgPool, Row};

/// Count the number of active (non-revoked, non-expired) API keys.
///
/// Used to derive the `api_keys_enabled` flag on the `app_proxy` table so
/// the flag can never drift from the actual key state. The flag is a
/// derived value — the source of truth is the `api_keys` table.
pub async fn count_active_keys(pool: &PgPool) -> Result<i64> {
    let row = sqlx::query(
        "SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())",
    )
    .fetch_one(pool)
    .await?;
    Ok(row.get::<i64, _>(0))
}
