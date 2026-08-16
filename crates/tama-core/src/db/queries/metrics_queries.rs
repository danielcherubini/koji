//! System metrics database query functions (Postgres, plan-190 Task 4).
//!
//! All functions are async and take a `&PgPool` — the caller (the metrics
//! collector) owns the pool.

use anyhow::{bail, Context, Result};
use sqlx::{PgPool, Row};

/// One sample of system-level metrics, persisted in `system_metrics_history`.
#[derive(Debug, Clone)]
pub struct SystemMetricsRow {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: i64,
    pub ram_total_mib: i64,
    pub gpu_utilization_pct: Option<i64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub models_loaded: i64,
    pub tps: Option<f64>,
    pub prompt_tps: Option<f64>,
    pub cache_hit_pct: Option<f64>,
    pub spec_accept_pct: Option<f64>,
    pub net_rx_bytes: Option<i64>,
    pub net_tx_bytes: Option<i64>,
}

/// Decode a `system_metrics_history` row into [`SystemMetricsRow`].
fn decode_system_metrics_row(row: &sqlx::postgres::PgRow) -> SystemMetricsRow {
    SystemMetricsRow {
        ts_unix_ms: row.get("ts_unix_ms"),
        cpu_usage_pct: row.get::<f64, _>("cpu_usage_pct") as f32,
        ram_used_mib: row.get("ram_used_mib"),
        ram_total_mib: row.get("ram_total_mib"),
        gpu_utilization_pct: row.get("gpu_utilization_pct"),
        vram_used_mib: row.get("vram_used_mib"),
        vram_total_mib: row.get("vram_total_mib"),
        models_loaded: row.get("models_loaded"),
        tps: row.get("tps"),
        prompt_tps: row.get("prompt_tps"),
        cache_hit_pct: row.get("cache_hit_pct"),
        spec_accept_pct: row.get("spec_accept_pct"),
        net_rx_bytes: row.get("net_rx_bytes"),
        net_tx_bytes: row.get("net_tx_bytes"),
    }
}

/// Insert one sample and prune anything older than `cutoff_ms` in a single
/// transaction. Both operations succeed or fail together so a crash never
/// leaves the table half-pruned.
pub async fn insert_system_metric(
    pool: &PgPool,
    row: &SystemMetricsRow,
    cutoff_ms: i64,
) -> Result<()> {
    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin system_metrics_history transaction")?;
    sqlx::query(
        "INSERT INTO system_metrics_history
             (ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
              gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded,
              tps, prompt_tps, cache_hit_pct, spec_accept_pct,
              net_rx_bytes, net_tx_bytes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(row.ts_unix_ms)
    .bind(row.cpu_usage_pct as f64)
    .bind(row.ram_used_mib)
    .bind(row.ram_total_mib)
    .bind(row.gpu_utilization_pct)
    .bind(row.vram_used_mib)
    .bind(row.vram_total_mib)
    .bind(row.models_loaded)
    .bind(row.tps)
    .bind(row.prompt_tps)
    .bind(row.cache_hit_pct)
    .bind(row.spec_accept_pct)
    .bind(row.net_rx_bytes)
    .bind(row.net_tx_bytes)
    .execute(&mut *tx)
    .await
    .context("Failed to insert system metric")?;
    sqlx::query("DELETE FROM system_metrics_history WHERE ts_unix_ms < $1")
        .bind(cutoff_ms)
        .execute(&mut *tx)
        .await
        .context("Failed to prune old system metrics")?;
    tx.commit()
        .await
        .context("Failed to commit system_metrics_history transaction")?;
    Ok(())
}

/// Fetch all samples newer than `since_ms` (exclusive), oldest-first.
pub async fn get_system_metrics_since(
    pool: &PgPool,
    since_ms: i64,
) -> Result<Vec<SystemMetricsRow>> {
    let rows = sqlx::query(
        "SELECT ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded,
                tps, prompt_tps, cache_hit_pct, spec_accept_pct,
                net_rx_bytes, net_tx_bytes
         FROM system_metrics_history WHERE ts_unix_ms > $1 ORDER BY ts_unix_ms ASC",
    )
    .bind(since_ms)
    .fetch_all(pool)
    .await
    .context("Failed to read system_metrics_history")?;
    Ok(rows
        .into_iter()
        .map(|row| decode_system_metrics_row(&row))
        .collect())
}

/// Fetch the most recent `limit` samples, oldest-first.
pub async fn get_recent_system_metrics(pool: &PgPool, limit: i64) -> Result<Vec<SystemMetricsRow>> {
    if limit < 0 {
        bail!("limit must be >= 0");
    }
    let rows = sqlx::query(
        "SELECT ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded,
                tps, prompt_tps, cache_hit_pct, spec_accept_pct,
                net_rx_bytes, net_tx_bytes
         FROM system_metrics_history ORDER BY ts_unix_ms DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("Failed to read recent system_metrics_history")?;
    let mut rows: Vec<SystemMetricsRow> = rows
        .into_iter()
        .map(|row| decode_system_metrics_row(&row))
        .collect();
    rows.reverse(); // reverse to return oldest-first
    Ok(rows)
}
