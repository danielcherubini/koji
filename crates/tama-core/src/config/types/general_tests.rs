use super::*;

/// Test that the default `metrics_retention_secs` equals 86_400 (24 hours).
#[test]
fn test_proxy_config_default_metrics_retention() {
    let config = ProxyConfig::default();
    assert_eq!(config.metrics_retention_secs, 86_400);
}

/// Test that deserializing `metrics_retention_secs = 3600` sets the field correctly.
/// Test that the default update check interval is applied when missing from config.
#[test]
fn test_general_config_update_check_interval_default() {
    let config: Config = toml::from_str(
        r#"
[general]
log_level = "info"
"#,
    )
    .unwrap();
    assert_eq!(config.general.update_check_interval, 12);
}

/// Test that the default `max_loaded_models` equals 1 (single-model mode).
#[test]
fn test_proxy_config_default_max_loaded_models() {
    let config = ProxyConfig::default();
    assert_eq!(config.max_loaded_models, 1);
}

/// Test that deserializing `max_loaded_models = 0` sets unlimited.
#[test]
fn test_proxy_config_max_loaded_models_zero() {
    let config: ProxyConfig = toml::from_str(
        r#"
max_loaded_models = 0
"#,
    )
    .unwrap();
    assert_eq!(config.max_loaded_models, 0);
}

/// Test that omitting `max_loaded_models` uses the default of 1.
#[test]
fn test_proxy_config_max_loaded_models_omitted() {
    let config: ProxyConfig = toml::from_str(
        r#"
host = "0.0.0.0"
"#,
    )
    .unwrap();
    assert_eq!(config.max_loaded_models, 1);
}
