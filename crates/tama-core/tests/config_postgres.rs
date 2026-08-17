//! Postgres-backed Config orchestrator tests (plan-190, Task 3).
//!
//! Covers `Config::load_from_pool` / `Config::save` against an isolated
//! migrated schema: fresh-DB defaults, full round-trip, derived
//! `api_keys_enabled`, the should-check interval, and the guarantee that
//! the saved config carries no bootstrap/`database` section.

mod common;

use common::with_schema;
use tama_core::config::{
    CompactionDevice, Config, General, LangfuseConfig, Lifecycle, LogLevel, OAuth2Config,
    ProxyConfig, RestartPolicy,
};
use tama_core::db::queries::{count_active_keys, upsert_general};

/// Insert an `api_keys` row directly (the ApiKeyStore port is Task 6).
async fn insert_api_key(pool: &sqlx::PgPool, name: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by) \
         VALUES ('{name}', 'tama_test', 'hash_{name}', '[\"inference\"]', 'test') \
         ON CONFLICT (key_hash) DO NOTHING"
    )))
    .execute(pool)
    .await
    .expect("insert api key");
}

/// Fresh schema → `load_from_pool` must return the full default config
/// (acceptance: app boots against a fresh Postgres with defaults).
#[tokio::test]
async fn test_load_from_fresh_schema_returns_defaults() {
    let guard = with_schema().await;
    let config = Config::load_from_pool(&guard.pool).await.unwrap();

    // General defaults
    assert_eq!(config.general.log_level, LogLevel::Info);
    assert_eq!(config.general.models_dir, None);
    assert_eq!(config.general.logs_dir, None);
    assert_eq!(config.general.hf_token, None);
    assert_eq!(config.general.update_check_interval, 12);

    // Lifecycle defaults
    assert_eq!(config.lifecycle.restart_policy, RestartPolicy::Always);
    assert_eq!(config.lifecycle.max_restarts, 10);
    assert_eq!(config.lifecycle.restart_delay_ms, 3000);
    assert_eq!(config.lifecycle.health_check_interval_ms, 5000);
    assert_eq!(config.lifecycle.health_check_timeout_ms, 30000);
    assert_eq!(config.lifecycle.health_check_retries, 3);

    // Proxy defaults
    assert_eq!(config.proxy.host, "0.0.0.0");
    assert_eq!(config.proxy.port, 11434);
    assert!(!config.proxy.auto_unload);
    assert_eq!(config.proxy.idle_timeout_secs, 300);
    assert_eq!(config.proxy.startup_timeout_secs, 120);
    assert_eq!(config.proxy.circuit_breaker_threshold, 3);
    assert_eq!(config.proxy.circuit_breaker_cooldown_seconds, 60);
    assert_eq!(config.proxy.metrics_retention_secs, 86400);
    assert_eq!(config.proxy.pull_queue_poll_interval_secs, 2);
    assert_eq!(config.proxy.max_loaded_models, 1);
    assert_eq!(config.proxy.authenticator_url, None);
    assert_eq!(
        config.proxy.authenticator_skip_paths,
        vec!["/health".to_string(), "/metrics".to_string()]
    );

    // Compaction defaults
    assert!(!config.compaction.enabled);
    assert_eq!(config.compaction.server_path, None);
    assert_eq!(config.compaction.device, CompactionDevice::Cpu);
    assert_eq!(config.compaction.port, None);
    assert_eq!(config.compaction.request_timeout_ms, 30000);

    // Langfuse defaults
    assert!(!config.langfuse.enabled);
    assert_eq!(config.langfuse.public_key, "");
    assert_eq!(config.langfuse.secret_key, "");
    assert_eq!(config.langfuse.host, "https://cloud.langfuse.com");
    assert_eq!(config.langfuse.environment, "default");
    assert!(config.langfuse.capture_input);
    assert!(config.langfuse.capture_output);
    assert!(config.langfuse.capture_streaming);
    assert_eq!(config.langfuse.telemetry_max_bytes, 1048576);
    assert_eq!(config.langfuse.electricity_price_per_kwh, 0.0);

    // 4 sampling templates seeded
    assert_eq!(config.sampling_templates.len(), 4);
    for name in ["coding", "chat", "analysis", "creative"] {
        assert!(
            config.sampling_templates.contains_key(name),
            "{name} missing"
        );
    }
    let coding = config.sampling_templates.get("coding").unwrap();
    assert_eq!(coding.temperature, Some(0.3));
    assert_eq!(coding.top_k, Some(50));
    assert_eq!(coding.top_p, Some(0.9));
    assert_eq!(coding.min_p, Some(0.05));
    assert_eq!(coding.presence_penalty, Some(0.1));

    guard.finish().await;
}

/// Full round-trip: save a fully-populated config, reload, verify equality.
#[tokio::test]
async fn test_config_roundtrip() {
    let guard = with_schema().await;

    let config = Config {
        general: General {
            log_level: LogLevel::Debug,
            models_dir: Some("/data/models".to_string()),
            logs_dir: Some("/var/log/tama".to_string()),
            hf_token: Some("hf_test123".to_string()),
            update_check_interval: 24,
        },
        backends: std::collections::HashMap::new(),
        lifecycle: Lifecycle {
            restart_policy: RestartPolicy::OnFailure,
            max_restarts: 5,
            restart_delay_ms: 5000,
            health_check_interval_ms: 3000,
            health_check_timeout_ms: 10000,
            health_check_retries: 2,
        },
        proxy: ProxyConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            auto_unload: true,
            idle_timeout_secs: 600,
            startup_timeout_secs: 180,
            circuit_breaker_threshold: 5,
            circuit_breaker_cooldown_seconds: 120,
            metrics_retention_secs: 43200,
            pull_queue_poll_interval_secs: 5,
            max_loaded_models: 2,
            authenticator_url: Some("http://auth:8080".to_string()),
            authenticator_skip_paths: vec![
                "/health".to_string(),
                "/metrics".to_string(),
                "/custom".to_string(),
            ],
            oauth2: OAuth2Config {
                enabled: true,
                client_id: "oidc-client".to_string(),
                client_secret: "oidc-secret".to_string(),
                authorize_url: "https://auth.example.com/authorize".to_string(),
                token_url: "https://auth.example.com/token".to_string(),
                userinfo_url: Some("https://auth.example.com/userinfo".to_string()),
                logout_url: Some("https://auth.example.com/logout".to_string()),
                redirect_uri: "http://localhost:11434/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "profile".to_string(),
                    "email".to_string(),
                ],
                session_ttl_secs: 3600,
            },
            api_keys_enabled: false,
        },
        compaction: tama_core::config::CompactionConfig {
            enabled: true,
            server_path: Some("/opt/compaction/main.py".to_string()),
            device: CompactionDevice::Cuda,
            port: Some(8888),
            request_timeout_ms: 60000,
        },
        langfuse: LangfuseConfig::default(),
        sampling_templates: {
            // The 4 built-in templates — `load_from_pool` re-seeds missing
            // built-ins (same behavior as the old SQLite `from_db`), so the
            // round-trip asserts the full built-in set.
            let mut templates = std::collections::HashMap::new();
            for (name, params) in [
                (
                    "coding",
                    tama_core::profiles::SamplingParams {
                        temperature: Some(0.3),
                        top_k: Some(50),
                        top_p: Some(0.9),
                        min_p: Some(0.05),
                        presence_penalty: Some(0.1),
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                ),
                (
                    "chat",
                    tama_core::profiles::SamplingParams {
                        temperature: Some(0.7),
                        top_k: Some(40),
                        top_p: Some(0.95),
                        min_p: Some(0.05),
                        presence_penalty: Some(0.0),
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                ),
                (
                    "analysis",
                    tama_core::profiles::SamplingParams {
                        temperature: Some(0.3),
                        top_k: Some(20),
                        top_p: Some(0.9),
                        min_p: Some(0.05),
                        presence_penalty: Some(0.0),
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                ),
                (
                    "creative",
                    tama_core::profiles::SamplingParams {
                        temperature: Some(0.9),
                        top_k: Some(50),
                        top_p: Some(0.95),
                        min_p: Some(0.02),
                        presence_penalty: Some(0.0),
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                ),
            ] {
                templates.insert(name.to_string(), params);
            }
            templates
        },
    };

    config.save(&guard.pool).await.unwrap();
    let loaded = Config::load_from_pool(&guard.pool).await.unwrap();

    // General
    assert_eq!(loaded.general.log_level, LogLevel::Debug);
    assert_eq!(loaded.general.models_dir, Some("/data/models".to_string()));
    assert_eq!(loaded.general.logs_dir, Some("/var/log/tama".to_string()));
    assert_eq!(loaded.general.hf_token, Some("hf_test123".to_string()));
    assert_eq!(loaded.general.update_check_interval, 24);

    // Lifecycle
    assert_eq!(loaded.lifecycle.restart_policy, RestartPolicy::OnFailure);
    assert_eq!(loaded.lifecycle.max_restarts, 5);
    assert_eq!(loaded.lifecycle.restart_delay_ms, 5000);
    assert_eq!(loaded.lifecycle.health_check_interval_ms, 3000);
    assert_eq!(loaded.lifecycle.health_check_timeout_ms, 10000);
    assert_eq!(loaded.lifecycle.health_check_retries, 2);

    // Proxy
    assert_eq!(loaded.proxy.host, "127.0.0.1");
    assert_eq!(loaded.proxy.port, 8080);
    assert!(loaded.proxy.auto_unload);
    assert_eq!(loaded.proxy.idle_timeout_secs, 600);
    assert_eq!(loaded.proxy.startup_timeout_secs, 180);
    assert_eq!(loaded.proxy.circuit_breaker_threshold, 5);
    assert_eq!(loaded.proxy.circuit_breaker_cooldown_seconds, 120);
    assert_eq!(loaded.proxy.metrics_retention_secs, 43200);
    assert_eq!(loaded.proxy.pull_queue_poll_interval_secs, 5);
    assert_eq!(loaded.proxy.max_loaded_models, 2);
    assert_eq!(
        loaded.proxy.authenticator_url,
        Some("http://auth:8080".to_string())
    );
    assert_eq!(
        loaded.proxy.authenticator_skip_paths,
        vec![
            "/health".to_string(),
            "/metrics".to_string(),
            "/custom".to_string()
        ]
    );
    // OAuth2 round-trip
    assert!(loaded.proxy.oauth2.enabled);
    assert_eq!(loaded.proxy.oauth2.client_id, "oidc-client");
    assert_eq!(loaded.proxy.oauth2.client_secret, "oidc-secret");
    assert_eq!(
        loaded.proxy.oauth2.authorize_url,
        "https://auth.example.com/authorize"
    );
    assert_eq!(
        loaded.proxy.oauth2.token_url,
        "https://auth.example.com/token"
    );
    assert_eq!(
        loaded.proxy.oauth2.userinfo_url,
        Some("https://auth.example.com/userinfo".to_string())
    );
    assert_eq!(
        loaded.proxy.oauth2.logout_url,
        Some("https://auth.example.com/logout".to_string())
    );
    assert_eq!(
        loaded.proxy.oauth2.redirect_uri,
        "http://localhost:11434/callback"
    );
    assert_eq!(
        loaded.proxy.oauth2.scopes,
        vec!["openid", "profile", "email"]
    );
    assert_eq!(loaded.proxy.oauth2.session_ttl_secs, 3600);

    // Compaction
    assert!(loaded.compaction.enabled);
    assert_eq!(
        loaded.compaction.server_path,
        Some("/opt/compaction/main.py".to_string())
    );
    assert_eq!(loaded.compaction.device, CompactionDevice::Cuda);
    assert_eq!(loaded.compaction.port, Some(8888));
    assert_eq!(loaded.compaction.request_timeout_ms, 60000);

    // Sampling templates (replaced wholesale — delete-then-insert, then
    // load_from_pool re-seeds any missing built-ins)
    assert_eq!(loaded.sampling_templates.len(), 4);
    assert!(loaded.sampling_templates.contains_key("coding"));
    assert!(loaded.sampling_templates.contains_key("chat"));
    assert!(loaded.sampling_templates.contains_key("analysis"));
    assert!(loaded.sampling_templates.contains_key("creative"));
    assert_eq!(
        loaded.sampling_templates.get("coding").unwrap(),
        config.sampling_templates.get("coding").unwrap()
    );

    guard.finish().await;
}

/// The plan's round-trip: load from a fresh schema (defaults) → mutate a
/// section → save → reload → persisted.
#[tokio::test]
async fn test_mutate_save_reload_persists() {
    let guard = with_schema().await;

    let mut config = Config::load_from_pool(&guard.pool).await.unwrap();
    config.general.update_check_interval = 42;
    config.lifecycle.max_restarts = 7;
    config.proxy.port = 19999;
    config.sampling_templates.insert(
        "custom".to_string(),
        tama_core::profiles::SamplingParams {
            temperature: Some(0.5),
            top_k: Some(100),
            ..Default::default()
        },
    );
    config.save(&guard.pool).await.unwrap();

    let reloaded = Config::load_from_pool(&guard.pool).await.unwrap();
    assert_eq!(reloaded.general.update_check_interval, 42);
    assert_eq!(reloaded.lifecycle.max_restarts, 7);
    assert_eq!(reloaded.proxy.port, 19999);
    let custom = reloaded
        .sampling_templates
        .get("custom")
        .expect("custom template must persist");
    assert_eq!(custom.temperature, Some(0.5));
    assert_eq!(custom.top_k, Some(100));

    guard.finish().await;
}

/// Regression: `api_keys_enabled` is a *derived* value. It must always
/// reflect the actual `api_keys` table after `save`, regardless of what the
/// caller puts in `proxy.api_keys_enabled`.
#[tokio::test]
async fn test_save_derives_api_keys_enabled_from_active_keys() {
    let guard = with_schema().await;

    // One active key.
    insert_api_key(&guard.pool, "a").await;

    // Save a config that explicitly says api_keys_enabled = false.
    let mut config_with_false_flag = Config::default();
    config_with_false_flag.proxy.api_keys_enabled = false;
    config_with_false_flag.save(&guard.pool).await.unwrap();

    // Reload — the flag must have been corrected to true.
    let loaded = Config::load_from_pool(&guard.pool).await.unwrap();
    assert!(
        loaded.proxy.api_keys_enabled,
        "api_keys_enabled must be derived from the api_keys table, not from the saved config value"
    );

    // Revoke the only active key and save a config with api_keys_enabled = true.
    sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE name = 'a'")
        .execute(&guard.pool)
        .await
        .unwrap();
    let mut config_with_true_flag = Config::default();
    config_with_true_flag.proxy.api_keys_enabled = true;
    config_with_true_flag.save(&guard.pool).await.unwrap();

    let loaded = Config::load_from_pool(&guard.pool).await.unwrap();
    assert!(
        !loaded.proxy.api_keys_enabled,
        "api_keys_enabled must be derived from the api_keys table; with no active keys it must be false"
    );

    guard.finish().await;
}

/// Regression: `load_from_pool` must re-derive `api_keys_enabled` from the
/// actual `api_keys` table — a poisoned stored value must not be trusted.
#[tokio::test]
async fn test_load_rederives_api_keys_enabled_over_poisoned_row() {
    let guard = with_schema().await;

    insert_api_key(&guard.pool, "a").await;
    // Poison the stored value to false.
    sqlx::query("UPDATE app_proxy SET api_keys_enabled = FALSE")
        .execute(&guard.pool)
        .await
        .unwrap();

    let loaded = Config::load_from_pool(&guard.pool).await.unwrap();
    assert!(
        loaded.proxy.api_keys_enabled,
        "load_from_pool must re-derive api_keys_enabled from the api_keys table; \
         a stale false in the DB must not be trusted"
    );

    guard.finish().await;
}

/// `count_active_keys` counts only non-revoked, non-expired keys.
#[tokio::test]
async fn test_count_active_keys_counts_only_active() {
    let guard = with_schema().await;

    assert_eq!(count_active_keys(&guard.pool).await.unwrap(), 0);

    // One active key
    insert_api_key(&guard.pool, "a").await;
    assert_eq!(count_active_keys(&guard.pool).await.unwrap(), 1);

    // One revoked key — must NOT be counted
    sqlx::query(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, revoked_at) \
         VALUES ('b', 'tama_bbb', 'hash_b', '[\"inference\"]', 'test', '2020-01-01T00:00:00Z')",
    )
    .execute(&guard.pool)
    .await
    .unwrap();
    assert_eq!(count_active_keys(&guard.pool).await.unwrap(), 1);

    // One expired key — must NOT be counted
    sqlx::query(
        "INSERT INTO api_keys (name, key_prefix, key_hash, scopes, created_by, expires_at) \
         VALUES ('c', 'tama_ccc', 'hash_c', '[\"inference\"]', 'test', '2020-01-01T00:00:00Z')",
    )
    .execute(&guard.pool)
    .await
    .unwrap();
    assert_eq!(count_active_keys(&guard.pool).await.unwrap(), 1);

    guard.finish().await;
}

/// The app config no longer contains the bootstrap DSN: the serialized
/// saved config has no `database` section, and no `database` column/table
/// exists in the config tables.
#[tokio::test]
async fn test_saved_config_contains_no_database_section() {
    let guard = with_schema().await;

    let config = Config::default();
    config.save(&guard.pool).await.unwrap();

    // The serialized config carries no `database` key (the bootstrap DSN
    // lives only in config.toml, never in the DB rows or the API payload).
    let json = serde_json::to_value(&config).unwrap();
    assert!(
        json.get("database").is_none(),
        "saved config must not contain a database section"
    );
    let reloaded = Config::load_from_pool(&guard.pool).await.unwrap();
    let reloaded_json = serde_json::to_value(&reloaded).unwrap();
    assert!(
        reloaded_json.get("database").is_none(),
        "reloaded config must not contain a database section"
    );

    // No `database`-named table exists in the schema.
    let rows = sqlx::query(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_name ILIKE '%database%'",
    )
    .fetch_all(&guard.pool)
    .await
    .unwrap();
    assert!(
        rows.is_empty(),
        "no database-named table should exist in the schema"
    );

    guard.finish().await;
}

/// `should_check` reads the update-check interval from the Postgres-backed
/// config (plan-190 Task 3 port of the SQLite test).
#[tokio::test]
async fn test_should_check_uses_db_interval() {
    let guard = with_schema().await;

    // No records yet, should return true
    let checker = tama_core::updates::checker::UpdateChecker::new();
    assert!(checker.should_check(&guard.pool).await.unwrap());

    // Interval = 1h; a record from 2 hours ago must trigger a check.
    upsert_general(&guard.pool, &LogLevel::Info, None, None, None, 1)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let two_hours_ago = now - 7200;
    tama_core::db::queries::upsert_update_check(
        &guard.pool,
        tama_core::db::queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "test",
            current_version: None,
            latest_version: None,
            update_available: false,
            status: "unknown",
            error_message: None,
            details_json: None,
            checked_at: two_hours_ago,
        },
    )
    .await
    .unwrap();
    assert!(checker.should_check(&guard.pool).await.unwrap());

    guard.finish().await;
}
