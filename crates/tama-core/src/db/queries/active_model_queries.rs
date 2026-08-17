//! Active model database query functions (Postgres, plan-190 Task 5).
//!
//! All functions are async and take a `&PgPool` — the caller owns the pool.
//!
//! `loaded_at`/`last_accessed` are `TIMESTAMPTZ`; the shared
//! [`ActiveModelRecord`] type stores them as `String` in the v2 format, so
//! reads project with `to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')`.

use anyhow::Result;
use sqlx::{PgPool, Row};

use super::types::ActiveModelRecord;

const LIST_SQL: &str = "SELECT server_name, model_name, backend, pid, port, backend_url, \
     to_char(loaded_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS loaded_at, \
     to_char(last_accessed AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"') AS last_accessed \
     FROM active_models";

/// Insert or replace an active model entry when a backend is loaded.
pub async fn insert_active_model(
    pool: &PgPool,
    server_name: &str,
    model_name: &str,
    backend: &str,
    pid: i64,
    port: i64,
    backend_url: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO active_models
            (server_name, model_name, backend, pid, port, backend_url, loaded_at, last_accessed)
         VALUES ($1, $2, $3, $4, $5, $6, now(), now())
         ON CONFLICT (server_name) DO UPDATE SET
             model_name = EXCLUDED.model_name,
             backend = EXCLUDED.backend,
             pid = EXCLUDED.pid,
             port = EXCLUDED.port,
             backend_url = EXCLUDED.backend_url,
             loaded_at = EXCLUDED.loaded_at,
             last_accessed = EXCLUDED.last_accessed",
    )
    .bind(server_name)
    .bind(model_name)
    .bind(backend)
    .bind(pid)
    .bind(port)
    .bind(backend_url)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove an active model entry when a backend is unloaded.
pub async fn remove_active_model(pool: &PgPool, server_name: &str) -> Result<()> {
    sqlx::query("DELETE FROM active_models WHERE server_name = $1")
        .bind(server_name)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all active model entries (for status / cleanup).
pub async fn get_active_models(pool: &PgPool) -> Result<Vec<ActiveModelRecord>> {
    let rows = sqlx::query(LIST_SQL).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| ActiveModelRecord {
            server_name: r.get("server_name"),
            model_name: r.get("model_name"),
            backend: r.get("backend"),
            pid: r.get("pid"),
            port: r.get("port"),
            backend_url: r.get("backend_url"),
            loaded_at: r.get("loaded_at"),
            last_accessed: r.get("last_accessed"),
        })
        .collect())
}

/// Remove all active model entries (for startup cleanup).
pub async fn clear_active_models(pool: &PgPool) -> Result<()> {
    sqlx::query("DELETE FROM active_models")
        .execute(pool)
        .await?;
    Ok(())
}

/// Update last_accessed timestamp for an active model.
pub async fn touch_active_model(pool: &PgPool, server_name: &str) -> Result<()> {
    sqlx::query("UPDATE active_models SET last_accessed = now() WHERE server_name = $1")
        .bind(server_name)
        .execute(pool)
        .await?;
    Ok(())
}

/// Rename an active model by updating its primary key (server_name).
pub async fn rename_active_model(pool: &PgPool, old_name: &str, new_name: &str) -> Result<()> {
    sqlx::query("UPDATE active_models SET server_name = $2 WHERE server_name = $1")
        .bind(old_name)
        .bind(new_name)
        .execute(pool)
        .await?;
    Ok(())
}
