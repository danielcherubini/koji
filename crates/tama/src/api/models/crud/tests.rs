use super::*;
use std::collections::BTreeMap;
use tama_core::config::{ModelConfig, QuantEntry, QuantKind};

fn body_with_quants(quants: BTreeMap<String, QuantEntry>) -> ModelBody {
    ModelBody {
        backend: "llama".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: Some(true),
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        display_name: None,
        gpu_layers: None,
        quants: Some(quants),
        modalities: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    }
}

fn existing_with_size(name: &str, file: &str, size: Option<u64>) -> ModelConfig {
    let mut quants = BTreeMap::new();
    quants.insert(
        name.to_string(),
        QuantEntry {
            file: file.to_string(),
            kind: QuantKind::Model,
            size_bytes: size,
            context_length: Some(4096),
        },
    );
    ModelConfig {
        backend: "llama".into(),
        gpu_variant: None,
        gpu_device: None,
        args: vec![],
        sampling: None,
        model: Some("org/repo".into()),
        quant: Some("Q4_K_M".into()),
        mmproj: None,
        mtp_model: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants,
        modalities: None,
        display_name: None,
        kv_unified: false,
        cache_type_k: None,
        cache_type_v: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        db_id: None,
        spec_decoding: Default::default(),
    }
}

/// When an existing entry has a stored `size_bytes`, a PUT that tries to
/// change it must be silently ignored — the server-side value wins.
#[test]
fn apply_model_body_preserves_existing_size_bytes() {
    let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(1_234_567));

    let mut attacker_quants = BTreeMap::new();
    attacker_quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "Model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(42), // malicious / stale
            context_length: Some(8192),
        },
    );

    let result = apply_model_body(body_with_quants(attacker_quants), Some(existing));
    let q = result.quants.get("Q4_K_M").unwrap();
    assert_eq!(
        q.size_bytes,
        Some(1_234_567),
        "existing size_bytes must be preserved against client override"
    );
    assert_eq!(q.context_length, Some(8192));
}

/// When an existing entry has no stored size, we still accept the client
/// value to avoid regressing fresh creates that haven't been verified yet.
#[test]
fn apply_model_body_accepts_client_size_when_none_stored() {
    let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", None);

    let mut incoming = BTreeMap::new();
    incoming.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "Model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(9_999),
            context_length: Some(4096),
        },
    );

    let result = apply_model_body(body_with_quants(incoming), Some(existing));
    assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(9_999));
}

/// A brand-new model (no existing config) still honours whatever size the
/// client supplies, so create flows aren't broken.
#[test]
fn apply_model_body_accepts_client_size_for_new_model() {
    let mut incoming = BTreeMap::new();
    incoming.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "Model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(5_000),
            context_length: None,
        },
    );

    let result = apply_model_body(body_with_quants(incoming), None);
    assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(5_000));
}

/// A new quant key (not in the existing config) on an existing model still
/// accepts the client value — preservation is per-key.
#[test]
fn apply_model_body_accepts_client_size_for_new_quant_key() {
    let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(1_000));

    let mut incoming = BTreeMap::new();
    incoming.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "Model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(7),
            context_length: None,
        },
    );
    incoming.insert(
        "Q8_0".to_string(),
        QuantEntry {
            file: "Model-Q8_0.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(2_000),
            context_length: None,
        },
    );

    let result = apply_model_body(body_with_quants(incoming), Some(existing));
    assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(1_000));
    assert_eq!(result.quants.get("Q8_0").unwrap().size_bytes, Some(2_000));
}

// ── apply_model_body additional tests ─────────────────────────────────

#[test]
fn test_apply_model_body_preserves_existing_size() {
    let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(10_000));

    let mut incoming = BTreeMap::new();
    incoming.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "Model-Q4_K_M-new.gguf".to_string(), // different file
            kind: QuantKind::Model,
            size_bytes: Some(5_000), // client sends smaller size
            context_length: None,
        },
    );

    let result = apply_model_body(body_with_quants(incoming), Some(existing));
    // Existing size_bytes should be preserved (server-side authoritative)
    assert_eq!(
        result.quants.get("Q4_K_M").unwrap().size_bytes,
        Some(10_000)
    );
}

#[test]
fn test_apply_model_body_enabled_override() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: Some(false),
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert!(!result.enabled);
}

#[test]
fn test_apply_model_body_enabled_default() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None, // Not specified
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    // Default enabled is true
    assert!(result.enabled);
}

#[test]
fn test_apply_model_body_with_api_name() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: Some("my-api-name".to_string()),
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.api_name, Some("my-api-name".to_string()));
}

#[test]
fn test_apply_model_body_with_gpu_layers() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: Some(32),
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.gpu_layers, Some(32));
}

#[test]
fn test_apply_model_body_with_display_name() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: Some("My Model".to_string()),
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.display_name, Some("My Model".to_string()));
}

/// Verify that body.gpu_device overrides base.gpu_device.
#[test]
fn test_apply_model_body_gpu_device_override() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: Some("CUDA1".to_string()),
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        gpu_variant: None,
        gpu_device: Some("CUDA0".to_string()),
        model: Some("model.gguf".into()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
        port: None,
        health_check: None,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        kv_unified: false,
        cache_type_k: None,
        cache_type_v: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        db_id: None,
        spec_decoding: Default::default(),
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(result.gpu_device, Some("CUDA1".to_string()));
}

/// Verify that body.gpu_device = None preserves base.gpu_device.
#[test]
fn test_apply_model_body_gpu_device_preserves_base_when_omitted() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        gpu_variant: None,
        gpu_device: Some("CUDA0".to_string()),
        model: Some("model.gguf".into()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
        port: None,
        health_check: None,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        kv_unified: false,
        cache_type_k: None,
        cache_type_v: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        db_id: None,
        spec_decoding: Default::default(),
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(result.gpu_device, Some("CUDA0".to_string()));
}

#[test]
fn test_apply_model_body_context_length() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: Some(8192),
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.context_length, Some(8192));
}

/// Verify that num_parallel flows from body through to ModelConfig.
#[test]
fn test_apply_model_body_num_parallel_passthrough() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        num_parallel: Some(4),
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.num_parallel, Some(4));
}

#[test]
fn test_apply_model_body_num_parallel_default() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        num_parallel: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.num_parallel, None);
}

#[test]
fn test_apply_model_body_empty_quants() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: Some(BTreeMap::new()), // empty map
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert!(result.quants.is_empty());
}

/// When an existing model has `kv_unified: false` and the body omits the
/// field, the existing value must be preserved (not overwritten to true).
#[test]
fn test_apply_model_body_kv_unified_passthrough() {
    let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", None);
    assert!(!existing.kv_unified, "helper must create kv_unified=false");

    let body = ModelBody {
        backend: "llama".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("org/repo".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        display_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        kv_unified: None, // omitted — should preserve existing
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert!(
        !result.kv_unified,
        "existing kv_unified=false must be preserved when body omits the field"
    );
}

/// When creating a new model (no existing config) and the body omits
/// `kv_unified`, the result must default to `true`.
#[test]
fn test_apply_model_body_kv_unified_default_true_for_new() {
    let body = ModelBody {
        backend: "llama".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("org/repo".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        display_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        kv_unified: None, // omitted — should default to true
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert!(
        result.kv_unified,
        "new model must default kv_unified to true when body omits the field"
    );
}

/// Verify that cache_type_k and cache_type_v flow from body through to ModelConfig.
#[test]
fn test_apply_model_body_cache_type_passthrough() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: Some("q4_0".to_string()),
        cache_type_v: Some("q8_0".to_string()),
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.cache_type_k, Some("q4_0".to_string()));
    assert_eq!(result.cache_type_v, Some("q8_0".to_string()));
}

/// cache_type_k that exceeds MAX_CACHE_TYPE must be rejected.
#[test]
fn test_validate_cache_type_k_too_long() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        display_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        kv_unified: None,
        cache_type_k: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        cache_type_v: None,
        spec_decoding: None,
    };
    let result = validate_model_body(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_k"));
}

/// cache_type_v that exceeds MAX_CACHE_TYPE must be rejected.
#[test]
fn test_validate_cache_type_v_too_long() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        display_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        spec_decoding: None,
    };
    let result = validate_model_body(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_v"));
}

/// cache_type_k/v at exactly MAX_CACHE_TYPE must pass.
#[test]
fn test_validate_cache_type_at_limit() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        display_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        kv_unified: None,
        cache_type_k: Some("a".repeat(MAX_CACHE_TYPE)),
        cache_type_v: Some("b".repeat(MAX_CACHE_TYPE)),
        spec_decoding: None,
    };
    assert!(validate_model_body(&body).is_ok());
}

/// When cache_type_k/v are omitted in the body, they should be None.
#[test]
fn test_apply_model_body_cache_type_defaults_none() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.cache_type_k, None);
    assert_eq!(result.cache_type_v, None);
}

/// Whitespace-only cache_type_k/v must be normalized to None.
#[test]
fn test_apply_model_body_cache_type_whitespace_only_becomes_none() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: Some("   ".to_string()),
        cache_type_v: Some("\t\n".to_string()),
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(
        result.cache_type_k, None,
        "whitespace-only cache_type_k must become None"
    );
    assert_eq!(
        result.cache_type_v, None,
        "whitespace-only cache_type_v must become None"
    );
}

/// cache_type_k/v with leading/trailing whitespace must be trimmed.
#[test]
fn test_apply_model_body_cache_type_trims_whitespace() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: Some("  q4_0  ".to_string()),
        cache_type_v: Some(" q8_0 ".to_string()),
        spec_decoding: None,
    };

    let result = apply_model_body(body, None);
    assert_eq!(result.cache_type_k, Some("q4_0".to_string()));
    assert_eq!(result.cache_type_v, Some("q8_0".to_string()));
}

// ── Partial update preservation tests ──────────────────────────────────────

/// When the body omits `context_length` (None), the existing DB value must be
/// preserved — this is a partial update, not a full replacement.
#[test]
fn test_apply_model_body_context_length_preserves_base_when_omitted() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        context_length: Some(4096),
        ..Default::default()
    };

    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None, // omitted — should preserve existing
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.context_length,
        Some(4096),
        "context_length must be preserved when body omits the field"
    );
}

/// When the body omits `cache_type_k` (None), the existing DB value must be
/// preserved — this is a partial update, not a full replacement.
#[test]
fn test_apply_model_body_cache_type_k_preserves_base_when_omitted() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        cache_type_k: Some("q4_0".to_string()),
        ..Default::default()
    };

    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None, // omitted — should preserve existing
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.cache_type_k,
        Some("q4_0".to_string()),
        "cache_type_k must be preserved when body omits the field"
    );
}

/// When the body omits `cache_type_v` (None), the existing DB value must be
/// preserved — this is a partial update, not a full replacement.
#[test]
fn test_apply_model_body_cache_type_v_preserves_base_when_omitted() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        cache_type_v: Some("q8_0".to_string()),
        ..Default::default()
    };

    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None, // omitted — should preserve existing
        spec_decoding: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.cache_type_v,
        Some("q8_0".to_string()),
        "cache_type_v must be preserved when body omits the field"
    );
}

/// When the body sends whitespace-only `cache_type_k`, it is filtered to None
/// and the existing DB value should be preserved (new behavior after fix).
#[test]
fn test_apply_model_body_cache_type_k_whitespace_preserves_base_when_existing() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        cache_type_k: Some("q4_0".to_string()),
        ..Default::default()
    };

    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: Some("   ".to_string()), // whitespace-only — filtered to None
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.cache_type_k,
        Some("q4_0".to_string()),
        "whitespace-only cache_type_k must fall back to existing value"
    );
}

/// When the body sends whitespace-only `cache_type_v`, it is filtered to None
/// and the existing DB value should be preserved (new behavior after fix).
#[test]
fn test_apply_model_body_cache_type_v_whitespace_preserves_base_when_existing() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        cache_type_v: Some("q8_0".to_string()),
        ..Default::default()
    };

    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: None,
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: Some("   ".to_string()), // whitespace-only — filtered to None
        spec_decoding: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.cache_type_v,
        Some("q8_0".to_string()),
        "whitespace-only cache_type_v must fall back to existing value"
    );
}

/// When the body explicitly provides `context_length`, it must override the
/// existing DB value — body wins on explicit assignment.
#[test]
fn test_apply_model_body_context_length_body_wins_over_base() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        context_length: Some(4096),
        ..Default::default()
    };

    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: None,
        model: Some("model.gguf".to_string()),
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: vec![],
        sampling: None,
        enabled: None,
        context_length: Some(8192), // explicit override
        num_parallel: None,
        port: None,
        api_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        display_name: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.context_length,
        Some(8192),
        "body context_length must override existing value when explicitly provided"
    );
}
