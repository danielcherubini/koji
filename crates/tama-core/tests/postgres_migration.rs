//! Postgres migration tests.
//!
//! Proves the squashed `00000000000001_initial.sql` migration produces the
//! correct final schema on a fresh Postgres, and cross-checks it against the
//! committed v49 SQLite DDL fixture to catch squash drift.

mod common;

use common::{test_pool, with_schema};
use sqlx::types::time::{OffsetDateTime, UtcOffset};

/// The committed v49 SQLite DDL fixture (ground truth for table/column sets).
const V49_FIXTURE: &str = include_str!("fixtures/v49_schema.sql");

/// Shadow tables the plan-193 T7 migrations DROP out of the v49 baseline: the
/// proxy's last v49-era model-state table dies with it (the tamad's wire rows
/// are the source of truth now). `desired_models` (post-v49, added by
/// `00000000000002`) is dropped by `00000000000004`;
/// `active_models` (v49-fixture table) by the unconditional drop
/// `00000000000005`. The zero-rows probe is diagnostic (NOTICE-only). The cross-checks below assert their
/// ABSENCE on a fresh schema — the pin that the shadow is dead.
const DROPPED_SHADOW_TABLES: &[&str] = &["desired_models", "active_models"];

/// Parse the table names declared in a chunk of SQL DDL.
///
/// Handles both `CREATE TABLE foo` and `CREATE TABLE "foo"` forms.
fn table_names_in(sql: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in sql.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("CREATE TABLE") else {
            continue;
        };
        let rest = rest.trim_start();
        let ident = if let Some(quoted) = rest.strip_prefix('"') {
            quoted.split('"').next().unwrap_or_default()
        } else {
            rest.split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
                .unwrap_or_default()
        };
        if !ident.is_empty() {
            names.push(ident.to_string());
        }
    }
    names
}

/// Fetch (table -> ordered columns) from the Postgres test schema.
async fn postgres_columns(guard: &common::SchemaGuard) -> Vec<(String, Vec<String>)> {
    let rows: Vec<(String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT table_name, column_name FROM information_schema.columns \
             WHERE table_schema = '{}' AND table_name <> '_sqlx_migrations' \
               ORDER BY table_name, ordinal_position",
        guard.schema
    )))
    .fetch_all(&guard.pool)
    .await
    .expect("query information_schema.columns");

    let mut tables: Vec<(String, Vec<String>)> = Vec::new();
    for (table, column) in rows {
        match tables.last_mut() {
            Some((t, cols)) if t == &table => cols.push(column),
            _ => tables.push((table, vec![column])),
        }
    }
    tables
}

#[tokio::test]
async fn test_run_migrations_ok_re_run_is_noop() {
    let guard = with_schema().await;

    // Re-running against an already-migrated schema is a successful no-op.
    tama_core::db::postgres::run_migrations(&guard.pool)
        .await
        .expect("re-run of migrations should succeed");

    let row: (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM {}._sqlx_migrations WHERE success = true",
        guard.schema
    )))
    .fetch_one(&guard.pool)
    .await
    .expect("query _sqlx_migrations");
    assert_eq!(
        row.0, 5,
        "one success row per migration file (initial + desired_models + pull_backend + the two T7 shadow drops)"
    );

    let _ = guard.finish().await;
}

#[tokio::test]
async fn test_all_squashed_tables_exist() {
    let guard = with_schema().await;

    // The v49 baseline minus the T7 drop (`active_models`); `desired_models`
    // was never part of the v49 fixture, so the list only shrinks.
    let mut expected: Vec<String> = table_names_in(V49_FIXTURE);
    expected.retain(|t| !DROPPED_SHADOW_TABLES.iter().any(|d| d == t));
    expected.sort();

    let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = '{}' AND table_type = 'BASE TABLE' \
           AND table_name <> '_sqlx_migrations' ORDER BY table_name",
        guard.schema
    )))
    .fetch_all(&guard.pool)
    .await
    .expect("query information_schema.tables");

    assert_eq!(
        rows, expected,
        "every table from the squashed schema must exist in the test schema"
    );

    let _ = guard.finish().await;
}

#[tokio::test]
async fn test_singleton_upsert_keeps_one_row() {
    let guard = with_schema().await;
    let sql = "INSERT INTO app_general (id) VALUES (1) ON CONFLICT (id) DO NOTHING";

    for _ in 0..2 {
        sqlx::query(sql)
            .execute(&guard.pool)
            .await
            .expect("singleton upsert");
    }

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM app_general")
        .fetch_one(&guard.pool)
        .await
        .expect("count app_general");
    assert_eq!(
        count, 1,
        "double insert must keep exactly one singleton row"
    );

    let _ = guard.finish().await;
}

#[tokio::test]
async fn test_timestamptz_round_trips_to_offset_datetime() {
    let guard = with_schema().await;

    let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000)
        .expect("valid timestamp")
        .to_offset(UtcOffset::from_whole_seconds(7_200).expect("valid offset"));

    sqlx::query(
        "INSERT INTO pull_log (repo_id, filename, started_at, completed_at, success) \
         VALUES ('acme/model', 'm.gguf', $1, now(), false)",
    )
    .bind(ts)
    .execute(&guard.pool)
    .await
    .expect("insert pull_log row");

    let (got,): (OffsetDateTime,) =
        sqlx::query_as("SELECT started_at FROM pull_log WHERE repo_id = 'acme/model'")
            .fetch_one(&guard.pool)
            .await
            .expect("read back started_at");

    assert_eq!(
        got, ts,
        "TIMESTAMPTZ must round-trip to OffsetDateTime preserving instant + offset"
    );

    let _ = guard.finish().await;
}

#[tokio::test]
async fn test_identity_auto_increments() {
    let guard = with_schema().await;

    for i in 0..2 {
        sqlx::query(
            "INSERT INTO pull_log (repo_id, filename, started_at, success) \
             VALUES ('acme/model', $1, now(), false)",
        )
        .bind(format!("f{i}.gguf"))
        .execute(&guard.pool)
        .await
        .expect("insert without explicit id");
    }

    let rows: Vec<i64> = sqlx::query_scalar("SELECT id FROM pull_log ORDER BY id")
        .fetch_all(&guard.pool)
        .await
        .expect("select ids");

    assert_eq!(rows, vec![1, 2], "identity ids must auto-increment from 1");

    let _ = guard.finish().await;
}

#[tokio::test]
async fn test_identity_accepts_explicit_ids() {
    let guard = with_schema().await;

    // Explicit ids work: proves BY DEFAULT (not ALWAYS) identity choice.
    sqlx::query(
        "INSERT INTO pull_log (id, repo_id, filename, started_at, success) \
         VALUES (100, 'acme/model', 'explicit.gguf', now(), false)",
    )
    .execute(&guard.pool)
    .await
    .expect("explicit id insert");

    sqlx::query(
        "INSERT INTO pull_log (repo_id, filename, started_at, success) \
         VALUES ('acme/model', 'next.gguf', now(), false)",
    )
    .execute(&guard.pool)
    .await
    .expect("auto insert after explicit id");

    // A second auto row: the identity sequence is unaffected by explicit
    // inserts (same as SQLite AUTOINCREMENT), so this takes id 2.
    sqlx::query(
        "INSERT INTO pull_log (repo_id, filename, started_at, success) \
         VALUES ('acme/model', 'third.gguf', now(), false)",
    )
    .execute(&guard.pool)
    .await
    .expect("auto insert after explicit id");

    let rows: Vec<i64> = sqlx::query_scalar("SELECT id FROM pull_log ORDER BY id")
        .fetch_all(&guard.pool)
        .await
        .expect("select ids");

    assert_eq!(
        rows,
        vec![1, 2, 100],
        "identity must keep auto-incrementing alongside explicit ids (BY DEFAULT semantics)"
    );

    let _ = guard.finish().await;
}

#[tokio::test]
async fn test_shared_pool_is_available() {
    // The shared container pool is up and serving queries.
    let pool = test_pool().await;
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&pool)
        .await
        .expect("select 1 on shared pool");
    assert_eq!(one, 1);
}

#[tokio::test]
async fn test_pg_schema_matches_v49_fixture() {
    let guard = with_schema().await;

    // Apply the committed v49 SQLite fixture to a temp file.
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("v49.db");
    let conn = rusqlite::Connection::open(&db_path).expect("open temp sqlite db");
    conn.execute_batch(V49_FIXTURE).expect("apply v49 fixture");

    let sqlite_tables: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .expect("list tables");
        stmt.query_map([], |r| r.get::<_, String>(0))
            .expect("iter tables")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect tables")
    };
    let mut sqlite_entries: Vec<(String, Vec<String>)> = Vec::new();
    for table in &sqlite_tables {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .expect("prepare table_info");
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("iter columns")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect columns");
        sqlite_entries.push((table.clone(), cols));
    }
    drop(conn);

    // (table -> ordered columns) from the migrated Postgres schema. The T7 drop
    // migrations carve these tables out of the baseline, so the cross-check pins
    // their ABSENCE on a fresh schema and compares the rest strictly against
    // the v49 baseline (minus the drops).
    let all_pg: Vec<(String, Vec<String>)> = postgres_columns(&guard).await;

    for t in DROPPED_SHADOW_TABLES {
        assert!(
            !all_pg.iter().any(|(name, _)| name == t),
            "shadow table '{t}' must be dropped on a fresh schema (plan-193 T7)"
        );
    }

    let mut sqlite_tables: Vec<String> = sqlite_tables.clone();
    sqlite_tables.retain(|t| !DROPPED_SHADOW_TABLES.iter().any(|d| d == t));
    sqlite_tables.sort();
    let mut sqlite_entries_cmp: Vec<(String, Vec<String>)> = sqlite_entries.clone();
    sqlite_entries_cmp.retain(|(t, _)| !DROPPED_SHADOW_TABLES.iter().any(|d| d == t));
    sqlite_entries_cmp.sort();

    let mut pg: Vec<(String, Vec<String>)> = postgres_columns(&guard).await;
    pg.retain(|(t, _)| !DROPPED_SHADOW_TABLES.iter().any(|d| d == t));
    pg.sort_by(|a, b| a.0.cmp(&b.0));
    let mut pg_table_names: Vec<String> = pg.iter().map(|(t, _)| t.clone()).collect();
    pg_table_names.sort();

    assert_eq!(
        pg_table_names, sqlite_tables,
        "table sets must match between the Postgres schema and the v49 fixture (minus T7 drops)"
    );
    assert_eq!(
        pg, sqlite_entries_cmp,
        "column lists must match (modulo type mapping + T7 drops)"
    );

    let _ = guard.finish().await;
}
