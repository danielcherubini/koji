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

/// Postgres-native tables added after the v49 SQLite baseline (plan-191
/// Task 5). These have no SQLite counterpart, so the v49-fixture
/// cross-checks account for them explicitly. They are applied by a
/// separate numbered migration — the shipped
/// `00000000000001_initial.sql` is never rewritten (sqlx
/// checksum-validates it on already-migrated databases).
const POST_V49_TABLES: &[&str] = &["desired_models"];

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
        row.0, 3,
        "one success row per migration file (initial + post-v49 desired_models + pull_backend)"
    );

    let _ = guard.finish().await;
}

#[tokio::test]
async fn test_all_squashed_tables_exist() {
    let guard = with_schema().await;

    let mut expected: Vec<String> = table_names_in(V49_FIXTURE);
    expected.extend(POST_V49_TABLES.iter().map(|t| t.to_string()));
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
async fn test_desired_models_tamad_index_exists() {
    let guard = with_schema().await;

    let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT indexname FROM pg_indexes \
         WHERE schemaname = '{}' AND tablename = 'desired_models' ORDER BY indexname",
        guard.schema
    )))
    .fetch_all(&guard.pool)
    .await
    .expect("query pg_indexes");

    assert!(
        rows.contains(&"idx_desired_models_tamad".to_string()),
        "idx_desired_models_tamad must exist on desired_models(tamad_id); found: {rows:?}"
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

    // (table -> ordered columns) from the migrated Postgres schema.
    // Drop the post-v49 Postgres-native tables (no SQLite counterpart) so
    // the comparison is strictly against the v49 baseline; assert those
    // tables exist separately.
    let all_pg: Vec<(String, Vec<String>)> = postgres_columns(&guard).await;
    let mut pg = all_pg.clone();
    pg.retain(|(t, _)| !POST_V49_TABLES.contains(&t.as_str()));
    pg.sort_by(|a, b| a.0.cmp(&b.0));
    let mut pg_table_names: Vec<String> = pg.iter().map(|(t, _)| t.clone()).collect();
    pg_table_names.sort();

    for t in POST_V49_TABLES {
        assert!(
            all_pg.iter().any(|(name, _)| name == t),
            "post-v49 table '{t}' must exist in the migrated schema"
        );
    }

    assert_eq!(
        pg_table_names, sqlite_tables,
        "table sets must match between Postgres schema and v49 fixture"
    );
    assert_eq!(
        pg, sqlite_entries,
        "column lists must match between Postgres schema and v49 fixture (modulo type mapping)"
    );

    let _ = guard.finish().await;
}
