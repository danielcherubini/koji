use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ── Re-exports from core_shared (shared source on both ssr and csr) ─────────

pub use crate::core_shared::QuantKind;

/// Mirrors `tama_core::models::pull::HfModelMetadata` for frontend use.
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct HfModelMetadata {
    #[serde(default)]
    pub hf_format: Option<String>,
    #[serde(default)]
    pub hf_base_model: Option<String>,
    #[serde(default)]
    pub hf_pipeline_tag: Option<String>,
    #[serde(default)]
    pub hf_total_params: Option<String>,
    #[serde(default)]
    pub hf_active_params: Option<String>,
    #[serde(default)]
    pub hf_architecture_type: Option<String>,
    #[serde(default)]
    pub hf_context_length: Option<u32>,
    #[serde(default)]
    pub hf_num_layers: Option<u32>,
    #[serde(default)]
    pub hf_last_modified: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct QuantEntry {
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default)]
    pub shards: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct JobProgress {
    #[allow(dead_code)] // Populated for API fidelity but never read by UI
    pub job_id: String,
    pub filename: String,
    pub status: String,
    pub bytes_pulled: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Returned by `POST /tama/v1/pulls` (each element of the array)
#[derive(Deserialize, Clone)]
pub struct PullJobEntry {
    pub job_id: String,
    pub filename: String,
    pub status: String,
}

// ── Wizard step enum ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum WizardStep {
    RepoInput,
    LoadingQuants,
    SelectQuants,
    Downloading,
    SetContext,
    Done,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn format_bytes(bytes: i64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{bytes} B")
    }
}

pub fn step_class(current: &WizardStep, target: &WizardStep, target_idx: usize) -> &'static str {
    let order = [
        WizardStep::RepoInput,
        WizardStep::LoadingQuants,
        WizardStep::SelectQuants,
        WizardStep::Downloading,
        WizardStep::SetContext,
        WizardStep::Done,
    ];
    let current_idx = order.iter().position(|s| s == current).unwrap_or(0);
    if current == target {
        "wizard-step active"
    } else if current_idx > target_idx {
        "wizard-step completed"
    } else {
        "wizard-step"
    }
}

pub use crate::core_shared::infer_quant_from_filename;

// ── Request body type ────────────────────────────────────────────────────────

/// Simplified pull request: just filenames, no per-quant metadata.
/// Context length is a model-level property populated from GGUF parsing.
#[derive(Serialize)]
pub struct PullRequest {
    pub repo_id: String,
    /// Pre-created model DB id (from POST /tama/v1/models). When set, updates existing row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<u32>,
    pub filenames: Vec<String>,
    pub mmproj_filenames: Vec<String>,
    pub mtp_filenames: Vec<String>,
}

// ── Public types ─────────────────────────────────────────────────────────────

/// A quant that was successfully pulled by the wizard. Emitted via the
/// `on_complete` callback so the host can merge new quants into its own state.
/// Context length is model-level (same for all quants), populated from GGUF parsing.
#[derive(Clone, Debug)]
pub struct CompletedQuant {
    #[allow(dead_code)]
    pub repo_id: String,
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<u64>,
}

/// Settings configured in the SetContext step.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ContextSettings {
    pub context_length: Option<u32>,
    pub kv_unified: bool,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
}

/// KV quantization options for the dropdown.
pub const KV_QUANT_OPTIONS: &[&str] = &[
    "f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1",
];

pub mod components;
