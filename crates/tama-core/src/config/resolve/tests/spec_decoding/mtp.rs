use super::super::super::*;
use crate::config::types::{QuantEntry, SpecDecodingConfig};
use std::collections::BTreeMap;
use tempfile::tempdir;

/// Tests that spec decoding flags (--spec-type, --spec-draft-n-max, --spec-draft-n-min)
/// are injected when spec_decoding is configured on a llama.cpp backend.
#[test]
fn test_spec_decoding_flags_injected() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should be injected with comma-separated types
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"draft-mtp,ngram-simple".to_string()),
        "Expected spec types value, got: {:?}",
        args
    );

    // --spec-draft-n-max should be injected
    assert!(
        args.contains(&"--spec-draft-n-max".to_string()),
        "Expected --spec-draft-n-max flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"4".to_string()),
        "Expected n_max=4, got: {:?}",
        args
    );

    // --spec-draft-n-min should be injected
    assert!(
        args.contains(&"--spec-draft-n-min".to_string()),
        "Expected --spec-draft-n-min flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"2".to_string()),
        "Expected n_min=2, got: {:?}",
        args
    );

    // --spec-draft-ngl should be injected (draft-mtp is in spec_types)
    assert!(
        args.contains(&"--spec-draft-ngl".to_string()),
        "Expected --spec-draft-ngl flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"16".to_string()),
        "Expected draft_ngl=16, got: {:?}",
        args
    );
}

/// Tests that if the user already has --spec-type in their args, we don't inject another.
#[test]
fn test_spec_decoding_no_duplicate_when_in_args() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // User manually added --spec-type in args
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec!["--spec-type draft-mtp".to_string()],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: None,
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should appear exactly once (user's version, not duplicated)
    let spec_type_count = args.iter().filter(|a| *a == "--spec-type").count();
    assert_eq!(
        spec_type_count, 1,
        "--spec-type should appear exactly once, got {} in: {:?}",
        spec_type_count, args
    );

    // n_max and n_min should still be injected (they weren't in user args)
    assert!(
        args.contains(&"--spec-draft-n-max".to_string()),
        "Expected --spec-draft-n-max flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"--spec-draft-n-min".to_string()),
        "Expected --spec-draft-n-min flag, got: {:?}",
        args
    );
}

/// Tests that --spec-draft-ngl is only injected when "draft-mtp" is in spec_types.
#[test]
fn test_spec_decoding_draft_ngl_only_for_mtp() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // spec_types does NOT contain "draft-mtp", so draft_ngl should NOT be injected
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["ngram-simple".to_string()], // No draft-mtp
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16), // Set but should be ignored without draft-mtp
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should be injected
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );

    // --spec-draft-ngl should NOT be injected (no draft-mtp in spec_types)
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl when draft-mtp not in spec_types, got: {:?}",
        args
    );
}

/// Tests that draft_ngl=99 is injected as-is (not truncated, not quoted).
#[test]
fn test_spec_decoding_draft_ngl_value_99() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(99),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-ngl should be present
    assert!(
        args.contains(&"--spec-draft-ngl".to_string()),
        "Expected --spec-draft-ngl flag, got: {:?}",
        args
    );

    // Value should be "99" — not truncated, not quoted
    assert!(
        args.contains(&"99".to_string()),
        "Expected draft_ngl value 99, got: {:?}",
        args
    );

    // Verify the value is exactly "99" (not "9" or "'99'")
    let ngl_pos = args
        .iter()
        .position(|a| a == "--spec-draft-ngl")
        .expect("--spec-draft-ngl not found");
    let ngl_value = &args[ngl_pos + 1];
    assert_eq!(
        ngl_value, "99",
        "draft_ngl value should be exactly '99', got '{}'",
        ngl_value
    );
}

/// Tests that empty spec_types produces no spec decoding flags.
#[test]
fn test_spec_decoding_empty_types_no_flags() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // Empty spec_types → no spec decoding flags should be injected
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig {
            spec_types: vec![], // Empty
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // No spec decoding flags should be present
    assert!(
        !args.contains(&"--spec-type".to_string()),
        "Expected no --spec-type when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-max".to_string()),
        "Expected no --spec-draft-n-max when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-min".to_string()),
        "Expected no --spec-draft-n-min when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl when spec_types is empty, got: {:?}",
        args
    );
}
