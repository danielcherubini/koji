use super::*;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use std::collections::BTreeMap;
use std::sync::Arc;
use tama_core::config::{ModelConfig, QuantEntry, QuantKind};
use tama_core::proxy::tama_handlers::{ModelMutationResponse, OkResponse};
use tower::ServiceExt;

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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: Default::default(),

        n_batch: None,

        n_ubatch: None,
    }
}

/// When an existing entry has a stored `size_bytes`, a PUT that tries to
/// change it must be silently ignored — the server-side value wins.
#[test]
fn test_apply_model_body_preserves_existing_size_bytes() {
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
fn test_apply_model_body_accepts_client_size_when_none_stored() {
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
fn test_apply_model_body_accepts_client_size_for_new_model() {
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
fn test_apply_model_body_accepts_client_size_for_new_quant_key() {
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

/// Minimal ModelBody for tests — only required fields set, all optional None.
fn body_minimal() -> ModelBody {
    ModelBody {
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
    }
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: Default::default(),

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: Default::default(),

        n_batch: None,

        n_ubatch: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(result.gpu_device, Some("CUDA0".to_string()));
}

/// Verify that body.gpu_device = Some("__clear__") clears the existing gpu_device
/// (the "None" option in the model editor means "don't isolate to a GPU").
#[test]
fn test_apply_model_body_gpu_device_clear_sentinel() {
    let body = ModelBody {
        backend: "llama-cpp".to_string(),
        gpu_variant: None,
        gpu_device: Some("__clear__".to_string()),
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: Default::default(),

        n_batch: None,

        n_ubatch: None,
    };

    let result = apply_model_body(body, Some(existing));
    assert_eq!(result.gpu_device, None);
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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
        vllm: None,

        n_batch: None,

        n_ubatch: None,
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

    let result = apply_model_body(body_minimal(), Some(existing));
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

    let result = apply_model_body(body_minimal(), Some(existing));
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

    let result = apply_model_body(body_minimal(), Some(existing));
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

    let mut body = body_minimal();
    body.cache_type_k = Some("   ".to_string()); // whitespace-only — filtered to None

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

    let mut body = body_minimal();
    body.cache_type_v = Some("   ".to_string()); // whitespace-only — filtered to None

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

    let mut body = body_minimal();
    body.context_length = Some(8192); // explicit override

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.context_length,
        Some(8192),
        "body context_length must override existing value when explicitly provided"
    );
}

/// When the body explicitly provides `cache_type_k`, it must override the
/// existing DB value — body wins on explicit assignment.
#[test]
fn test_apply_model_body_cache_type_k_body_wins_over_base() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        cache_type_k: Some("q4_0".to_string()),
        ..Default::default()
    };

    let mut body = body_minimal();
    body.cache_type_k = Some("f8".to_string()); // explicit override

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.cache_type_k,
        Some("f8".to_string()),
        "body cache_type_k must override existing value when explicitly provided"
    );
}

/// When the body explicitly provides `cache_type_v`, it must override the
/// existing DB value — body wins on explicit assignment.
#[test]
fn test_apply_model_body_cache_type_v_body_wins_over_base() {
    let existing = ModelConfig {
        backend: "llama-cpp".into(),
        cache_type_v: Some("q8_0".to_string()),
        ..Default::default()
    };

    let mut body = body_minimal();
    body.cache_type_v = Some("f16".to_string()); // explicit override

    let result = apply_model_body(body, Some(existing));
    assert_eq!(
        result.cache_type_v,
        Some("f16".to_string()),
        "body cache_type_v must override existing value when explicitly provided"
    );
}

// ── PATCH /tama/v1/models/:id tests ────────────────────────────────────────

/// Helper: create a ModelPatchBody with every field set to None.
fn patch_body_all_none() -> ModelPatchBody {
    ModelPatchBody {
        backend: None,
        gpu_variant: None,
        gpu_device: None,
        model: None,
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: None,
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
        cache_type_v: None,
        spec_decoding: None,
        vllm: None,

        n_batch: None,

        n_ubatch: None,
    }
}

/// Helper: create a ModelPatchBody with a single field changed.
fn patch_body_single_context_length(val: u32) -> ModelPatchBody {
    ModelPatchBody {
        backend: None,
        gpu_variant: None,
        gpu_device: None,
        model: None,
        quant: None,
        mmproj: None,
        mtp_model: None,
        args: None,
        sampling: None,
        enabled: None,
        context_length: Some(val),
        num_parallel: None,
        port: None,
        api_name: None,
        display_name: None,
        gpu_layers: None,
        quants: None,
        modalities: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: None,
        spec_decoding: None,
        vllm: None,

        n_batch: None,

        n_ubatch: None,
    }
}

/// Helper: create a ModelConfig with rich fields for patch testing.
fn existing_config_rich() -> ModelConfig {
    let mut quants = BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(4_567_890),
            context_length: Some(4096),
        },
    );
    ModelConfig {
        backend: "llama-cpp".into(),
        gpu_variant: Some(tama_core::gpu::GpuVariant::Cuda {
            version: String::new(),
        }),
        gpu_device: Some("0".into()),
        model: Some("org/repo".into()),
        quant: Some("Q4_K_M".into()),
        mmproj: None,
        mtp_model: None,
        args: vec!["--ctx-size".to_string(), "4096".to_string()],
        sampling: Some(tama_core::profiles::SamplingParams {
            temperature: Some(0.7),
            ..Default::default()
        }),
        enabled: true,
        context_length: Some(4096),
        num_parallel: Some(1),
        port: Some(18910),
        health_check: None,
        profile: Some("default".to_string()),
        api_name: Some("my-api".into()),
        gpu_layers: Some(32),
        quants,
        modalities: None,
        display_name: Some("My Model".into()),
        kv_unified: false,
        cache_type_k: Some("q4_0".into()),
        cache_type_v: Some("q8_0".into()),
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        db_id: Some(42),
        spec_decoding: Default::default(),
        vllm: Default::default(),

        n_batch: None,

        n_ubatch: None,
    }
}

// ── apply_model_patch unit tests ───────────────────────────────────────────

/// An all-None patch body must preserve every field in the existing config.
#[test]
fn test_apply_model_patch_all_none_preserves_all_fields() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);

    assert_eq!(result.backend, "llama-cpp");
    assert_eq!(
        result.gpu_variant,
        Some(tama_core::gpu::GpuVariant::Cuda {
            version: String::new()
        })
    );
    assert_eq!(result.gpu_device, Some("0".into()));
    assert_eq!(result.model, Some("org/repo".into()));
    assert_eq!(result.quant, Some("Q4_K_M".into()));
    assert_eq!(
        result.args,
        vec!["--ctx-size".to_string(), "4096".to_string()]
    );
    assert!(result.sampling.is_some());
    assert_eq!(result.context_length, Some(4096));
    assert_eq!(result.num_parallel, Some(1));
    assert_eq!(result.port, Some(18910));
    assert_eq!(result.api_name, Some("my-api".into()));
    assert_eq!(result.gpu_layers, Some(32));
    assert_eq!(result.display_name, Some("My Model".into()));
    assert!(!result.kv_unified);
    assert_eq!(result.cache_type_k, Some("q4_0".into()));
    assert_eq!(result.cache_type_v, Some("q8_0".into()));
    assert_eq!(result.profile, Some("default".to_string()));
    assert_eq!(result.db_id, Some(42));
    // quants preserved
    let q = result.quants.get("Q4_K_M").unwrap();
    assert_eq!(q.size_bytes, Some(4_567_890));
    assert_eq!(q.file, "model-Q4_K_M.gguf");
}

/// A body with only `context_length: Some(8192)` changes only that field.
#[test]
fn test_apply_model_patch_single_field_changes_only_that_field() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_single_context_length(8192), &existing);

    assert_eq!(result.context_length, Some(8192));
    // All other fields preserved
    assert_eq!(result.backend, "llama-cpp");
    assert_eq!(result.model, Some("org/repo".into()));
    assert_eq!(result.cache_type_k, Some("q4_0".into()));
}

/// `args: Some(vec![])` must clear args to empty.
#[test]
fn test_apply_model_patch_args_some_empty_clears() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.args = Some(vec![]);

    let result = apply_model_patch(body, &existing);
    assert!(result.args.is_empty());
}

/// `args: None` must preserve existing args.
#[test]
fn test_apply_model_patch_args_none_preserves() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);

    assert_eq!(
        result.args,
        vec!["--ctx-size".to_string(), "4096".to_string()]
    );
}

/// `backend: None` must preserve existing backend.
#[test]
fn test_apply_model_patch_backend_none_preserves() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);

    assert_eq!(result.backend, "llama-cpp");
}

/// `backend: Some("new")` must override the existing backend.
#[test]
fn test_apply_model_patch_backend_some_overrides() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.backend = Some("llama".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.backend, "llama");
}

/// Quants size_bytes must be preserved per-key (security check).
#[test]
fn test_apply_model_patch_quants_size_bytes_preserved() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();

    // Client sends a different size_bytes for the same key
    let mut incoming_quants = BTreeMap::new();
    incoming_quants.insert(
        "Q4_K_M".to_string(),
        QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(1), // malicious / stale
            context_length: Some(8192),
        },
    );
    body.quants = Some(incoming_quants);

    let result = apply_model_patch(body, &existing);
    let q = result.quants.get("Q4_K_M").unwrap();
    assert_eq!(
        q.size_bytes,
        Some(4_567_890),
        "existing size_bytes must be preserved against client override"
    );
    assert_eq!(q.context_length, Some(8192)); // non-size fields still update
}

/// `cache_type_k: Some("__custom")` must be filtered to None and fall back
/// to the existing base value.
#[test]
fn test_apply_model_patch_cache_type_custom_filtered() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.cache_type_k = Some("__custom".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(
        result.cache_type_k,
        Some("q4_0".into()),
        "__custom sentinel must be filtered, falling back to existing"
    );
}

/// `profile` must be preserved by PATCH (deviation from PUT which sets None).
#[test]
fn test_apply_model_patch_profile_preserved() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);

    assert_eq!(
        result.profile,
        Some("default".to_string()),
        "PATCH must preserve profile (PUT sets None)"
    );
}

/// `sampling: None` must preserve existing sampling params.
#[test]
fn test_apply_model_patch_sampling_preserved() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);

    assert!(
        result.sampling.is_some(),
        "sampling must be preserved when body omits the field"
    );
}

/// `cache_type_k` with valid value must override existing.
#[test]
fn test_apply_model_patch_cache_type_k_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.cache_type_k = Some("f16".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.cache_type_k, Some("f16".into()));
}

/// `cache_type_v` with valid value must override existing.
#[test]
fn test_apply_model_patch_cache_type_v_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.cache_type_v = Some("f8".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.cache_type_v, Some("f8".into()));
}

/// Server-side fields (hf_*) must always be preserved.
#[test]
fn test_apply_model_patch_server_side_fields_preserved() {
    let mut existing = existing_config_rich();
    existing.hf_format = Some("gguf".into());
    existing.hf_base_model = Some("meta-llama/Llama-3".into());
    existing.hf_pipeline_tag = Some("text-generation".into());
    existing.hf_total_params = Some("7B".into());
    existing.hf_active_params = Some("7B".into());
    existing.hf_architecture_type = Some("llama".into());
    existing.hf_context_length = Some(8192u32);
    existing.hf_num_layers = Some(32u32);
    existing.hf_last_modified = Some("2024-01-15T10:30:00Z".into());

    let result = apply_model_patch(patch_body_all_none(), &existing);

    assert_eq!(result.hf_format, Some("gguf".into()));
    assert_eq!(result.hf_base_model, Some("meta-llama/Llama-3".into()));
    assert_eq!(result.hf_pipeline_tag, Some("text-generation".into()));
    assert_eq!(result.hf_total_params, Some("7B".into()));
    assert_eq!(result.hf_active_params, Some("7B".into()));
    assert_eq!(result.hf_architecture_type, Some("llama".into()));
    assert_eq!(result.hf_context_length, Some(8192u32));
    assert_eq!(result.hf_num_layers, Some(32u32));
    assert_eq!(result.hf_last_modified, Some("2024-01-15T10:30:00Z".into()));
}

// ── validate_model_patch unit tests ────────────────────────────────────────

/// `backend: Some("")` must be rejected.
#[test]
fn test_validate_model_patch_empty_backend_rejected() {
    let body = ModelPatchBody {
        backend: Some("".to_string()),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err(), "empty backend must be rejected");
    assert!(result.unwrap_err().contains("backend"));
}

/// `backend: Some("valid")` must pass validation.
#[test]
fn test_validate_model_patch_valid_backend_accepted() {
    let body = ModelPatchBody {
        backend: Some("llama-cpp".to_string()),
        ..patch_body_all_none()
    };
    assert!(validate_model_patch(&body).is_ok());
}

/// An all-None body must pass validation (no-op).
#[test]
fn test_validate_model_patch_all_none_valid() {
    let body = patch_body_all_none();
    assert!(
        validate_model_patch(&body).is_ok(),
        "all-None body must be valid (no-op)"
    );
}

/// `model: Some("")` must be rejected.
#[test]
fn test_validate_model_patch_empty_model_rejected() {
    let body = ModelPatchBody {
        model: Some("".to_string()),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err(), "empty model must be rejected");
}

/// `model` exceeding MAX_MODEL must be rejected.
#[test]
fn test_validate_model_patch_model_too_long_rejected() {
    let body = ModelPatchBody {
        model: Some("a".repeat(MAX_MODEL + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("model"));
}

/// `quant` exceeding MAX_QUANT must be rejected.
#[test]
fn test_validate_model_patch_quant_too_long_rejected() {
    let body = ModelPatchBody {
        quant: Some("a".repeat(MAX_QUANT + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("quant"));
}

/// `cache_type_k: Some("__custom")` must be rejected during validation.
#[test]
fn test_validate_model_patch_cache_type_k_custom_rejected() {
    let body = ModelPatchBody {
        cache_type_k: Some("__custom".to_string()),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("__custom"));
}

/// `cache_type_k` exceeding MAX_CACHE_TYPE must be rejected.
#[test]
fn test_validate_model_patch_cache_type_k_too_long() {
    let body = ModelPatchBody {
        cache_type_k: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_k"));
}

/// `cache_type_v` exceeding MAX_CACHE_TYPE must be rejected.
#[test]
fn test_validate_model_patch_cache_type_v_too_long() {
    let body = ModelPatchBody {
        cache_type_v: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_v"));
}

/// `api_name` exceeding MAX_API_NAME must be rejected.
#[test]
fn test_validate_model_patch_api_name_too_long() {
    let body = ModelPatchBody {
        api_name: Some("a".repeat(MAX_API_NAME + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("api_name"));
}

/// `display_name` exceeding MAX_DISPLAY_NAME must be rejected.
#[test]
fn test_validate_model_patch_display_name_too_long() {
    let body = ModelPatchBody {
        display_name: Some("a".repeat(MAX_DISPLAY_NAME + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("display_name"));
}

/// `mmproj` exceeding MAX_MMPROJ must be rejected.
#[test]
fn test_validate_model_patch_mmproj_too_long() {
    let body = ModelPatchBody {
        mmproj: Some("a".repeat(MAX_MMPROJ + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("mmproj"));
}

/// `backend` exceeding MAX_BACKEND must be rejected.
#[test]
fn test_validate_model_patch_backend_too_long() {
    let body = ModelPatchBody {
        backend: Some("a".repeat(MAX_BACKEND + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("backend"));
}

/// `kv_unified: Some(false)` must override existing true.
#[test]
fn test_apply_model_patch_kv_unified_override() {
    let mut existing = existing_config_rich();
    existing.kv_unified = true;
    let mut body = patch_body_all_none();
    body.kv_unified = Some(false);

    let result = apply_model_patch(body, &existing);
    assert!(!result.kv_unified);
}

/// `kv_unified: None` must preserve existing value.
#[test]
fn test_apply_model_patch_kv_unified_preserves() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);
    assert!(!result.kv_unified);
}

/// `enabled: Some(false)` must override existing true.
#[test]
fn test_apply_model_patch_enabled_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.enabled = Some(false);

    let result = apply_model_patch(body, &existing);
    assert!(!result.enabled);
}

/// `enabled: None` must preserve existing value.
#[test]
fn test_apply_model_patch_enabled_preserves() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);
    assert!(result.enabled);
}

/// `spec_decoding: Some(...)` must override existing.
#[test]
fn test_apply_model_patch_spec_decoding_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();

    let new_spec = tama_core::config::SpecDecodingConfig {
        spec_types: vec!["draft-mtp".to_string()],
        n_max: Some(4),
        n_min: Some(2),
        draft_ngl: Some(20),
    };
    body.spec_decoding = Some(new_spec.clone());

    let result = apply_model_patch(body, &existing);
    assert_eq!(
        result.spec_decoding.spec_types,
        vec!["draft-mtp".to_string()]
    );
    assert_eq!(result.spec_decoding.n_max, Some(4));
}

/// `spec_decoding: None` must preserve existing spec_decoding.
#[test]
fn test_apply_model_patch_spec_decoding_preserves() {
    let existing = existing_config_rich();
    let result = apply_model_patch(patch_body_all_none(), &existing);
    // Default spec_decoding is preserved
    assert_eq!(result.spec_decoding, Default::default());
}

/// `display_name: Some("New Name")` must override existing.
#[test]
fn test_apply_model_patch_display_name_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.display_name = Some("New Name".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.display_name, Some("New Name".into()));
}

/// `api_name: Some("new-api")` must override existing.
#[test]
fn test_apply_model_patch_api_name_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.api_name = Some("new-api".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.api_name, Some("new-api".into()));
}

/// `gpu_layers: Some(40)` must override existing.
#[test]
fn test_apply_model_patch_gpu_layers_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.gpu_layers = Some(40);

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.gpu_layers, Some(40));
}

/// `modalities: Some(...)` must override existing.
#[test]
fn test_apply_model_patch_modalities_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.modalities = Some(tama_core::config::ModelModalities {
        input: vec!["text".to_string()],
        output: vec!["text".to_string()],
    });

    let result = apply_model_patch(body, &existing);
    assert!(result.modalities.is_some());
    let m = result.modalities.unwrap();
    assert_eq!(m.input, vec!["text".to_string()]);
    assert_eq!(m.output, vec!["text".to_string()]);
}

/// `port: Some(1234)` must override existing.
#[test]
fn test_apply_model_patch_port_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.port = Some(1234);

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.port, Some(1234));
}

/// `num_parallel: Some(4)` must override existing.
#[test]
fn test_apply_model_patch_num_parallel_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.num_parallel = Some(4);

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.num_parallel, Some(4));
}

/// `sampling: Some(...)` must override existing sampling params.
#[test]
fn test_apply_model_patch_sampling_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.sampling = Some(tama_core::profiles::SamplingParams {
        temperature: Some(1.0),
        ..Default::default()
    });

    let result = apply_model_patch(body, &existing);
    assert!(result.sampling.is_some());
    assert_eq!(result.sampling.unwrap().temperature, Some(1.0));
}

/// `model: Some("new/repo")` must override existing.
#[test]
fn test_apply_model_patch_model_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.model = Some("new/repo".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.model, Some("new/repo".into()));
}

/// `quant: Some("Q8_0")` must override existing.
#[test]
fn test_apply_model_patch_quant_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.quant = Some("Q8_0".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.quant, Some("Q8_0".into()));
}

/// `gpu_device: Some("__clear__")` must clear the existing gpu_device
/// (the "None" option in the model editor means "don't isolate to a GPU").
#[test]
fn test_apply_model_patch_gpu_device_clear_sentinel() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.gpu_device = Some("__clear__".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.gpu_device, None);
}

/// `gpu_variant: Some(RocM)` must override existing.
#[test]
fn test_apply_model_patch_gpu_variant_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.gpu_variant = Some(tama_core::gpu::GpuVariant::RocM {
        version: String::new(),
    });

    let result = apply_model_patch(body, &existing);
    assert_eq!(
        result.gpu_variant,
        Some(tama_core::gpu::GpuVariant::RocM {
            version: String::new()
        })
    );
}

/// `gpu_device: Some("1")` must override existing.
#[test]
fn test_apply_model_patch_gpu_device_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.gpu_device = Some("1".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.gpu_device, Some("1".into()));
}

/// `mmproj: Some("mmproj.gguf")` must override existing.
#[test]
fn test_apply_model_patch_mmproj_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.mmproj = Some("mmproj.gguf".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.mmproj, Some("mmproj.gguf".into()));
}

/// `mtp_model: Some("mtp.gguf")` must override existing.
#[test]
fn test_apply_model_patch_mtp_model_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.mtp_model = Some("mtp.gguf".to_string());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.mtp_model, Some("mtp.gguf".into()));
}

// ── Route-level tests ──────────────────────────────────────────────────────\n
/// Regression test: DELETE /tama/v1/models/:id removes the DB row via
/// Repository::delete_config (no raw SQL, no ModelManager).
#[tokio::test]
async fn test_delete_model_removes_db_row() {
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use std::sync::Arc;
    use tower::ServiceExt;

    let tmp_dir = tempfile::tempdir().expect("tempdir");

    // Seed a model config in the DB.
    let open_result = tama_core::db::open(tmp_dir.path()).unwrap();
    let model_id = tama_core::db::queries::upsert_model_config(
        &open_result.conn,
        &tama_core::db::queries::ModelConfigRecord {
            id: 0,
            repo_id: "test-org/test-model".to_string(),
            display_name: None,
            backend: "llama_cpp".to_string(),
            gpu_variant: None,
            gpu_device: None,
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            selected_mtp_model: None,
            context_length: None,
            num_parallel: None,
            kv_unified: false,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: Some("test-org/test-model".to_string()),
            health_check: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            spec_decoding: None,

            n_batch: None,

            n_ubatch: None,
            vllm_config: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        },
    )
    .unwrap();

    // Build the proxy state with the tempdir as db_dir.
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        Some(tmp_dir.path().to_path_buf()),
    ));

    // Reuse the test WebState from backends/manage/tests.rs pattern.
    let web_state = Arc::new(crate::web_types::WebState {
        jobs: Some(Arc::new(crate::web_types::JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        repository: Some(Arc::new(std::sync::Mutex::new(
            tama_core::db::repository::Repository::open(tmp_dir.path()).unwrap(),
        ))),
    });

    let router = crate::router::build_web_routes(web_state.clone())
        .with_state(state)
        .layer(axum::extract::Extension(web_state.as_ref().clone()));

    // DELETE /tama/v1/models/:id — CSRF middleware allows DELETE without token.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/tama/v1/models/{}", model_id))
        .body(Body::empty())
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify the DB row is gone.
    let conn = tama_core::db::open(tmp_dir.path()).unwrap();
    let record = tama_core::db::queries::get_model_config(&conn.conn, model_id).unwrap();
    assert!(record.is_none(), "model config should be deleted from DB");
}

/// `context_length: Some(16384)` must override existing.
#[test]
fn test_apply_model_patch_context_length_override() {
    let existing = existing_config_rich();
    let mut body = patch_body_all_none();
    body.context_length = Some(16384);

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.context_length, Some(16384));
}

// ── Drift-guard: CRUD response round-trips ─────────────────────────────────

/// Model create response must deserialize into ModelMutationResponse with
/// ok && id > 0.
#[tokio::test]
async fn test_create_model_response_deserializes_into_mutation_response() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");

    // Seed a model so we have a valid DB to create against.
    {
        let conn = tama_core::db::open(tmp_dir.path()).unwrap();
        tama_core::db::queries::upsert_model_config(
            &conn.conn,
            &tama_core::db::queries::ModelConfigRecord {
                id: 0,
                repo_id: "test-org/seed".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
                gpu_variant: None,
                gpu_device: None,
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                selected_mtp_model: None,
                context_length: None,
                num_parallel: None,
                kv_unified: false,
                gpu_layers: None,
                cache_type_k: None,
                cache_type_v: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                hf_format: None,
                hf_base_model: None,
                hf_pipeline_tag: None,
                hf_total_params: None,
                hf_active_params: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                hf_last_modified: None,
                spec_decoding: None,

                n_batch: None,

                n_ubatch: None,
                vllm_config: None,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            },
        )
        .unwrap();
    }

    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        Some(tmp_dir.path().to_path_buf()),
    ));

    let web_state = Arc::new(crate::web_types::WebState {
        jobs: Some(Arc::new(crate::web_types::JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        repository: Some(Arc::new(std::sync::Mutex::new(
            tama_core::db::repository::Repository::open(tmp_dir.path()).unwrap(),
        ))),
    });

    let router = crate::router::build_web_routes(web_state.clone())
        .with_state(state)
        .layer(axum::extract::Extension(web_state.as_ref().clone()));

    // POST /tama/v1/models — create a new model.
    let body = serde_json::json!({
        "repo_id": "org/create-drift",
        "backend": "llama_cpp",
        "model": "org/create-drift"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/tama/v1/models")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body must be readable");

    // Deserialize into ModelMutationResponse.
    let parsed: ModelMutationResponse = serde_json::from_slice(&body_bytes)
        .expect("create response must deserialize into ModelMutationResponse");
    assert!(parsed.ok, "ok must be true");
    assert!(parsed.id > 0, "id must be > 0");

    // Lossless round-trip.
    let raw_value: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("body must be valid JSON");
    assert_eq!(
        serde_json::to_value(parsed).expect("parsed must serialize"),
        raw_value,
        "ModelMutationResponse round-trip must be lossless"
    );
}

/// Model delete response must deserialize into OkResponse with ok.
#[tokio::test]
async fn test_delete_model_response_deserializes_into_ok_response() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");

    // Seed a model to delete.
    {
        let conn = tama_core::db::open(tmp_dir.path()).unwrap();
        tama_core::db::queries::upsert_model_config(
            &conn.conn,
            &tama_core::db::queries::ModelConfigRecord {
                id: 0,
                repo_id: "org/delete-drift".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
                gpu_variant: None,
                gpu_device: None,
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                selected_mtp_model: None,
                context_length: None,
                num_parallel: None,
                kv_unified: false,
                gpu_layers: None,
                cache_type_k: None,
                cache_type_v: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                hf_format: None,
                hf_base_model: None,
                hf_pipeline_tag: None,
                hf_total_params: None,
                hf_active_params: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                hf_last_modified: None,
                spec_decoding: None,

                n_batch: None,

                n_ubatch: None,
                vllm_config: None,
                created_at: "2024-01-01T00:00:00Z".to_string(),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            },
        )
        .unwrap();
    }

    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        Some(tmp_dir.path().to_path_buf()),
    ));

    let web_state = Arc::new(crate::web_types::WebState {
        jobs: Some(Arc::new(crate::web_types::JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        repository: Some(Arc::new(std::sync::Mutex::new(
            tama_core::db::repository::Repository::open(tmp_dir.path()).unwrap(),
        ))),
    });

    let router = crate::router::build_web_routes(web_state.clone())
        .with_state(state)
        .layer(axum::extract::Extension(web_state.as_ref().clone()));

    // Get the model id to delete.
    let conn = tama_core::db::open(tmp_dir.path()).unwrap();
    let record = tama_core::db::queries::get_model_config(&conn.conn, 1).expect("model exists");
    let model_id = record.unwrap().id;

    // DELETE /tama/v1/models/:id.
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/tama/v1/models/{}", model_id))
        .body(Body::empty())
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body must be readable");

    // Deserialize into OkResponse.
    let parsed: OkResponse = serde_json::from_slice(&body_bytes)
        .expect("delete response must deserialize into OkResponse");
    assert!(parsed.ok, "ok must be true");

    // Lossless round-trip.
    let raw_value: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("body must be valid JSON");
    assert_eq!(
        serde_json::to_value(parsed).expect("parsed must serialize"),
        raw_value,
        "OkResponse round-trip must be lossless"
    );
}
