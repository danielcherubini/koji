use super::*;
use crate::db::queries::ModelConfigRecord;

/// Test that SpecDecodingConfig round-trips through TOML serialization.
#[test]
fn test_spec_decoding_in_model_config_toml_roundtrip() {
    let spec = SpecDecodingConfig {
        spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
        n_max: Some(4),
        n_min: Some(2),
        draft_ngl: Some(16),
    };
    let config = ModelConfig {
        backend: "llama.cpp".to_string(),
        spec_decoding: spec.clone(),
        ..Default::default()
    };

    let toml_str = toml::to_string_pretty(&config).unwrap();
    let loaded: ModelConfig = toml::from_str(&toml_str).unwrap();

    assert_eq!(loaded.spec_decoding.spec_types, spec.spec_types);
    assert_eq!(loaded.spec_decoding.n_max, spec.n_max);
    assert_eq!(loaded.spec_decoding.n_min, spec.n_min);
    assert_eq!(loaded.spec_decoding.draft_ngl, spec.draft_ngl);
}

/// Test that omitting spec_decoding in TOML yields SpecDecodingConfig::default().
#[test]
fn test_spec_decoding_missing_in_toml_defaults() {
    let toml_str = r#"
backend = "llama.cpp"
"#;
    let config: ModelConfig = toml::from_str(toml_str).unwrap();

    assert!(config.spec_decoding.spec_types.is_empty());
    assert_eq!(config.spec_decoding.n_max, None);
    assert_eq!(config.spec_decoding.n_min, None);
    assert_eq!(config.spec_decoding.draft_ngl, None);
}

/// Test that a ModelConfig survives a round-trip through the DB record.
#[test]
fn test_model_config_round_trip() {
    let mc = ModelConfig {
        backend: "llama.cpp".to_string(),
        args: vec!["--n-gpu-layers".to_string(), "32".to_string()],
        sampling: Some(SamplingParams {
            temperature: Some(0.7),
            top_p: Some(0.9),
            ..Default::default()
        }),
        model: Some("owner/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: Some("mmproj-model.gguf".to_string()),
        port: Some(8080),
        health_check: Some(HealthCheck {
            url: Some("/health".to_string()),
            interval_ms: Some(1000),
            timeout_ms: Some(500),
        }),
        enabled: true,
        context_length: Some(4096),
        num_parallel: Some(2),
        api_name: Some("my-model".to_string()),
        gpu_layers: Some(32),
        cache_type_k: None,
        cache_type_v: None,
        gpu_device: Some("ROCm0".to_string()),
        hf_architecture_type: Some("MoE".to_string()),
        hf_total_params: Some("35B".to_string()),
        modalities: Some(ModelModalities {
            input: vec!["text".to_string(), "image".to_string()],
            output: vec!["text".to_string()],
        }),
        display_name: Some("My Custom Model".to_string()),
        kv_unified: true,
        ..Default::default()
    };

    let record = mc.to_db_record("owner/repo");
    let round_trip = ModelConfig::from_db_record(&record);

    assert_eq!(round_trip.backend, mc.backend);
    assert_eq!(round_trip.args, mc.args);
    assert_eq!(round_trip.sampling, mc.sampling);
    assert_eq!(round_trip.model, Some("owner/repo".to_string()));
    assert_eq!(round_trip.quant, mc.quant);
    assert_eq!(round_trip.mmproj, mc.mmproj);
    assert_eq!(round_trip.port, mc.port);
    assert_eq!(round_trip.health_check, mc.health_check);
    assert_eq!(round_trip.enabled, mc.enabled);
    assert_eq!(round_trip.context_length, mc.context_length);
    assert_eq!(round_trip.num_parallel, mc.num_parallel);
    assert_eq!(round_trip.api_name, mc.api_name);
    assert_eq!(round_trip.gpu_layers, mc.gpu_layers);
    assert_eq!(round_trip.cache_type_k, mc.cache_type_k);
    assert_eq!(round_trip.cache_type_v, mc.cache_type_v);
    assert_eq!(round_trip.gpu_device, mc.gpu_device);
    assert_eq!(round_trip.modalities, mc.modalities);
    assert_eq!(round_trip.display_name, mc.display_name);
    assert_eq!(round_trip.kv_unified, mc.kv_unified);
    assert_eq!(round_trip.hf_architecture_type, mc.hf_architecture_type);
    assert_eq!(round_trip.hf_total_params, mc.hf_total_params);
    assert_eq!(round_trip.spec_decoding, mc.spec_decoding);

    // quants should be empty as it's not persisted
    assert!(round_trip.quants.is_empty());
}

/// Test that SpecDecodingConfig survives a round-trip through the DB record.
#[test]
fn test_model_config_spec_decoding_db_roundtrip() {
    let spec = SpecDecodingConfig {
        spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
        n_max: Some(4),
        n_min: Some(2),
        draft_ngl: Some(16),
    };
    let mc = ModelConfig {
        backend: "llama.cpp".to_string(),
        spec_decoding: spec.clone(),
        ..Default::default()
    };

    let record = mc.to_db_record("owner/repo");
    let round_trip = ModelConfig::from_db_record(&record);

    assert_eq!(round_trip.spec_decoding, spec);
}

/// Test that SpecDecodingConfig serializes to camelCase JSON and deserializes back.
#[test]
fn test_spec_decoding_json_camel_case_roundtrip() {
    let spec = SpecDecodingConfig {
        spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
        n_max: Some(4),
        n_min: Some(2),
        draft_ngl: Some(16),
    };

    let json = serde_json::to_string(&spec).expect("Failed to serialize SpecDecodingConfig");

    // Verify camelCase keys in JSON output
    assert!(
        json.contains("\"specTypes\""),
        "Expected 'specTypes' key in JSON: {}",
        json
    );
    assert!(
        json.contains("\"nMax\""),
        "Expected 'nMax' key in JSON: {}",
        json
    );
    assert!(
        json.contains("\"nMin\""),
        "Expected 'nMin' key in JSON: {}",
        json
    );
    assert!(
        json.contains("\"draftNgl\""),
        "Expected 'draftNgl' key in JSON: {}",
        json
    );

    // Verify no snake_case keys
    assert!(
        !json.contains("spec_types"),
        "Should not contain snake_case 'spec_types': {}",
        json
    );
    assert!(
        !json.contains("n_max"),
        "Should not contain snake_case 'n_max': {}",
        json
    );
    assert!(
        !json.contains("n_min"),
        "Should not contain snake_case 'n_min': {}",
        json
    );
    assert!(
        !json.contains("draft_ngl"),
        "Should not contain snake_case 'draft_ngl': {}",
        json
    );

    // Deserialize back and verify
    let deserialized: SpecDecodingConfig =
        serde_json::from_str(&json).expect("Failed to deserialize SpecDecodingConfig");
    assert_eq!(deserialized, spec);
}

/// Test that legacy `-b` / `--batch-size` args normalize into `n_batch`.
#[test]
fn test_normalize_legacy_batch_args_short_flag() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        args: Some(serde_json::to_string(&vec!["-ngl", "99", "-b", "2048"]).unwrap()),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    assert_eq!(config.n_batch, Some(2048));
    assert_eq!(config.args, vec!["-ngl", "99"]);
}

/// Test that legacy `--ubatch-size=VALUE` arg normalizes into `n_ubatch`.
#[test]
fn test_normalize_legacy_ubatch_flag_equals_form() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        args: Some(serde_json::to_string(&vec!["-ngl", "99", "--ubatch-size=512"]).unwrap()),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    assert_eq!(config.n_ubatch, Some(512));
    assert_eq!(config.args, vec!["-ngl", "99"]);
}

/// Test that both `-b` and `--ubatch-size` normalize together.
#[test]
fn test_normalize_both_batch_and_ubatch() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        args: Some(
            serde_json::to_string(&vec!["-ngl", "99", "-b", "2048", "--ubatch-size=512"]).unwrap(),
        ),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    assert_eq!(config.n_batch, Some(2048));
    assert_eq!(config.n_ubatch, Some(512));
    assert_eq!(config.args, vec!["-ngl", "99"]);
}

/// Test that explicit column values win over args flags.
/// Legacy `-b`/`--batch-size` and `-ub`/`--ubatch-size` flags are always
/// removed from args once processed, regardless of whether the column
/// already has a value (the column value takes priority for spawn args).
#[test]
fn test_column_values_win_over_args() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        n_batch: Some(4096),
        n_ubatch: Some(1024),
        args: Some(
            serde_json::to_string(&vec!["-ngl", "99", "-b", "2048", "--ubatch-size=512"]).unwrap(),
        ),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    // Column values should win
    assert_eq!(config.n_batch, Some(4096));
    assert_eq!(config.n_ubatch, Some(1024));
    // Legacy flags are removed from args even when columns already have values
    assert_eq!(config.args, vec!["-ngl", "99"]);
}

/// Test that unparseable values are left in args.
#[test]
fn test_unparseable_values_left_in_args() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        args: Some(serde_json::to_string(&vec!["-b", "not_a_number"]).unwrap()),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    assert_eq!(config.n_batch, None);
    assert_eq!(config.args, vec!["-b", "not_a_number"]);
}

/// Test that `--batch-size VALUE` (long flag with space) normalizes.
#[test]
fn test_normalize_long_flag_with_space() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        args: Some(serde_json::to_string(&vec!["--batch-size", "8192"]).unwrap()),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    assert_eq!(config.n_batch, Some(8192));
    assert!(config.args.is_empty());
}

/// Test that `-ub VALUE` (short flag) normalizes.
#[test]
fn test_normalize_short_ub_flag() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        args: Some(serde_json::to_string(&vec!["-ub", "256"]).unwrap()),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    assert_eq!(config.n_ubatch, Some(256));
    assert!(config.args.is_empty());
}

/// Test that ModelConfig with n_batch/n_ubatch round-trips through DB record.
#[test]
fn test_model_config_n_batch_n_ubatch_roundtrip() {
    let mc = ModelConfig {
        backend: "llama_cpp".to_string(),
        n_batch: Some(2048),
        n_ubatch: Some(512),
        ..Default::default()
    };

    let record = mc.to_db_record("owner/repo");
    assert_eq!(record.n_batch, Some(2048));
    assert_eq!(record.n_ubatch, Some(512));

    let round_trip = ModelConfig::from_db_record(&record);
    assert_eq!(round_trip.n_batch, Some(2048));
    assert_eq!(round_trip.n_ubatch, Some(512));
}

/// Test that None values round-trip as NULL.
#[test]
fn test_model_config_none_n_batch_n_ubatch_roundtrip() {
    let mc = ModelConfig {
        backend: "llama_cpp".to_string(),
        n_batch: None,
        n_ubatch: None,
        ..Default::default()
    };

    let record = mc.to_db_record("owner/repo");
    assert_eq!(record.n_batch, None);
    assert_eq!(record.n_ubatch, None);

    let round_trip = ModelConfig::from_db_record(&record);
    assert_eq!(round_trip.n_batch, None);
    assert_eq!(round_trip.n_ubatch, None);
}

/// Test that args without legacy flags are left untouched.
#[test]
fn test_no_legacy_flags_unchanged() {
    let record = ModelConfigRecord {
        repo_id: "test/model".to_string(),
        backend: "llama_cpp".to_string(),
        args: Some(serde_json::to_string(&vec!["-ngl", "99", "--log-tokens"]).unwrap()),
        ..Default::default()
    };

    let config = ModelConfig::from_db_record(&record);
    assert_eq!(config.n_batch, None);
    assert_eq!(config.n_ubatch, None);
    assert_eq!(config.args, vec!["-ngl", "99", "--log-tokens"]);
}
