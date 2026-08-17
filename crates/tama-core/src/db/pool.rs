//! Postgres pool creation and startup retry (plan-190, Task 2).
//!
//! `main.rs` is the single owner of the production pool: it creates the
//! pool here, waits for the server with [`connect_with_retry`], runs the
//! migrations, and shares the `Arc<PgPool>` with `ProxyState` and `WebState`.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::config::database::DatabaseConfig;

/// Maximum connections in the production pool.
const MAX_CONNECTIONS: u32 = 10;
/// Cap for the exponential startup-retry backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Create a *lazy* Postgres pool from a bootstrap config.
///
/// The pool never dials at construction time — call [`connect_with_retry`]
/// to wait for the server to accept connections. The password is resolved
/// from a possible `${VAR}` reference (fails on a missing env var).
pub async fn create_pool(cfg: &DatabaseConfig) -> anyhow::Result<PgPool> {
    let password = cfg.resolved_password()?;
    let mut resolved = cfg.clone();
    resolved.password = password;
    let pool = PgPoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        // Bound how long `acquire()` waits for a connection — also keeps
        // startup retries (connect_with_retry) from stalling 30s each.
        .acquire_timeout(Duration::from_secs(10))
        .connect_lazy(&resolved.dsn())?;
    Ok(pool)
}

/// Wait until the pool can acquire a connection, retrying with exponential
/// backoff (capped at 30s) forever. Intended for daemon startup:
/// the process stays alive while Postgres comes up, logging each attempt.
pub async fn connect_with_retry(pool: &PgPool, initial_backoff: Duration) -> anyhow::Result<()> {
    connect_with_retry_capped(pool, initial_backoff, None).await
}

/// Like [`connect_with_retry`] but bounded: gives up after `max_attempts`
/// consecutive acquire failures, returning an error. `None` = unbounded
/// (production behavior).
pub async fn connect_with_retry_capped(
    pool: &PgPool,
    initial_backoff: Duration,
    max_attempts: Option<u32>,
) -> anyhow::Result<()> {
    let mut backoff = initial_backoff;
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match pool.acquire().await {
            Ok(_conn) => return Ok(()),
            Err(error) => {
                if max_attempts.is_some_and(|max| attempts >= max) {
                    return Err(anyhow::anyhow!(
                        "Postgres is not reachable after {attempts} attempt(s): {error}"
                    ));
                }
                tracing::warn!(
                    error = %error,
                    attempt = attempts,
                    backoff_secs = backoff.as_secs_f64(),
                    "Postgres connection attempt failed; retrying"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// A lazily-created pool for tests that must hold a pool but never touch
/// the database. Re-exported from `tama-test-support` for in-crate tests.
#[cfg(test)]
pub use tama_test_support::test_dummy_pool;

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    fn dead_config() -> DatabaseConfig {
        // Port 1 (tcpmux) is virtually guaranteed closed on 127.0.0.1.
        DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 1,
            name: "tama".to_string(),
            user: "tama".to_string(),
            password: String::new(),
        }
    }

    /// A pool aimed at a dead port with a short acquire timeout so the
    /// tests fail fast instead of blocking on the 10s production default.
    fn dead_pool() -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://tama:tama@127.0.0.1:1/tama")
            .expect("valid pool config")
    }

    /// `create_pool` is lazy: it must succeed without dialing.
    #[tokio::test]
    async fn test_create_pool_is_lazy() {
        let pool = create_pool(&dead_config())
            .await
            .expect("lazy pool creation must not dial");
        pool.close().await;
    }

    /// Bounded retry gives up after the attempt cap against a dead port.
    #[tokio::test]
    async fn test_connect_with_retry_capped_gives_up() {
        let pool = dead_pool();
        let err = connect_with_retry_capped(&pool, Duration::from_millis(10), Some(2))
            .await
            .expect_err("closed port must fail");
        assert!(
            err.to_string().contains("2 attempt"),
            "error should name the attempt cap: {err:#}"
        );
        pool.close().await;
    }
}
