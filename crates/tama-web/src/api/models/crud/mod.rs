#[allow(unused_imports)]
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
#[allow(unused_imports)]
use std::sync::Arc;

#[allow(unused_imports)]
use super::resolve_model_id;
#[allow(unused_imports)]
use crate::api::{load_config_from_state, trigger_proxy_reload};
#[allow(unused_imports)]
use tama_core::proxy::ProxyState;

/// Maximum lengths for ModelBody fields.
const MAX_BACKEND: usize = 256;
const MAX_MODEL: usize = 256;
const MAX_QUANT: usize = 128;
const MAX_MMPROJ: usize = 128;
const MAX_API_NAME: usize = 128;
const MAX_DISPLAY_NAME: usize = 256;
const MAX_CACHE_TYPE: usize = 32;

/// Body for create/update model.
#[derive(serde::Deserialize)]
pub struct ModelBody {
    pub backend: String,
    #[serde(default)]
    pub gpu_variant: Option<String>,
    #[serde(default)]
    pub gpu_device: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    #[serde(default)]
    pub mtp_model: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<tama_core::profiles::SamplingParams>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub num_parallel: Option<u32>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub api_name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub quants: Option<std::collections::BTreeMap<String, tama_core::config::QuantEntry>>,
    #[serde(default)]
    pub modalities: Option<tama_core::config::ModelModalities>,
    #[serde(default)]
    pub kv_unified: Option<bool>,
    #[serde(default)]
    pub cache_type_k: Option<String>,
    #[serde(default)]
    pub cache_type_v: Option<String>,
    pub spec_decoding: Option<tama_core::config::SpecDecodingConfig>,
}

fn apply_model_body(
    body: ModelBody,
    existing: Option<tama_core::config::ModelConfig>,
) -> tama_core::config::ModelConfig {
    // Extract spec_decoding before consuming existing to avoid cloning the
    // entire ModelConfig. Only the small SpecDecodingConfig is cloned if needed.
    let existing_spec_decoding = existing.as_ref().map(|m| m.spec_decoding.clone());

    let base = existing.unwrap_or_else(|| tama_core::config::ModelConfig {
        gpu_variant: None,
        gpu_device: None,
        backend: String::new(),
        args: vec![],
        sampling: None,
        model: None,
        quant: None,
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
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        kv_unified: true,
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
    });

    // Handle sampling from body
    let sampling = body.sampling;

    tama_core::config::ModelConfig {
        backend: body.backend,
        gpu_variant: body.gpu_variant.or(base.gpu_variant),
        gpu_device: body.gpu_device.or(base.gpu_device),
        model: body.model.or(base.model),
        quant: body.quant.or(base.quant),
        mmproj: body.mmproj.or(base.mmproj),
        mtp_model: body.mtp_model.or(base.mtp_model),
        args: body.args,
        sampling,
        enabled: body.enabled.unwrap_or(base.enabled),
        context_length: body.context_length,
        num_parallel: body.num_parallel.or(base.num_parallel),
        port: body.port.or(base.port),
        health_check: base.health_check,
        profile: None,
        api_name: body.api_name.or(base.api_name),
        gpu_layers: body.gpu_layers.or(base.gpu_layers),
        modalities: body.modalities.or(base.modalities),
        display_name: body.display_name.or(base.display_name),
        // Preserve server-side `size_bytes` on update: the UI exposes the field
        // read-only and callers must not be able to rewrite it via the API. The
        // authoritative value comes from the download pipeline
        // (`std::fs::metadata` after pull + the HF blob metadata that later
        // populates `model_files.size_bytes` during verify/refresh). If no
        // prior entry exists, accept the client's value to avoid regressing
        // freshly-created entries that don't yet have a stored size.
        quants: body
            .quants
            .unwrap_or_else(|| base.quants.clone())
            .into_iter()
            .map(|(k, v)| {
                let preserved_size = base
                    .quants
                    .get(&k)
                    .and_then(|existing| existing.size_bytes)
                    .or(v.size_bytes);
                (
                    k,
                    tama_core::config::QuantEntry {
                        file: v.file,
                        kind: v.kind,
                        size_bytes: preserved_size,
                        context_length: v.context_length,
                    },
                )
            })
            .collect(),
        kv_unified: body.kv_unified.unwrap_or(base.kv_unified),
        cache_type_k: body
            .cache_type_k
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "__custom"),
        cache_type_v: body
            .cache_type_v
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "__custom"),
        hf_format: base.hf_format,
        hf_base_model: base.hf_base_model,
        hf_pipeline_tag: base.hf_pipeline_tag,
        hf_total_params: base.hf_total_params,
        hf_active_params: base.hf_active_params,
        hf_architecture_type: base.hf_architecture_type,
        hf_context_length: base.hf_context_length,
        hf_num_layers: base.hf_num_layers,
        hf_last_modified: base.hf_last_modified,
        db_id: base.db_id,
        spec_decoding: body
            .spec_decoding
            .unwrap_or_else(|| existing_spec_decoding.unwrap_or_default()),
    }
}

// ── Validation helpers ──────────────────────────────────────────────────────

/// Validate that a string is a valid repo_id: non-empty, only alphanumeric, dots, underscores, hyphens, slashes.
fn is_valid_repo_id(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    for ch in input.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' | '/' => continue,
            _ => return false,
        }
    }
    true
}

/// Validate ModelBody field lengths. Returns an error message string if invalid.
fn validate_model_body(body: &ModelBody) -> Result<(), String> {
    if body.backend.is_empty() {
        return Err("backend cannot be empty".to_string());
    }
    if body.backend.len() > MAX_BACKEND {
        return Err(format!("backend must be at most {MAX_BACKEND} characters"));
    }
    if let Some(ref model) = body.model {
        if model.is_empty() {
            return Err("model cannot be empty".to_string());
        }
        if model.len() > MAX_MODEL {
            return Err(format!("model must be at most {MAX_MODEL} characters"));
        }
    }
    if let Some(ref quant) = body.quant {
        if !quant.is_empty() && quant.len() > MAX_QUANT {
            return Err(format!("quant must be at most {MAX_QUANT} characters"));
        }
    }
    if let Some(ref mmproj) = body.mmproj {
        if !mmproj.is_empty() && mmproj.len() > MAX_MMPROJ {
            return Err(format!("mmproj must be at most {MAX_MMPROJ} characters"));
        }
    }
    if let Some(ref api_name) = body.api_name {
        if !api_name.is_empty() && api_name.len() > MAX_API_NAME {
            return Err(format!(
                "api_name must be at most {MAX_API_NAME} characters"
            ));
        }
    }
    if let Some(ref display_name) = body.display_name {
        if !display_name.is_empty() && display_name.len() > MAX_DISPLAY_NAME {
            return Err(format!(
                "display_name must be at most {MAX_DISPLAY_NAME} characters"
            ));
        }
    }
    if let Some(ref cache_type_k) = body.cache_type_k {
        let trimmed = cache_type_k.trim();
        if trimmed == "__custom" {
            return Err("cache_type_k cannot be the sentinel value __custom".to_string());
        }
        if !trimmed.is_empty() && trimmed.len() > MAX_CACHE_TYPE {
            return Err(format!(
                "cache_type_k must be at most {MAX_CACHE_TYPE} characters"
            ));
        }
    }
    if let Some(ref cache_type_v) = body.cache_type_v {
        let trimmed = cache_type_v.trim();
        if trimmed == "__custom" {
            return Err("cache_type_v cannot be the sentinel value __custom".to_string());
        }
        if !trimmed.is_empty() && trimmed.len() > MAX_CACHE_TYPE {
            return Err(format!(
                "cache_type_v must be at most {MAX_CACHE_TYPE} characters"
            ));
        }
    }
    Ok(())
}

// ── Sub-modules ─────────────────────────────────────────────────────────────

pub mod create;
pub mod delete;
pub mod rename;
pub mod update;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use create::create_model;
pub use delete::{delete_model, delete_quant};
pub use rename::rename_model;
pub use update::update_model;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
