use super::super::*;
use crate::config::types::{QuantEntry, SpecDecodingConfig};
use std::collections::BTreeMap;
use tempfile::tempdir;

/// Test that --alias is injected for llama.cpp backends when api_name is set.
#[test]
fn test_build_full_args_injects_alias_for_llama_cpp() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("unsloth").join("gemma-4-E2B-it-GGUF");
    let quant_file = org_dir.join("gemma-4-E2B-it-UD-IQ3_XXS.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = BTreeMap::new();
    quants.insert(
        "IQ3_XXS".to_string(),
        QuantEntry {
            file: "gemma-4-E2B-it-UD-IQ3_XXS.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: None,
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        sampling: None,
        model: Some("unsloth/gemma-4-E2B-it-GGUF".to_string()),
        quant: Some("IQ3_XXS".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: Some("unsloth/gemma-4-E2B-it-GGUF".to_string()),
        gpu_layers: Some(999),
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig::default(),
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

    // Should contain --alias with the api_name value
    assert!(
        args.contains(&"--alias".to_string()),
        "Expected --alias in args, got: {:?}",
        args
    );
    let alias_idx = args.iter().position(|a| a == "--alias").unwrap();
    assert_eq!(
        args[alias_idx + 1],
        "unsloth/gemma-4-E2B-it-GGUF",
        "Alias value should match api_name"
    );
}

/// Test that --alias is NOT injected for non-llama.cpp backends.
#[test]
fn test_build_full_args_no_alias_for_non_llama_cpp() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    let server = ModelConfig {
        backend: "tts_kokoro".to_string(),
        args: vec![],
        sampling: None,
        model: None,
        quant: None,
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
        kv_unified: false,
        profile: None,
        api_name: Some("tts-model".to_string()),
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants: BTreeMap::new(),
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig::default(),
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

    assert!(
        !args.contains(&"--alias".to_string()),
        "Expected no --alias for non-llama.cpp backend, got: {:?}",
        args
    );
}

/// Test that --alias falls back to model field when api_name is not set.
#[test]
fn test_build_full_args_alias_falls_back_to_model() {
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
            context_length: None,
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

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
        api_name: None, // No api_name set
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig::default(),
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

    // Should fall back to model field
    let alias_idx = args.iter().position(|a| a == "--alias");
    assert!(
        alias_idx.is_some(),
        "Expected --alias when model is set (even without api_name)"
    );
    if let Some(idx) = alias_idx {
        assert_eq!(
            args[idx + 1],
            "org/repo",
            "Alias should fall back to model field"
        );
    }
}

/// Test that --alias is not injected when user already set it in args.
#[test]
fn test_build_full_args_respects_user_alias() {
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
            context_length: None,
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec!["--alias".to_string(), "my-custom-alias".to_string()],
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
        api_name: Some("api-name".to_string()),
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        spec_decoding: SpecDecodingConfig::default(),
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

    // Should only have one --alias (the user's), not a duplicate
    let alias_count = args.iter().filter(|a| *a == "--alias").count();
    assert_eq!(
        alias_count, 1,
        "Expected exactly one --alias, got {} in: {:?}",
        alias_count, args
    );
    let alias_idx = args.iter().position(|a| a == "--alias").unwrap();
    assert_eq!(
        args[alias_idx + 1],
        "my-custom-alias",
        "User's alias should be preserved"
    );
}
