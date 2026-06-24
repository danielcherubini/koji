use std::collections::BTreeMap;

use crate::config::types::QuantEntry;
use tempfile::tempdir;

use super::super::*;

/// When `gpu_device = Some("ROCm0")` and backend is llama_cpp, `--device ROCm0` appears in built args.
#[test]
fn test_gpu_device_injected_for_rocm() {
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
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

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
        num_parallel: None,
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
        gpu_device: Some("ROCm0".to_string()),
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        args.windows(2).any(|w| w == ["--device", "ROCm0"]),
        "Expected --device ROCm0 in args, got: {:?}",
        args
    );
}

/// When `gpu_device = None`, no `--device` flag is added.
#[test]
fn test_gpu_device_none_no_injection() {
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
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

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
        num_parallel: None,
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
        gpu_device: None,
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "--device"),
        "Expected no --device when gpu_device is None, got: {:?}",
        args
    );
}

/// When `--device` is already in `server.args`, it is NOT duplicated.
#[test]
fn test_gpu_device_no_duplicate_when_already_set() {
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
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec!["--device cuda0".to_string()],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
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
        gpu_device: Some("ROCm0".to_string()),
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    let device_count = args.iter().filter(|a| *a == "--device").count();
    assert_eq!(
        device_count, 1,
        "Expected exactly one --device (no duplicate), got {} in: {:?}",
        device_count, args
    );
    // The user's value should be preserved
    assert!(
        args.windows(2).any(|w| w == ["--device", "cuda0"]),
        "User's --device cuda0 should be preserved, got: {:?}",
        args
    );
}

/// When `gpu_device` is set but backend is non-llama.cpp, no `--device` flag is added.
#[test]
fn test_gpu_device_not_injected_for_non_llama_cpp() {
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
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let server = ModelConfig {
        backend: "ik_llama".to_string(),
        args: vec![],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
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
        gpu_device: Some("ROCm0".to_string()),
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "--device"),
        "Expected no --device for non-llama.cpp backend, got: {:?}",
        args
    );
}

/// When `gpu_device = Some("   ")`, no `--device` flag is added (whitespace-only).
#[test]
fn test_gpu_device_empty_string_no_injection() {
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
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

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
        num_parallel: None,
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
        gpu_device: Some("   ".to_string()),
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "--device"),
        "Expected no --device when gpu_device is whitespace-only, got: {:?}",
        args
    );
}
