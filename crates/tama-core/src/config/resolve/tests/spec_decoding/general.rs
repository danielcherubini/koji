use super::super::super::*;
use crate::config::types::{QuantEntry, SpecDecodingConfig};
use std::collections::BTreeMap;
use tempfile::tempdir;

/// Tests that multiple spec_types are joined with commas in --spec-type.
#[test]
fn test_spec_decoding_multi_type_comma_separated() {
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
            spec_types: vec![
                "draft-mtp".to_string(),
                "ngram-simple".to_string(),
                "ngram-mod".to_string(),
            ],
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

    // --spec-type value should be comma-separated
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"draft-mtp,ngram-simple,ngram-mod".to_string()),
        "Expected comma-separated spec types, got: {:?}",
        args
    );
}

/// Tests that a non-llama backend does NOT inject spec decoding flags.
#[test]
fn test_spec_decoding_non_llama_backend_no_flags() {
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

    // Use a non-llama backend
    let server = ModelConfig {
        backend: "ollama".to_string(),
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

    // No spec decoding flags should be present for non-llama backend
    assert!(
        !args.contains(&"--spec-type".to_string()),
        "Expected no --spec-type for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-max".to_string()),
        "Expected no --spec-draft-n-max for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-min".to_string()),
        "Expected no --spec-draft-n-min for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl for non-llama backend, got: {:?}",
        args
    );
}

/// Tests that each of the 4 spec decoding flags has its own already_has guard,
/// so pre-existing flags in user args are not duplicated.
#[test]
fn test_spec_decoding_all_already_has_checks() {
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

    // User already has all 4 flags in args
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![
            "--spec-type user-type".to_string(),
            "--spec-draft-n-max 8".to_string(),
            "--spec-draft-n-min 1".to_string(),
            "--spec-draft-ngl 32".to_string(),
        ],
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

    // Each flag should appear exactly once (user's version, not duplicated)
    let spec_type_count = args.iter().filter(|a| *a == "--spec-type").count();
    assert_eq!(
        spec_type_count, 1,
        "--spec-type should appear exactly once, got {} in: {:?}",
        spec_type_count, args
    );

    let n_max_count = args.iter().filter(|a| *a == "--spec-draft-n-max").count();
    assert_eq!(
        n_max_count, 1,
        "--spec-draft-n-max should appear exactly once, got {} in: {:?}",
        n_max_count, args
    );

    let n_min_count = args.iter().filter(|a| *a == "--spec-draft-n-min").count();
    assert_eq!(
        n_min_count, 1,
        "--spec-draft-n-min should appear exactly once, got {} in: {:?}",
        n_min_count, args
    );

    let ngl_count = args.iter().filter(|a| *a == "--spec-draft-ngl").count();
    assert_eq!(
        ngl_count, 1,
        "--spec-draft-ngl should appear exactly once, got {} in: {:?}",
        ngl_count, args
    );
}
