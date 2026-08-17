//! Postgres ports of the `metrics_queries` tests (plan-190, Task 4 —
//! system metrics history moves to Postgres).
//!
//! These mirror the former in-file SQLite tests 1:1 against the async
//! `&PgPool` API on an isolated migrated schema, plus the insert+prune
//! atomicity test for the single-transaction write path.

mod common;

use common::with_schema;
use tama_core::db::queries::{
    get_recent_system_metrics, get_system_metrics_since, insert_system_metric, SystemMetricsRow,
};

/// Helper to create a test metrics row.
fn make_row(ts: i64, cpu: f32, ram: i64) -> SystemMetricsRow {
    SystemMetricsRow {
        ts_unix_ms: ts,
        cpu_usage_pct: cpu,
        ram_used_mib: ram,
        ram_total_mib: 16384,
        gpu_utilization_pct: Some(75),
        vram_used_mib: Some(8192),
        vram_total_mib: Some(16384),
        models_loaded: 1,
        tps: None,
        prompt_tps: None,
        cache_hit_pct: None,
        spec_accept_pct: None,
        net_rx_bytes: None,
        net_tx_bytes: None,
    }
}

#[tokio::test]
async fn test_insert_system_metric() {
    let guard = with_schema().await;
    let row = make_row(1000, 45.5, 8192);
    insert_system_metric(&guard.pool, &row, 0).await.unwrap();

    let metrics = get_system_metrics_since(&guard.pool, 0).await.unwrap();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].ts_unix_ms, 1000);
    assert!((metrics[0].cpu_usage_pct - 45.5).abs() < f32::EPSILON);

    guard.finish().await;
}

#[tokio::test]
async fn test_insert_system_metric_with_cutoff() {
    let guard = with_schema().await;
    // Insert an old row
    insert_system_metric(&guard.pool, &make_row(100, 10.0, 1000), 500)
        .await
        .unwrap();
    // Insert a new row
    insert_system_metric(&guard.pool, &make_row(1000, 45.5, 8192), 500)
        .await
        .unwrap();

    // Old row should have been pruned
    let metrics = get_system_metrics_since(&guard.pool, 0).await.unwrap();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].ts_unix_ms, 1000);

    guard.finish().await;
}

/// The insert and the prune-by-cutoff are one transaction: two rows inserted
/// with an old cutoff, then a third row whose cutoff falls between the first
/// two — only the newest row remains.
#[tokio::test]
async fn test_insert_and_prune_atomic_within_one_transaction() {
    let guard = with_schema().await;
    // Two rows, no pruning yet
    insert_system_metric(&guard.pool, &make_row(100, 10.0, 1000), 0)
        .await
        .unwrap();
    insert_system_metric(&guard.pool, &make_row(300, 30.0, 3000), 0)
        .await
        .unwrap();
    assert_eq!(
        get_system_metrics_since(&guard.pool, 0)
            .await
            .unwrap()
            .len(),
        2
    );

    // Insert a new row with a cutoff between the two old rows — both old
    // rows must be pruned in the same transaction as the insert.
    insert_system_metric(&guard.pool, &make_row(500, 50.0, 5000), 400)
        .await
        .unwrap();

    let metrics = get_system_metrics_since(&guard.pool, 0).await.unwrap();
    assert_eq!(metrics.len(), 1, "only the newest row should remain");
    assert_eq!(metrics[0].ts_unix_ms, 500);

    guard.finish().await;
}

#[tokio::test]
async fn test_get_system_metrics_since_empty() {
    let guard = with_schema().await;
    let metrics = get_system_metrics_since(&guard.pool, 0).await.unwrap();
    assert!(metrics.is_empty());

    guard.finish().await;
}

#[tokio::test]
async fn test_get_system_metrics_since_filter() {
    let guard = with_schema().await;
    insert_system_metric(&guard.pool, &make_row(100, 10.0, 1000), 0)
        .await
        .unwrap();
    insert_system_metric(&guard.pool, &make_row(200, 20.0, 2000), 0)
        .await
        .unwrap();
    insert_system_metric(&guard.pool, &make_row(300, 30.0, 3000), 0)
        .await
        .unwrap();

    let metrics = get_system_metrics_since(&guard.pool, 150).await.unwrap();
    assert_eq!(metrics.len(), 2);
    assert_eq!(metrics[0].ts_unix_ms, 200);
    assert_eq!(metrics[1].ts_unix_ms, 300);

    guard.finish().await;
}

#[tokio::test]
async fn test_get_system_metrics_since_ordered() {
    let guard = with_schema().await;
    insert_system_metric(&guard.pool, &make_row(300, 30.0, 3000), 0)
        .await
        .unwrap();
    insert_system_metric(&guard.pool, &make_row(100, 10.0, 1000), 0)
        .await
        .unwrap();
    insert_system_metric(&guard.pool, &make_row(200, 20.0, 2000), 0)
        .await
        .unwrap();

    let metrics = get_system_metrics_since(&guard.pool, 0).await.unwrap();
    assert_eq!(metrics[0].ts_unix_ms, 100);
    assert_eq!(metrics[1].ts_unix_ms, 200);
    assert_eq!(metrics[2].ts_unix_ms, 300);

    guard.finish().await;
}

#[tokio::test]
async fn test_get_recent_system_metrics_empty() {
    let guard = with_schema().await;
    let metrics = get_recent_system_metrics(&guard.pool, 10).await.unwrap();
    assert!(metrics.is_empty());

    guard.finish().await;
}

#[tokio::test]
async fn test_get_recent_system_metrics_limit() {
    let guard = with_schema().await;
    for i in 1..=5 {
        insert_system_metric(
            &guard.pool,
            &make_row(i * 100, i as f32 * 10.0, i * 1000),
            0,
        )
        .await
        .unwrap();
    }

    let metrics = get_recent_system_metrics(&guard.pool, 3).await.unwrap();
    assert_eq!(metrics.len(), 3);
    // Should return the 3 most recent, oldest-first
    assert_eq!(metrics[0].ts_unix_ms, 300);
    assert_eq!(metrics[1].ts_unix_ms, 400);
    assert_eq!(metrics[2].ts_unix_ms, 500);

    guard.finish().await;
}

#[tokio::test]
async fn test_get_recent_system_metrics_ordered() {
    let guard = with_schema().await;
    insert_system_metric(&guard.pool, &make_row(300, 30.0, 3000), 0)
        .await
        .unwrap();
    insert_system_metric(&guard.pool, &make_row(100, 10.0, 1000), 0)
        .await
        .unwrap();
    insert_system_metric(&guard.pool, &make_row(200, 20.0, 2000), 0)
        .await
        .unwrap();

    let metrics = get_recent_system_metrics(&guard.pool, 10).await.unwrap();
    // Should be ordered oldest-first
    assert_eq!(metrics[0].ts_unix_ms, 100);
    assert_eq!(metrics[1].ts_unix_ms, 200);
    assert_eq!(metrics[2].ts_unix_ms, 300);

    guard.finish().await;
}

#[tokio::test]
async fn test_get_recent_system_metrics_zero_limit() {
    let guard = with_schema().await;
    insert_system_metric(&guard.pool, &make_row(100, 10.0, 1000), 0)
        .await
        .unwrap();

    let metrics = get_recent_system_metrics(&guard.pool, 0).await.unwrap();
    assert!(metrics.is_empty());

    guard.finish().await;
}

#[tokio::test]
async fn test_get_recent_system_metrics_negative_limit_error() {
    let guard = with_schema().await;
    let result = get_recent_system_metrics(&guard.pool, -1).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("limit must be >= 0"));

    guard.finish().await;
}

#[tokio::test]
async fn test_system_metrics_row_with_null_gpu() {
    let guard = with_schema().await;
    let row = SystemMetricsRow {
        ts_unix_ms: 1000,
        cpu_usage_pct: 50.0,
        ram_used_mib: 8192,
        ram_total_mib: 16384,
        gpu_utilization_pct: None,
        vram_used_mib: None,
        vram_total_mib: None,
        models_loaded: 0,
        tps: None,
        prompt_tps: None,
        cache_hit_pct: None,
        spec_accept_pct: None,
        net_rx_bytes: None,
        net_tx_bytes: None,
    };
    insert_system_metric(&guard.pool, &row, 0).await.unwrap();

    let metrics = get_system_metrics_since(&guard.pool, 0).await.unwrap();
    assert_eq!(metrics.len(), 1);
    assert!(metrics[0].gpu_utilization_pct.is_none());
    assert!(metrics[0].vram_used_mib.is_none());

    guard.finish().await;
}

/// The 4 inference columns (tps, prompt_tps, cache_hit_pct, spec_accept_pct)
/// and the network byte columns round-trip through insert + query.
#[tokio::test]
async fn test_inference_columns_exist_and_queryable() {
    let guard = with_schema().await;

    // Insert a row with non-null inference values
    let row = SystemMetricsRow {
        ts_unix_ms: 1000,
        cpu_usage_pct: 50.0,
        ram_used_mib: 8192,
        ram_total_mib: 16384,
        gpu_utilization_pct: Some(75),
        vram_used_mib: Some(8192),
        vram_total_mib: Some(16384),
        models_loaded: 1,
        tps: Some(25.5),
        prompt_tps: Some(150.0),
        cache_hit_pct: Some(92.3),
        spec_accept_pct: Some(67.8),
        net_rx_bytes: Some(1048576),
        net_tx_bytes: Some(524288),
    };
    insert_system_metric(&guard.pool, &row, 0).await.unwrap();

    // Query back and verify the values
    let metrics = get_system_metrics_since(&guard.pool, 0).await.unwrap();
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].tps, Some(25.5));
    assert_eq!(metrics[0].prompt_tps, Some(150.0));
    assert_eq!(metrics[0].cache_hit_pct, Some(92.3));
    assert_eq!(metrics[0].spec_accept_pct, Some(67.8));
    assert_eq!(metrics[0].net_rx_bytes, Some(1048576));
    assert_eq!(metrics[0].net_tx_bytes, Some(524288));

    guard.finish().await;
}
