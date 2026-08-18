//! Postgres support: the embedded migrations and their runner.
//!
//! `00000000000001_initial.sql` is the squashed final state of all 49
//! SQLite migrations (plan-190), mapped to Postgres types. Later schema
//! additions land as new numbered migration files — the initial migration
//! is never rewritten (sqlx checksum-validates applied migrations on
//! already-migrated databases). sqlx manages the `_sqlx_migrations`
//! bookkeeping table.

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
