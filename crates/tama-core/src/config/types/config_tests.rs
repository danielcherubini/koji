//! Pure-logic Config tests.
//!
//! The DB-backed round-trip tests (load_from_pool / save against Postgres)
//! live in `crates/tama-core/tests/config_postgres.rs` on the testcontainer
//! harness (plan-190, Task 3).

use super::*;
use crate::config::types::RestartPolicy;

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

/// Regression: old config payloads using the `"supervisor"` JSON key must
/// still deserialize successfully thanks to the `#[serde(alias = "supervisor")]`
/// attribute on the `lifecycle` field.
#[test]
fn test_config_deserializes_legacy_supervisor_key() {
    let legacy_json = serde_json::json!({
        "general": {
            "log_level": "info",
            "update_check_interval": 12
        },
        "supervisor": {
            "restart_policy": "on-failure",
            "max_restarts": 5,
            "restart_delay_ms": 4000,
            "health_check_interval_ms": 6000,
            "health_check_timeout_ms": 20000,
            "health_check_retries": 3
        },
        "proxy": {
            "host": "0.0.0.0",
            "port": 18910
        }
    });

    let config: Config = serde_json::from_value(legacy_json)
        .expect("Config should deserialize with legacy 'supervisor' key thanks to serde alias");

    // Verify the data landed in the `lifecycle` field correctly
    assert_eq!(config.lifecycle.restart_policy, RestartPolicy::OnFailure);
    assert_eq!(config.lifecycle.max_restarts, 5);
    assert_eq!(config.lifecycle.restart_delay_ms, 4000);
    assert_eq!(config.lifecycle.health_check_interval_ms, 6000);
}
