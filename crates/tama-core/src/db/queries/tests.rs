//! Tests for database query functions.
//!
//! The SQLite in-memory model-config CRUD tests moved to the Postgres
//! harness (plan-190 Task 5): `crates/tama-core/tests/model_config_queries.rs`.
//! The pure (non-DB) TOML round-trip test stays here.

/// `mtp_model` round-trips through TOML serialization on ModelConfig.
#[test]
fn test_mtp_model_toml_round_trip() {
    use crate::config::ModelConfig;

    let mc = ModelConfig {
        backend: "llama_cpp".to_string(),
        mtp_model: Some("mtp-F16.gguf".to_string()),
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&mc).unwrap();
    let loaded: ModelConfig = toml::from_str(&toml_str).unwrap();

    assert_eq!(loaded.mtp_model.as_deref(), Some("mtp-F16.gguf"));
}
