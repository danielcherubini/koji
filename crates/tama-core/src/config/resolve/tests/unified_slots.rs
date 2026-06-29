use super::super::*;
use tempfile::tempdir;

/// Tests that kv_unified=true uses per-slot context (no multiplication).
#[test]
fn test_build_full_args_unified_n_slots() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    // kv_unified=true, num_parallel=4, context_length=8192 → -c 8192 (not 32768)
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
        num_parallel: Some(4),
        kv_unified: true, // Unified KV
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

    // With kv_unified=true, -c should be per-slot context (8192), not multiplied
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"8192".to_string()),
        "Expected -c 8192 (unified: no multiplication), got: {:?}",
        args
    );
    // --kv-unified flag should be injected
    assert!(
        args.contains(&"--kv-unified".to_string()),
        "Expected --kv-unified flag in args, got: {:?}",
        args
    );
}

/// Tests that kv_unified=false uses context_length * num_parallel.
#[test]
fn test_build_full_args_non_unified_n_slots() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    // kv_unified=false, num_parallel=4, context_length=8192 → -c 32768
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
        num_parallel: Some(4),
        kv_unified: false, // Non-unified (default)
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

    // With kv_unified=false, -c should be 8192 * 4 = 32768
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"32768".to_string()),
        "Expected -c 32768 (non-unified: 8192*4), got: {:?}",
        args
    );
    // --kv-unified flag should NOT be injected
    assert!(
        !args.contains(&"--kv-unified".to_string()),
        "Expected no --kv-unified flag when kv_unified=false, got: {:?}",
        args
    );
}

/// Tests that default (kv_unified omitted/false) preserves non-unified behavior.
#[test]
fn test_build_full_args_unified_default() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    // kv_unified defaults to false via serde, num_parallel=2 → -c = 8192 * 2 = 16384
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        context_length: Some(8192),
        num_parallel: Some(2),
        quants,
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

    // Default (false) should use non-unified formula: 8192 * 2 = 16384
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"16384".to_string()),
        "Expected -c 16384 (default non-unified: 8192*2), got: {:?}",
        args
    );
    // --kv-unified flag should NOT be injected
    assert!(
        !args.contains(&"--kv-unified".to_string()),
        "Expected no --kv-unified flag with default kv_unified, got: {:?}",
        args
    );
}

/// Tests that ctx_override is treated as raw per-slot context with unified KV.
#[test]
fn test_build_full_args_ctx_override_unified() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    // ctx_override=Some(4096), kv_unified=true, num_parallel=3 → -c 4096 (not 12288)
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
        context_length: Some(8192), // Ignored because ctx_override takes priority
        num_parallel: Some(3),
        kv_unified: true, // Unified KV
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

    // ctx_override=4096, kv_unified=true → -c 4096 (not 12288)
    let args = config
        .build_full_args(&server, &backend, Some(4096), &[])
        .expect("build_full_args failed");

    // With kv_unified=true and ctx_override=4096, -c should be 4096 (per-slot)
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"4096".to_string()),
        "Expected -c 4096 (unified ctx_override), got: {:?}",
        args
    );
    // --kv-unified flag should be injected
    assert!(
        args.contains(&"--kv-unified".to_string()),
        "Expected --kv-unified flag in args, got: {:?}",
        args
    );
}

/// Tests that --kv-unified is not duplicated when the user manually adds it
/// in their args array AND server.kv_unified=true.
#[test]
fn test_build_full_args_kv_unified_not_duplicated_when_in_user_args() {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");

    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    // loaded_from removed — Config methods use Config::config_dir() (static)

    // User manually added --kv-unified in args, AND kv_unified=true in config.
    // The flag should appear exactly once (not duplicated).
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec!["--kv-unified".to_string()], // User manually added it
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(2),
        kv_unified: true, // Config also says unified
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

    let kv_count = args.iter().filter(|a| *a == "--kv-unified").count();
    assert_eq!(
        kv_count, 1,
        "--kv-unified should appear exactly once, got {} in: {:?}",
        kv_count, args
    );
}
