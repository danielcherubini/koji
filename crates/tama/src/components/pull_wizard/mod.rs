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
    #[serde(default)]
    pub hf_total_size_bytes: Option<u64>,
    #[serde(default)]
    pub hf_file_count: Option<u32>,
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

// ── Wizard branch ────────────────────────────────────────────────────────────

/// Which download flow the wizard is running. Decided once per search,
/// from `hf_format` + the quant listing. GGUF wins when both are present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WizardBranch {
    #[default]
    Gguf,
    Transformers,
}

/// Returns the wizard branch for a search result, or `None` when the repo
/// contains no recognizable model files.
///
/// Exact semantics:
/// - `hf_format == Some("transformers")` → `Some(Transformers)`
///   (the server's `detect_hf_format` only reports "transformers" when no
///   GGUF files exist, so this implies a safetensors-only repo)
/// - `hf_format == Some("gguf")` → `Some(Gguf)` iff `has_gguf_files`;
///   `Some("gguf")` with an empty listing → `None` (the server's
///   `detect_hf_format` falls back to `"gguf"` as a backward-compatible
///   default for repos with no model files at all, so an empty listing
///   means no recognizable model files)
/// - `hf_format == None` (metadata fetch failed) → `Some(Gguf)` iff
///   `has_gguf_files`, else `None` (degrade to today's flow when the listing
///   says GGUF files exist)
/// - any other `hf_format` value → `None`
pub fn resolve_branch(hf_format: Option<&str>, has_gguf_files: bool) -> Option<WizardBranch> {
    match hf_format {
        Some("transformers") => Some(WizardBranch::Transformers),
        // An empty listing with "gguf" format is the server's
        // no-model-files fallback → handled by the catch-all `None` arm.
        Some("gguf") if has_gguf_files => Some(WizardBranch::Gguf),
        Some(_) => None,
        None => {
            if has_gguf_files {
                Some(WizardBranch::Gguf)
            } else {
                None
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn format_bytes(bytes: u64) -> String {
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

// ── Transformers (whole-repo pull) types ────────────────────────────────────

/// Request body for `POST /tama/v1/pulls/repo` (whole-repo `hf` CLI pull).
#[derive(Serialize)]
pub struct RepoPullStartRequest {
    /// Hugging Face repo id (e.g. `owner/repo`).
    pub repo_id: String,
    /// Stub model DB id created at search time; omitted when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<u32>,
}

/// Live status of a whole-repo pull job, polled from
/// `GET /tama/v1/pulls/repo/{job_id}`. Mirrors the server's status DTO; the
/// DTO's `job_id` field is intentionally omitted (the poll loop and the
/// `repo_pull_job_id` signal track it separately) — serde ignores it on
/// deserialization.
#[derive(Deserialize, Clone, Debug)]
pub struct RepoPullStatus {
    /// One of: `running`, `completed`, `failed`, `cancelled`.
    pub status: String,
    /// Bytes downloaded so far.
    #[serde(default)]
    pub bytes_done: u64,
    /// Expected total size in bytes, if known at start.
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// Error message for failed jobs (capped stderr tail).
    #[serde(default)]
    pub error: Option<String>,
    /// Context length from config.json, populated on completion.
    #[serde(default)]
    pub context_length: Option<u32>,
}

impl RepoPullStatus {
    /// True when the job reached a terminal state
    /// (`completed` / `failed` / `cancelled`).
    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed" | "cancelled")
    }

    /// True only when the pull completed successfully.
    pub fn is_completed(&self) -> bool {
        self.status == "completed"
    }
}

/// Consecutive failed repo-pull polls after which the poll loop gives up.
/// The job is presumed lost (e.g. a server restart cleared the in-memory
/// job map, so the poll 404s forever) and the UI must surface a terminal
/// error instead of polling forever.
pub const REPO_POLL_FAILURE_THRESHOLD: u32 = 5;

/// Error message surfaced when the repo-pull poll loop gives up after
/// `REPO_POLL_FAILURE_THRESHOLD` consecutive failed polls.
pub const REPO_PULL_JOB_LOST_MESSAGE: &str =
    "Download job was lost (the server may have restarted). Retry to start a new download.";

/// What the repo-pull poll loop should do after a run of failed polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoPollFailureAction {
    /// The consecutive-failure count is below the threshold — keep polling.
    KeepPolling,
    /// The threshold is reached — stop polling and surface a terminal error.
    SurfaceError,
}

/// Decide the repo-pull poll loop's fate after `consecutive` failed
/// (non-2xx / transport-error) polls in a row. At or above `threshold`, the
/// job is presumed lost and the loop must surface a terminal error instead
/// of polling forever.
pub fn repo_poll_consecutive_failures_action(
    consecutive: u32,
    threshold: u32,
) -> RepoPollFailureAction {
    if consecutive >= threshold {
        RepoPollFailureAction::SurfaceError
    } else {
        RepoPollFailureAction::KeepPolling
    }
}

/// vLLM settings configured in the SetContext step (transformers branch).
/// `None` fields are omitted from the save overlay, so the model's stored
/// value is kept when the pre-entry fetch succeeded (see
/// [`apply_vllm_wizard_overlays`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VllmWizardSettings {
    pub max_model_len: Option<u32>,
    pub kv_cache_dtype: Option<String>,
    pub tensor_parallel_size: Option<u32>,
    pub gpu_memory_utilization: Option<f64>,
    pub trust_remote_code: bool,
}

/// Build the 5-field vLLM overlay from wizard settings.
///
/// **Server contract — read before calling:** `PUT /tama/v1/models/{id}`
/// (`apply_model_body`) treats the body's `vllm` object as a **whole-struct
/// replace**, not a field merge: any `VllmConfig` field missing from the body
/// is reset to its default (see the regression tests in
/// `crates/tama/src/api/models/crud/tests.rs`). Omitting a `None` field here
/// does NOT preserve the server's value — it wipes it.
///
/// This function therefore produces only the *overlay*: the five fields the
/// wizard exposes. Callers must merge it onto the model's existing vllm config
/// (fetched via `GET /tama/v1/models/{id}`) with
/// [`apply_vllm_wizard_overlays`] so that fields the wizard does not expose
/// (`attention_backend`, `spec_decoding`, `enable_prefix_caching`, …) survive.
/// Sending this body directly is only safe for models with no stored
/// advanced vLLM settings (fresh pulls).
pub fn vllm_patch_body(s: &VllmWizardSettings) -> serde_json::Value {
    let mut vllm = serde_json::Map::new();
    if let Some(v) = s.max_model_len {
        vllm.insert("max_model_len".to_string(), serde_json::json!(v));
    }
    if let Some(v) = &s.kv_cache_dtype {
        vllm.insert("kv_cache_dtype".to_string(), serde_json::json!(v));
    }
    if let Some(v) = s.tensor_parallel_size {
        vllm.insert("tensor_parallel_size".to_string(), serde_json::json!(v));
    }
    if let Some(v) = s.gpu_memory_utilization {
        vllm.insert("gpu_memory_utilization".to_string(), serde_json::json!(v));
    }
    vllm.insert(
        "trust_remote_code".to_string(),
        serde_json::json!(s.trust_remote_code),
    );
    serde_json::json!({
        "backend": "vllm",
        "vllm": vllm,
    })
}

/// Build the full `PUT /tama/v1/models/{id}` body for the wizard's
/// transformers branch by overlaying the wizard's five fields
/// ([`vllm_patch_body`]) onto the model's existing vllm config.
///
/// `base` is the model's stored `vllm` JSON object (the `vllm` field of
/// `GET /tama/v1/models/{id}`), or `Value::Null` / missing when the model has
/// no stored config (fresh pull, or the pre-entry fetch failed).
///
/// Because the server's `apply_model_body` replaces the whole `vllm` struct
/// (a body field left out is reset to its default), this starts from a clone
/// of `base` so every field the wizard does not expose (`attention_backend`,
/// `spec_decoding`, `enable_prefix_caching`, `quantization`, …) survives, then
/// overlays the five wizard fields:
/// - `max_model_len` / `kv_cache_dtype` / `tensor_parallel_size` /
///   `gpu_memory_utilization` — only when `Some` (with prefill-on-entry these
///   are normally `Some`; a `None` field leaves the base value untouched)
/// - `trust_remote_code` — always set from the settings bool
///
/// The result is `{"backend": "vllm", "vllm": {…merged…}}` — the complete
/// intended vLLM state, safe for the server's whole-replace semantics.
pub fn apply_vllm_wizard_overlays(
    base: &serde_json::Value,
    s: &VllmWizardSettings,
) -> serde_json::Value {
    // Clone the existing vllm object ({} when null/absent) so fields the
    // wizard does not expose survive the server's whole-struct replace.
    let mut vllm = match base.as_object() {
        Some(obj) => obj.clone(),
        None => serde_json::Map::new(),
    };
    // Overlay the five wizard fields (the `vllm` part of the patch body).
    for (key, value) in vllm_patch_body(s)
        .get("vllm")
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
    {
        vllm.insert(key.clone(), value.clone());
    }
    serde_json::json!({
        "backend": "vllm",
        "vllm": vllm,
    })
}

/// Prefill the wizard's vLLM settings from the model's existing vllm config
/// (the `vllm` field of `GET /tama/v1/models/{id}`; `Value::Null` when the
/// model has no stored config).
///
/// - `max_model_len`: the existing `max_model_len` if present, else
///   `fallback_max_model_len` (the job's config.json context length, else the
///   repo metadata's `hf_context_length`)
/// - `kv_cache_dtype` / `tensor_parallel_size` / `gpu_memory_utilization`:
///   the existing value when present, else `None`
/// - `trust_remote_code`: the existing value (default `false`)
///
/// Prefilling every field the user can see from the stored config makes the
/// overlay-save honest under the server's whole-replace semantics: each field
/// is either the existing value or an explicit user edit.
pub fn vllm_settings_prefill(
    existing: &serde_json::Value,
    fallback_max_model_len: Option<u32>,
) -> VllmWizardSettings {
    let obj = existing.as_object();
    VllmWizardSettings {
        max_model_len: obj
            .and_then(|o| o.get("max_model_len"))
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .or(fallback_max_model_len),
        kv_cache_dtype: obj
            .and_then(|o| o.get("kv_cache_dtype"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        tensor_parallel_size: obj
            .and_then(|o| o.get("tensor_parallel_size"))
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok()),
        gpu_memory_utilization: obj
            .and_then(|o| o.get("gpu_memory_utilization"))
            .and_then(|v| v.as_f64()),
        trust_remote_code: obj
            .and_then(|o| o.get("trust_remote_code"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Consecutive failed polls below the threshold → keep polling
    /// (a transient 404/503 blip must not kill a live download).
    #[test]
    fn test_repo_poll_failure_action_below_threshold() {
        assert_eq!(
            repo_poll_consecutive_failures_action(0, REPO_POLL_FAILURE_THRESHOLD),
            RepoPollFailureAction::KeepPolling
        );
        assert_eq!(
            repo_poll_consecutive_failures_action(
                REPO_POLL_FAILURE_THRESHOLD - 1,
                REPO_POLL_FAILURE_THRESHOLD
            ),
            RepoPollFailureAction::KeepPolling
        );
    }

    /// Exactly at the threshold → surface the terminal error (the job is
    /// presumed lost, e.g. the server restarted and cleared its job map).
    #[test]
    fn test_repo_poll_failure_action_at_threshold() {
        assert_eq!(
            repo_poll_consecutive_failures_action(
                REPO_POLL_FAILURE_THRESHOLD,
                REPO_POLL_FAILURE_THRESHOLD
            ),
            RepoPollFailureAction::SurfaceError
        );
    }

    /// Above the threshold → still surface (the decision is idempotent once
    /// the threshold is crossed; the loop breaks on the first crossing).
    #[test]
    fn test_repo_poll_failure_action_above_threshold() {
        assert_eq!(
            repo_poll_consecutive_failures_action(
                REPO_POLL_FAILURE_THRESHOLD + 1,
                REPO_POLL_FAILURE_THRESHOLD
            ),
            RepoPollFailureAction::SurfaceError
        );
    }

    /// Base with advanced fields (attention_backend, spec_decoding) +
    /// half-filled settings → advanced fields the wizard does NOT expose are
    /// preserved, and the wizard's five fields are overlaid on top.
    #[test]
    fn test_apply_vllm_wizard_overlays_preserves_advanced_fields() {
        let base = serde_json::json!({
            "max_model_len": 32768,
            "kv_cache_dtype": "fp8",
            "enable_prefix_caching": true,
            "attention_backend": "flashinfer",
            "spec_decoding": {
                "method": "ngram",
                "num_speculative_tokens": 3
            },
            "trust_remote_code": false,
        });
        // Half-filled: only max_model_len set (user edited it), trust default.
        let s = VllmWizardSettings {
            max_model_len: Some(16384),
            ..Default::default()
        };
        let v = apply_vllm_wizard_overlays(&base, &s);
        assert_eq!(
            v,
            serde_json::json!({
                "backend": "vllm",
                "vllm": {
                    "max_model_len": 16384,
                    "kv_cache_dtype": "fp8",
                    "enable_prefix_caching": true,
                    "attention_backend": "flashinfer",
                    "spec_decoding": {
                        "method": "ngram",
                        "num_speculative_tokens": 3
                    },
                    "trust_remote_code": false,
                },
            }),
        );
    }

    /// Null/absent base + all-default settings → exactly the fresh-pull body
    /// `{"backend":"vllm","vllm":{"trust_remote_code":false}}` (behavior
    /// unchanged for first-time pulls).
    #[test]
    fn test_apply_vllm_wizard_overlays_null_base_all_default() {
        let s = VllmWizardSettings::default();
        let v = apply_vllm_wizard_overlays(&serde_json::Value::Null, &s);
        assert_eq!(
            v,
            serde_json::json!({
                "backend": "vllm",
                "vllm": {
                    "trust_remote_code": false,
                },
            }),
        );
        // An empty object base behaves like null.
        let v = apply_vllm_wizard_overlays(&serde_json::json!({}), &s);
        assert_eq!(
            v,
            serde_json::json!({ "backend": "vllm", "vllm": { "trust_remote_code": false } })
        );
    }

    /// Base with `max_model_len` + settings `max_model_len = None` → the base
    /// value is preserved (a None wizard field means "leave it alone").
    #[test]
    fn test_apply_vllm_wizard_overlays_none_setting_preserves_base() {
        let base = serde_json::json!({ "max_model_len": 32768 });
        let s = VllmWizardSettings::default();
        let v = apply_vllm_wizard_overlays(&base, &s);
        assert_eq!(
            v["vllm"]["max_model_len"],
            serde_json::json!(32768),
            "base max_model_len must survive a None wizard setting"
        );
        assert_eq!(v["vllm"]["trust_remote_code"], serde_json::json!(false));
    }

    /// Fully filled settings override every overlapping base field, while
    /// non-wizard base fields still survive.
    #[test]
    fn test_apply_vllm_wizard_overlays_settings_override_base() {
        let base = serde_json::json!({
            "max_model_len": 32768,
            "kv_cache_dtype": "auto",
            "tensor_parallel_size": 1,
            "gpu_memory_utilization": 0.9,
            "trust_remote_code": false,
            "attention_backend": "flashinfer",
        });
        let s = VllmWizardSettings {
            max_model_len: Some(8192),
            kv_cache_dtype: Some("fp8".to_string()),
            tensor_parallel_size: Some(4),
            gpu_memory_utilization: Some(0.7),
            trust_remote_code: true,
        };
        let v = apply_vllm_wizard_overlays(&base, &s);
        assert_eq!(v["vllm"]["max_model_len"], serde_json::json!(8192));
        assert_eq!(v["vllm"]["kv_cache_dtype"], serde_json::json!("fp8"));
        assert_eq!(v["vllm"]["tensor_parallel_size"], serde_json::json!(4));
        assert_eq!(v["vllm"]["gpu_memory_utilization"], serde_json::json!(0.7));
        assert_eq!(v["vllm"]["trust_remote_code"], serde_json::json!(true));
        assert_eq!(
            v["vllm"]["attention_backend"],
            serde_json::json!("flashinfer")
        );
    }

    /// A fully populated existing vllm config prefills all five wizard fields
    /// (the fallback max model length is ignored when the existing config has
    /// one).
    #[test]
    fn test_vllm_settings_prefill_from_existing() {
        let existing = serde_json::json!({
            "max_model_len": 32768,
            "kv_cache_dtype": "fp8",
            "tensor_parallel_size": 2,
            "gpu_memory_utilization": 0.85,
            "trust_remote_code": true,
            "attention_backend": "flashinfer",
        });
        let s = vllm_settings_prefill(&existing, Some(4096));
        assert_eq!(s.max_model_len, Some(32768));
        assert_eq!(s.kv_cache_dtype, Some("fp8".to_string()));
        assert_eq!(s.tensor_parallel_size, Some(2));
        assert_eq!(s.gpu_memory_utilization, Some(0.85));
        assert!(s.trust_remote_code);
    }

    /// Null existing config → the fallback (job context length) is used for
    /// max_model_len and every other field stays at its default.
    #[test]
    fn test_vllm_settings_prefill_null_existing_falls_back() {
        let s = vllm_settings_prefill(&serde_json::Value::Null, Some(4096));
        assert_eq!(s.max_model_len, Some(4096));
        assert_eq!(s.kv_cache_dtype, None);
        assert_eq!(s.tensor_parallel_size, None);
        assert_eq!(s.gpu_memory_utilization, None);
        assert!(!s.trust_remote_code);
    }

    /// Partial existing config (the GET response only carries set fields —
    /// VllmConfig Options use skip_serializing_if): present fields prefill,
    /// absent ones fall back (max_model_len) or stay None.
    #[test]
    fn test_vllm_settings_prefill_partial_existing() {
        let existing = serde_json::json!({
            "kv_cache_dtype": "fp8",
            "enable_prefix_caching": true,
            "trust_remote_code": false,
        });
        let s = vllm_settings_prefill(&existing, Some(8192));
        assert_eq!(
            s.max_model_len,
            Some(8192),
            "missing max_model_len must fall back to the job/metadata length"
        );
        assert_eq!(s.kv_cache_dtype, Some("fp8".to_string()));
        assert_eq!(s.tensor_parallel_size, None);
        assert_eq!(s.gpu_memory_utilization, None);
        assert!(!s.trust_remote_code);
    }

    /// Null existing + no fallback → exactly the default settings.
    #[test]
    fn test_vllm_settings_prefill_no_fallback() {
        let s = vllm_settings_prefill(&serde_json::Value::Null, None);
        assert_eq!(s, VllmWizardSettings::default());
    }

    /// `hf_format == Some("transformers")` always resolves to the transformers
    /// branch, even if the listing were (impossibly) non-empty — the server's
    /// `detect_hf_format` only reports "transformers" when no GGUF files exist.
    #[test]
    fn test_resolve_branch_transformers() {
        assert_eq!(
            resolve_branch(Some("transformers"), false),
            Some(WizardBranch::Transformers),
        );
    }

    /// `hf_format == Some("gguf")` with GGUF files in the listing resolves to
    /// the GGUF branch. A mixed repo (gguf + safetensors) also reports "gguf"
    /// — GGUF wins in `detect_hf_format` — but still has GGUF files listed.
    #[test]
    fn test_resolve_branch_gguf() {
        // Mixed repo: GGUF wins — `detect_hf_format` reports "gguf" and the
        // listing contains the GGUF files.
        assert_eq!(resolve_branch(Some("gguf"), true), Some(WizardBranch::Gguf));
    }

    /// `hf_format == Some("gguf")` with an EMPTY listing means the repo
    /// contains no model files at all. The server's `detect_hf_format`
    /// falls back to `"gguf"` as a backward-compatible default for repos
    /// with no `.gguf`/`.safetensors`/`.bin` files, so this case is the
    /// no-model-files case, not a GGUF repo → no branch.
    #[test]
    fn test_resolve_branch_gguf_format_empty_listing() {
        assert_eq!(resolve_branch(Some("gguf"), false), None);
    }

    /// `hf_format == None` (metadata fetch failed) with GGUF files in the listing
    /// degrades to today's flow: the GGUF branch.
    #[test]
    fn test_resolve_branch_none_with_files() {
        assert_eq!(resolve_branch(None, true), Some(WizardBranch::Gguf));
    }

    /// `hf_format == None` with an empty listing means no recognizable model
    /// files → no branch. The `("transformers", true)` case documents that the
    /// GGUF-wins guarantee comes from the server: if GGUF files existed, the
    /// format would have been "gguf", so the transformers branch still wins.
    #[test]
    fn test_resolve_branch_none_empty() {
        assert_eq!(resolve_branch(None, false), None);
        assert_eq!(
            resolve_branch(Some("transformers"), true),
            Some(WizardBranch::Transformers),
        );
    }

    /// Unknown `hf_format` values are never produced by the server today; treat
    /// them as "no recognizable model files" instead of guessing.
    #[test]
    fn test_resolve_branch_unknown_format() {
        assert_eq!(resolve_branch(Some("foo"), true), None);
        assert_eq!(resolve_branch(Some("foo"), false), None);
    }

    /// Half-filled settings (only `max_model_len` Some) → the JSON has exactly
    /// `backend`, `vllm.max_model_len`, and `vllm.trust_remote_code`; unset
    /// options are omitted entirely.
    #[test]
    fn test_vllm_patch_body_half_filled() {
        let s = VllmWizardSettings {
            max_model_len: Some(8192),
            ..Default::default()
        };
        let v = vllm_patch_body(&s);
        assert_eq!(
            v,
            serde_json::json!({
                "backend": "vllm",
                "vllm": {
                    "max_model_len": 8192,
                    "trust_remote_code": false,
                },
            }),
        );
    }

    /// All-default settings → the `vllm` object contains only
    /// `trust_remote_code: false` (always sent, since it is a bool).
    #[test]
    fn test_vllm_patch_body_all_default() {
        let s = VllmWizardSettings::default();
        let v = vllm_patch_body(&s);
        assert_eq!(
            v,
            serde_json::json!({
                "backend": "vllm",
                "vllm": {
                    "trust_remote_code": false,
                },
            }),
        );
    }

    /// Fully filled settings → every field present with its value.
    #[test]
    fn test_vllm_patch_body_all_filled() {
        let s = VllmWizardSettings {
            max_model_len: Some(32768),
            kv_cache_dtype: Some("fp8".to_string()),
            tensor_parallel_size: Some(2),
            gpu_memory_utilization: Some(0.85),
            trust_remote_code: true,
        };
        let v = vllm_patch_body(&s);
        assert_eq!(
            v,
            serde_json::json!({
                "backend": "vllm",
                "vllm": {
                    "max_model_len": 32768,
                    "kv_cache_dtype": "fp8",
                    "tensor_parallel_size": 2,
                    "gpu_memory_utilization": 0.85,
                    "trust_remote_code": true,
                },
            }),
        );
    }
}
