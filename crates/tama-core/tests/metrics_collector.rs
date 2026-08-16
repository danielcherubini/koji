//! Collector integration test (plan-190, Task 4) — the 2s metrics collector
//! persists `SystemMetricsRow` samples to Postgres.

mod common;

use common::with_schema;
use std::sync::Arc;
use std::time::Duration;
use tama_core::proxy::server::ProxyServer;
use tama_core::proxy::ProxyState;

/// The collector ticks every 2s and must persist at least one sample to the
/// Postgres `system_metrics_history` table.
#[tokio::test]
async fn test_metrics_task_persists_to_db() {
    let guard = with_schema().await;
    let pool = Arc::new(guard.pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let config = tama_core::config::Config::default();
    let state = Arc::new(ProxyState::new(
        config,
        Some(tmp.path().to_path_buf()),
        Some(pool),
    ));

    let _server = ProxyServer::new(state.clone()).await;

    // The collector's first tick persists immediately and then sleeps 2s;
    // wait a bit past the first tick.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM system_metrics_history")
        .fetch_one(&*state.db_pool().unwrap())
        .await
        .unwrap();
    assert!(
        count >= 1,
        "Expected at least 1 row in system_metrics_history after 3s, got {}",
        count
    );

    guard.finish().await;
}
