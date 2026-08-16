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
        reasoning_levels: None,
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
        provider_name: None,
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        provider_name: None,
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        provider_name: None,
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        provider_name: None,
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
    let mut body = ModelBody {
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
        reasoning_levels: None,
        kv_unified: None,
        cache_type_k: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        cache_type_v: None,
        spec_decoding: None,
        vllm: None,

        n_batch: None,

        n_ubatch: None,
    };
    let result = validate_model_body(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_k"));
}

/// cache_type_v that exceeds MAX_CACHE_TYPE must be rejected.
#[test]
fn test_validate_cache_type_v_too_long() {
    let mut body = ModelBody {
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
        reasoning_levels: None,
        kv_unified: None,
        cache_type_k: None,
        cache_type_v: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        spec_decoding: None,
        vllm: None,

        n_batch: None,

        n_ubatch: None,
    };
    let result = validate_model_body(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_v"));
}

/// cache_type_k/v at exactly MAX_CACHE_TYPE must pass.
#[test]
fn test_validate_cache_type_at_limit() {
    let mut body = ModelBody {
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
        reasoning_levels: None,
        kv_unified: None,
        cache_type_k: Some("a".repeat(MAX_CACHE_TYPE)),
        cache_type_v: Some("b".repeat(MAX_CACHE_TYPE)),
        spec_decoding: None,
        vllm: None,

        n_batch: None,

        n_ubatch: None,
    };
    assert!(validate_model_body(&mut body).is_ok());
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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

/// `reasoning_levels: Some([...])` on a new model sets the field.
#[test]
fn test_apply_model_body_reasoning_levels_set_on_create() {
    let mut body = body_minimal();
    body.reasoning_levels = Some(vec!["off".to_string(), "low".to_string()]);

    let result = apply_model_body(body, None);
    assert_eq!(
        result.reasoning_levels,
        Some(vec!["off".to_string(), "low".to_string()])
    );
}

/// Omitted `reasoning_levels` on a PUT preserves the base value.
#[test]
fn test_apply_model_body_reasoning_levels_preserves_base_when_omitted() {
    let mut base = existing_with_size("Q4_K_M", "model-Q4_K_M.gguf", Some(100));
    base.reasoning_levels = Some(vec!["low".to_string()]);

    let body = body_minimal(); // reasoning_levels: None
    let result = apply_model_body(body, Some(base));
    assert_eq!(result.reasoning_levels, Some(vec!["low".to_string()]));
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
        reasoning_levels: None,
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
        reasoning_levels: None,
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
        provider_name: None,
        reasoning_levels: None,
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

/// `reasoning_levels: Some([...])` overrides the existing value.
#[test]
fn test_apply_model_patch_reasoning_levels_some_overrides() {
    let mut existing = existing_config_rich();
    existing.reasoning_levels = Some(vec!["low".to_string()]);
    let mut body = patch_body_all_none();
    body.reasoning_levels = Some(vec!["off".to_string(), "high".to_string()]);

    let result = apply_model_patch(body, &existing);
    assert_eq!(
        result.reasoning_levels,
        Some(vec!["off".to_string(), "high".to_string()])
    );
}

/// `reasoning_levels: None` preserves the existing value.
#[test]
fn test_apply_model_patch_reasoning_levels_none_preserves() {
    let mut existing = existing_config_rich();
    existing.reasoning_levels = Some(vec!["low".to_string()]);

    let result = apply_model_patch(patch_body_all_none(), &existing);
    assert_eq!(result.reasoning_levels, Some(vec!["low".to_string()]));
}

/// `reasoning_levels: Some(vec![])` clears the existing value — the
/// editor sends `[]` (never `null`) when the input is emptied.
#[test]
fn test_apply_model_patch_reasoning_levels_empty_clears() {
    let mut existing = existing_config_rich();
    existing.reasoning_levels = Some(vec!["low".to_string()]);
    let mut body = patch_body_all_none();
    body.reasoning_levels = Some(Vec::new());

    let result = apply_model_patch(body, &existing);
    assert_eq!(result.reasoning_levels, Some(Vec::<String>::new()));
}

// ── validate_model_patch unit tests ────────────────────────────────────────

/// `backend: Some("")` must be rejected.
#[test]
fn test_validate_model_patch_empty_backend_rejected() {
    let mut body = ModelPatchBody {
        backend: Some("".to_string()),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err(), "empty backend must be rejected");
    assert!(result.unwrap_err().contains("backend"));
}

/// `backend: Some("valid")` must pass validation.
#[test]
fn test_validate_model_patch_valid_backend_accepted() {
    let mut body = ModelPatchBody {
        backend: Some("llama-cpp".to_string()),
        ..patch_body_all_none()
    };
    assert!(validate_model_patch(&mut body).is_ok());
}

/// An all-None body must pass validation (no-op).
#[test]
fn test_validate_model_patch_all_none_valid() {
    let mut body = patch_body_all_none();
    assert!(
        validate_model_patch(&mut body).is_ok(),
        "all-None body must be valid (no-op)"
    );
}

/// `model: Some("")` must be rejected.
#[test]
fn test_validate_model_patch_empty_model_rejected() {
    let mut body = ModelPatchBody {
        model: Some("".to_string()),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err(), "empty model must be rejected");
}

/// `model` exceeding MAX_MODEL must be rejected.
#[test]
fn test_validate_model_patch_model_too_long_rejected() {
    let mut body = ModelPatchBody {
        model: Some("a".repeat(MAX_MODEL + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("model"));
}

/// `quant` exceeding MAX_QUANT must be rejected.
#[test]
fn test_validate_model_patch_quant_too_long_rejected() {
    let mut body = ModelPatchBody {
        quant: Some("a".repeat(MAX_QUANT + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("quant"));
}

/// `cache_type_k: Some("__custom")` must be rejected during validation.
#[test]
fn test_validate_model_patch_cache_type_k_custom_rejected() {
    let mut body = ModelPatchBody {
        cache_type_k: Some("__custom".to_string()),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("__custom"));
}

/// `cache_type_k` exceeding MAX_CACHE_TYPE must be rejected.
#[test]
fn test_validate_model_patch_cache_type_k_too_long() {
    let mut body = ModelPatchBody {
        cache_type_k: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_k"));
}

/// `cache_type_v` exceeding MAX_CACHE_TYPE must be rejected.
#[test]
fn test_validate_model_patch_cache_type_v_too_long() {
    let mut body = ModelPatchBody {
        cache_type_v: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cache_type_v"));
}

/// `api_name` exceeding MAX_API_NAME must be rejected.
#[test]
fn test_validate_model_patch_api_name_too_long() {
    let mut body = ModelPatchBody {
        api_name: Some("a".repeat(MAX_API_NAME + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("api_name"));
}

/// `display_name` exceeding MAX_DISPLAY_NAME must be rejected.
#[test]
fn test_validate_model_patch_display_name_too_long() {
    let mut body = ModelPatchBody {
        display_name: Some("a".repeat(MAX_DISPLAY_NAME + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("display_name"));
}

/// `mmproj` exceeding MAX_MMPROJ must be rejected.
#[test]
fn test_validate_model_patch_mmproj_too_long() {
    let mut body = ModelPatchBody {
        mmproj: Some("a".repeat(MAX_MMPROJ + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("mmproj"));
}

/// `backend` exceeding MAX_BACKEND must be rejected.
#[test]
fn test_validate_model_patch_backend_too_long() {
    let mut body = ModelPatchBody {
        backend: Some("a".repeat(MAX_BACKEND + 1)),
        ..patch_body_all_none()
    };
    let result = validate_model_patch(&mut body);
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

// ── reasoning-levels validation tests ───────────────────────────────────

/// Some `reasoning_levels` are normalized in place (trim + lowercase +
/// dedupe) so the cleaned values are what gets persisted.
#[test]
fn test_validate_model_body_reasoning_levels_normalized_in_place() {
    let mut body = body_minimal();
    body.reasoning_levels = Some(vec![
        " Off ".to_string(),
        "LOW".to_string(),
        "low".to_string(),
    ]);

    validate_model_body(&mut body).expect("valid levels accepted");
    assert_eq!(
        body.reasoning_levels,
        Some(vec!["off".to_string(), "low".to_string()])
    );
}

/// An invalid reasoning level is rejected, naming the offender and
/// listing the full valid set.
#[test]
fn test_validate_model_body_invalid_reasoning_level_rejected() {
    let mut body = body_minimal();
    body.reasoning_levels = Some(vec!["bogus".to_string()]);

    let result = validate_model_body(&mut body);
    assert!(result.is_err(), "invalid level must be rejected");
    let err = result.unwrap_err();
    assert!(
        err.contains("bogus"),
        "error must name the offender, got: {err}"
    );
    assert!(
        err.contains("off, minimal, low, medium, high, xhigh, max"),
        "error must list the valid set, got: {err}"
    );
}

/// Absent `reasoning_levels` (None) passes validation and stays None.
#[test]
fn test_validate_model_body_reasoning_levels_none_skips() {
    let mut body = body_minimal();
    assert!(validate_model_body(&mut body).is_ok());
    assert_eq!(body.reasoning_levels, None);
}

/// Patch: Some `reasoning_levels` are normalized in place.
#[test]
fn test_validate_model_patch_reasoning_levels_normalized_in_place() {
    let mut body = patch_body_all_none();
    body.reasoning_levels = Some(vec![
        " HIGH ".to_string(),
        "high".to_string(),
        "max".to_string(),
    ]);

    validate_model_patch(&mut body).expect("valid levels accepted");
    assert_eq!(
        body.reasoning_levels,
        Some(vec!["high".to_string(), "max".to_string()])
    );
}

/// Patch: an invalid reasoning level is rejected, naming the offender
/// and listing the full valid set.
#[test]
fn test_validate_model_patch_invalid_reasoning_level_rejected() {
    let mut body = patch_body_all_none();
    body.reasoning_levels = Some(vec!["off".to_string(), "bogus".to_string()]);

    let result = validate_model_patch(&mut body);
    assert!(result.is_err(), "invalid level must be rejected");
    let err = result.unwrap_err();
    assert!(
        err.contains("bogus"),
        "error must name the offender, got: {err}"
    );
    assert!(
        err.contains("off, minimal, low, medium, high, xhigh, max"),
        "error must list the valid set, got: {err}"
    );
}

// ── Route-level tests ──────────────────────────────────────────────────────\n
/// Build a WebState + ProxyState wired to the Postgres pool (plan-190 Task 5).
fn crud_web_state(
    pool: Arc<sqlx::PgPool>,
    tmp_dir: &std::path::Path,
) -> (
    Arc<tama_core::proxy::ProxyState>,
    Arc<crate::web_types::WebState>,
) {
    let config = tama_core::config::Config::default();
    let state = Arc::new(tama_core::proxy::ProxyState::new(
        config,
        Some(tmp_dir.to_path_buf()),
        pool.clone(),
    ));
    let web_state = Arc::new(crate::web_types::WebState {
        jobs: Some(Arc::new(crate::web_types::JobManager::new())),
        capabilities: None,
        update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        db_pool: pool,
    });
    (state, web_state)
}

fn crud_router(
    state: Arc<tama_core::proxy::ProxyState>,
    web_state: Arc<crate::web_types::WebState>,
) -> axum::Router {
    crate::router::build_web_routes(web_state.clone())
        .with_state(state)
        .layer(axum::extract::Extension(web_state.as_ref().clone()))
}

/// Seed a minimal model config row in Postgres and return its id.
async fn seed_model_record(
    pool: &sqlx::PgPool,
    repo_id: &str,
    backend: &str,
    api_name: Option<&str>,
    vllm_config: Option<String>,
    reasoning_levels: Option<String>,
) -> i64 {
    tama_core::db::queries::upsert_model_config(
        pool,
        &tama_core::db::queries::ModelConfigRecord {
            id: 0,
            repo_id: repo_id.to_string(),
            display_name: None,
            backend: backend.to_string(),
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
            api_name: api_name.map(str::to_string),
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
            vllm_config,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
            provider_name: None,
            reasoning_levels,
        },
    )
    .await
    .unwrap()
}

/// Regression test: DELETE /tama/v1/models/:id removes the DB row via
/// the Postgres pool (no raw SQL, no ModelManager).
#[tokio::test]
async fn test_delete_model_removes_db_row() {
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");

    // Seed a model config in the DB.
    let model_id = seed_model_record(
        &pool,
        "test-org/test-model",
        "llama_cpp",
        Some("test-org/test-model"),
        None,
        None,
    )
    .await;

    let (state, web_state) = crud_web_state(Arc::new(pool.clone()), tmp_dir.path());
    let router = crud_router(state, web_state);

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
    let record = tama_core::db::queries::get_model_config(&pool, model_id)
        .await
        .unwrap();
    assert!(record.is_none(), "model config should be deleted from DB");

    guard.finish().await;
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
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");

    // Seed a model so we have a valid DB to create against.
    seed_model_record(&pool, "test-org/seed", "llama_cpp", None, None, None).await;

    let (state, web_state) = crud_web_state(Arc::new(pool.clone()), tmp_dir.path());
    let router = crud_router(state, web_state);

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

    guard.finish().await;
}

/// Model delete response must deserialize into OkResponse with ok.
#[tokio::test]
async fn test_delete_model_response_deserializes_into_ok_response() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");

    // Seed a model to delete.
    let model_id =
        seed_model_record(&pool, "org/delete-drift", "llama_cpp", None, None, None).await;

    let (state, web_state) = crud_web_state(Arc::new(pool.clone()), tmp_dir.path());
    let router = crud_router(state, web_state);

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

    guard.finish().await;
}

// ── PUT /tama/v1/models/:id — vllm whole-replace contract ────────────────

/// Route-level harness for `PUT /tama/v1/models/:id` vllm tests: seeds a
/// single vllm model row (with an optional stored `vllm_config` JSON) and
/// returns the web router plus the seeded model id. Mirrors the sibling
/// route-level tests above.
async fn vllm_put_harness(
    pool: &sqlx::PgPool,
    tmp_dir: &tempfile::TempDir,
    seed_vllm_config: Option<String>,
) -> (axum::Router, i64) {
    let model_id = seed_model_record(
        pool,
        "test-org/vllm-model",
        "vllm",
        Some("test-org/vllm-model"),
        seed_vllm_config,
        None,
    )
    .await;

    let (state, web_state) = crud_web_state(Arc::new(pool.clone()), tmp_dir.path());

    (crud_router(state, web_state), model_id)
}

/// Read the stored `vllm_config` column of a model row back into a VllmConfig.
async fn read_stored_vllm(pool: &sqlx::PgPool, model_id: i64) -> tama_core::config::VllmConfig {
    let record = tama_core::db::queries::get_model_config(pool, model_id)
        .await
        .unwrap()
        .expect("model row must exist");
    serde_json::from_str(record.vllm_config.as_deref().expect("vllm_config stored"))
        .expect("vllm_config must be valid JSON")
}

/// Regression test for the pull wizard's vLLM save (whole-replace contract,
/// protected half): the server's `apply_model_body` REPLACES the whole `vllm`
/// struct from the PUT body — a field missing from the body is reset to its
/// default. The wizard therefore fetches the model's stored vllm config when
/// entering the SetContext step and overlays its five fields onto it (see
/// `apply_vllm_wizard_overlays` in `components/pull_wizard/mod.rs`), so the
/// body INCLUDES the advanced fields — and they must survive the round trip
/// into the DB row unchanged.
#[tokio::test]
async fn test_update_model_vllm_body_with_advanced_fields_preserves_them() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let seed_vllm = serde_json::json!({
        "max_model_len": 32768,
        "kv_cache_dtype": "fp8",
        "tensor_parallel_size": 2,
        "gpu_memory_utilization": 0.85,
        "trust_remote_code": false,
        "enable_prefix_caching": true,
        "attention_backend": "flashinfer",
        "spec_decoding": {
            "method": "ngram",
            "num_speculative_tokens": 3,
            "draft_tensor_parallel_size": 1
        },
    });
    let seed_vllm = seed_vllm.to_string();
    let (router, model_id) = vllm_put_harness(&pool, &tmp_dir, Some(seed_vllm)).await;

    // The wizard's overlay body: the stored vllm object with the five wizard
    // fields re-applied (the user lowered max_model_len, left the rest as-is).
    let body = serde_json::json!({
        "backend": "vllm",
        "vllm": {
            "max_model_len": 16384,
            "kv_cache_dtype": "fp8",
            "tensor_parallel_size": 2,
            "gpu_memory_utilization": 0.85,
            "trust_remote_code": false,
            "enable_prefix_caching": true,
            "attention_backend": "flashinfer",
            "spec_decoding": {
                "method": "ngram",
                "num_speculative_tokens": 3,
                "draft_tensor_parallel_size": 1
            },
        },
    });

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/tama/v1/models/{}", model_id))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    // The wizard edit is applied AND every advanced field survived.
    let stored = read_stored_vllm(&pool, model_id).await;
    assert_eq!(
        stored.max_model_len,
        Some(16384),
        "wizard's max_model_len edit must be applied"
    );
    assert_eq!(
        stored.attention_backend.as_deref(),
        Some("flashinfer"),
        "attention_backend must survive the overlay body"
    );
    assert_eq!(
        stored.spec_decoding.method.as_deref(),
        Some("ngram"),
        "spec_decoding must survive the overlay body"
    );
    assert_eq!(stored.spec_decoding.num_speculative_tokens, Some(3));
    assert!(
        stored.enable_prefix_caching,
        "enable_prefix_caching must survive"
    );

    guard.finish().await;
}

/// Documents the server's WHOLE-REPLACE contract for the `vllm` body field:
/// a PUT whose `vllm` object omits advanced fields RESETS them to their
/// defaults — that is the documented server behavior (the same semantics as
/// `spec_decoding`). The pull wizard protects against this data loss by
/// overlaying its five fields onto the fetched stored config before the PUT
/// (see `apply_vllm_wizard_overlays`); a bare 5-field body like the one below
/// would wipe `attention_backend`, `spec_decoding`, and `enable_prefix_caching`.
#[tokio::test]
async fn test_update_model_vllm_body_missing_advanced_fields_resets_them() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let seed_vllm = serde_json::json!({
        "max_model_len": 32768,
        "enable_prefix_caching": true,
        "attention_backend": "flashinfer",
        "spec_decoding": { "method": "ngram", "num_speculative_tokens": 3 },
    });
    let seed_vllm = seed_vllm.to_string();
    let (router, model_id) = vllm_put_harness(&pool, &tmp_dir, Some(seed_vllm)).await;

    // A bare 5-field-style body (what the wizard sent before the overlay
    // fix): the advanced fields are simply absent.
    let body = serde_json::json!({
        "backend": "vllm",
        "vllm": {
            "max_model_len": 4096,
            "trust_remote_code": false,
        },
    });

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/tama/v1/models/{}", model_id))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    // Whole-replace: the body wins, and omitted fields reset to defaults.
    let stored = read_stored_vllm(&pool, model_id).await;
    assert_eq!(stored.max_model_len, Some(4096));
    assert_eq!(
        stored.attention_backend, None,
        "omitted field must reset (whole-replace)"
    );
    assert!(
        stored.spec_decoding.is_empty(),
        "omitted spec_decoding must reset (whole-replace)"
    );
    assert!(
        !stored.enable_prefix_caching,
        "omitted bool must reset to false (whole-replace)"
    );

    guard.finish().await;
}

// ── normalize_reasoning_levels unit tests ─────────────────────────────────

/// Mixed case, whitespace, and duplicates are all normalized: values are
/// trimmed + lowercased, empties dropped, and duplicates removed while
/// preserving first-seen order.
#[test]
fn test_normalize_reasoning_levels_happy_path() {
    let levels = vec![
        " Off ".to_string(),
        "LOW".to_string(),
        "low".to_string(),
        "xhigh".to_string(),
    ];
    let result = normalize_reasoning_levels(&levels).expect("valid levels accepted");
    assert_eq!(
        result,
        vec!["off".to_string(), "low".to_string(), "xhigh".to_string()]
    );
}

/// Empty input normalizes to an empty vec (the "clear" contract).
#[test]
fn test_normalize_reasoning_levels_empty_input() {
    assert_eq!(
        normalize_reasoning_levels(&[]).expect("empty input is valid"),
        Vec::<String>::new()
    );
}

/// Whitespace-only entries are dropped, not treated as invalid.
#[test]
fn test_normalize_reasoning_levels_whitespace_only_dropped() {
    let levels = vec![" ".to_string(), "   ".to_string(), "off".to_string()];
    let result = normalize_reasoning_levels(&levels).expect("whitespace dropped");
    assert_eq!(result, vec!["off".to_string()]);
}

/// An invalid token produces an error that names the offender and lists
/// the full valid set.
#[test]
fn test_normalize_reasoning_levels_invalid_token_named() {
    let levels = vec!["off".to_string(), "xhig".to_string()];
    let err = normalize_reasoning_levels(&levels).expect_err("bogus level must fail");
    assert!(
        err.contains("xhig"),
        "error must name the offender, got: {err}"
    );
    assert!(
        err.contains("off, minimal, low, medium, high, xhigh, max"),
        "error must list the valid set, got: {err}"
    );
}

/// A single all-valid value passes through unchanged.
#[test]
fn test_normalize_reasoning_levels_all_valid_single() {
    let levels = vec!["max".to_string()];
    let result = normalize_reasoning_levels(&levels).expect("valid level accepted");
    assert_eq!(result, vec!["max".to_string()]);
}

// ── PUT/PATCH /tama/v1/models/:id — reasoningLevels ──────────────────────

/// Route-level harness for reasoning-levels tests: seeds a single model row
/// (optionally with a pre-set `reasoning_levels` JSON) and returns the web
/// router plus the seeded model id. Mirrors `vllm_put_harness`.
async fn reasoning_levels_harness(
    pool: &sqlx::PgPool,
    tmp_dir: &tempfile::TempDir,
    seed_reasoning_levels: Option<String>,
) -> (axum::Router, i64) {
    let model_id = seed_model_record(
        pool,
        "test-org/reasoning-model",
        "llama-cpp",
        Some("test-org/reasoning-model"),
        None,
        seed_reasoning_levels,
    )
    .await;

    let (state, web_state) = crud_web_state(Arc::new(pool.clone()), tmp_dir.path());

    (crud_router(state, web_state), model_id)
}

/// Fetch the model's detail JSON via `GET /tama/v1/models/:id`.
async fn get_model_detail(router: &axum::Router, model_id: i64) -> serde_json::Value {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/tama/v1/models/{model_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK, "detail GET must succeed");
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body must be readable");
    serde_json::from_slice(&body_bytes).expect("detail must be valid JSON")
}

/// A PUT sending `reasoningLevels` with mixed case, whitespace, and a
/// duplicate persists the normalized values (trim + lowercase + dedupe).
#[tokio::test]
async fn test_update_model_reasoning_levels_put_persists_normalized() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let (router, model_id) = reasoning_levels_harness(&pool, &tmp_dir, None).await;

    let body = serde_json::json!({
        "backend": "llama-cpp",
        "reasoningLevels": [" Off ", "low", "LOW"],
    });

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/tama/v1/models/{}", model_id))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    let detail = get_model_detail(&router, model_id).await;
    assert_eq!(
        detail["reasoningLevels"],
        serde_json::json!(["off", "low"]),
        "levels must be persisted normalized (trim + lowercase + dedupe)"
    );

    guard.finish().await;
}

/// A PUT with an invalid level is rejected (422 ValidationError), the
/// error names the offender and the valid set, and the stored value is
/// untouched.
#[tokio::test]
async fn test_update_model_reasoning_levels_put_invalid_rejected() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let (router, model_id) =
        reasoning_levels_harness(&pool, &tmp_dir, Some(r#"["off","low"]"#.to_string())).await;

    let body = serde_json::json!({
        "backend": "llama-cpp",
        "reasoningLevels": ["bogus"],
    });

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/tama/v1/models/{}", model_id))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body must be readable");
    let err: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("error must be valid JSON");
    let message = err["error"]["message"]
        .as_str()
        .expect("error message must be a string");
    assert!(
        message.contains("bogus"),
        "error must name the offender: {message}"
    );
    assert!(
        message.contains("off, minimal, low, medium, high, xhigh, max"),
        "error must list the valid set: {message}"
    );

    // The rejected PUT must not have modified the stored levels.
    let detail = get_model_detail(&router, model_id).await;
    assert_eq!(
        detail["reasoningLevels"],
        serde_json::json!(["off", "low"]),
        "rejected PUT must not modify stored levels"
    );

    guard.finish().await;
}

/// A PUT sending `reasoningLevels: []` on a model that has levels clears
/// them: the detail shows `[]` and no `supportsReasoningEffort` (the
/// derived boolean is a client-endpoint concern, not a management one).
#[tokio::test]
async fn test_update_model_reasoning_levels_put_empty_clears() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let (router, model_id) =
        reasoning_levels_harness(&pool, &tmp_dir, Some(r#"["off","low"]"#.to_string())).await;

    let body = serde_json::json!({
        "backend": "llama-cpp",
        "reasoningLevels": [],
    });

    let req = Request::builder()
        .method("PUT")
        .uri(format!("/tama/v1/models/{}", model_id))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    let detail = get_model_detail(&router, model_id).await;
    assert_eq!(
        detail["reasoningLevels"],
        serde_json::json!([]),
        "empty array must clear stored levels"
    );
    assert!(
        detail.get("supportsReasoningEffort").is_none(),
        "management detail must not expose the derived boolean"
    );

    guard.finish().await;
}

/// A PATCH that omits `reasoningLevels` leaves the stored levels unchanged.
#[tokio::test]
async fn test_patch_model_reasoning_levels_absent_preserves() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let (router, model_id) =
        reasoning_levels_harness(&pool, &tmp_dir, Some(r#"["off","low"]"#.to_string())).await;

    let body = serde_json::json!({
        "display_name": "Renamed",
    });

    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/tama/v1/models/{}", model_id))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    let detail = get_model_detail(&router, model_id).await;
    assert_eq!(
        detail["reasoningLevels"],
        serde_json::json!(["off", "low"]),
        "PATCH without the field must preserve stored levels"
    );

    guard.finish().await;
}

/// A PATCH sending `reasoningLevels: []` clears the stored levels.
#[tokio::test]
async fn test_patch_model_reasoning_levels_empty_clears() {
    let guard = crate::testing::postgres::with_schema().await;
    let pool = guard.pool.clone();
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let (router, model_id) =
        reasoning_levels_harness(&pool, &tmp_dir, Some(r#"["off","low"]"#.to_string())).await;

    let body = serde_json::json!({
        "reasoningLevels": [],
    });

    let req = Request::builder()
        .method("PATCH")
        .uri(format!("/tama/v1/models/{}", model_id))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = router
        .clone()
        .oneshot(req)
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);

    let detail = get_model_detail(&router, model_id).await;
    assert_eq!(
        detail["reasoningLevels"],
        serde_json::json!([]),
        "PATCH empty array must clear stored levels"
    );
    assert!(
        detail.get("supportsReasoningEffort").is_none(),
        "management detail must not expose the derived boolean"
    );

    guard.finish().await;
}
