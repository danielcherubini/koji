use super::super::*;
use crate::config::types::QuantEntry;
use std::collections::BTreeMap;
use tempfile::tempdir;

#[test]
fn test_build_full_args_unified() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    // Create the model directory structure and file
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
    // loaded_from removed — Config methods use Config::config_dir() (static)

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        sampling: Some(crate::profiles::SamplingParams {
            temperature: Some(0.3),
            ..Default::default()
        }),
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(4096),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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

    // Verify model path arg
    assert!(
        args.iter().any(|a| a.contains("model-Q4_K_M.gguf")),
        "Args should contain model path: {:?}",
        args
    );

    // Verify context length from server
    assert!(args.contains(&"-c".to_string()));
    assert!(args.contains(&"4096".to_string()));

    // Verify gpu_layers
    assert!(args.contains(&"-ngl".to_string()));
    assert!(args.contains(&"99".to_string()));

    // Verify sampling args (flattened)
    assert!(args.iter().any(|a| a == "--temp"));
    assert!(args.iter().any(|a| a == "0.30"));
}

#[test]
fn test_build_full_args_ctx_override() {
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
    // loaded_from removed — Config methods use Config::config_dir() (static)

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![],
        sampling: Some(crate::profiles::SamplingParams {
            temperature: Some(0.3),
            ..Default::default()
        }),
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(4096),
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    // ctx_override should take priority over server.context_length
    let args = config
        .build_full_args(&server, &backend, Some(2048), &[])
        .expect("build_full_args failed");

    assert!(args.contains(&"-c".to_string()));
    assert!(args.contains(&"2048".to_string()));
    assert!(!args.contains(&"4096".to_string()));
}

#[test]
fn test_build_full_args_no_sampling() {
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
        sampling: None, // No sampling params
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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

    // Verify no sampling args
    assert!(!args.iter().any(|a| a.starts_with("--temp")));
    assert!(!args.iter().any(|a| a.starts_with("--top-k")));
    assert!(!args.iter().any(|a| a.starts_with("--top-p")));
}

#[test]
fn test_build_full_args_no_quants() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");

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
        context_length: None,
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants: BTreeMap::new(), // Empty quants map
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    // Should not crash when quants is empty
    let args = config.build_full_args(&server, &backend, None, &[]);
    assert!(args.is_ok());

    // Should not emit -m arg when quant lookup fails
    let args = args.expect("build_full_args failed");
    assert!(!args.iter().any(|a| a == "-m"));
}

/// Tests that inline temperature in args is overridden by sampling params
#[test]
fn test_build_args_sampling_overrides_inline_temp_in_args() {
    // Requires SamplingParams::to_args to already be in grouped form
    // (done earlier in this same task, section 2a.1). If this test
    // fails with a flat-token mismatch instead of a dedup failure,
    // the to_args rewrite was skipped.
    let mut config = Config::default();
    config.backends.insert(
        "test_backend".to_string(),
        BackendConfig {
            path: None,
            version: None,
            gpu_variant: None,
        },
    );

    let server = ModelConfig {
        backend: "test_backend".to_string(),
        // inline --temp in args should be overridden by sampling.temperature
        args: vec!["--temp 0.10".to_string()],
        sampling: Some(crate::profiles::SamplingParams {
            temperature: Some(0.5),
            ..Default::default()
        }),
        model: None,
        quant: None,
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = config.backends.get("test_backend").unwrap().clone();
    let flat = config.build_args(&server, &backend, &[]);

    // --temp appears exactly once with value 0.50 (flattened)
    let temp_count = flat.iter().filter(|t| *t == "--temp").count();
    assert_eq!(
        temp_count, 1,
        "expected exactly one --temp flag, got {:?}",
        flat
    );
    assert!(flat.iter().any(|t| *t == "--temp"));
    assert!(flat.iter().any(|t| *t == "0.50"));
    assert!(!flat.iter().any(|t| t.contains("0.10")));
}

/// Tests that flat tokens are preserved with quoted paths in full args
#[test]
fn test_build_full_args_returns_flat_tokens_with_quoted_path() {
    // Path with spaces must round-trip through grouped → flat correctly.
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models with space");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model.gguf");
    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4".to_string(),
        crate::config::types::QuantEntry {
            file: "model.gguf".to_string(),
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
        quant: Some("Q4".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
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

    // -m and the path must appear as adjacent flat tokens, with the
    // space-containing path preserved as a single token.
    let m_pos = args.iter().position(|t| t == "-m").expect("-m not found");
    let path_token = &args[m_pos + 1];
    assert!(
        path_token.contains("models with space"),
        "expected path with spaces preserved as a single token, got {:?}",
        path_token
    );
    assert!(path_token.ends_with("model.gguf"));
}
