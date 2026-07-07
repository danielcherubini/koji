use super::*;
use crate::config::types::CompactionDevice;

/// Test that CompactionConfig fields have correct defaults.
#[test]
fn test_compaction_config_defaults() {
    let config = CompactionConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.server_path, None);
    assert_eq!(config.device, CompactionDevice::Cpu);
    assert_eq!(config.port, None);
    assert_eq!(config.request_timeout_ms, 30_000);
}

/// Test that CompactionConfig survives a TOML round-trip.
#[test]
fn test_compaction_config_toml_roundtrip() {
    let compaction = CompactionConfig {
        enabled: true,
        server_path: Some("/opt/compaction/main.py".to_string()),
        device: CompactionDevice::CudaDevice(0),
        port: Some(8081),
        request_timeout_ms: 60_000,
    };
    let toml_str = toml::to_string_pretty(&compaction).unwrap();
    let loaded: CompactionConfig = toml::from_str(&toml_str).unwrap();
    assert_eq!(loaded.enabled, compaction.enabled);
    assert_eq!(loaded.server_path, compaction.server_path);
    assert_eq!(loaded.device, compaction.device);
    assert_eq!(loaded.port, compaction.port);
    assert_eq!(loaded.request_timeout_ms, compaction.request_timeout_ms);
}

/// Test that CompactionConfig is disabled by default.
#[test]
fn test_compaction_config_disabled_by_default() {
    let config = CompactionConfig::default();
    assert!(!config.enabled, "compaction should be disabled by default");
}
