//! Regression tests for the migration runner and individual migrations.
//!
//! Tests exercise `run()`, `run_up_to()`, and `FkGuard` behaviour.

use super::{run, run_up_to, FkGuard, LATEST_VERSION, MIGRATIONS};
use rusqlite::Connection;

/// Compile-time safety: MIGRATIONS must be strictly ordered by version with no
/// duplicates, and the last entry must match LATEST_VERSION.
#[test]
fn test_migrations_registry_is_ordered_and_complete() {
    assert!(!MIGRATIONS.is_empty(), "MIGRATIONS must not be empty");

    for window in MIGRATIONS.windows(2) {
        let (prev_version, _, _) = window[0];
        let (curr_version, _, _) = window[1];
        assert!(
            curr_version > prev_version,
            "migration versions must be strictly increasing: found {} then {}",
            prev_version,
            curr_version,
        );
    }

    let last_version = MIGRATIONS.last().map(|(v, _, _)| *v).unwrap();
    assert_eq!(
        last_version, LATEST_VERSION,
        "last migration version ({}) must equal LATEST_VERSION ({})",
        last_version, LATEST_VERSION,
    );
}

#[test]
fn test_migration_v6_creates_update_checks_table() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='update_checks'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let idx_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_update_checks_type'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(idx_count, 1);
}

#[test]
fn test_migration_v7_creates_model_configs_table() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='model_configs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let kind_column_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_files') WHERE name='kind'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind_column_exists, 1);
}

/// Migration v9 rebuilds model_configs with COLLATE NOCASE on repo_id so
/// inserting the same repo id in different cases is rejected as a conflict
/// and ON CONFLICT(repo_id) upserts fire.
#[test]
fn test_migration_v9_repo_id_is_case_insensitive() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    run(&conn).unwrap();

    conn.execute(
        "INSERT INTO model_configs (repo_id, backend) VALUES ('Foo/Bar', 'llama_cpp')",
        [],
    )
    .unwrap();

    // Case-variant insert must fail as a UNIQUE violation.
    let err = conn.execute(
        "INSERT INTO model_configs (repo_id, backend) VALUES ('foo/bar', 'llama_cpp')",
        [],
    );
    assert!(
        err.is_err(),
        "case-variant repo_id should conflict with UNIQUE COLLATE NOCASE"
    );

    // ON CONFLICT(repo_id) should fire across case variants too.
    conn.execute(
        "INSERT INTO model_configs (repo_id, backend) VALUES (?, 'llama_cpp')
         ON CONFLICT(repo_id) DO UPDATE SET backend = 'ik_llama'",
        ["FOO/BAR"],
    )
    .unwrap();
    let backend: String = conn
        .query_row(
            "SELECT backend FROM model_configs WHERE repo_id = 'Foo/Bar'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        backend, "ik_llama",
        "ON CONFLICT(repo_id) must match case-insensitively"
    );

    // WHERE repo_id = ? must match case-insensitively too.
    let row_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_configs WHERE repo_id = 'FOO/BAR'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(row_count, 1, "WHERE should match ignoring case");
}

/// Migration v9 must deduplicate pre-existing case-variant rows rather
/// than fail on the new UNIQUE constraint.
#[test]
fn test_migration_v9_dedupes_existing_case_variants() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

    conn.execute_batch(
        r#"
        CREATE TABLE tmp_cfg (id INTEGER PRIMARY KEY, repo_id TEXT NOT NULL);
        INSERT INTO tmp_cfg (id, repo_id) VALUES (1, 'Foo/Bar'), (2, 'foo/bar'), (3, 'Other');
        DELETE FROM tmp_cfg WHERE id NOT IN (
            SELECT MIN(id) FROM tmp_cfg GROUP BY LOWER(repo_id)
        );
        "#,
    )
    .unwrap();
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM tmp_cfg", [], |row| row.get(0))
        .unwrap();
    assert_eq!(remaining, 2, "dedupe should keep one per lower(repo_id)");

    // Also verify the full migration applies cleanly on an empty DB.
    run(&conn).unwrap();
}

/// Regression test for the v9 FK-cascade bug. Before the fix, running v9
/// on a DB with existing `model_files` rows would wipe those rows because
/// `DROP TABLE model_configs` (with `foreign_keys=ON`) performs an
/// implicit `DELETE FROM model_configs`, which cascades through
/// `ON DELETE CASCADE` to `model_files`. The migration must preserve all
/// referencing rows.
#[test]
fn test_migration_v9_preserves_model_files_rows() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

    // Bring the DB up to v8 — the pre-v9 schema where model_configs
    // exists with a case-sensitive UNIQUE constraint on repo_id.
    run_up_to(&conn, 8).unwrap();

    // Seed a model_configs row and two model_files rows that reference it.
    conn.execute(
        "INSERT INTO model_configs (id, repo_id, backend) VALUES (1, 'unsloth/Qwen3.6-35B-A3B-GGUF', 'llama_cpp')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO model_files (model_id, repo_id, filename, quant, size_bytes, downloaded_at, kind) \
         VALUES (1, 'unsloth/Qwen3.6-35B-A3B-GGUF', 'Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf', 'UD-Q4_K_XL', 22360456160, '2026-04-16T20:00:00Z', 'model')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO model_files (model_id, repo_id, filename, quant, size_bytes, downloaded_at, kind) \
         VALUES (1, 'unsloth/Qwen3.6-35B-A3B-GGUF', 'mmproj-F16.gguf', NULL, 899283680, '2026-04-16T20:00:00Z', 'mmproj')",
        [],
    )
    .unwrap();

    // Sanity: rows are present before the migration.
    let files_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(files_before, 2);

    // Apply v9.
    run(&conn).unwrap();

    // The model_configs row must survive (same id, same repo_id).
    let configs_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_configs WHERE id=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        configs_after, 1,
        "model_configs row 1 must survive the rebuild"
    );

    // All referencing model_files rows must survive. Before the fix this
    // was 0 because DROP TABLE cascaded.
    let files_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_files WHERE model_id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        files_after, 2,
        "both model_files rows must survive migration v9"
    );

    // Foreign keys must be re-enabled after the migration completes, so
    // subsequent DB activity enforces referential integrity.
    let fk_on: i32 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    assert_eq!(fk_on, 1, "foreign_keys must be re-enabled after migration");
}

/// Regression test for the v10 ON CONFLICT bug. Before the fix,
/// `upsert_model_pull` used `ON CONFLICT(model_id)` but the
/// `model_pulls` table had no UNIQUE constraint on `model_id`,
/// causing `refresh_metadata` to fail and leave all file hashes
/// unbaked.
#[test]
fn test_migration_v10_adds_model_pulls_unique_index() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring the DB up to v9 — the pre-v10 schema.
    run_up_to(&conn, 9).unwrap();

    // Verify the unique index does NOT exist yet.
    let idx_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_model_pulls_model_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(idx_before, 0);

    // Apply v10.
    run(&conn).unwrap();

    // Verify the unique index now exists.
    let idx_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_model_pulls_model_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(idx_after, 1);

    // Verify ON CONFLICT(model_id) now works.
    conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    conn.execute(
        "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
         VALUES (1, 'test/repo', 'abc123', '2024-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
         VALUES (1, 'test/repo', 'def456', '2024-01-02T00:00:00Z') \
         ON CONFLICT(model_id) DO UPDATE SET commit_sha=excluded.commit_sha",
        [],
    )
    .unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

    // Verify the row was upserted (commit_sha updated).
    let commit_sha: String = conn
        .query_row(
            "SELECT commit_sha FROM model_pulls WHERE model_id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(commit_sha, "def456");
}

/// Regression test for the FK toggle not restored on error path.
///
/// Before the RAII guard fix, if a migration that required `foreign_keys=OFF`
/// failed mid-execution (e.g., invalid SQL), the subsequent
/// `PRAGMA foreign_keys=ON` would never run, permanently disabling FK
/// enforcement for the rest of the session.
///
/// This test verifies that the `FkGuard` struct properly re-enables FKs
/// even when an error occurs inside its scope.
#[test]
fn test_fk_guard_restores_on_error() {
    let conn = Connection::open_in_memory().unwrap();

    // Verify FKs start enabled.
    let fk_before: i32 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    assert_eq!(fk_before, 1);

    // Disable FKs via guard and trigger an error inside its scope.
    let guard_result = FkGuard::disable(&conn).unwrap();

    // Verify FKs are now off.
    let fk_off: i32 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    assert_eq!(fk_off, 0);

    // Simulate an error occurring inside the guard's scope by dropping it early.
    drop(guard_result);

    // FKs must be re-enabled after the guard is dropped (even without an explicit ON call).
    let fk_after: i32 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    assert_eq!(fk_after, 1, "FKs must be re-enabled after FkGuard drops");
}

/// Test that FKs remain enabled when guard is not used (normal path).
#[test]
fn test_fk_guard_noop_when_not_used() {
    let conn = Connection::open_in_memory().unwrap();

    let fk_before: i32 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    assert_eq!(fk_before, 1);

    // Don't use the guard — FKs should stay enabled.
    let _ = ();

    let fk_after: i32 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    assert_eq!(fk_after, 1);
}

/// Regression test: migration v18 must add cache_type_k and cache_type_v
/// columns to model_configs.
#[test]
fn test_migration_v18_adds_cache_type_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let k_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='cache_type_k'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let v_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='cache_type_v'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(k_exists, 1);
    assert_eq!(v_exists, 1);
}

/// Regression test: migration v19 must add 9 HF metadata columns to model_configs.
#[test]
fn test_migration_v19_adds_hf_metadata_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let columns = [
        "hf_format",
        "hf_base_model",
        "hf_pipeline_tag",
        "hf_total_params",
        "hf_active_params",
        "hf_architecture_type",
        "hf_context_length",
        "hf_num_layers",
        "hf_last_modified",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 1,
            "column '{}' should exist after migration v19",
            col
        );
    }
}

/// Regression test: migration v20 must add gpu_variant column to backend_installations
/// with UNIQUE(name, gpu_variant, version) constraint.
#[test]
fn test_migration_v20_adds_gpu_variant() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    // gpu_variant column must exist
    let variant_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('backend_installations') WHERE name='gpu_variant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(variant_exists, 1, "gpu_variant column must exist");

    // Index on (name, gpu_variant) must exist
    let idx_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_backend_installations_name_variant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        idx_exists, 1,
        "idx_backend_installations_name_variant must exist"
    );

    // Test that UNIQUE(name, gpu_variant, version) works: same name + variant + version fails
    conn.execute(
        "INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_variant, is_active) \
         VALUES ('llama_cpp', 'llama_cpp', 'b8407', '/tmp/a', 1, 'cpu', 1)",
        [],
    )
    .unwrap();

    // Same name + variant + version should fail
    let err = conn.execute(
        "INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_variant, is_active) \
         VALUES ('llama_cpp', 'llama_cpp', 'b8407', '/tmp/b', 2, 'cpu', 0)",
        [],
    );
    assert!(
        err.is_err(),
        "duplicate (name, gpu_variant, version) must fail"
    );

    // Same name + different variant should succeed
    conn.execute(
        "INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_variant, is_active) \
         VALUES ('llama_cpp', 'llama_cpp', 'b8407', '/tmp/c', 3, 'vulkan', 0)",
        [],
    )
    .unwrap();

    // Verify both rows exist
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM backend_installations WHERE name='llama_cpp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

/// Migration v20 must preserve existing backend_installations rows and set gpu_variant = 'cpu'.
#[test]
fn test_migration_v20_preserves_existing_data() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

    // Bring DB up to v19 (pre-v20 schema)
    run_up_to(&conn, 19).unwrap();

    // Insert a backend installation (without gpu_variant column)
    conn.execute(
        "INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_type, source, is_active) \
         VALUES ('llama_cpp', 'llama_cpp', 'b8407', '/tmp/llama', 1000, NULL, NULL, 1)",
        [],
    )
    .unwrap();

    // Apply v20 only
    run_up_to(&conn, 20).unwrap();

    // The row must survive with gpu_variant = 'cpu'
    let variant: String = conn
        .query_row(
            "SELECT gpu_variant FROM backend_installations WHERE name='llama_cpp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(variant, "cpu");

    // Other fields must be preserved
    let version: String = conn
        .query_row(
            "SELECT version FROM backend_installations WHERE name='llama_cpp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "b8407");
}

/// Regression test: migration v23 must create the backend_configs table
/// with the correct schema and index.
#[test]
fn test_migration_v23_creates_backend_configs() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    // Table must exist
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='backend_configs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_exists, 1);

    // Columns must exist
    let columns = [
        "id",
        "name",
        "gpu_variant",
        "default_args",
        "health_check_url",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('backend_configs') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist", col);
    }

    // Test UNIQUE(name, gpu_variant) constraint
    conn.execute(
        "INSERT INTO backend_configs (name, gpu_variant, default_args, health_check_url) \
         VALUES ('llama_cpp', 'cpu', '[\"-fa 1\"]', 'http://localhost:8080/health')",
        [],
    )
    .unwrap();

    // Duplicate (name, gpu_variant) must fail
    let err = conn.execute(
        "INSERT INTO backend_configs (name, gpu_variant, default_args) \
         VALUES ('llama_cpp', 'cpu', '[]')",
        [],
    );
    assert!(err.is_err(), "duplicate (name, gpu_variant) must fail");

    // Same name, different variant must succeed
    conn.execute(
        "INSERT INTO backend_configs (name, gpu_variant, default_args) \
         VALUES ('llama_cpp', 'vulkan', '[]')",
        [],
    )
    .unwrap();

    // Verify both rows exist
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM backend_configs WHERE name='llama_cpp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

/// Regression test: migration v24 must add spec_decoding column to model_configs.
#[test]
fn test_migration_v24_adds_spec_decoding_column() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v23 (pre-v24 schema)
    run_up_to(&conn, 23).unwrap();

    // Verify spec_decoding column does NOT exist yet
    let col_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='spec_decoding'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(col_before, 0, "spec_decoding should not exist before v24");

    // Insert a row before v24
    conn.execute(
        "INSERT INTO model_configs (repo_id, backend) VALUES ('test/model', 'llama_cpp')",
        [],
    )
    .unwrap();

    // Apply v24
    run(&conn).unwrap();

    // Verify spec_decoding column now exists
    let col_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='spec_decoding'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(col_after, 1, "spec_decoding column must exist after v24");

    // Existing row should have NULL for spec_decoding
    let null_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_configs WHERE spec_decoding IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        null_count, 1,
        "existing rows should have NULL spec_decoding"
    );

    // Insert a row with JSON spec_decoding
    let json = serde_json::json!({
        "specTypes": ["draft-mtp", "ngram-simple"],
        "nMax": 4,
        "nMin": 2,
        "draftNgl": 16
    });
    conn.execute(
        "INSERT INTO model_configs (repo_id, backend, spec_decoding) VALUES ('test/model2', 'llama_cpp', ?)",
        [json.to_string()],
    )
    .unwrap();

    // Verify round-trip
    let stored: String = conn
        .query_row(
            "SELECT spec_decoding FROM model_configs WHERE repo_id = 'test/model2'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(
        parsed["specTypes"],
        serde_json::json!(["draft-mtp", "ngram-simple"])
    );
    assert_eq!(parsed["nMax"], 4);
    assert_eq!(parsed["nMin"], 2);
    assert_eq!(parsed["draftNgl"], 16);
}

/// Regression test: migration v25 must create the last_used_model table
/// with the correct schema.
#[test]
fn test_migration_v25_creates_last_used_model_table() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v24 (pre-v25 schema)
    run_up_to(&conn, 24).unwrap();

    // Verify table does NOT exist yet
    let table_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='last_used_model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        table_before, 0,
        "last_used_model should not exist before v25"
    );

    // Apply v25 (stop here — v27 drops last_used_model)
    run_up_to(&conn, 25).unwrap();

    // Verify table now exists
    let table_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='last_used_model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_after, 1, "last_used_model must exist after v25");

    // Verify columns
    let columns = ["id", "server_name", "model_name", "used_at"];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('last_used_model') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist", col);
    }

    // Verify INSERT OR REPLACE works (single row, id = 1)
    conn.execute(
        "INSERT OR REPLACE INTO last_used_model (id, server_name, model_name, used_at) \
         VALUES (1, 'test-server', 'test-model.gguf', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();

    // Verify round-trip
    let server_name: String = conn
        .query_row(
            "SELECT server_name FROM last_used_model WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(server_name, "test-server");
}

#[test]
fn test_migration_v26_rebuilds_model_configs() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v25
    run_up_to(&conn, 25).unwrap();

    // Insert a model config with num_parallel = 1 (to simulate existing data)
    conn.execute(
        "INSERT INTO model_configs (repo_id, display_name, num_parallel) \
         VALUES ('org/repo', 'Test Model', 1)",
        [],
    )
    .unwrap();

    // Apply v26
    run(&conn).unwrap();

    // Verify we can insert num_parallel = 0 (which would have failed under CHECK of v25)
    conn.execute(
        "INSERT INTO model_configs (repo_id, display_name, num_parallel) \
         VALUES ('org/repo-auto', 'Auto Model', 0)",
        [],
    )
    .unwrap();

    // Verify the original model config is still there and correct
    let num_parallel: i32 = conn
        .query_row(
            "SELECT num_parallel FROM model_configs WHERE repo_id = 'org/repo'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(num_parallel, 1);
}

/// Regression test: migration v27 must create model_aliases table, drop
/// last_used_model, and seed the default alias when enabled models exist.
#[test]
fn test_migration_v27_creates_model_aliases_and_drops_last_used_model() {
    use rusqlite::OptionalExtension;

    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v26 (pre-v27 schema)
    run_up_to(&conn, 26).unwrap();

    // Verify model_aliases does NOT exist yet
    let aliases_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='model_aliases'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        aliases_before, 0,
        "model_aliases should not exist before v27"
    );

    // Verify last_used_model DOES exist (created by v25)
    let last_used_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='last_used_model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        last_used_before, 1,
        "last_used_model should exist before v27"
    );

    // Insert an enabled model config so the seed INSERT has something to pick
    conn.execute(
        "INSERT INTO model_configs (repo_id, backend, enabled) \
         VALUES ('test/seed-model', 'llama_cpp', 1)",
        [],
    )
    .unwrap();

    // Apply v27
    run(&conn).unwrap();

    // (a) model_aliases table must exist with correct columns
    let aliases_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='model_aliases'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(aliases_after, 1, "model_aliases must exist after v27");

    let columns = [
        "id",
        "name",
        "model_id",
        "description",
        "enabled",
        "created_at",
        "updated_at",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_aliases') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist", col);
    }

    // (b) last_used_model table must NOT exist
    let last_used_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='last_used_model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(last_used_after, 0, "last_used_model must be dropped by v27");

    // (c) Default alias seeded when enabled models exist
    let default_alias: Option<String> = conn
        .query_row(
            "SELECT name FROM model_aliases WHERE name = 'whatevers-hot-n-fresh'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(
        default_alias,
        Some("whatevers-hot-n-fresh".to_string()),
        "default alias must be seeded"
    );

    // Verify the default alias points to the first enabled model
    let model_id: i64 = conn
        .query_row(
            "SELECT model_id FROM model_aliases WHERE name = 'whatevers-hot-n-fresh'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(model_id, 1);
}

/// When no enabled models exist, v27 must gracefully skip seeding.
#[test]
fn test_migration_v27_no_seed_without_enabled_models() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v26
    run_up_to(&conn, 26).unwrap();

    // Insert a model config but with enabled = 0
    conn.execute(
        "INSERT INTO model_configs (repo_id, backend, enabled) \
         VALUES ('test/disabled-model', 'llama_cpp', 0)",
        [],
    )
    .unwrap();

    // Apply v27
    run(&conn).unwrap();

    // No default alias should be seeded
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM model_aliases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "no default alias should be seeded when no enabled models exist"
    );
}

/// Migration v28 must add a `selected_mtp_model` column to `model_configs`.
/// The column must use COLLATE NOCASE so case-variant upserts behave the same
/// as for `selected_mmproj`.
#[test]
fn test_migration_v28_adds_selected_mtp_model_column() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let col_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='selected_mtp_model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        col_exists, 1,
        "selected_mtp_model column must exist after migration v28"
    );

    // Existing rows from earlier migrations should have NULL for the new column.
    let _null_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM model_configs WHERE selected_mtp_model IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // _null_count is informational; we don't fail on the count, but we want to
    // at least exercise the query path.

    // Insert a row with a value and confirm it round-trips.
    conn.execute(
        "INSERT INTO model_configs (repo_id, backend, selected_mtp_model) \
         VALUES ('test/mtp', 'llama_cpp', 'mtp-F16.gguf')",
        [],
    )
    .unwrap();
    let stored: Option<String> = conn
        .query_row(
            "SELECT selected_mtp_model FROM model_configs WHERE repo_id = 'test/mtp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored.as_deref(), Some("mtp-F16.gguf"));
}

/// Regression test: migration v29 must add the gpu_device column to model_configs.
#[test]
fn test_migration_v29_adds_gpu_device_column() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let col_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='gpu_device'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        col_exists, 1,
        "gpu_device column must exist after migration v29"
    );
}

/// Regression test: migration v30 must add net_rx_bytes and net_tx_bytes
/// columns to system_metrics_history.
#[test]
fn test_migration_v30_adds_network_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    for col in ["net_rx_bytes", "net_tx_bytes"] {
        let exists: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('system_metrics_history') WHERE name='{}'", col),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "{} column must exist after migration v30", col);
    }
}

/// Migration v31 must create all five app config tables.
#[test]
fn test_migration_v31_creates_app_config_tables() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let tables = [
        "app_general",
        "app_proxy",
        "app_lifecycle",
        "app_compaction",
        "sampling_templates",
    ];
    for table in &tables {
        let count: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{}'",
                    table
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "{} must exist after migration v31", table);
    }
}

/// Migration v31 must create all expected columns in app_general.
#[test]
fn test_migration_v31_app_general_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let columns = [
        "id",
        "log_level",
        "models_dir",
        "logs_dir",
        "hf_token",
        "update_check_interval",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_general') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist in app_general", col);
    }
}

/// Migration v31 must create all expected columns in app_proxy.
#[test]
fn test_migration_v31_app_proxy_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let columns = [
        "id",
        "host",
        "port",
        "auto_unload",
        "idle_timeout_secs",
        "startup_timeout_secs",
        "circuit_breaker_threshold",
        "circuit_breaker_cooldown_seconds",
        "metrics_retention_secs",
        "pull_queue_poll_interval_secs",
        "max_loaded_models",
        "authenticator_url",
        "authenticator_skip_paths",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_proxy') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist in app_proxy", col);
    }
}

/// Migration v31 must create all expected columns in app_lifecycle
/// (formerly app_supervisor; renamed by v40).
#[test]
fn test_migration_v31_app_supervisor_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let columns = [
        "id",
        "restart_policy",
        "max_restarts",
        "restart_delay_ms",
        "health_check_interval_ms",
        "health_check_timeout_ms",
        "health_check_retries",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_lifecycle') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist in app_lifecycle", col);
    }
}

/// Migration v31 must create all expected columns in app_compaction.
#[test]
fn test_migration_v31_app_compaction_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let columns = [
        "id",
        "enabled",
        "server_path",
        "device",
        "port",
        "request_timeout_ms",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_compaction') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist in app_compaction", col);
    }
}

/// Migration v31 must create all expected columns in sampling_templates.
#[test]
fn test_migration_v31_sampling_templates_columns() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    let columns = [
        "id",
        "name",
        "temperature",
        "top_k",
        "top_p",
        "min_p",
        "presence_penalty",
        "frequency_penalty",
        "repeat_penalty",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sampling_templates') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 1,
            "column '{}' must exist in sampling_templates",
            col
        );
    }
}

/// CHECK (id = 1) constraint on app_general must reject id != 1.
#[test]
fn test_migration_v31_app_general_check_constraint() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    // Valid insert with id = 1
    conn.execute(
        "INSERT INTO app_general (id, log_level) VALUES (1, 'info')",
        [],
    )
    .unwrap();

    // Invalid insert with id = 2 must fail
    let err = conn.execute(
        "INSERT INTO app_general (id, log_level) VALUES (2, 'debug')",
        [],
    );
    assert!(
        err.is_err(),
        "id != 1 must fail CHECK constraint on app_general"
    );
}

/// CHECK (id = 1) constraint on app_proxy must reject id != 1.
#[test]
fn test_migration_v31_app_proxy_check_constraint() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    conn.execute(
        "INSERT INTO app_proxy (id, host, port) VALUES (1, '0.0.0.0', 11434)",
        [],
    )
    .unwrap();

    let err = conn.execute(
        "INSERT INTO app_proxy (id, host, port) VALUES (2, '127.0.0.1', 8080)",
        [],
    );
    assert!(
        err.is_err(),
        "id != 1 must fail CHECK constraint on app_proxy"
    );
}

/// CHECK (id = 1) constraint on app_lifecycle must reject id != 1
/// (formerly app_supervisor; renamed by v40).
#[test]
fn test_migration_v31_app_supervisor_check_constraint() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    conn.execute(
        "INSERT INTO app_lifecycle (id, restart_policy) VALUES (1, 'always')",
        [],
    )
    .unwrap();

    let err = conn.execute(
        "INSERT INTO app_lifecycle (id, restart_policy) VALUES (2, 'never')",
        [],
    );
    assert!(
        err.is_err(),
        "id != 1 must fail CHECK constraint on app_lifecycle"
    );
}

/// CHECK (id = 1) constraint on app_compaction must reject id != 1.
#[test]
fn test_migration_v31_app_compaction_check_constraint() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    conn.execute(
        "INSERT INTO app_compaction (id, enabled, device) VALUES (1, 0, 'cpu')",
        [],
    )
    .unwrap();

    let err = conn.execute(
        "INSERT INTO app_compaction (id, enabled, device) VALUES (2, 1, 'cuda')",
        [],
    );
    assert!(
        err.is_err(),
        "id != 1 must fail CHECK constraint on app_compaction"
    );
}

/// UNIQUE constraint on sampling_templates.name must reject duplicate names.
#[test]
fn test_migration_v31_sampling_templates_unique_name() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    conn.execute(
        "INSERT INTO sampling_templates (name, temperature) VALUES ('coding', 0.3)",
        [],
    )
    .unwrap();

    // Duplicate name must fail
    let err = conn.execute(
        "INSERT INTO sampling_templates (name, temperature) VALUES ('coding', 0.7)",
        [],
    );
    assert!(
        err.is_err(),
        "duplicate name must fail UNIQUE constraint on sampling_templates"
    );
}

/// sampling_templates uses AUTOINCREMENT for id.
#[test]
fn test_migration_v31_sampling_templates_autoincrement() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    conn.execute(
        "INSERT INTO sampling_templates (name) VALUES ('coding')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO sampling_templates (name) VALUES ('chat')", [])
        .unwrap();

    // Autoincremented ids should be 1 and 2
    let id1: i64 = conn
        .query_row(
            "SELECT id FROM sampling_templates WHERE name = 'coding'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let id2: i64 = conn
        .query_row(
            "SELECT id FROM sampling_templates WHERE name = 'chat'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
}

/// Migration v35 must add all OAuth2 columns to app_proxy.
#[test]
fn test_migration_v35_adds_oauth2_columns() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v34 (pre-v35 schema)
    run_up_to(&conn, 34).unwrap();

    // Verify OAuth2 columns do NOT exist yet
    for col in [
        "oauth2_enabled",
        "oauth2_client_id",
        "oauth2_client_secret",
        "oauth2_authorize_url",
        "oauth2_token_url",
        "oauth2_userinfo_url",
        "oauth2_logout_url",
        "oauth2_redirect_uri",
        "oauth2_scopes",
        "oauth2_session_ttl_secs",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_proxy') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0, "column '{}' should NOT exist before v35", col);
    }

    // Apply v35
    run(&conn).unwrap();

    // Verify all OAuth2 columns now exist
    for col in [
        "oauth2_enabled",
        "oauth2_client_id",
        "oauth2_client_secret",
        "oauth2_authorize_url",
        "oauth2_token_url",
        "oauth2_userinfo_url",
        "oauth2_logout_url",
        "oauth2_redirect_uri",
        "oauth2_scopes",
        "oauth2_session_ttl_secs",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_proxy') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist after v35", col);
    }

    // Verify defaults: insert a row with only id, check OAuth2 defaults
    conn.execute("INSERT INTO app_proxy (id) VALUES (1)", [])
        .unwrap();

    let enabled: i32 = conn
        .query_row(
            "SELECT oauth2_enabled FROM app_proxy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(enabled, 0);

    let client_id: String = conn
        .query_row(
            "SELECT oauth2_client_id FROM app_proxy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(client_id, "");

    let ttl: i64 = conn
        .query_row(
            "SELECT oauth2_session_ttl_secs FROM app_proxy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ttl, 86400);

    // Verify scopes default
    let scopes_str: String = conn
        .query_row(
            "SELECT oauth2_scopes FROM app_proxy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let scopes: Vec<String> = serde_json::from_str(&scopes_str).unwrap();
    assert_eq!(scopes, vec!["openid", "profile", "email"]);

    // Verify NULL-able columns default to NULL
    let userinfo: Option<String> = conn
        .query_row(
            "SELECT oauth2_userinfo_url FROM app_proxy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(userinfo, None);

    let logout: Option<String> = conn
        .query_row(
            "SELECT oauth2_logout_url FROM app_proxy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(logout, None);
}

/// Migration v37 must create the app_langfuse table with correct schema.
#[test]
fn test_migration_v37_creates_app_langfuse_table() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v36 (pre-v37 schema)
    run_up_to(&conn, 36).unwrap();

    // Verify table does NOT exist yet
    let table_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_langfuse'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_before, 0, "app_langfuse should not exist before v37");

    // Apply v37 only (not run() which would apply all remaining migrations)
    run_up_to(&conn, 37).unwrap();

    // Verify table now exists
    let table_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_langfuse'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_after, 1, "app_langfuse must exist after v37");

    // Verify all columns
    let columns = [
        "id",
        "enabled",
        "public_key",
        "secret_key",
        "host",
        "environment",
        "capture_input",
        "capture_output",
        "capture_streaming",
        "telemetry_max_bytes",
        "electricity_price_per_kwh",
    ];
    for col in &columns {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('app_langfuse') WHERE name=?",
                [col],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "column '{}' must exist", col);
    }

    // Verify CHECK (id = 1) constraint
    conn.execute("INSERT INTO app_langfuse (id) VALUES (1)", [])
        .unwrap();

    let err = conn.execute("INSERT INTO app_langfuse (id) VALUES (2)", []);
    assert!(
        err.is_err(),
        "id != 1 must fail CHECK constraint on app_langfuse"
    );

    // Verify defaults: insert with only id, check defaults
    conn.execute("INSERT OR REPLACE INTO app_langfuse (id) VALUES (1)", [])
        .unwrap();

    let enabled: i32 = conn
        .query_row("SELECT enabled FROM app_langfuse WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(enabled, 0);

    let host: String = conn
        .query_row("SELECT host FROM app_langfuse WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(host, "https://cloud.langfuse.com");

    let environment: String = conn
        .query_row(
            "SELECT environment FROM app_langfuse WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(environment, "default");
}

/// Singleton tables must use default values when no explicit values are given.
#[test]
fn test_migration_v31_singleton_defaults() {
    let conn = Connection::open_in_memory().unwrap();
    run(&conn).unwrap();

    // Insert with only id (let defaults fill the rest)
    conn.execute("INSERT INTO app_general (id) VALUES (1)", [])
        .unwrap();
    let log_level: String = conn
        .query_row(
            "SELECT log_level FROM app_general WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_level, "info");

    let update_interval: i32 = conn
        .query_row(
            "SELECT update_check_interval FROM app_general WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(update_interval, 12);

    // app_proxy defaults
    conn.execute("INSERT INTO app_proxy (id) VALUES (1)", [])
        .unwrap();
    let host: String = conn
        .query_row("SELECT host FROM app_proxy WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(host, "0.0.0.0");

    let port: i32 = conn
        .query_row("SELECT port FROM app_proxy WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(port, 11434);

    // app_lifecycle defaults (formerly app_supervisor; renamed by v40)
    conn.execute("INSERT INTO app_lifecycle (id) VALUES (1)", [])
        .unwrap();
    let policy: String = conn
        .query_row(
            "SELECT restart_policy FROM app_lifecycle WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(policy, "always");

    // app_compaction defaults
    conn.execute("INSERT INTO app_compaction (id) VALUES (1)", [])
        .unwrap();
    let device: String = conn
        .query_row(
            "SELECT device FROM app_compaction WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(device, "cpu");
}

/// Migration v38 renames app_proxy.download_queue_poll_interval_secs → pull_queue_poll_interval_secs
#[test]
fn test_migration_v38_rename_app_proxy_poll_interval() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v37 (pre-v38 schema)
    run_up_to(&conn, 37).unwrap();

    // Verify old column exists and new one does not
    let old_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('app_proxy') WHERE name='download_queue_poll_interval_secs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        old_col, 1,
        "old column download_queue_poll_interval_secs must exist before v38"
    );

    let new_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('app_proxy') WHERE name='pull_queue_poll_interval_secs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        new_col, 0,
        "new column pull_queue_poll_interval_secs must not exist before v38"
    );

    // Insert a row with the old column name
    conn.execute(
        "INSERT INTO app_proxy (id, download_queue_poll_interval_secs) VALUES (1, 5)",
        [],
    )
    .unwrap();

    // Apply v38
    run_up_to(&conn, 38).unwrap();

    // Verify new column exists and old one does not
    let old_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('app_proxy') WHERE name='download_queue_poll_interval_secs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_col, 0, "old column must not exist after v38");

    let new_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('app_proxy') WHERE name='pull_queue_poll_interval_secs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(new_col, 1, "new column must exist after v38");

    // Verify data was preserved
    let value: i32 = conn
        .query_row(
            "SELECT pull_queue_poll_interval_secs FROM app_proxy WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, 5, "data must be preserved after column rename");
}

/// Migration v39 renames model_files.downloaded_at → pulled_at
#[test]
fn test_migration_v39_rename_model_files_pulled_at() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v38 (pre-v39 schema)
    run_up_to(&conn, 38).unwrap();

    // Verify old column exists and new one does not
    let old_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_files') WHERE name='downloaded_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_col, 1, "old column downloaded_at must exist before v39");

    let new_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_files') WHERE name='pulled_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(new_col, 0, "new column pulled_at must not exist before v39");

    // Disable FK so we can insert model_files without a real model_configs row
    conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();

    // Insert a row with the old column name
    conn.execute(
        "INSERT INTO model_files (id, model_id, repo_id, filename, downloaded_at) VALUES (1, 1, 'test/repo', 'model.gguf', '2025-01-01T00:00:00Z')",
        [],
    )
    .unwrap();

    // Re-enable FK
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

    // Apply v39
    run_up_to(&conn, 39).unwrap();

    // Verify new column exists and old one does not
    let old_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_files') WHERE name='downloaded_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_col, 0, "old column must not exist after v39");

    let new_col: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('model_files') WHERE name='pulled_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(new_col, 1, "new column must exist after v39");

    // Verify data was preserved
    let value: String = conn
        .query_row(
            "SELECT pulled_at FROM model_files WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        value, "2025-01-01T00:00:00Z",
        "data must be preserved after column rename"
    );
}

/// Migration v40 renames app_supervisor → app_lifecycle
#[test]
fn test_migration_v40_rename_app_supervisor_to_app_lifecycle() {
    let conn = Connection::open_in_memory().unwrap();

    // Bring DB up to v39 (pre-v40 schema)
    run_up_to(&conn, 39).unwrap();

    // Verify old table exists and new one does not
    let old_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_supervisor'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_table, 1, "app_supervisor must exist before v40");

    let new_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_lifecycle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(new_table, 0, "app_lifecycle must not exist before v40");

    // Seed a row with custom values (not defaults)
    conn.execute(
        "INSERT INTO app_supervisor (id, restart_policy, max_restarts, restart_delay_ms,
            health_check_interval_ms, health_check_timeout_ms, health_check_retries)
         VALUES (1, 'on-failure', 7, 4000, 6000, 15000, 4)",
        [],
    )
    .unwrap();

    // Verify the seeded row
    let max_restarts_before: i64 = conn
        .query_row(
            "SELECT max_restarts FROM app_supervisor WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(max_restarts_before, 7);

    // Apply v40
    run_up_to(&conn, 40).unwrap();

    // Verify old table no longer exists
    let old_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_supervisor'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_table, 0, "app_supervisor must not exist after v40");

    // Verify new table exists
    let new_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_lifecycle'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(new_table, 1, "app_lifecycle must exist after v40");

    // Verify data was preserved
    let max_restarts_after: i64 = conn
        .query_row(
            "SELECT max_restarts FROM app_lifecycle WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        max_restarts_after, 7,
        "data must be preserved after table rename"
    );

    // Verify all columns are intact
    let policy: String = conn
        .query_row(
            "SELECT restart_policy FROM app_lifecycle WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(policy, "on-failure");

    let delay: i64 = conn
        .query_row(
            "SELECT restart_delay_ms FROM app_lifecycle WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(delay, 4000);
}
