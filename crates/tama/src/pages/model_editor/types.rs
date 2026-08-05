use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What kind of file a quant entry represents. Mirrors `tama_core::config::QuantKind`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    #[default]
    Model,
    Mmproj,
    Mtp,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantInfo {
    pub file: String,
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub context_length: Option<u32>,
    // --- DB-enriched fields returned by /api/models/:id ---
    // These are skipped on save so the backend never receives them (it
    // authoritatively owns this data in the SQLite DB).
    #[serde(default, skip_serializing)]
    pub lfs_oid: Option<String>,
    #[serde(default, skip_serializing)]
    pub db_size_bytes: Option<u64>,
    #[serde(default, skip_serializing)]
    pub last_verified_at: Option<String>,
    #[serde(default, skip_serializing)]
    pub verified_ok: Option<bool>,
    #[serde(default, skip_serializing)]
    pub verify_error: Option<String>,
}

/// Model modality configuration (input/output types like "text", "image").
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[cfg(feature = "ssr")]
pub use tama_core::backends::BackendOption;

#[cfg(not(feature = "ssr"))]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendOption {
    pub name: String,
    #[serde(default)]
    pub variant: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDetail {
    pub id: i64,
    pub backend: String,
    pub gpu_variant: Option<String>,
    /// GPU device name (e.g. "CUDA0", "ROCm0") for per-model GPU placement.
    /// Passed as `--device` to llama.cpp backends. When None, the backend
    /// uses its default device selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_device: Option<String>,
    pub model: Option<String>,
    pub quant: Option<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    #[serde(default)]
    pub mtp_model: Option<String>,
    pub args: Vec<String>,
    pub sampling: Option<serde_json::Value>,
    pub enabled: bool,
    pub context_length: Option<u32>,
    pub num_parallel: Option<u32>,
    pub port: Option<u16>,
    pub api_name: Option<String>,
    pub display_name: Option<String>,
    #[serde(default = "default_kv_unified")]
    pub kv_unified: bool,
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub cache_type_k: Option<String>,
    #[serde(default)]
    pub cache_type_v: Option<String>,
    #[serde(default)]
    pub hf_context_length: Option<u32>,
    pub quants: BTreeMap<String, QuantInfo>,
    pub backends: Vec<BackendOption>,
    #[serde(default)]
    pub repo_commit_sha: Option<String>,
    #[serde(default)]
    pub repo_pulled_at: Option<String>,
    #[serde(default)]
    pub modalities: Option<ModelModalities>,
    #[serde(default)]
    pub spec_decoding: Option<serde_json::Value>,
    /// Pre-allocated context KV cache size (llama.cpp --batch). None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_batch: Option<u32>,
    /// Maximum number of unique sequences to process in a single batch (llama.cpp --ubatch).
    /// None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_ubatch: Option<u32>,
    /// HuggingFace model format (e.g. "transformers", "gguf"). Used by the UI to
    /// render the correct form for a given model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    pub models: Vec<serde_json::Value>,
    pub backends: Vec<BackendOption>,
    pub sampling_templates: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SamplingField {
    pub enabled: bool,
    pub value: String,
}

/// Speculative decoding configuration for the model editor form.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpecDecodingForm {
    #[serde(default)]
    pub spec_types: Vec<String>,
    pub n_max: Option<u32>,
    pub n_min: Option<u32>,
    pub draft_ngl: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelForm {
    pub id: String,
    pub backend: String,
    pub gpu_variant: Option<String>,
    /// GPU device name (e.g. "CUDA0", "ROCm0") for per-model GPU placement.
    /// Passed as `--device` to llama.cpp backends. When None, the backend
    /// uses its default device selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_device: Option<String>,
    pub model: Option<String>,
    pub quant: Option<String>,
    pub mmproj: Option<String>,
    #[serde(default)]
    pub mtp_model: Option<String>,
    pub args: String,
    pub sampling: std::collections::HashMap<String, SamplingField>,
    pub enabled: bool,
    pub context_length: Option<u32>,
    pub num_parallel: Option<u32>,
    pub port: Option<u16>,
    pub api_name: Option<String>,
    pub display_name: Option<String>,
    #[serde(default = "default_kv_unified")]
    pub kv_unified: bool,
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub cache_type_k: Option<String>,
    #[serde(default)]
    pub cache_type_v: Option<String>,
    #[serde(default)]
    pub hf_context_length: Option<u32>,
    pub quants: BTreeMap<String, QuantInfo>,
    #[serde(default)]
    pub modalities: Option<ModelModalities>,
    #[serde(default)]
    pub spec_decoding: SpecDecodingForm,
    /// Pre-allocated context KV cache size (llama.cpp --batch). None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_batch: Option<u32>,
    /// Maximum number of unique sequences to process in a single batch (llama.cpp --ubatch).
    /// None = backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_ubatch: Option<u32>,
    /// HuggingFace model format (e.g. "transformers", "gguf"). Used by the UI to
    /// render the correct form for a given model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_format: Option<String>,
}

fn default_kv_unified() -> bool {
    true
}

/// Response from POST /api/models/:id/refresh — surfaces the updated repo
/// commit SHA and the full per-file DB records for merging back into the editor.
#[derive(Debug, Clone, Deserialize)]
pub struct RefreshResponse {
    #[serde(default)]
    pub repo_commit_sha: Option<String>,
    #[serde(default)]
    pub repo_pulled_at: Option<String>,
    #[serde(default)]
    pub files: Vec<FileRecordJson>,
}

/// Response from POST /api/models/:id/verify.
#[derive(Debug, Clone, Deserialize)]
pub struct VerifyResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub any_unknown: bool,
    #[serde(default)]
    pub files: Vec<FileRecordJson>,
}

/// Subset of `ModelFileRecord` as serialized by `file_record_json` in the
/// web backend — carries the DB-authoritative size, LFS hash and verify state
/// for a single file. Used to merge refresh/verify responses back into the
/// editor `quants` signal without a full page reload.
#[derive(Debug, Clone, Deserialize)]
pub struct FileRecordJson {
    pub filename: String,
    #[serde(default)]
    pub lfs_oid: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub last_verified_at: Option<String>,
    #[serde(default)]
    pub verified_ok: Option<bool>,
    #[serde(default)]
    pub verify_error: Option<String>,
}

/// GPU device information returned by the backend discovery API.
#[derive(Debug, Clone, Deserialize)]
pub struct GpuDeviceInfo {
    #[allow(dead_code)] // Deserialized from API but not displayed
    pub device_id: String,
    pub name: String,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API but not displayed
    pub vendor: String,
    #[serde(default)]
    pub vram_total_mib: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)] // Deserialized from API but not displayed
    pub vram_free_mib: Option<u64>,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Returns a human-readable format label for display in the model editor.
pub fn format_label(hf_format: Option<&str>) -> String {
    match hf_format {
        Some("transformers") => "Safetensors".to_string(),
        Some("gguf") => "GGUF".to_string(),
        _ => "Format unknown".to_string(),
    }
}

/// Returns true when the model uses the transformers (safetensors) format.
pub fn is_transformers(hf_format: Option<&str>) -> bool {
    hf_format == Some("transformers")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── format_label tests ───────────────────────────────────────────────

    #[test]
    fn test_format_label_transformers() {
        assert_eq!(format_label(Some("transformers")), "Safetensors");
    }

    #[test]
    fn test_format_label_gguf() {
        assert_eq!(format_label(Some("gguf")), "GGUF");
    }

    #[test]
    fn test_format_label_none() {
        assert_eq!(format_label(None), "Format unknown");
    }

    #[test]
    fn test_format_label_unknown_value() {
        assert_eq!(format_label(Some("some_other_format")), "Format unknown");
    }

    // ── is_transformers tests ────────────────────────────────────────────

    #[test]
    fn test_is_transformers_true() {
        assert!(is_transformers(Some("transformers")));
    }

    #[test]
    fn test_is_transformers_false_gguf() {
        assert!(!is_transformers(Some("gguf")));
    }

    #[test]
    fn test_is_transformers_false_none() {
        assert!(!is_transformers(None));
    }

    #[test]
    fn test_is_transformers_false_unknown() {
        assert!(!is_transformers(Some("unknown")));
    }

    // ── ModelDetail round-trip tests ─────────────────────────────────────

    /// Round-trip: `ModelDetail` with `hf_format: Some(...)` serializes and
    /// deserializes back to the same value.
    #[test]
    fn test_model_detail_hf_format_round_trip() {
        let detail = ModelDetail {
            id: 1,
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            gpu_device: None,
            model: Some("test/model".to_string()),
            quant: None,
            args: vec![],
            sampling: None,
            enabled: true,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            kv_unified: true,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            hf_context_length: None,
            quants: std::collections::BTreeMap::new(),
            backends: vec![],
            mmproj: None,
            mtp_model: None,
            repo_commit_sha: None,
            repo_pulled_at: None,
            modalities: None,
            spec_decoding: None,
            n_batch: None,
            n_ubatch: None,
            hf_format: Some("transformers".to_string()),
        };

        let serialized = serde_json::to_value(&detail).unwrap();
        assert_eq!(
            serialized["hf_format"].as_str(),
            Some("transformers"),
            "serialized JSON should contain hf_format"
        );

        let deserialized: ModelDetail = serde_json::from_value(serialized).unwrap();
        assert_eq!(
            deserialized.hf_format,
            Some("transformers".to_string()),
            "deserialized hf_format should match original"
        );
    }

    /// Missing `hf_format` in the JSON payload deserializes to `None`.
    #[test]
    fn test_model_detail_hf_format_missing_defaults_to_none() {
        let json = serde_json::json!({
            "id": 42,
            "backend": "llama-cpp",
            "gpu_variant": null,
            "gpu_device": null,
            "model": null,
            "quant": null,
            "args": [],
            "sampling": null,
            "enabled": true,
            "context_length": null,
            "num_parallel": null,
            "port": null,
            "api_name": null,
            "display_name": null,
            "kv_unified": true,
            "gpu_layers": null,
            "cache_type_k": null,
            "cache_type_v": null,
            "hf_context_length": null,
            "quants": {},
            "backends": [],
            "mmproj": null,
            "mtp_model": null,
            "repo_commit_sha": null,
            "repo_pulled_at": null,
            "modalities": null,
            "spec_decoding": null,
            "n_batch": null,
            "n_ubatch": null
        });

        let detail: ModelDetail = serde_json::from_value(json).unwrap();
        assert_eq!(detail.hf_format, None);
    }
}
