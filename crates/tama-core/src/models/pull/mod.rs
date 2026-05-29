use std::path::PathBuf;

use anyhow::{Context, Result};
use hf_hub::api::tokio::{Api, ApiBuilder};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

pub mod api;
pub mod download;
pub mod metadata;
pub mod quant;

static HF_API: OnceCell<Api> = OnceCell::const_new();

/// Resolve HF token from environment or token file.
/// Priority: `HF_TOKEN` env → `$HF_HOME/token` → `~/.cache/huggingface/token`
pub(crate) fn get_hf_token() -> Option<String> {
    // 1. HF_TOKEN env var
    if let Ok(token) = std::env::var("HF_TOKEN") {
        let trimmed = token.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }

    // 2. $HF_HOME/token
    if let Ok(hf_home) = std::env::var("HF_HOME") {
        let token_path = PathBuf::from(&hf_home).join("token");
        if let Ok(content) = std::fs::read_to_string(&token_path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }

    // 3. ~/.cache/huggingface/token
    if let Ok(home) = std::env::var("HOME") {
        let token_path = PathBuf::from(&home).join(".cache/huggingface/token");
        if let Ok(content) = std::fs::read_to_string(&token_path) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }

    None
}

/// Get or create the shared HuggingFace API client.
/// Configured with max_files=8 for parallel file downloads.
///
/// **Note:** This uses `ApiBuilder::new()` which respects the `HF_HOME` environment
/// variable for cache location. No explicit cache path is set, so `hf-hub` will use
/// its default behavior:
/// - If `HF_HOME` is set: `$HF_HOME/hub`
/// - Otherwise: `~/.cache/huggingface/hub`
pub(crate) async fn hf_api() -> Result<&'static Api> {
    HF_API
        .get_or_try_init(|| async {
            let token = get_hf_token();
            ApiBuilder::new()
                .with_token(token)
                .with_max_files(8) // Allow 8 concurrent file downloads
                .build()
                .context("Failed to initialise HuggingFace API client")
        })
        .await
}

/// Information about a GGUF file in a HuggingFace repo.
#[derive(Debug, Clone)]
pub struct RemoteGguf {
    /// Filename, e.g. "OmniCoder-8B-Q4_K_M.gguf"
    pub filename: String,
    /// Inferred quant type from filename, e.g. "Q4_K_M"
    pub quant: Option<String>,
}

/// Result of listing GGUF files from a HuggingFace repo.
#[derive(Debug, Clone)]
pub struct RepoGgufListing {
    /// Resolved repo ID (may differ from input if `-GGUF` was appended)
    pub repo_id: String,
    /// HF repo HEAD commit SHA at time of listing
    pub commit_sha: String,
    /// Available GGUF files
    pub files: Vec<RemoteGguf>,
}

/// Per-file blob metadata returned by the HuggingFace blobs API.
#[derive(Debug, Clone)]
pub struct BlobInfo {
    pub filename: String,
    pub blob_id: Option<String>,
    pub size: Option<i64>,
    pub lfs_sha256: Option<String>,
}

/// Metadata extracted from HuggingFace API and README for a model.
/// Internal data-transfer type between the fetcher and the DB update helper.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HfModelMetadata {
    pub hf_format: Option<String>,
    pub hf_base_model: Option<String>,
    pub hf_pipeline_tag: Option<String>,
    pub hf_total_params: Option<String>,
    pub hf_active_params: Option<String>,
    pub hf_architecture_type: Option<String>,
    pub hf_context_length: Option<u32>,
    pub hf_num_layers: Option<u32>,
    pub hf_last_modified: Option<String>,
}

// ── Re-exports from sub-modules ──────────────────────────────────────────────

pub use super::gguf::GgufMetadata;
pub use api::{
    fetch_blob_metadata, fetch_hf_metadata, fetch_model_pipeline_tag,
    infer_modalities_from_pipeline, list_gguf_files, parse_blob_siblings,
};
pub use download::{cleanup_hf_cache, ProgressAdapter};
pub use metadata::{fetch_community_card, parse_readme_metadata};
pub use quant::infer_quant_from_filename;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Guard to serialize env var tests without needing serial_test
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    /// HF_TOKEN env var takes priority over all other sources
    #[test]
    fn test_get_hf_token_env_var_priority() {
        let _guard = ENV_GUARD.lock().unwrap();

        // Clean slate
        std::env::remove_var("HF_TOKEN");
        std::env::remove_var("HF_HOME");

        // Set HF_TOKEN
        std::env::set_var("HF_TOKEN", "env_token_value");

        let token = get_hf_token();
        assert_eq!(token, Some("env_token_value".to_string()));

        std::env::remove_var("HF_TOKEN");
    }

    /// $HF_HOME/token is used when HF_TOKEN env var not set
    #[test]
    fn test_get_hf_token_hf_home_file() {
        let _guard = ENV_GUARD.lock().unwrap();

        // Clean slate
        std::env::remove_var("HF_TOKEN");
        std::env::remove_var("HF_HOME");

        let temp_dir = tempfile::tempdir().unwrap();
        let token_path = temp_dir.path().join("token");
        std::fs::write(&token_path, "hf_home_token_value").unwrap();

        std::env::set_var("HF_HOME", temp_dir.path().to_str().unwrap());

        let token = get_hf_token();
        assert_eq!(token, Some("hf_home_token_value".to_string()));

        std::env::remove_var("HF_HOME");
    }

    /// ~/.cache/huggingface/token is used as last file fallback
    #[test]
    fn test_get_hf_token_home_cache_file() {
        let _guard = ENV_GUARD.lock().unwrap();

        // Clean slate
        std::env::remove_var("HF_TOKEN");
        std::env::remove_var("HF_HOME");

        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join(".cache/huggingface");
        std::fs::create_dir_all(&cache_dir).unwrap();
        let token_path = cache_dir.join("token");
        std::fs::write(&token_path, "home_cache_token_value").unwrap();

        std::env::set_var("HOME", temp_dir.path().to_str().unwrap());

        let token = get_hf_token();
        assert_eq!(token, Some("home_cache_token_value".to_string()));

        std::env::remove_var("HOME");
    }

    /// Empty/whitespace-only HF_TOKEN is treated as None
    #[test]
    fn test_get_hf_token_empty_env_var() {
        let _guard = ENV_GUARD.lock().unwrap();

        std::env::remove_var("HF_HOME");

        // Empty string
        std::env::set_var("HF_TOKEN", "");
        assert!(get_hf_token().is_none());

        // Whitespace only
        std::env::set_var("HF_TOKEN", "   ");
        assert!(get_hf_token().is_none());

        std::env::remove_var("HF_TOKEN");
    }

    /// Empty/whitespace-only token file is treated as None
    #[test]
    fn test_get_hf_token_empty_file() {
        let _guard = ENV_GUARD.lock().unwrap();

        std::env::remove_var("HF_TOKEN");

        let temp_dir = tempfile::tempdir().unwrap();
        let token_path = temp_dir.path().join("token");

        // Empty file
        std::fs::write(&token_path, "").unwrap();
        std::env::set_var("HF_HOME", temp_dir.path().to_str().unwrap());
        assert!(get_hf_token().is_none());

        // Whitespace only
        std::fs::write(&token_path, "   \n  ").unwrap();
        assert!(get_hf_token().is_none());

        std::env::remove_var("HF_HOME");
    }

    /// Returns None when no token source is available
    #[test]
    fn test_get_hf_token_no_source() {
        let _guard = ENV_GUARD.lock().unwrap();

        std::env::remove_var("HF_TOKEN");
        std::env::remove_var("HF_HOME");
        std::env::remove_var("HOME");

        let token = get_hf_token();
        assert!(token.is_none());
    }

    /// HF_TOKEN env var takes priority over $HF_HOME/token
    #[test]
    fn test_get_hf_token_env_overrides_file() {
        let _guard = ENV_GUARD.lock().unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let token_path = temp_dir.path().join("token");
        std::fs::write(&token_path, "file_token").unwrap();

        std::env::set_var("HF_HOME", temp_dir.path().to_str().unwrap());
        std::env::set_var("HF_TOKEN", "env_token");

        let token = get_hf_token();
        assert_eq!(token, Some("env_token".to_string()));

        std::env::remove_var("HF_TOKEN");
        std::env::remove_var("HF_HOME");
    }
}
