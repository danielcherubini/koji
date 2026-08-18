//! Postgres ports of the `app_config_queries` singleton-table CRUD tests
//! (plan-190, Task 3 — app config moves to Postgres).
//!
//! These mirror the former in-file SQLite tests 1:1 against the async
//! `&PgPool` API on an isolated migrated schema.

mod common;

use common::with_schema;
use tama_core::config::{CompactionDevice, LogLevel, RestartPolicy};
use tama_core::db::queries::{
    delete_all_sampling_templates, get_all_sampling_templates, get_compaction, get_general,
    get_langfuse, get_lifecycle, get_proxy, insert_tamad, seed_defaults, upsert_compaction,
    upsert_general, upsert_langfuse, upsert_lifecycle, upsert_proxy, upsert_sampling_template,
    LangfuseRecord,
};

/// Helper: an isolated schema with migrations applied.
async fn test_schema() -> common::SchemaGuard {
    with_schema().await
}

// ── seed_defaults ──────────────────────────────────────────────────

#[tokio::test]
async fn test_seed_defaults_creates_all_rows() {
    let guard = test_schema().await;

    // No rows before seeding
    assert!(get_general(&guard.pool).await.unwrap().is_none());
    assert!(get_proxy(&guard.pool).await.unwrap().is_none());
    assert!(get_lifecycle(&guard.pool).await.unwrap().is_none());
    assert!(get_compaction(&guard.pool).await.unwrap().is_none());
    assert!(get_all_sampling_templates(&guard.pool)
        .await
        .unwrap()
        .is_empty());

    seed_defaults(&guard.pool).await.unwrap();

    // All singleton tables should have a row now
    let general = get_general(&guard.pool).await.unwrap().unwrap();
    assert_eq!(general.log_level, "info");
    assert_eq!(general.update_check_interval, 12);

    let proxy = get_proxy(&guard.pool).await.unwrap().unwrap();
    assert_eq!(proxy.host, "0.0.0.0");
    assert_eq!(proxy.port, 11434);

    let lifecycle = get_lifecycle(&guard.pool).await.unwrap().unwrap();
    assert_eq!(lifecycle.restart_policy, "always");
    assert_eq!(lifecycle.max_restarts, 10);

    let compaction = get_compaction(&guard.pool).await.unwrap().unwrap();
    assert!(!compaction.enabled);
    assert_eq!(compaction.device, "cpu");

    // Langfuse defaults
    let langfuse = get_langfuse(&guard.pool).await.unwrap().unwrap();
    assert!(!langfuse.enabled);
    assert_eq!(langfuse.public_key, "");
    assert_eq!(langfuse.secret_key, "");
    assert_eq!(langfuse.host, "https://cloud.langfuse.com");
    assert_eq!(langfuse.environment, "default");
    assert!(langfuse.capture_input);
    assert!(langfuse.capture_output);
    assert!(langfuse.capture_streaming);
    assert_eq!(langfuse.telemetry_max_bytes, 1048576);
    assert_eq!(langfuse.electricity_price_per_kwh, 0.0);

    // 4 sampling templates
    let templates = get_all_sampling_templates(&guard.pool).await.unwrap();
    assert_eq!(templates.len(), 4);
    let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"coding"));
    assert!(names.contains(&"chat"));
    assert!(names.contains(&"analysis"));
    assert!(names.contains(&"creative"));

    // Verify seed is idempotent — calling again should not duplicate
    seed_defaults(&guard.pool).await.unwrap();
    assert_eq!(
        get_all_sampling_templates(&guard.pool).await.unwrap().len(),
        4
    );

    guard.finish().await;
}

// ── general ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_general_roundtrip() {
    let guard = test_schema().await;

    // Before upsert, get returns None
    assert!(get_general(&guard.pool).await.unwrap().is_none());

    // Upsert
    upsert_general(
        &guard.pool,
        &LogLevel::Debug,
        Some("/data/models"),
        Some("/var/log/tama"),
        Some("hf_abc123"),
        30,
    )
    .await
    .unwrap();

    // Get and verify
    let general = get_general(&guard.pool).await.unwrap().unwrap();
    assert_eq!(general.log_level, "debug");
    assert_eq!(general.models_dir, Some("/data/models".to_string()));
    assert_eq!(general.logs_dir, Some("/var/log/tama".to_string()));
    assert_eq!(general.hf_token, Some("hf_abc123".to_string()));
    assert_eq!(general.update_check_interval, 30);

    // Upsert again (update)
    upsert_general(&guard.pool, &LogLevel::Warn, None, None, None, 60)
        .await
        .unwrap();

    let general = get_general(&guard.pool).await.unwrap().unwrap();
    assert_eq!(general.log_level, "warn");
    assert_eq!(general.models_dir, None);
    assert_eq!(general.logs_dir, None);
    assert_eq!(general.hf_token, None);
    assert_eq!(general.update_check_interval, 60);

    guard.finish().await;
}

// ── proxy ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_proxy_roundtrip() {
    let guard = test_schema().await;

    assert!(get_proxy(&guard.pool).await.unwrap().is_none());

    let skip_paths = vec![
        "/health".to_string(),
        "/metrics".to_string(),
        "/custom".to_string(),
    ];
    upsert_proxy(
        &guard.pool,
        "127.0.0.1",
        8080,
        true,
        600,
        180,
        5,
        120,
        43200,
        5,
        2,
        Some("http://auth:8080"),
        &skip_paths,
        false, // oauth2_enabled
        "",
        "",
        "",
        "",
        None,
        None,
        "",
        &[],
        0,
        false,
        None,
    )
    .await
    .unwrap();

    let proxy = get_proxy(&guard.pool).await.unwrap().unwrap();
    assert_eq!(proxy.host, "127.0.0.1");
    assert_eq!(proxy.port, 8080);
    assert!(proxy.auto_unload);
    assert_eq!(proxy.idle_timeout_secs, 600);
    assert_eq!(proxy.startup_timeout_secs, 180);
    assert_eq!(proxy.circuit_breaker_threshold, 5);
    assert_eq!(proxy.circuit_breaker_cooldown_seconds, 120);
    assert_eq!(proxy.metrics_retention_secs, 43200);
    assert_eq!(proxy.pull_queue_poll_interval_secs, 5);
    assert_eq!(proxy.max_loaded_models, 2);
    assert_eq!(
        proxy.authenticator_url,
        Some("http://auth:8080".to_string())
    );
    assert_eq!(proxy.authenticator_skip_paths, skip_paths);

    // Update with empty skip paths
    upsert_proxy(
        &guard.pool,
        "0.0.0.0",
        11434,
        false,
        300,
        120,
        3,
        60,
        86400,
        2,
        1,
        None,
        &[],
        false, // oauth2_enabled
        "",
        "",
        "",
        "",
        None,
        None,
        "",
        &[],
        0,
        false,
        None,
    )
    .await
    .unwrap();

    let proxy = get_proxy(&guard.pool).await.unwrap().unwrap();
    assert_eq!(proxy.authenticator_skip_paths, Vec::<String>::new());
    assert_eq!(proxy.authenticator_url, None);

    guard.finish().await;
}

// ── lifecycle ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_lifecycle_roundtrip() {
    let guard = test_schema().await;

    assert!(get_lifecycle(&guard.pool).await.unwrap().is_none());

    upsert_lifecycle(
        &guard.pool,
        &RestartPolicy::OnFailure,
        5,
        5000,
        3000,
        10000,
        2,
    )
    .await
    .unwrap();

    let lifecycle = get_lifecycle(&guard.pool).await.unwrap().unwrap();
    assert_eq!(lifecycle.restart_policy, "on-failure");
    assert_eq!(lifecycle.max_restarts, 5);
    assert_eq!(lifecycle.restart_delay_ms, 5000);
    assert_eq!(lifecycle.health_check_interval_ms, 3000);
    assert_eq!(lifecycle.health_check_timeout_ms, 10000);
    assert_eq!(lifecycle.health_check_retries, 2);

    // Update
    upsert_lifecycle(
        &guard.pool,
        &RestartPolicy::Always,
        20,
        1000,
        10000,
        60000,
        5,
    )
    .await
    .unwrap();

    let lifecycle = get_lifecycle(&guard.pool).await.unwrap().unwrap();
    assert_eq!(lifecycle.restart_policy, "always");
    assert_eq!(lifecycle.max_restarts, 20);
    assert_eq!(lifecycle.restart_delay_ms, 1000);

    guard.finish().await;
}

// ── compaction ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_compaction_roundtrip() {
    let guard = test_schema().await;

    assert!(get_compaction(&guard.pool).await.unwrap().is_none());

    upsert_compaction(
        &guard.pool,
        true,
        Some("/usr/local/bin/llmlingua"),
        &CompactionDevice::Cuda,
        Some(8888),
        60000,
    )
    .await
    .unwrap();

    let compaction = get_compaction(&guard.pool).await.unwrap().unwrap();
    assert!(compaction.enabled);
    assert_eq!(
        compaction.server_path,
        Some("/usr/local/bin/llmlingua".to_string())
    );
    assert_eq!(compaction.device, "cuda");
    assert_eq!(compaction.port, Some(8888));
    assert_eq!(compaction.request_timeout_ms, 60000);

    // Update with defaults
    upsert_compaction(
        &guard.pool,
        false,
        None,
        &CompactionDevice::Cpu,
        None,
        30000,
    )
    .await
    .unwrap();

    let compaction = get_compaction(&guard.pool).await.unwrap().unwrap();
    assert!(!compaction.enabled);
    assert_eq!(compaction.server_path, None);
    assert_eq!(compaction.device, "cpu");
    assert_eq!(compaction.port, None);
    assert_eq!(compaction.request_timeout_ms, 30000);

    guard.finish().await;
}

// ── langfuse ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_langfuse_roundtrip() {
    let guard = test_schema().await;

    assert!(get_langfuse(&guard.pool).await.unwrap().is_none());

    upsert_langfuse(
        &guard.pool,
        &LangfuseRecord {
            enabled: true,
            public_key: "langpubkey123".to_string(),
            secret_key: "langsecretkey456".to_string(),
            host: "https://custom.langfuse.example.com".to_string(),
            environment: "production".to_string(),
            capture_input: false,
            capture_output: true,
            capture_streaming: false,
            telemetry_max_bytes: 2097152, // 2 MB
            electricity_price_per_kwh: 0.05,
        },
    )
    .await
    .unwrap();

    let langfuse = get_langfuse(&guard.pool).await.unwrap().unwrap();
    assert!(langfuse.enabled);
    assert_eq!(langfuse.public_key, "langpubkey123");
    assert_eq!(langfuse.secret_key, "langsecretkey456");
    assert_eq!(langfuse.host, "https://custom.langfuse.example.com");
    assert_eq!(langfuse.environment, "production");
    assert!(!langfuse.capture_input);
    assert!(langfuse.capture_output);
    assert!(!langfuse.capture_streaming);
    assert_eq!(langfuse.telemetry_max_bytes, 2097152);
    assert_eq!(langfuse.electricity_price_per_kwh, 0.05);

    // Update with defaults
    upsert_langfuse(
        &guard.pool,
        &LangfuseRecord {
            enabled: false,
            public_key: String::new(),
            secret_key: String::new(),
            host: "https://cloud.langfuse.com".to_string(),
            environment: "default".to_string(),
            capture_input: true,
            capture_output: true,
            capture_streaming: true,
            telemetry_max_bytes: 1048576, // 1 MB
            electricity_price_per_kwh: 0.0,
        },
    )
    .await
    .unwrap();

    let langfuse = get_langfuse(&guard.pool).await.unwrap().unwrap();
    assert!(!langfuse.enabled);
    assert_eq!(langfuse.public_key, "");
    assert_eq!(langfuse.host, "https://cloud.langfuse.com");
    assert_eq!(langfuse.environment, "default");

    guard.finish().await;
}

// ── sampling templates ─────────────────────────────────────────────

#[tokio::test]
async fn test_sampling_template_crud() {
    let guard = test_schema().await;

    // Initially empty
    assert!(get_all_sampling_templates(&guard.pool)
        .await
        .unwrap()
        .is_empty());

    // Upsert a new template
    upsert_sampling_template(
        &guard.pool,
        "coding",
        Some(0.3),
        Some(50),
        Some(0.9),
        Some(0.05),
        Some(0.1),
        None,
        None,
    )
    .await
    .unwrap();

    // Upsert another
    upsert_sampling_template(
        &guard.pool,
        "chat",
        Some(0.7),
        Some(40),
        Some(0.95),
        Some(0.05),
        Some(0.0),
        None,
        None,
    )
    .await
    .unwrap();

    let templates = get_all_sampling_templates(&guard.pool).await.unwrap();
    assert_eq!(templates.len(), 2);

    // Verify values
    let coding = templates.iter().find(|t| t.name == "coding").unwrap();
    assert_eq!(coding.temperature, Some(0.3));
    assert_eq!(coding.top_k, Some(50));
    assert_eq!(coding.top_p, Some(0.9));
    assert_eq!(coding.min_p, Some(0.05));
    assert_eq!(coding.presence_penalty, Some(0.1));

    // Upsert updates existing template (coding)
    upsert_sampling_template(
        &guard.pool,
        "coding",
        Some(0.5),
        Some(100),
        Some(0.95),
        Some(0.1),
        Some(0.2),
        Some(0.1),
        Some(1.5),
    )
    .await
    .unwrap();

    let templates = get_all_sampling_templates(&guard.pool).await.unwrap();
    assert_eq!(templates.len(), 2); // Still 2, not 3

    let coding = templates.iter().find(|t| t.name == "coding").unwrap();
    assert_eq!(coding.temperature, Some(0.5));
    assert_eq!(coding.top_k, Some(100));
    assert_eq!(coding.repeat_penalty, Some(1.5));

    // Delete all
    delete_all_sampling_templates(&guard.pool).await.unwrap();
    assert!(get_all_sampling_templates(&guard.pool)
        .await
        .unwrap()
        .is_empty());

    guard.finish().await;
}

// ── proxy OAuth2 ───────────────────────────────────────────────────

#[tokio::test]
async fn test_oauth2_proxy_roundtrip() {
    let guard = test_schema().await;

    assert!(get_proxy(&guard.pool).await.unwrap().is_none());

    let scopes = vec![
        "openid".to_string(),
        "profile".to_string(),
        "email".to_string(),
    ];
    upsert_proxy(
        &guard.pool,
        "0.0.0.0",
        11434,
        false,
        300,
        120,
        3,
        60,
        86400,
        2,
        1,
        None,
        &[],
        true, // oauth2_enabled
        "my-client-id",
        "my-client-secret",
        "https://auth.example.com/authorize",
        "https://auth.example.com/token",
        Some("https://auth.example.com/userinfo"),
        Some("https://auth.example.com/logout"),
        "http://localhost:11434/callback",
        &scopes,
        3600,  // session_ttl_secs
        false, // api_keys_enabled
        None,
    )
    .await
    .unwrap();

    let proxy = get_proxy(&guard.pool).await.unwrap().unwrap();
    assert!(proxy.oauth2_enabled);
    assert_eq!(proxy.oauth2_client_id, "my-client-id");
    assert_eq!(proxy.oauth2_client_secret, "my-client-secret");
    assert_eq!(
        proxy.oauth2_authorize_url,
        "https://auth.example.com/authorize"
    );
    assert_eq!(proxy.oauth2_token_url, "https://auth.example.com/token");
    assert_eq!(
        proxy.oauth2_userinfo_url,
        Some("https://auth.example.com/userinfo".to_string())
    );
    assert_eq!(
        proxy.oauth2_logout_url,
        Some("https://auth.example.com/logout".to_string())
    );
    assert_eq!(proxy.oauth2_redirect_uri, "http://localhost:11434/callback");
    assert_eq!(proxy.oauth2_scopes, scopes);
    assert_eq!(proxy.oauth2_session_ttl_secs, 3600);

    // Update with disabled OAuth2 and default scopes
    upsert_proxy(
        &guard.pool,
        "0.0.0.0",
        11434,
        false,
        300,
        120,
        3,
        60,
        86400,
        2,
        1,
        None,
        &[],
        false, // oauth2_enabled
        "",
        "",
        "",
        "",
        None,
        None,
        "",
        &["openid".to_string(), "profile".to_string()],
        86400,
        false, // api_keys_enabled
        None,
    )
    .await
    .unwrap();

    let proxy = get_proxy(&guard.pool).await.unwrap().unwrap();
    assert!(!proxy.oauth2_enabled);
    assert_eq!(proxy.oauth2_scopes, vec!["openid", "profile"]);
    assert_eq!(proxy.oauth2_session_ttl_secs, 86400);

    guard.finish().await;
}

// ── proxy pull_backend FK ─────────────────────────────────

/// `pull_backend` references `tamad_registry(id)`: an unregistered tamad id
/// must fail loudly instead of persisting, and a real tamad round-trips.
#[tokio::test]
async fn test_pull_backend_fk_rejects_unknown_tamad() {
    let guard = test_schema().await;
    seed_defaults(&guard.pool).await.unwrap();

    let ghost = sqlx::query("UPDATE app_proxy SET pull_backend = $1 WHERE id = 1")
        .bind("ghost-tamad")
        .execute(&guard.pool)
        .await;
    assert!(
        ghost.is_err(),
        "pull_backend must not accept an unregistered tamad (FK to tamad_registry)"
    );

    insert_tamad(
        &guard.pool,
        "pk-tamad-fk-1",
        "puller",
        "grpc://host:50051",
        "grpc",
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE app_proxy SET pull_backend = $1 WHERE id = 1")
        .bind("pk-tamad-fk-1")
        .execute(&guard.pool)
        .await
        .unwrap();
    let proxy = get_proxy(&guard.pool).await.unwrap().unwrap();
    assert_eq!(proxy.pull_backend, Some("pk-tamad-fk-1".to_string()));

    guard.finish().await;
}
