//! Postgres pool startup integration tests (plan-190, Task 2).
//!
//! Real `postgres:16` container (shared harness): pool creation connects,
//! `run_migrations` applies the schema, and retry behavior is verified
//! against both the live container and a closed port.

mod common;

use std::time::Duration;

use common::with_schema;
use sqlx::postgres::PgPoolOptions;
use tama_core::config::database::DatabaseConfig;
use tama_core::db::pool::{connect_with_retry, connect_with_retry_capped, create_pool};

/// `create_pool` + `connect_with_retry` reach the shared container.
#[tokio::test]
async fn test_pool_connects_to_container() {
    let (host, port) = common::container_host_port();
    let cfg = DatabaseConfig {
        host,
        port,
        name: "tama".to_string(),
        user: "tama".to_string(),
        password: "tama".to_string(),
    };
    let pool = create_pool(&cfg).await.expect("pool creation");
    connect_with_retry(&pool, Duration::from_millis(100))
        .await
        .expect("container must be reachable");
    pool.close().await;
}

/// `run_migrations` produces a usable schema on a fresh test schema.
#[tokio::test]
async fn test_migrations_applied_by_pool_startup() {
    let guard = with_schema().await;
    let row: (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_name = 'model_configs'",
    ))
    .fetch_one(&guard.pool)
    .await
    .expect("information_schema query");
    assert_eq!(row.0, 1, "model_configs table must exist after migrations");
    guard.finish().await;
}

/// Closed port: bounded retry fails after the 2-attempt cap.
#[tokio::test]
async fn test_retry_closed_port_gives_up() {
    // Find a port that is almost certainly closed: bind, read, drop.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let cfg = DatabaseConfig {
        host: "127.0.0.1".to_string(),
        port,
        name: "tama".to_string(),
        user: "tama".to_string(),
        password: String::new(),
    };
    // Short acquire timeout so the bounded retries fail fast.
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy(&cfg.dsn())
        .expect("valid pool config");
    let err = connect_with_retry_capped(&pool, Duration::from_millis(20), Some(2))
        .await
        .expect_err("closed port must fail");
    assert!(
        err.to_string().contains("2 attempt"),
        "error should name the attempt cap: {err:#}"
    );
    pool.close().await;
}
