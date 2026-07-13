use super::*;
use crate::config::types::{CompactionDevice, LogLevel, OAuth2Config, RestartPolicy};
use crate::proxy::api_keys::{self, Scope};

#[test]
fn test_default_sampling_templates() {
    let config = Config::default();
    let templates = &config.sampling_templates;

    // Verify all 4 built-in profiles are present
    assert!(templates.contains_key("coding"));
    assert!(templates.contains_key("chat"));
    assert!(templates.contains_key("analysis"));
    assert!(templates.contains_key("creative"));

    // Verify coding template has expected values
    let coding = templates.get("coding").unwrap();
    assert_eq!(coding.temperature, Some(0.3));
    assert_eq!(coding.top_p, Some(0.9));

    // Verify creative template has expected values
    let creative = templates.get("creative").unwrap();
    assert_eq!(creative.temperature, Some(0.9));
    assert_eq!(creative.top_p, Some(0.95));
}

#[test]
fn test_sampling_templates_db_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tama.db");

    let config = Config::default();
    config.to_db(&db_path).unwrap();

    let loaded = Config::from_db(&db_path).unwrap();

    // Verify all profile values match after DB round-trip
    let profile_names = vec![
        "coding".to_string(),
        "chat".to_string(),
        "analysis".to_string(),
        "creative".to_string(),
    ];
    for profile_name in profile_names {
        let default = config.sampling_templates.get(&profile_name).unwrap();
        let loaded = loaded.sampling_templates.get(&profile_name).unwrap();
        assert_eq!(default, loaded);
    }
}

#[test]
fn test_sampling_templates_db_custom() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tama.db");

    let mut templates = HashMap::new();
    let custom = SamplingParams {
        temperature: Some(0.5),
        top_k: Some(100),
        ..Default::default()
    };
    templates.insert("custom".to_string(), custom.clone());

    let config = Config {
        sampling_templates: templates,
        ..Default::default()
    };

    config.to_db(&db_path).unwrap();
    let loaded = Config::from_db(&db_path).unwrap();

    let loaded_custom = loaded.sampling_templates.get("custom").unwrap();
    assert_eq!(loaded_custom.temperature, Some(0.5));
    assert_eq!(loaded_custom.top_k, Some(100));
}

/// Test that Config round-trips through the SQLite DB: write all fields, read back, verify equality.
#[test]
fn test_config_db_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tama.db");

    // Build a Config with all fields set to non-default values
    let mut sampling_templates = HashMap::new();
    sampling_templates.insert(
        "coding".to_string(),
        SamplingParams {
            temperature: Some(0.3),
            top_k: Some(50),
            top_p: Some(0.9),
            min_p: Some(0.05),
            presence_penalty: Some(0.1),
            frequency_penalty: None,
            repeat_penalty: None,
        },
    );
    sampling_templates.insert(
        "chat".to_string(),
        SamplingParams {
            temperature: Some(0.7),
            top_k: Some(40),
            top_p: Some(0.95),
            min_p: Some(0.05),
            presence_penalty: Some(0.0),
            frequency_penalty: None,
            repeat_penalty: None,
        },
    );
    sampling_templates.insert(
        "analysis".to_string(),
        SamplingParams {
            temperature: Some(0.3),
            top_k: Some(20),
            top_p: Some(0.9),
            min_p: Some(0.05),
            presence_penalty: Some(0.0),
            frequency_penalty: None,
            repeat_penalty: None,
        },
    );
    sampling_templates.insert(
        "creative".to_string(),
        SamplingParams {
            temperature: Some(0.9),
            top_k: Some(50),
            top_p: Some(0.95),
            min_p: Some(0.02),
            presence_penalty: Some(0.0),
            frequency_penalty: None,
            repeat_penalty: None,
        },
    );

    let config = Config {
        general: General {
            log_level: LogLevel::Debug,
            models_dir: Some("/data/models".to_string()),
            logs_dir: Some("/var/log/tama".to_string()),
            hf_token: Some("hf_test123".to_string()),
            update_check_interval: 24,
        },
        backends: HashMap::new(),
        supervisor: Supervisor {
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
            download_queue_poll_interval_secs: 5,
            max_loaded_models: 2,
            authenticator_url: Some("http://auth:8080".to_string()),
            authenticator_skip_paths: vec![
                "/health".to_string(),
                "/metrics".to_string(),
                "/custom".to_string(),
            ],
            oauth2: OAuth2Config::default(),
            api_keys_enabled: false,
        },
        compaction: CompactionConfig {
            enabled: true,
            server_path: Some("/opt/compaction/main.py".to_string()),
            device: CompactionDevice::Cuda,
            port: Some(8888),
            request_timeout_ms: 60000,
        },
        sampling_templates,
    };

    // Write to DB
    config.to_db(&db_path).unwrap();

    // Read back
    let loaded = Config::from_db(&db_path).unwrap();

    // Verify general
    assert_eq!(loaded.general.log_level, LogLevel::Debug);
    assert_eq!(loaded.general.models_dir, Some("/data/models".to_string()));
    assert_eq!(loaded.general.logs_dir, Some("/var/log/tama".to_string()));
    assert_eq!(loaded.general.hf_token, Some("hf_test123".to_string()));
    assert_eq!(loaded.general.update_check_interval, 24);

    // Verify supervisor
    assert_eq!(loaded.supervisor.restart_policy, RestartPolicy::OnFailure);
    assert_eq!(loaded.supervisor.max_restarts, 5);
    assert_eq!(loaded.supervisor.restart_delay_ms, 5000);
    assert_eq!(loaded.supervisor.health_check_interval_ms, 3000);
    assert_eq!(loaded.supervisor.health_check_timeout_ms, 10000);
    assert_eq!(loaded.supervisor.health_check_retries, 2);

    // Verify proxy
    assert_eq!(loaded.proxy.host, "127.0.0.1");
    assert_eq!(loaded.proxy.port, 8080);
    assert!(loaded.proxy.auto_unload);
    assert_eq!(loaded.proxy.idle_timeout_secs, 600);
    assert_eq!(loaded.proxy.startup_timeout_secs, 180);
    assert_eq!(loaded.proxy.circuit_breaker_threshold, 5);
    assert_eq!(loaded.proxy.circuit_breaker_cooldown_seconds, 120);
    assert_eq!(loaded.proxy.metrics_retention_secs, 43200);
    assert_eq!(loaded.proxy.download_queue_poll_interval_secs, 5);
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

    // Verify compaction
    assert!(loaded.compaction.enabled);
    assert_eq!(
        loaded.compaction.server_path,
        Some("/opt/compaction/main.py".to_string())
    );
    assert_eq!(loaded.compaction.device, CompactionDevice::Cuda);
    assert_eq!(loaded.compaction.port, Some(8888));
    assert_eq!(loaded.compaction.request_timeout_ms, 60000);

    // Verify sampling templates
    assert_eq!(loaded.sampling_templates.len(), 4);
    let coding = loaded.sampling_templates.get("coding").unwrap();
    assert_eq!(coding.temperature, Some(0.3));
    assert_eq!(coding.top_k, Some(50));
    assert_eq!(coding.top_p, Some(0.9));
    assert_eq!(coding.min_p, Some(0.05));
    assert_eq!(coding.presence_penalty, Some(0.1));
    let creative = loaded.sampling_templates.get("creative").unwrap();
    assert_eq!(creative.temperature, Some(0.9));
    assert_eq!(creative.top_p, Some(0.95));
    assert_eq!(creative.min_p, Some(0.02));
}

/// Test that loading from an empty DB seeds all defaults.
#[test]
fn test_config_from_empty_db_seeds_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tama.db");

    // Load from empty DB — should seed defaults
    let config = Config::from_db(&db_path).unwrap();

    // Verify general defaults
    assert_eq!(config.general.log_level, LogLevel::Info);
    assert_eq!(config.general.models_dir, None);
    assert_eq!(config.general.logs_dir, None);
    assert_eq!(config.general.hf_token, None);
    assert_eq!(config.general.update_check_interval, 12);

    // Verify supervisor defaults
    assert_eq!(config.supervisor.restart_policy, RestartPolicy::Always);
    assert_eq!(config.supervisor.max_restarts, 10);
    assert_eq!(config.supervisor.restart_delay_ms, 3000);
    assert_eq!(config.supervisor.health_check_interval_ms, 5000);
    assert_eq!(config.supervisor.health_check_timeout_ms, 30000);
    assert_eq!(config.supervisor.health_check_retries, 3);

    // Verify proxy defaults
    assert_eq!(config.proxy.host, "0.0.0.0");
    assert_eq!(config.proxy.port, 11434);
    assert!(!config.proxy.auto_unload);
    assert_eq!(config.proxy.idle_timeout_secs, 300);
    assert_eq!(config.proxy.startup_timeout_secs, 120);
    assert_eq!(config.proxy.circuit_breaker_threshold, 3);
    assert_eq!(config.proxy.circuit_breaker_cooldown_seconds, 60);
    assert_eq!(config.proxy.metrics_retention_secs, 86400);
    assert_eq!(config.proxy.download_queue_poll_interval_secs, 2);
    assert_eq!(config.proxy.max_loaded_models, 1);
    assert_eq!(config.proxy.authenticator_url, None);
    assert_eq!(
        config.proxy.authenticator_skip_paths,
        vec!["/health".to_string(), "/metrics".to_string()]
    );

    // Verify compaction defaults
    assert!(!config.compaction.enabled);
    assert_eq!(config.compaction.server_path, None);
    assert_eq!(config.compaction.device, CompactionDevice::Cpu);
    assert_eq!(config.compaction.port, None);
    assert_eq!(config.compaction.request_timeout_ms, 30000);

    // Verify 4 sampling templates seeded
    assert_eq!(config.sampling_templates.len(), 4);
    assert!(config.sampling_templates.contains_key("coding"));
    assert!(config.sampling_templates.contains_key("chat"));
    assert!(config.sampling_templates.contains_key("analysis"));
    assert!(config.sampling_templates.contains_key("creative"));

    // Verify coding template values
    let coding = config.sampling_templates.get("coding").unwrap();
    assert_eq!(coding.temperature, Some(0.3));
    assert_eq!(coding.top_k, Some(50));
    assert_eq!(coding.top_p, Some(0.9));
    assert_eq!(coding.min_p, Some(0.05));
    assert_eq!(coding.presence_penalty, Some(0.1));
}

/// Regression: `api_keys_enabled` is a *derived* value, not a user-editable
/// config field. It must always reflect the actual `api_keys` table state
/// after `to_db`, regardless of what the caller puts in `proxy.api_keys_enabled`.
///
/// This prevents a class of bugs where a stale client (e.g. the config editor
/// with a missing field in its mirror type) would silently lock the operator
/// out of their own proxy by writing `api_keys_enabled = false` on every save.
#[test]
fn test_to_db_derives_api_keys_enabled_from_active_keys() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tama.db");

    // Initialize the DB and create one active key.
    let init_config = Config::default();
    init_config.to_db(&db_path).unwrap();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let raw_key = api_keys::generate_key();
        api_keys::create_key(
            &conn,
            "test-key",
            &raw_key,
            &[Scope::Inference],
            "admin",
            None,
        )
        .unwrap();
    }

    // Save a config that explicitly says `api_keys_enabled = false`. The
    // stale-client bug would persist this and lock the operator out.
    let mut config_with_false_flag = Config::default();
    config_with_false_flag.proxy.api_keys_enabled = false;
    config_with_false_flag.to_db(&db_path).unwrap();

    // Reload — the flag must have been corrected to `true` based on the
    // active key in the table.
    let loaded = Config::from_db(&db_path).unwrap();
    assert!(
        loaded.proxy.api_keys_enabled,
        "api_keys_enabled must be derived from the api_keys table, not from the saved config value"
    );

    // Now revoke the only active key and save a config with `api_keys_enabled = true`.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        api_keys::revoke_key(&conn, 1).unwrap();
    }
    let mut config_with_true_flag = Config::default();
    config_with_true_flag.proxy.api_keys_enabled = true;
    config_with_true_flag.to_db(&db_path).unwrap();

    // Reload — the flag must have been corrected to `false` since no keys remain.
    let loaded = Config::from_db(&db_path).unwrap();
    assert!(
        !loaded.proxy.api_keys_enabled,
        "api_keys_enabled must be derived from the api_keys table; with no active keys it must be false"
    );
}

/// Regression: `from_db` must re-derive `api_keys_enabled` from the actual
/// `api_keys` table, not trust the stored value. A stale `api_keys_enabled = 0`
/// in the DB must not lock the operator out of their own proxy after a restart.
#[test]
fn test_from_db_derives_api_keys_enabled_from_active_keys() {
    use crate::proxy::api_keys;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("tama.db");

    // Initialize the DB and create one active key.
    let init_config = Config::default();
    init_config.to_db(&db_path).unwrap();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let raw_key = api_keys::generate_key();
        api_keys::create_key(
            &conn,
            "test-key",
            &raw_key,
            &[Scope::Inference],
            "admin",
            None,
        )
        .unwrap();
    }

    // Manually poison the stored value to `0` (simulates a stale DB after
    // a buggy config save, which was the original bug).
    rusqlite::Connection::open(&db_path)
        .unwrap()
        .execute("UPDATE app_proxy SET api_keys_enabled = 0", [])
        .unwrap();

    // from_db must correct this to `true` based on the active key.
    let loaded = Config::from_db(&db_path).unwrap();
    assert!(
        loaded.proxy.api_keys_enabled,
        "from_db must re-derive api_keys_enabled from the api_keys table; \
         a stale `false` in the DB must not be trusted"
    );
}

