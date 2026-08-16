//! Postgres support: the squashed embedded migration and its runner.
//!
//! The single `00000000000001_initial.sql` migration is the final state of
//! all 49 SQLite migrations (plan-190), mapped to Postgres types. sqlx
//! manages the `_sqlx_migrations` bookkeeping table.

use anyhow::Context;
use sqlx::PgPool;

/// Embedded Postgres migrations (squashed final schema).
pub const MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Apply all pending Postgres migrations to the pool's database.
pub async fn run_migrations(pool: &PgPool) -> anyhow::Result<()> {
    MIGRATIONS
        .run(pool)
        .await
        .context("running Postgres migrations")
}
