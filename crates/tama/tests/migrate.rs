//! Integration tests for `tama migrate` (plan-190 Task 10).
//!
//! Fixture: the committed v49 SQLite schema
//! (`crates/tama/tests/fixtures/v49_schema.sql`) applied to a temp db via
//! rusqlite, with 1-3 rows per table. Target: the shared postgres:16 test
//! container — one dedicated schema per test (created by the harness with
//! the squashed migrations already applied), addressed through the
//! `options=-c search_path=<schema>` DSN parameter, which `migrate`
//! preserves verbatim from the user's URL.
//!
//! The bootstrap config.toml is written to a per-test tempdir via
//! `MigrateOpts::config_dir_override` (no XDG_CONFIG_HOME env races under
//! parallel test threads).

mod common;

use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const V49_SCHEMA: &str = include_str!("fixtures/v49_schema.sql");

/// SHA-256 of "sk-tama-test-key-0001" — the fixture's known API key.
const KNOWN_KEY_HASH: &str = "40ccee018c46de25a1d2d4af0f08c0569e4a8d430f10f739574f924202fa37e5";

/// Env var name the tests use for `--password-env` (value: "tama").
const TEST_PW_ENV: &str = "TAMA_MIGRATE_TEST_PW";

/// Expected row count per table in the standard fixture.
fn expected_counts() -> BTreeMap<&'static str, u64> {
    [
        ("app_compaction", 1),
        ("app_general", 1),
        ("app_langfuse", 1),
        ("app_lifecycle", 1),
        ("app_proxy", 1),
        ("model_configs", 2),
        ("model_files", 2),
        ("model_pulls", 1),
        ("model_aliases", 1),
        ("provider_configs", 1),
        ("provider_installations", 1),
        ("provider_registry", 1),
        ("api_keys", 1),
        ("pull_log", 1),
        ("pull_queue", 1),
        ("sampling_templates", 1),
        ("system_metrics_history", 1),
        ("tamad_registry", 1),
        ("tts_configs", 1),
        ("update_checks", 1),
        ("active_models", 1),
        ("benchmarks", 1),
    ]
    .into_iter()
    .collect()
}

/// DSN for the harness schema: user/password in the URL, the schema
/// selected via the `options` runtime parameter (preserved by `migrate`).
fn dsn_for(schema: &str) -> String {
    let (host, port) = common::container_host_port();
    format!("postgres://tama:tama@{host}:{port}/tama?options=-c+search_path={schema}")
}

fn opts_for(
    sqlite: PathBuf,
    schema: &str,
    config_dir: PathBuf,
    force: bool,
    dry_run: bool,
) -> tama_web::migrate::MigrateOpts {
    // Same key/value in every test — parallel set_var is harmless.
    std::env::set_var(TEST_PW_ENV, "tama");
    tama_web::migrate::MigrateOpts {
        sqlite_path: sqlite,
        db_url: dsn_for(schema),
        password_env: Some(TEST_PW_ENV.to_string()),
        dry_run,
        force,
        config_dir_override: Some(config_dir),
    }
}

/// Apply the v49 schema to `dir/tama.db`, insert the fixture rows, set
/// `PRAGMA user_version`. Returns the path + original file bytes.
///
/// `invalid = true` adds a type-invalid model_configs row (text in a
/// boolean column) and a model_files child row pointing at it.
fn build_fixture(dir: &Path, user_version: i32, invalid: bool) -> (PathBuf, Vec<u8>) {
    let path = dir.join("tama.db");
    let conn = Connection::open(&path).expect("open fixture db");
    conn.execute_batch(V49_SCHEMA).expect("apply v49 schema");
    insert_rows(&conn, invalid);
    conn.pragma_update(None, "user_version", user_version)
        .expect("set user_version");
    conn.close().expect("close fixture db");
    let path = dir.join("tama.db");
    (
        path.clone(),
        std::fs::read(&path).expect("read fixture bytes"),
    )
}

fn insert_rows(conn: &Connection, invalid: bool) {
    conn.execute_batch(
        r#"
INSERT INTO app_compaction (id, enabled, server_path, device, port, request_timeout_ms)
    VALUES (1, 0, NULL, 'cpu', NULL, 30000);
INSERT INTO app_general (id, log_level, models_dir, logs_dir, hf_token, update_check_interval)
    VALUES (1, 'debug', '/data/models', '/data/logs', 'hf_tok_123', 24);
INSERT INTO app_langfuse (id, enabled, public_key, secret_key, host, environment,
                          capture_input, capture_output, capture_streaming,
                          telemetry_max_bytes, electricity_price_per_kwh)
    VALUES (1, 1, 'pk-1', 'sk-1', 'https://lf.example.com', 'prod', 1, 0, 1, 2097152, 0.12);
INSERT INTO app_lifecycle (id, restart_policy, max_restarts, restart_delay_ms,
                           health_check_interval_ms, health_check_timeout_ms, health_check_retries)
    VALUES (1, 'on-failure', 5, 1000, 4000, 20000, 2);
INSERT INTO app_proxy (id, host, port, auto_unload, idle_timeout_secs, startup_timeout_secs,
                       circuit_breaker_threshold, circuit_breaker_cooldown_seconds,
                       metrics_retention_secs, pull_queue_poll_interval_secs, max_loaded_models,
                       authenticator_url, authenticator_skip_paths, oauth2_enabled,
                       oauth2_client_id, oauth2_client_secret, oauth2_authorize_url,
                       oauth2_token_url, oauth2_userinfo_url, oauth2_logout_url,
                       oauth2_redirect_uri, oauth2_scopes, oauth2_session_ttl_secs, api_keys_enabled)
    VALUES (1, '127.0.0.1', 11500, 1, 600, 120, 3, 60, 43200, 2, 2, NULL,
            '["/health"]', 0, '', '', '', '', NULL, NULL, '', '["openid"]', 86400, 1);

INSERT INTO model_configs (id, repo_id, display_name, backend, gpu_variant, enabled,
                           selected_quant, context_length, num_parallel, kv_unified, gpu_layers,
                           port, hf_format, hf_base_model, hf_pipeline_tag, hf_total_params,
                           hf_active_params, hf_architecture_type, hf_context_length,
                           hf_num_layers, hf_last_modified, created_at, updated_at,
                           gpu_device, n_batch, n_ubatch, backend_id, provider_name)
    VALUES (1, 'org/alpha', 'Alpha 7B', 'llama_cpp', 'cpu', 1, 'Q4_K_M', 8192, 1, 0, 99,
            12345, 'GGUF', 'org/alpha-base', 'text-generation', '7b', '7b', 'llama', 8192, 32,
            '2024-01-01T00:00:00.000Z', '2024-01-01T00:00:00.000Z', '2024-01-01T00:00:00.000Z',
            'cuda:0', 512, 128, 'llama-server', 'org');
INSERT INTO model_configs (id, repo_id, backend, gpu_variant, enabled, selected_quant,
                           context_length, num_parallel, kv_unified, created_at, updated_at,
                           backend_id)
    VALUES (2, 'org/beta', 'vllm', 'cuda', 1, 'FP8', 4096, 0, 0,
            '2024-02-02T00:00:00.000Z', '2024-02-02T00:00:00.000Z', 'vllm-server');

INSERT INTO model_files (id, model_id, repo_id, filename, quant, lfs_oid, size_bytes,
                         pulled_at, last_verified_at, verified_ok, verify_error, kind)
    VALUES (1, 1, 'org/alpha', 'alpha-Q4_K_M.gguf', 'Q4_K_M', 'oid-q4', 4000000000,
            '2024-01-02T00:00:00.000Z', '2024-01-03T00:00:00.000Z', 1, NULL, 'model');
INSERT INTO model_files (id, model_id, repo_id, filename, quant, lfs_oid, size_bytes,
                         pulled_at, last_verified_at, verified_ok, verify_error, kind)
    VALUES (2, 1, 'org/alpha', 'alpha-Q8_0.gguf', 'Q8_0', 'oid-q8', 8000000000,
            '2024-01-02T00:00:00.000Z', NULL, NULL, NULL, 'model');
INSERT INTO model_pulls (id, model_id, repo_id, commit_sha, pulled_at)
    VALUES (1, 1, 'org/alpha', 'abc123def456', '2024-01-02T00:00:00.000Z');
INSERT INTO model_aliases (id, name, model_id, description, enabled, created_at, updated_at)
    VALUES (1, 'alpha', 1, 'fast alias', 1, '2024-01-05T00:00:00.000Z', '2024-01-05T00:00:00.000Z');

INSERT INTO provider_configs (id, logical_id, name, gpu_variant, default_args,
                              health_check_url, default_env)
    VALUES (1, 'llama', 'llama-server', 'cpu', NULL, 'http://127.0.0.1:12345/health', NULL);
INSERT INTO provider_installations (id, name, backend_type, version, path, installed_at,
                                    gpu_variant, source, is_active, docker_config, logical_id)
    VALUES (1, 'llama-server', 'llama_cpp', 'b5000', '/opt/llama-server', 1700000000, 'cpu',
            'build', 1, NULL, 'llama');
INSERT INTO provider_registry (id, name, provider_type, engine, tamad_id, base_url, api_key,
                               created_at)
    VALUES (1, 'local', 'local', 'llama_cpp', NULL, NULL, NULL, 1700000000);

INSERT INTO api_keys (id, name, key_prefix, key_hash, scopes, created_by, created_at,
                      last_used_at, revoked_at, expires_at)
    VALUES (1, 'test-key', 'sk-ta',
            '40ccee018c46de25a1d2d4af0f08c0569e4a8d430f10f739574f924202fa37e5',
            'proxy', 'operator', '2024-03-01T00:00:00.000Z', NULL, NULL, NULL);

INSERT INTO pull_log (id, repo_id, filename, started_at, completed_at, size_bytes,
                      duration_ms, success, error_message)
    VALUES (1, 'org/alpha', 'alpha-Q4_K_M.gguf', '2024-01-01T10:00:00.000Z',
            '2024-01-01T11:00:00.000Z', 4000000000, 3600000, 1, NULL);
INSERT INTO pull_queue (id, job_id, repo_id, filename, display_name, status, bytes_pulled,
                        total_bytes, error_message, started_at, completed_at, queued_at, kind,
                        quant, context_length)
    VALUES (1, 'job-001', 'org/beta', 'beta-FP8.gguf', 'Beta', 'queued', 0, 8000000000, NULL,
            NULL, NULL, '2024-03-02T00:00:00.000Z', 'model', 'FP8', 4096);

INSERT INTO sampling_templates (id, name, temperature, top_k, top_p, min_p, presence_penalty,
                                frequency_penalty, repeat_penalty)
    VALUES (1, 'fast', 0.2, 20, 0.9, 0.0, 0.0, 0.0, 1.05);
INSERT INTO system_metrics_history (id, ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                                    gpu_utilization_pct, vram_used_mib, vram_total_mib,
                                    models_loaded, tps, prompt_tps, cache_hit_pct,
                                    spec_accept_pct, net_rx_bytes, net_tx_bytes)
    VALUES (1, 1700000000000, 42.5, 8192, 32768, 50, 1024, 24576, 1, 55.5, 300.0, 12.5, NULL,
            1000, 2000);
INSERT INTO tamad_registry (id, name, url, protocol, token, status)
    VALUES ('tamad-1', 'local-tamad', 'http://127.0.0.1:9090', 'grpc', 'tok-1', 'online');
INSERT INTO tts_configs (id, engine, default_voice, speed, format, enabled, created_at, updated_at)
    VALUES (1, 'kokoro', 'af_sky', 1.1, 'mp3', 1, '2024-01-01T00:00:00.000Z',
            '2024-01-01T00:00:00.000Z');
INSERT INTO update_checks (id, item_type, item_id, current_version, latest_version,
                           update_available, status, error_message, details_json, checked_at)
    VALUES (1, 'backend', 'llama-server', 'b4900', 'b5000', 1, 'checked', NULL, NULL, 1700000000);
INSERT INTO active_models (server_name, model_name, backend, pid, port, backend_url, loaded_at,
                           last_accessed, backend_id)
    VALUES ('my-coding-model', 'org/alpha', 'llama-server', 4242, 12345,
            'http://127.0.0.1:12345', '2024-04-01T00:00:00.000Z', '2024-04-02T00:00:00.000Z',
            'llama-server');
INSERT INTO benchmarks (id, created_at, model_id, display_name, quant, backend, engine, pp_sizes,
                        tg_sizes, threads, ngl_range, runs, warmup, results, load_time_ms,
                        vram_used_mib, vram_total_mib, duration_seconds, status, benchmark_type,
                        suite_id)
    VALUES (1, 1700000000, 'org/alpha', 'Alpha 7B', 'Q4_K_M', 'llama_cpp', 'llama_bench',
            '[512]', '[128]', NULL, NULL, 3, 1, '[{"pp":512,"tg":128,"tps":55.5}]', 1234.5,
            8000, 24576, 60.5, 'success', 'prompt', 'suite-1');
"#
    )
    .expect("insert fixture rows");

    if invalid {
        // Text in a boolean column: valid SQLite, invalid for Postgres.
        conn.execute_batch(
            r#"
INSERT INTO model_configs (id, repo_id, backend, enabled, created_at, updated_at)
    VALUES (3, 'org/gamma', 'llama_cpp', 'banana', '2024-05-01T00:00:00.000Z',
            '2024-05-01T00:00:00.000Z');
INSERT INTO model_files (id, model_id, repo_id, filename, quant, lfs_oid, size_bytes,
                         pulled_at, last_verified_at, verified_ok, verify_error, kind)
    VALUES (3, 3, 'org/gamma', 'gamma.gguf', NULL, NULL, NULL,
            '2024-05-01T00:00:00.000Z', NULL, NULL, NULL, 'model');
"#,
        )
        .expect("insert invalid rows");
    }
}

async fn pg_count(pool: &sqlx::PgPool, table: &str) -> u64 {
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .expect("count query") as u64
}

/// Find the migrate-report-*.json written next to the fixture db.
fn report_file(dir: &Path) -> PathBuf {
    std::fs::read_dir(dir)
        .expect("read fixture dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("migrate-report-"))
                .unwrap_or(false)
        })
        .expect("migrate report file not found")
}

#[tokio::test]
async fn test_dry_run_reports_counts_and_writes_nothing() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, bytes) = build_fixture(tmp.path(), 49, false);
    let config_dir = tmp.path().join("config");
    let opts = opts_for(db, &guard.schema, config_dir.clone(), false, true);

    let report = tama_web::migrate::run(opts)
        .await
        .expect("dry run succeeds");
    assert!(report.dry_run);
    for (table, n) in expected_counts() {
        assert_eq!(report.counts[table], [n, 0], "dry-run counts for {table}");
    }
    // Nothing was written to Postgres.
    assert_eq!(pg_count(&guard.pool, "model_configs").await, 0);
    assert_eq!(pg_count(&guard.pool, "api_keys").await, 0);
    // No bootstrap config, no report file.
    assert!(!config_dir.join("config.toml").exists());
    assert!(report_path_absent(tmp.path()));
    // SQLite untouched.
    assert_eq!(
        std::fs::read(tmp.path().join("tama.db")).unwrap(),
        bytes,
        "dry run must not modify the SQLite file"
    );
    guard.finish().await;
}

fn report_path_absent(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok()).all(|e| {
                !e.file_name()
                    .to_string_lossy()
                    .starts_with("migrate-report-")
            })
        })
        .unwrap_or(true)
}

#[tokio::test]
async fn test_full_migration_copies_all_tables() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, bytes) = build_fixture(tmp.path(), 49, false);
    let config_dir = tmp.path().join("config");
    let opts = opts_for(db, &guard.schema, config_dir.clone(), false, false);

    let report = tama_web::migrate::run(opts)
        .await
        .expect("migration succeeds");
    assert!(!report.dry_run);
    assert!(
        report.skipped.is_empty(),
        "no rows should be skipped: {:?}",
        report.skipped
    );

    // Per-table counts match the fixture; report agrees.
    for (table, n) in expected_counts() {
        assert_eq!(
            pg_count(&guard.pool, table).await,
            n,
            "postgres count for {table}"
        );
        assert_eq!(
            report.inserted[table], n,
            "report inserted count for {table}"
        );
    }

    // Spot values per table class.
    let row: (String, String, String, String, String) = sqlx::query_as(
        "SELECT repo_id, backend, selected_quant, backend_id, gpu_device \
         FROM model_configs WHERE id = 1",
    )
    .fetch_one(&guard.pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "org/alpha".into(),
            "llama_cpp".into(),
            "Q4_K_M".into(),
            "llama-server".into(),
            "cuda:0".into()
        )
    );
    let hash: String = sqlx::query_scalar("SELECT key_hash FROM api_keys WHERE name = 'test-key'")
        .fetch_one(&guard.pool)
        .await
        .unwrap();
    assert_eq!(hash, KNOWN_KEY_HASH, "API key hash must survive verbatim");
    let quant: String =
        sqlx::query_scalar("SELECT quant FROM model_files WHERE filename = 'alpha-Q8_0.gguf'")
            .fetch_one(&guard.pool)
            .await
            .unwrap();
    assert_eq!(quant, "Q8_0");
    let backend_url: String = sqlx::query_scalar(
        "SELECT backend_url FROM active_models WHERE server_name = 'my-coding-model'",
    )
    .fetch_one(&guard.pool)
    .await
    .unwrap();
    assert_eq!(backend_url, "http://127.0.0.1:12345");
    let tamad_status: String =
        sqlx::query_scalar("SELECT status FROM tamad_registry WHERE id = 'tamad-1'")
            .fetch_one(&guard.pool)
            .await
            .unwrap();
    assert_eq!(tamad_status, "online");
    let log_level: String = sqlx::query_scalar("SELECT log_level FROM app_general WHERE id = 1")
        .fetch_one(&guard.pool)
        .await
        .unwrap();
    assert_eq!(log_level, "debug");
    let results: String = sqlx::query_scalar("SELECT results FROM benchmarks WHERE id = 1")
        .fetch_one(&guard.pool)
        .await
        .unwrap();
    assert_eq!(results, "[{\"pp\":512,\"tg\":128,\"tps\":55.5}]");

    // Sequences continue past the max migrated id.
    let new_id: i64 =
        sqlx::query_scalar("INSERT INTO model_configs (repo_id) VALUES ('org/post') RETURNING id")
            .fetch_one(&guard.pool)
            .await
            .unwrap();
    assert!(new_id > 2, "expected sequence id > 2, got {new_id}");

    // SQLite file byte-identical.
    assert_eq!(
        std::fs::read(tmp.path().join("tama.db")).unwrap(),
        bytes,
        "migrate must not modify the SQLite file"
    );

    // Report JSON written next to the SQLite file.
    let report_path = report.report_path.clone().expect("report path set");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();
    assert_eq!(parsed["inserted"]["model_configs"], 2);
    assert_eq!(parsed["skipped"].as_array().unwrap().len(), 0);

    // Bootstrap config.toml created with the ${VAR} password form.
    let cfg_path = config_dir.join("config.toml");
    assert!(cfg_path.exists(), "bootstrap config.toml must be created");
    let cfg: toml::Value = toml::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        cfg["database"]["password"].as_str(),
        Some("${TAMA_MIGRATE_TEST_PW}")
    );
    let (host, port) = common::container_host_port();
    assert_eq!(cfg["database"]["host"].as_str(), Some(host.as_str()));
    assert_eq!(cfg["database"]["port"].as_integer(), Some(port as i64));
    assert_eq!(cfg["database"]["name"].as_str(), Some("tama"));
    assert_eq!(cfg["database"]["user"].as_str(), Some("tama"));

    guard.finish().await;
}

#[tokio::test]
async fn test_rerun_refuses_without_force() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, _bytes) = build_fixture(tmp.path(), 49, false);
    let config_dir = tmp.path().join("config");

    tama_web::migrate::run(opts_for(
        db.clone(),
        &guard.schema,
        config_dir.clone(),
        false,
        false,
    ))
    .await
    .expect("first run succeeds");

    let err = tama_web::migrate::run(opts_for(db, &guard.schema, config_dir, false, false))
        .await
        .expect_err("re-run without --force must refuse");
    assert!(
        err.to_string().to_lowercase().contains("already"),
        "refusal should mention existing data/config: {err}"
    );
    guard.finish().await;
}

#[tokio::test]
async fn test_force_rerun_adds_no_dupes() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, _bytes) = build_fixture(tmp.path(), 49, false);
    let config_dir = tmp.path().join("config");

    tama_web::migrate::run(opts_for(
        db.clone(),
        &guard.schema,
        config_dir.clone(),
        false,
        false,
    ))
    .await
    .expect("first run succeeds");

    let report = tama_web::migrate::run(opts_for(db, &guard.schema, config_dir, true, false))
        .await
        .expect("force re-run succeeds");
    assert_eq!(
        report.inserted["model_configs"], 0,
        "ON CONFLICT DO NOTHING: no rows re-inserted"
    );
    for (table, n) in expected_counts() {
        assert_eq!(pg_count(&guard.pool, table).await, n, "no dupes in {table}");
    }
    guard.finish().await;
}

#[tokio::test]
async fn test_force_preserves_comments_and_backs_up_existing_config() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, _bytes) = build_fixture(tmp.path(), 49, false);
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "# operator note: keep this comment\n[database]\nhost = \"old-host\"\n",
    )
    .unwrap();

    tama_web::migrate::run(opts_for(db, &guard.schema, config_dir.clone(), true, false))
        .await
        .expect("force run over existing config succeeds");

    let content = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(
        content.contains("# operator note: keep this comment"),
        "comments must be preserved, got:\n{content}"
    );
    let cfg: toml::Value = toml::from_str(&content).unwrap();
    let (host, _port) = common::container_host_port();
    assert_eq!(
        cfg["database"]["host"].as_str(),
        Some(host.as_str()),
        "host must be rewritten"
    );
    assert_eq!(
        cfg["database"]["password"].as_str(),
        Some("${TAMA_MIGRATE_TEST_PW}")
    );

    // A .bak-<ts> backup of the pre-existing file was created.
    let backups: Vec<_> = std::fs::read_dir(&config_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("config.toml.bak-")
        })
        .collect();
    assert_eq!(backups.len(), 1, "exactly one backup expected");
    let bak = std::fs::read_to_string(backups[0].path()).unwrap();
    assert!(bak.contains("old-host"));
    guard.finish().await;
}

#[tokio::test]
async fn test_invalid_rows_reported_valid_rows_committed() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, bytes) = build_fixture(tmp.path(), 49, true);
    let config_dir = tmp.path().join("config");
    let opts = opts_for(db, &guard.schema, config_dir, false, false);

    let report = tama_web::migrate::run(opts)
        .await
        .expect("run commits valid rows");
    assert_eq!(
        report.skipped.len(),
        2,
        "two rows must be skipped: {:?}",
        report.skipped
    );

    let mc = report
        .skipped
        .iter()
        .find(|s| s.table == "model_configs")
        .expect("model_configs row in report");
    assert_eq!(mc.id, "3");
    assert!(
        mc.error.to_lowercase().contains("boolean"),
        "conversion error should mention the type: {}",
        mc.error
    );
    let mf = report
        .skipped
        .iter()
        .find(|s| s.table == "model_files")
        .expect("child model_files row in report (FK violation)");
    assert_eq!(mf.id, "3");

    // Valid rows were still committed.
    assert_eq!(pg_count(&guard.pool, "model_configs").await, 2);
    assert_eq!(pg_count(&guard.pool, "model_files").await, 2);
    assert_eq!(pg_count(&guard.pool, "api_keys").await, 1);

    // SQLite untouched.
    assert_eq!(std::fs::read(tmp.path().join("tama.db")).unwrap(), bytes);

    // Skips land in the JSON report; the run committed.
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report_file(tmp.path())).unwrap()).unwrap();
    assert_eq!(
        parsed["committed"], true,
        "report must say the run committed"
    );
    let skipped = parsed["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 2);
    assert_eq!(skipped[0]["table"], "model_configs");
    assert_eq!(skipped[0]["id"], "3");
    guard.finish().await;
}

#[tokio::test]
async fn test_abort_writes_uncommitted_report() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, _bytes) = build_fixture(tmp.path(), 49, false);
    let config_dir = tmp.path().join("config");

    // Schema drift: a column in SQLite that the Postgres schema does not
    // have — copy_table bails, the run aborts, the transaction rolls back.
    {
        let conn = Connection::open(&db).expect("reopen fixture db");
        conn.execute("ALTER TABLE app_compaction ADD COLUMN drift_col TEXT", [])
            .expect("add drift column");
    }

    let err = tama_web::migrate::run(opts_for(db, &guard.schema, config_dir, false, false))
        .await
        .expect_err("schema drift must abort the migration");
    assert!(
        err.to_string().to_lowercase().contains("aborted"),
        "error should say the migration aborted: {err}"
    );

    // Target left untouched — the transaction rolled back.
    assert_eq!(pg_count(&guard.pool, "model_configs").await, 0);
    assert_eq!(pg_count(&guard.pool, "app_compaction").await, 0);
    assert_eq!(pg_count(&guard.pool, "api_keys").await, 0);

    // The report reflects the rollback: committed=false and no rows
    // inserted (the pre-failure counts were rolled back, not committed).
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report_file(tmp.path())).unwrap()).unwrap();
    assert_eq!(
        parsed["committed"], false,
        "report must say the run did not commit"
    );
    let inserted = parsed["inserted"].as_object().unwrap();
    for (table, n) in inserted {
        assert_eq!(n, 0, "inserted[{table}] must be 0 after a rollback");
    }
    guard.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn test_readonly_sqlite_file_still_migrates() {
    use std::os::unix::fs::PermissionsExt;
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, bytes) = build_fixture(tmp.path(), 49, false);
    let config_dir = tmp.path().join("config");

    // A read-only file (0o444) must not block the migration — the tool
    // opens the db read-only and never writes it.
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o444)).unwrap();

    let report = tama_web::migrate::run(opts_for(
        db.clone(),
        &guard.schema,
        config_dir,
        false,
        false,
    ))
    .await
    .expect("migration must succeed on a read-only SQLite file");
    assert_eq!(report.inserted["model_configs"], 2);
    assert_eq!(pg_count(&guard.pool, "model_configs").await, 2);

    // File bytes unchanged.
    assert_eq!(
        std::fs::read(&db).unwrap(),
        bytes,
        "migrate must not modify the read-only SQLite file"
    );
    // Restore so TempDir cleanup can remove the file.
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
    guard.finish().await;
}

#[tokio::test]
async fn test_refuses_old_user_version() {
    let guard = common::with_schema().await;
    let tmp = TempDir::new().unwrap();
    let (db, _bytes) = build_fixture(tmp.path(), 48, false);
    let config_dir = tmp.path().join("config");

    let err = tama_web::migrate::run(opts_for(db, &guard.schema, config_dir, false, false))
        .await
        .expect_err("v48 schema must be refused");
    assert!(
        err.to_string().to_lowercase().contains("upgrade"),
        "error should point at upgrading the v2 binary: {err}"
    );
    guard.finish().await;
}

#[test]
fn test_parse_db_url_decodes_percent_escaped_credentials() {
    let t = tama_web::migrate::parse_db_url(
        "postgres://my%40user:p%40ss%3A%25word@dbhost:5433/mydb?sslmode=disable",
    )
    .expect("parse");
    assert_eq!(t.user, "my@user");
    assert_eq!(t.password.as_deref(), Some("p@ss:%word"));
    assert_eq!(t.host, "dbhost");
    assert_eq!(t.port, 5433);
    assert_eq!(t.database, "mydb");
    assert_eq!(t.query, "sslmode=disable");

    // Re-encoded DSN survives round-trip for passwords with @ : %.
    let dsn = t.dsn("other:pass@%x");
    let reparsed = tama_web::migrate::parse_db_url(&dsn).expect("reparse");
    assert_eq!(reparsed.password.as_deref(), Some("other:pass@%x"));
    assert_eq!(reparsed.user, "my@user");
    assert_eq!(reparsed.query, "sslmode=disable");
}
