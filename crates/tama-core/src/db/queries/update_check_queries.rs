//! Update check database query functions (Postgres, plan-190 Task 4).
//!
//! All functions are async and take a `&PgPool` — the caller owns the pool.

use anyhow::Result;
use sqlx::{PgPool, Row};

use super::types::UpdateCheckRecord;

/// Params for upserting an update check record.
#[derive(Debug, Clone)]
pub struct UpdateCheckParams<'a> {
    pub item_type: &'a str,
    pub item_id: &'a str,
    pub current_version: Option<&'a str>,
    pub latest_version: Option<&'a str>,
    pub update_available: bool,
    pub status: &'a str,
    pub error_message: Option<&'a str>,
    pub details_json: Option<&'a str>,
    pub checked_at: i64,
}

pub async fn upsert_update_check(pool: &PgPool, params: UpdateCheckParams<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO update_checks (item_type, item_id, current_version, latest_version, update_available, status, error_message, details_json, checked_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (item_type, item_id) DO UPDATE SET
             current_version = EXCLUDED.current_version,
             latest_version = EXCLUDED.latest_version,
             update_available = EXCLUDED.update_available,
             status = EXCLUDED.status,
             error_message = EXCLUDED.error_message,
             details_json = EXCLUDED.details_json,
             checked_at = EXCLUDED.checked_at",
    )
    .bind(params.item_type)
    .bind(params.item_id)
    .bind(params.current_version)
    .bind(params.latest_version)
    .bind(params.update_available)
    .bind(params.status)
    .bind(params.error_message)
    .bind(params.details_json)
    .bind(params.checked_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Decode an `update_checks` row into [`UpdateCheckRecord`].
fn decode_update_check(row: &sqlx::postgres::PgRow) -> UpdateCheckRecord {
    UpdateCheckRecord {
        item_type: row.get("item_type"),
        item_id: row.get("item_id"),
        current_version: row.get("current_version"),
        latest_version: row.get("latest_version"),
        update_available: row.get("update_available"),
        status: row.get("status"),
        error_message: row.get("error_message"),
        details_json: row.get("details_json"),
        checked_at: row.get("checked_at"),
    }
}

const SELECT_ALL: &str = "SELECT item_type, item_id, current_version, latest_version, \
     update_available, status, error_message, details_json, checked_at \
     FROM update_checks ORDER BY item_type, item_id";
const SELECT_ONE: &str = "SELECT item_type, item_id, current_version, latest_version, \
     update_available, status, error_message, details_json, checked_at \
     FROM update_checks WHERE item_type = $1 AND item_id = $2";

pub async fn get_all_update_checks(pool: &PgPool) -> Result<Vec<UpdateCheckRecord>> {
    let rows = sqlx::query(SELECT_ALL).fetch_all(pool).await?;
    Ok(rows.iter().map(decode_update_check).collect())
}

pub async fn get_update_check(
    pool: &PgPool,
    item_type: &str,
    item_id: &str,
) -> Result<Option<UpdateCheckRecord>> {
    let row = sqlx::query(SELECT_ONE)
        .bind(item_type)
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| decode_update_check(&r)))
}

pub async fn delete_update_check(pool: &PgPool, item_type: &str, item_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM update_checks WHERE item_type = $1 AND item_id = $2")
        .bind(item_type)
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete update check records matching a SQL LIKE pattern.
/// The pattern should have `_` and `%` already escaped with `\\` for literal matching.
/// Uses `ESCAPE '\'` so that `\\_` matches a literal `_` and `\\%` matches a literal `%`.
pub async fn delete_update_checks_by_pattern(
    pool: &PgPool,
    item_type: &str,
    item_id_pattern: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM update_checks WHERE item_type = $1 AND item_id LIKE $2 ESCAPE '\\'")
        .bind(item_type)
        .bind(item_id_pattern)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete all update check records for a backend name, covering every
/// gpu_variant (`name:%`) plus the legacy variant-less row (`name`).
/// Handles the SQL LIKE escaping of `name` internally so callers never
/// hand-write patterns.
pub async fn delete_update_checks_for_backend(pool: &PgPool, name: &str) -> Result<()> {
    let escaped = name
        .replace('\\', "\\\\")
        .replace('_', "\\_")
        .replace('%', "\\%");
    delete_update_checks_by_pattern(pool, "backend", &format!("{}:%", escaped)).await?;
    delete_update_check(pool, "backend", name).await?;
    Ok(())
}

pub async fn get_oldest_check_time(pool: &PgPool) -> Result<Option<i64>> {
    let row = sqlx::query("SELECT MIN(checked_at) AS oldest FROM update_checks")
        .fetch_one(pool)
        .await?;
    Ok(row.get("oldest"))
}
