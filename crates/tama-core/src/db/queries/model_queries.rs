//! Model pull/file database query functions (Postgres, plan-190 Task 5).
//!
//! All functions are async and take a `&PgPool` — the caller owns the pool.
//!
//! Timestamps are `TIMESTAMPTZ`; the shared record types store them as
//! `String` in the v2 format, so reads project with
//! `to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')`.

use anyhow::Result;
use sqlx::{PgPool, Row};

use super::types::{ModelFileRecord, ModelPullRecord, PullLogEntry};

/// v2-format `to_char` projection of a `TIMESTAMPTZ` column.
const TS: &str = "YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"";

/// Insert or update the pull record for a model.
/// Uses `INSERT ... ON CONFLICT (model_id) DO UPDATE`.
pub async fn upsert_model_pull(
    pool: &PgPool,
    model_id: i64,
    repo_id: &str,
    commit_sha: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (model_id) DO UPDATE SET
             repo_id     = EXCLUDED.repo_id,
             commit_sha  = EXCLUDED.commit_sha,
             pulled_at   = EXCLUDED.pulled_at",
    )
    .bind(model_id)
    .bind(repo_id)
    .bind(commit_sha)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get the stored pull record for a model. Returns None if never pulled.
pub async fn get_model_pull(pool: &PgPool, model_id: i64) -> Result<Option<ModelPullRecord>> {
    const PULL_SQL: &str = "SELECT id, model_id, repo_id, commit_sha, \
         to_char(pulled_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS pulled_at \
         FROM model_pulls WHERE model_id = $1";
    let row = sqlx::query(PULL_SQL)
        .bind(model_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| ModelPullRecord {
        id: r.get("id"),
        model_id: r.get("model_id"),
        repo_id: r.get("repo_id"),
        commit_sha: r.get("commit_sha"),
        pulled_at: r.get("pulled_at"),
    }))
}

/// Get all stored pull records (backup manifest, plan-190 Task 9).
pub async fn get_all_model_pulls(pool: &PgPool) -> Result<Vec<ModelPullRecord>> {
    const PULL_SQL: &str = "SELECT id, model_id, repo_id, commit_sha, \
         to_char(pulled_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS pulled_at \
         FROM model_pulls ORDER BY repo_id";
    let rows = sqlx::query(PULL_SQL).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| ModelPullRecord {
            id: r.get("id"),
            model_id: r.get("model_id"),
            repo_id: r.get("repo_id"),
            commit_sha: r.get("commit_sha"),
            pulled_at: r.get("pulled_at"),
        })
        .collect())
}

/// Insert or update a file record for a pulled GGUF.
/// Uses `INSERT ... ON CONFLICT (model_id, filename) DO UPDATE`.
///
/// **Verification state preservation:** if a row already exists and the
/// incoming `lfs_oid` equals the stored one, the existing verification fields
/// are preserved. If the hash changed the verification columns are cleared so
/// the file will be re-verified.
pub async fn upsert_model_file(
    pool: &PgPool,
    model_id: i64,
    repo_id: &str,
    filename: &str,
    quant: Option<&str>,
    lfs_oid: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO model_files
             (model_id, repo_id, filename, quant, lfs_oid, size_bytes, pulled_at)
         VALUES ($1, $2, $3, $4, $5, $6, now())
         ON CONFLICT (model_id, filename) DO UPDATE SET
             repo_id       = EXCLUDED.repo_id,
             quant         = EXCLUDED.quant,
             lfs_oid       = EXCLUDED.lfs_oid,
             size_bytes    = EXCLUDED.size_bytes,
             pulled_at     = EXCLUDED.pulled_at,
             -- Only clear verification when the hash actually changed.
             last_verified_at = CASE
                 WHEN model_files.lfs_oid IS DISTINCT FROM EXCLUDED.lfs_oid THEN NULL
                 ELSE model_files.last_verified_at END,
             verified_ok = CASE
                 WHEN model_files.lfs_oid IS DISTINCT FROM EXCLUDED.lfs_oid THEN NULL
                 ELSE model_files.verified_ok END,
             verify_error = CASE
                 WHEN model_files.lfs_oid IS DISTINCT FROM EXCLUDED.lfs_oid THEN NULL
                 ELSE model_files.verify_error END",
    )
    .bind(model_id)
    .bind(repo_id)
    .bind(filename)
    .bind(quant)
    .bind(lfs_oid)
    .bind(size_bytes)
    .execute(pool)
    .await?;
    Ok(())
}

const FILE_SELECT: &str = "id, model_id, repo_id, filename, quant, lfs_oid, size_bytes, \
     to_char(pulled_at AT TIME ZONE 'UTC', '{TS}') AS pulled_at, \
     to_char(last_verified_at AT TIME ZONE 'UTC', '{TS}') AS last_verified_at, \
     verified_ok, verify_error";

/// Decode a row selected in `FILE_SELECT` order into a record.
fn decode_model_file(row: &sqlx::postgres::PgRow) -> ModelFileRecord {
    ModelFileRecord {
        id: row.get("id"),
        model_id: row.get("model_id"),
        repo_id: row.get("repo_id"),
        filename: row.get("filename"),
        quant: row.get("quant"),
        lfs_oid: row.get("lfs_oid"),
        size_bytes: row.get("size_bytes"),
        pulled_at: row.get("pulled_at"),
        last_verified_at: row.get("last_verified_at"),
        // Tri-state: Some(true) hash matched, Some(false) mismatch,
        // None never verified / no upstream hash.
        verified_ok: row.get("verified_ok"),
        verify_error: row.get("verify_error"),
    }
}

/// Get all stored file records for a model.
pub async fn get_model_files(pool: &PgPool, model_id: i64) -> Result<Vec<ModelFileRecord>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM model_files WHERE model_id = $1",
        FILE_SELECT.replace("{TS}", TS)
    )))
    .bind(model_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(decode_model_file).collect())
}

/// Get file records for a batch of models in one round trip
/// (`WHERE model_id = ANY($1)`), used by `db::load_model_configs` to avoid
/// the v2 per-model N+1 pattern.
pub async fn get_model_files_by_ids(
    pool: &PgPool,
    model_ids: &[i64],
) -> Result<Vec<ModelFileRecord>> {
    if model_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM model_files WHERE model_id = ANY($1)",
        FILE_SELECT.replace("{TS}", TS)
    )))
    .bind(model_ids.to_vec())
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(decode_model_file).collect())
}

/// Get all stored file records across all models.
pub async fn get_all_model_files(pool: &PgPool) -> Result<Vec<ModelFileRecord>> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM model_files",
        FILE_SELECT.replace("{TS}", TS)
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(decode_model_file).collect())
}

/// Delete a single model file record by (model_id, filename).
/// Does NOT touch model_pulls — the pull record stays.
pub async fn delete_model_file(pool: &PgPool, model_id: i64, filename: &str) -> Result<()> {
    sqlx::query("DELETE FROM model_files WHERE model_id = $1 AND filename = $2")
        .bind(model_id)
        .bind(filename)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update the `kind` column on a model file (model / mmproj / mtp).
pub async fn update_model_file_kind(
    pool: &PgPool,
    model_id: i64,
    filename: &str,
    kind: &str,
) -> Result<()> {
    sqlx::query("UPDATE model_files SET kind = $1 WHERE model_id = $2 AND filename = $3")
        .bind(kind)
        .bind(model_id)
        .bind(filename)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update the verification columns for a single file.
///
/// - `verified_ok = Some(true)`: hash matched; `verify_error` cleared.
/// - `verified_ok = Some(false)`: hash mismatch; caller should supply a short `verify_error`.
/// - `verified_ok = None`: no upstream hash available.
pub async fn update_verification(
    pool: &PgPool,
    model_id: i64,
    filename: &str,
    verified_ok: Option<bool>,
    verify_error: Option<&str>,
) -> Result<()> {
    let verify_error_param = if verified_ok == Some(true) {
        None
    } else {
        verify_error
    };
    let result = sqlx::query(
        "UPDATE model_files SET
              last_verified_at = now(),
              verified_ok      = $3,
              verify_error     = $4
          WHERE model_id = $1 AND filename = $2",
    )
    .bind(model_id)
    .bind(filename)
    .bind(verified_ok)
    .bind(verify_error_param)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        anyhow::bail!(
            "update_verification: no row found for model_id={} filename={}",
            model_id,
            filename
        );
    }
    Ok(())
}

/// Log a pull event (append-only).
pub async fn log_pull(pool: &PgPool, entry: &PullLogEntry) -> Result<()> {
    let _ = sqlx::query(
        "INSERT INTO pull_log
             (repo_id, filename, started_at, completed_at,
              size_bytes, duration_ms, success, error_message)
         VALUES ($1, $2, COALESCE(NULLIF($3, '')::timestamptz, now()), NULLIF($4, '')::timestamptz, $5, $6, $7, $8)",
    )
    .bind(&entry.repo_id)
    .bind(&entry.filename)
    .bind(&entry.started_at)
    .bind(&entry.completed_at)
    .bind(entry.size_bytes)
    .bind(entry.duration_ms)
    .bind(entry.success)
    .bind(&entry.error_message)
    .execute(pool)
    .await?;
    Ok(())
}
