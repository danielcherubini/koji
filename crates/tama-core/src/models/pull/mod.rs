use std::path::PathBuf;

use anyhow::{Context, Result};
use hf_hub::api::tokio::{Api, ApiBuilder};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

pub mod api;
pub mod hf_cli;
pub mod metadata;
pub mod quant;
pub mod tamad_result;

// TODO(2026-06-27): Consider storing Api in ProxyState instead of global static.
// This is tracked in the Code Quality Backlog in docs/plans/README.md.
static HF_API: OnceCell<Api> = OnceCell::const_new();

/// Base URL for HuggingFace, honoring the `HF_ENDPOINT` env var (mirror
/// support). Shared by the proxy's metadata endpoints and the tamad's
/// download URL resolution (plan-191 Task 10).
pub fn hf_endpoint() -> String {
    std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string())
}

/// `{endpoint}/api/models` — model-list/search API base.
pub(crate) fn hf_api_models_url() -> String {
    format!("{}/api/models", hf_endpoint())
}

/// `{endpoint}/api/models/{repo_id}`
pub(crate) fn hf_api_model_url(repo_id: &str) -> String {
    format!("{}/api/models/{}", hf_endpoint(), repo_id)
}

/// `{endpoint}/api/models/{repo_id}?blobs=true`
pub(crate) fn hf_api_model_blobs_url(repo_id: &str) -> String {
    format!("{}?blobs=true", hf_api_model_url(repo_id))
}

/// `{endpoint}/{repo_id}/raw/{branch}/{path}`
pub(crate) fn hf_raw_url(repo_id: &str, branch: &str, path: &str) -> String {
    format!("{}/{}/raw/{}/{}", hf_endpoint(), repo_id, branch, path)
}

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
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let token_path = base_dirs.home_dir().join(".cache/huggingface/token");
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
/// **Note:** `HF_ENDPOINT` is honored via `from_env` so tests and mirrors can
/// redirect the API; the `Api` is still cached process-wide in `HF_API`, so the
/// first initialisation wins. No explicit cache path is set, so `hf-hub` will use
/// its default behaviour:
/// - If `HF_HOME` is set: `$HF_HOME/hub`
/// - Otherwise: `~/.cache/huggingface/hub`
pub(crate) async fn hf_api() -> Result<&'static Api> {
    HF_API
        .get_or_try_init(|| async {
            let token = get_hf_token();
            ApiBuilder::from_env()
                .with_token(token)
                .with_max_files(8) // Allow 8 concurrent file pulls
                .build()
                .context("Failed to initialise HuggingFace API client")
        })
        .await
}

/// Information about a GGUF file in a HuggingFace repo.
#[derive(Debug, Clone, PartialEq)]
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
    /// Total size in bytes across ALL repo files, from the HF blobs API.
    /// Soft-fails to `None` when the blobs endpoint is unavailable.
    #[serde(default)]
    pub hf_total_size_bytes: Option<u64>,
    /// Number of files in the repo, from the HF blobs API.
    /// Soft-fails to `None` when the blobs endpoint is unavailable.
    #[serde(default)]
    pub hf_file_count: Option<u32>,
}

// ── Re-exports from sub-modules ──────────────────────────────────────────────

pub use super::gguf::GgufMetadata;
pub use api::{
    detect_hf_format, determine_primary_shard, directory_prefix, group_sharded_quants,
    infer_modalities_from_pipeline, list_gguf_files, lookup_blob_metadata, lookup_hf_metadata,
    lookup_model_pipeline_tag, lookup_repo_stats, parse_blob_siblings, parse_siblings_stats,
    GroupedQuant, RepoStats,
};
pub use metadata::{lookup_community_toml, parse_readme_metadata};
pub use quant::infer_quant_from_filename;
pub use tamad_result::{TamadGgufPullResult, TamadPulledFile, TamadRepoPullResult};

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

    /// hf_api() must respect HF_ENDPOINT (regression: ApiBuilder::new ignored it)
    /// Uses wiremock to verify the API client routes to a mock server.
    #[tokio::test]
    async fn test_list_gguf_files_respects_hf_endpoint() {
        // Set up the wiremock server and mount the mock (no env var needed).
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(r"^/api/models/test/repo/.*"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "sha": "abc123",
                    "siblings": [{"rfilename": "repo-Q4_K_M.gguf"}]
                })),
            )
            .mount(&server)
            .await;

        // Set HF_ENDPOINT under the guard so env-var tests stay serialised,
        // but drop it before any .await to avoid clippy::await_holding_lock.
        {
            let _guard = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
        }

        let listing = list_gguf_files("test/repo")
            .await
            .expect("listing from mock");

        std::env::remove_var("HF_ENDPOINT");
        assert_eq!(listing.files.len(), 1);
        assert_eq!(listing.files[0].filename, "repo-Q4_K_M.gguf");
        assert_eq!(listing.files[0].quant.as_deref(), Some("Q4_K_M"));
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

    // ── hf_api_model_blobs_url tests ────────────────────────────────────────

    /// `hf_api_model_blobs_url` uses the default endpoint and appends `?blobs=true`.
    #[test]
    fn test_hf_api_model_blobs_url() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::remove_var("HF_ENDPOINT");

        let url = hf_api_model_blobs_url("org/repo");
        assert_eq!(url, "https://huggingface.co/api/models/org/repo?blobs=true");
    }
}
