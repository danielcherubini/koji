use serde::{Deserialize, Serialize};

/// Unified model metadata — common fields that apply regardless of backend type.
/// Resolved from whichever source is populated: GGUF columns, vLLM config, or file parsing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelMetadata {
    /// Quantization name (e.g. "Q4_K_M", "fp8").
    pub quant: Option<String>,
    /// KV cache data type for K head (e.g. "f16", "q4_0", "fp8").
    pub kv_cache_k: Option<String>,
    /// KV cache data type for V head (e.g. "f16", "q8_0", "fp8").
    pub kv_cache_v: Option<String>,
    /// Context length in tokens.
    pub context_length: Option<u32>,
    /// Architecture type (e.g. "llama", "Qwen2ForCausalLM", "MoE", "Dense").
    pub architecture: Option<String>,
    /// Number of layers.
    pub num_layers: Option<u32>,
    /// Embedding/hidden dimension.
    pub embedding_length: Option<u32>,
    /// Number of attention heads.
    pub head_count: Option<u32>,
    /// Number of transformer blocks (same as num_layers for most architectures).
    pub block_count: Option<u32>,
}

impl ModelMetadata {
    /// Resolve unified metadata from a ModelConfig.
    /// Picks values from whichever source is populated:
    /// 1. GGUF columns (highest priority — explicit config)
    /// 2. vLLM config (fallback for transformers models)
    /// 3. HF metadata (fallback for architecture, context, layers)
    pub fn resolve(cfg: &crate::config::ModelConfig) -> Self {
        Self {
            quant: cfg.quant.clone().or_else(|| cfg.vllm.quantization.clone()),
            kv_cache_k: cfg
                .cache_type_k
                .clone()
                .or_else(|| cfg.vllm.kv_cache_dtype.clone()),
            kv_cache_v: cfg
                .cache_type_v
                .clone()
                .or_else(|| cfg.vllm.kv_cache_dtype.clone()),
            context_length: cfg
                .context_length
                .or(cfg.vllm.max_model_len)
                .or(cfg.hf_context_length),
            architecture: cfg.hf_architecture_type.clone(),
            num_layers: cfg.hf_num_layers,
            embedding_length: None,
            head_count: None,
            block_count: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VllmConfig;
    use std::collections::BTreeMap;

    #[allow(clippy::too_many_arguments)]
    fn make_config(
        quant: Option<String>,
        context_length: Option<u32>,
        cache_type_k: Option<String>,
        cache_type_v: Option<String>,
        vllm_quantization: Option<String>,
        vllm_max_model_len: Option<u32>,
        vllm_kv_cache_dtype: Option<String>,
        hf_context_length: Option<u32>,
        hf_architecture_type: Option<String>,
        hf_num_layers: Option<u32>,
    ) -> crate::config::ModelConfig {
        crate::config::ModelConfig {
            backend: "vllm".to_string(),
            quant,
            context_length,
            cache_type_k,
            cache_type_v,
            vllm: VllmConfig {
                quantization: vllm_quantization,
                kv_cache_dtype: vllm_kv_cache_dtype,
                max_model_len: vllm_max_model_len,
                ..Default::default()
            },
            hf_context_length,
            hf_architecture_type,
            hf_num_layers,
            quants: BTreeMap::new(),
            ..Default::default()
        }
    }

    // ── Quant resolution ────────────────────────────────────────────────────

    #[test]
    fn test_quant_gguf_wins_over_vllm() {
        let cfg = make_config(
            Some("Q4_K_M".to_string()), // GGUF quant
            None,
            None,
            None,
            Some("fp8".to_string()), // vLLM quantization
            None,
            None,
            None,
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.quant, Some("Q4_K_M".to_string()));
    }

    #[test]
    fn test_quant_falls_back_to_vllm() {
        let cfg = make_config(
            None, // GGUF quant is None
            None,
            None,
            None,
            Some("fp8".to_string()), // vLLM quantization
            None,
            None,
            None,
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.quant, Some("fp8".to_string()));
    }

    #[test]
    fn test_quant_both_none() {
        let cfg = make_config(None, None, None, None, None, None, None, None, None, None);
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.quant, None);
    }

    // ── Context length resolution ───────────────────────────────────────────

    #[test]
    fn test_context_length_gguf_wins_over_vllm_and_hf() {
        let cfg = make_config(
            None,
            Some(4096), // GGUF context_length
            None,
            None,
            None,
            Some(8192), // vLLM max_model_len
            None,
            Some(16384), // HF context_length
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.context_length, Some(4096));
    }

    #[test]
    fn test_context_length_falls_back_to_vllm() {
        let cfg = make_config(
            None,
            None, // GGUF context_length is None
            None,
            None,
            None,
            Some(8192), // vLLM max_model_len
            None,
            Some(16384), // HF context_length
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.context_length, Some(8192));
    }

    #[test]
    fn test_context_length_falls_back_to_hf() {
        let cfg = make_config(
            None,
            None, // GGUF context_length is None
            None,
            None,
            None,
            None, // vLLM max_model_len is None
            None,
            Some(16384), // HF context_length
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.context_length, Some(16384));
    }

    #[test]
    fn test_context_length_all_none() {
        let cfg = make_config(None, None, None, None, None, None, None, None, None, None);
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.context_length, None);
    }

    // ── KV cache type resolution ────────────────────────────────────────────

    #[test]
    fn test_kv_cache_k_gguf_wins_over_vllm() {
        let cfg = make_config(
            None,
            None,
            Some("q4_0".to_string()), // GGUF cache_type_k
            None,
            None,
            None,
            Some("fp8".to_string()), // vLLM kv_cache_dtype
            None,
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.kv_cache_k, Some("q4_0".to_string()));
    }

    #[test]
    fn test_kv_cache_k_falls_back_to_vllm() {
        let cfg = make_config(
            None,
            None,
            None, // GGUF cache_type_k is None
            None,
            None,
            None,
            Some("fp8".to_string()), // vLLM kv_cache_dtype
            None,
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.kv_cache_k, Some("fp8".to_string()));
    }

    #[test]
    fn test_kv_cache_v_gguf_wins_over_vllm() {
        let cfg = make_config(
            None,
            None,
            None,
            Some("q8_0".to_string()), // GGUF cache_type_v
            None,
            None,
            Some("fp8".to_string()), // vLLM kv_cache_dtype
            None,
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.kv_cache_v, Some("q8_0".to_string()));
    }

    #[test]
    fn test_kv_cache_v_falls_back_to_vllm() {
        let cfg = make_config(
            None,
            None,
            None,
            None, // GGUF cache_type_v is None
            None,
            None,
            Some("fp8".to_string()), // vLLM kv_cache_dtype
            None,
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.kv_cache_v, Some("fp8".to_string()));
    }

    #[test]
    fn test_kv_cache_types_resolve_independently() {
        // cache_type_k is Some, cache_type_v is None — only V should fall back
        let cfg = make_config(
            None,
            None,
            Some("q4_0".to_string()), // GGUF cache_type_k
            None,                     // GGUF cache_type_v is None
            None,
            None,
            Some("fp8".to_string()), // vLLM kv_cache_dtype
            None,
            None,
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.kv_cache_k, Some("q4_0".to_string()));
        assert_eq!(meta.kv_cache_v, Some("fp8".to_string()));
    }

    // ── HF metadata passthrough ─────────────────────────────────────────────

    #[test]
    fn test_architecture_from_hf() {
        let cfg = make_config(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("MoE".to_string()),
            None,
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.architecture, Some("MoE".to_string()));
    }

    #[test]
    fn test_num_layers_from_hf() {
        let cfg = make_config(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(42),
        );
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.num_layers, Some(42));
    }

    // ── All None when no sources have data ──────────────────────────────────

    #[test]
    fn test_all_none_when_no_sources() {
        let cfg = make_config(None, None, None, None, None, None, None, None, None, None);
        let meta = ModelMetadata::resolve(&cfg);
        assert_eq!(meta.quant, None);
        assert_eq!(meta.kv_cache_k, None);
        assert_eq!(meta.kv_cache_v, None);
        assert_eq!(meta.context_length, None);
        assert_eq!(meta.architecture, None);
        assert_eq!(meta.num_layers, None);
        assert_eq!(meta.embedding_length, None);
        assert_eq!(meta.head_count, None);
        assert_eq!(meta.block_count, None);
    }
}
