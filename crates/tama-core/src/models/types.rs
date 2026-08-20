//! Shared model-related types that don't belong to a single submodule.

use serde::{Deserialize, Serialize};

/// Per-model loaded/idle status snapshot, embedded in `MetricSample.models`
/// and `MetricsSnapshot.models` and streamed to the dashboard over SSE.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelStateSnapshot {
    pub id: String,
    /// Integer database id of the model_configs row, if known. Emitted so the
    /// dashboard can link to the editor by id rather than by config_key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_id: Option<i64>,
    pub api_name: Option<String>,
    pub display_name: Option<String>,
    pub backend: String,
    /// Current lifecycle state of the model's backend.
    /// One of: `idle`, `starting`, `ready`, `unloading`, `failed`.
    #[serde(default)]
    pub state: crate::gpu::ModelState,
    /// Quantization name (e.g. "Q4_K_M", "Q8_0"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    /// Model's configured context length in tokens. Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    /// Architecture type from HF metadata (e.g. "MoE", "Dense"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_architecture_type: Option<String>,
    /// Base model from HF metadata (e.g. "Qwen/Qwen3.6-27B"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_base_model: Option<String>,
    /// Model format from HF metadata ("gguf" / "transformers"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_format: Option<String>,
    /// GPU variant for the backend (e.g. "cpu", "cuda", "vulkan"). Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_variant: Option<String>,
    /// KV cache quant for K head (e.g. "q4_0", "f16"). Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_k: Option<String>,
    /// KV cache quant for V head (e.g. "q8_0", "f16"). Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_v: Option<String>,
    /// Speculative decoding types (e.g. ["draft-mtp", "ngram-simple"]). Display-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spec_types: Vec<String>,
    /// GPU device name this model is bound to (e.g. "CUDA0", "ROCm0"),
    /// taken from `ModelConfig.gpu_device`. None if the model is idle,
    /// unconfigured, or the backend is not llama.cpp. Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_device: Option<String>,
    /// Error message when `state == "failed"`, surfaced on the dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Token generation speed for this model's backend (tokens per second).
    /// None if the model is not actively generating or no stats observed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tps: Option<f32>,
    /// Prompt processing speed for this model's backend (tokens per second).
    /// None if the model is not actively generating or no stats observed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tps: Option<f32>,
    /// Whether this backend is running inside a Docker container.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_docker: bool,
    /// Display-only, source tamad/provider name when resolvable.
    /// None when the owning provider cannot be resolved (missing provider,
    /// ambiguous local providers, or a lookup error) — the dashboard then
    /// groups the model under "Unassigned".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `host_name` serde round-trip: a populated value survives
    /// serialize → deserialize, and `None` is omitted from the wire JSON
    /// (so old dashboards / partial rollouts keep deserializing).
    #[test]
    fn test_host_name_serde_roundtrip() {
        let json = r#"{
            "id": "m1",
            "api_name": null,
            "display_name": null,
            "backend": "llama_cpp",
            "host_name": "gpu-box"
        }"#;
        let snap: ModelStateSnapshot =
            serde_json::from_str(json).expect("snapshot with host_name deserializes");
        assert_eq!(snap.host_name.as_deref(), Some("gpu-box"));

        let out = serde_json::to_value(&snap).expect("serialize");
        assert_eq!(
            out.get("host_name").and_then(|v| v.as_str()),
            Some("gpu-box")
        );

        let mut without = snap.clone();
        without.host_name = None;
        let out = serde_json::to_value(&without).expect("serialize");
        assert!(
            out.get("host_name").is_none(),
            "None host_name must be skipped on the wire, got: {out}"
        );
        // …and its absence must still deserialize (old backend builds).
        let json2 = r#"{
            "id": "m1",
            "api_name": null,
            "display_name": null,
            "backend": "llama_cpp"
        }"#;
        let snap2: ModelStateSnapshot =
            serde_json::from_str(json2).expect("snapshot without host_name deserializes");
        assert!(snap2.host_name.is_none());
    }
}
