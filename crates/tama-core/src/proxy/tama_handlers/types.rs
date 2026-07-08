use serde::{Deserialize, Serialize};

/// Maximum number of quants that can be pulled concurrently in a single pull request.
///
/// Configurable via `TAMA_MAX_CONCURRENT_PULLS` environment variable.
/// Default is 8 (increased from original 4 for better parallelism).
/// For network I/O bound pulls, higher values improve throughput
/// without significant CPU/memory overhead.
pub fn max_concurrent_pulls() -> usize {
    std::env::var("TAMA_MAX_CONCURRENT_PULLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
}

/// A single quantisation variant available for a HuggingFace GGUF repo.
#[derive(Debug, Serialize)]
pub struct QuantEntry {
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<i64>,
    /// What kind of file this is (model quant vs vision projector). Used by
    /// the frontend wizard to group files into the correct step.
    pub kind: crate::config::QuantKind,
}

/// A single quantisation variant to pull (used in multi-quant wizard format).
#[derive(Debug, Deserialize, Clone)]
pub struct QuantDownloadSpec {
    pub filename: String,
    pub quant: Option<String>,
    /// Kept for backward compat with DB queue. Always None from new wizard requests.
    /// Populated from GGUF parsing during pull.
    #[serde(default)]
    pub context_length: Option<u32>,
}

/// Request body for pull job.
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub repo_id: String,
    /// DB id of a pre-created model stub (created before pulling).
    /// When set, `setup_model_after_pull` updates the existing row instead of creating a new one.
    #[serde(default)]
    pub model_id: Option<u32>,
    /// Legacy single-quant support (kept for backward compat).
    #[serde(default)]
    pub quant: Option<String>,
    /// Legacy multi-quant wizard format: list of quants to pull.
    #[serde(default)]
    pub quants: Vec<QuantDownloadSpec>,
    /// New simplified format: just filenames (model quants)
    #[serde(default)]
    pub filenames: Vec<String>,
    /// Vision projector files
    #[serde(default)]
    pub mmproj_filenames: Vec<String>,
    /// MTP draft model files
    #[serde(default)]
    pub mtp_filenames: Vec<String>,
    #[serde(default)]
    pub context_length: Option<u32>,
}

/// Response for a pull job.
#[derive(Debug, Serialize)]
pub struct PullResponse {
    pub job_id: String,
    pub status: String,
    pub repo_id: String,
    pub filename: String,
    pub bytes_pulled: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Response for model load/unload.
#[derive(Debug, Serialize)]
pub struct ModelResponse {
    pub id: String,
    pub loaded: bool,
}

/// A single model entry in the list models response.
#[derive(Debug, Serialize, Deserialize)]
pub struct ListedModelResponse {
    pub id: Option<i64>,
    pub display_name: Option<String>,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_name: Option<String>,
    /// Current lifecycle state: idle, loading, ready, unloading, failed.
    pub state: crate::gpu::ModelState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_time_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed_secs_ago: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout_remaining_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consecutive_failures: Option<u32>,
}

/// Response for listing all configured models.
#[derive(Debug, Serialize, Deserialize)]
pub struct ListModelsResponse {
    pub models: Vec<ListedModelResponse>,
}

/// Response for system restart.
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct RestartResponse {
    pub message: String,
}

/// Returns `false` if the path component contains traversal sequences or invalid characters.
pub(super) fn is_safe_path_component(s: &str) -> bool {
    !s.is_empty() && !s.contains("..") && !s.contains('/') && !s.contains('\\') && !s.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_max_concurrent_pulls_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var("TAMA_MAX_CONCURRENT_PULLS").ok();

        std::env::remove_var("TAMA_MAX_CONCURRENT_PULLS");
        assert_eq!(max_concurrent_pulls(), 8);

        if let Some(val) = original {
            std::env::set_var("TAMA_MAX_CONCURRENT_PULLS", val);
        }
    }

    #[test]
    fn test_max_concurrent_pulls_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("TAMA_MAX_CONCURRENT_PULLS", "16");
        }
        assert_eq!(max_concurrent_pulls(), 16);
        unsafe {
            std::env::remove_var("TAMA_MAX_CONCURRENT_PULLS");
        }
    }

    #[test]
    fn test_max_concurrent_pulls_invalid_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("TAMA_MAX_CONCURRENT_PULLS", "not_a_number");
        }
        // Should fall back to default
        assert_eq!(max_concurrent_pulls(), 8);
        unsafe {
            std::env::remove_var("TAMA_MAX_CONCURRENT_PULLS");
        }
    }

    #[test]
    fn test_is_safe_path_component_valid() {
        assert!(is_safe_path_component("model.gguf"));
        assert!(is_safe_path_component("Q4_K_M"));
        assert!(is_safe_path_component("unsloth"));
    }

    #[test]
    fn test_is_safe_path_component_invalid() {
        assert!(!is_safe_path_component(""));
        assert!(!is_safe_path_component(".."));
        assert!(!is_safe_path_component("../etc"));
        assert!(!is_safe_path_component("path/to/file"));
        assert!(!is_safe_path_component("path\\to\\file"));
        assert!(!is_safe_path_component("path\0null"));
    }
}
