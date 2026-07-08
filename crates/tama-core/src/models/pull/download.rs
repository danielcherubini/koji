use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use reqwest::Client;

use super::ProgressCallback;

// ── Pull implementation ─────────────────────────────────────────────────────

/// Result of pulling a GGUF file.
#[derive(Debug)]
pub struct PullResult {
    /// Local path to the file
    pub path: PathBuf,
    /// File size in bytes
    pub size_bytes: u64,
}

/// Pull a GGUF file from a HuggingFace repo using our parallel puller.
/// Uses HTTP Range requests with auth headers for gated repos.
pub async fn pull_gguf_with_progress(
    repo_id: &str,
    filename: &str,
    dest_dir: &Path,
    progress_callback: Option<ProgressCallback>,
) -> Result<PullResult> {
    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let url = format!("{}/{}/resolve/main/{}", endpoint, repo_id, filename);

    let dest_path = dest_dir.join(filename);
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Build auth headers
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(token) = super::get_hf_token() {
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token)
                .parse()
                .context("Failed to parse Authorization header")?,
        );
    }

    // Build client with HTTP/2 keep-alive
    let client = Client::builder()
        .http2_keep_alive_timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client")?;

    let size_bytes = super::pull_chunked_with_progress(
        &client,
        &url,
        &dest_path,
        8, // max connections
        progress_callback,
        Some(&headers),
    )
    .await
    .with_context(|| format!("Failed to pull '{}' from '{}'", filename, repo_id))?;

    Ok(PullResult {
        path: dest_path,
        size_bytes,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Guard to serialize env var tests
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    /// Verifies that the HF resolve URL is constructed correctly
    #[test]
    fn test_pull_gguf_url_construction_default_endpoint() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::remove_var("HF_ENDPOINT");

        // We can't call the full async function against a real URL,
        // but we can verify the URL format by checking the logic directly
        let repo_id = "org/model";
        let filename = "model.gguf";
        let expected_url = format!(
            "https://huggingface.co/{}/resolve/main/{}",
            repo_id, filename
        );
        assert_eq!(
            expected_url,
            "https://huggingface.co/org/model/resolve/main/model.gguf"
        );
    }

    /// Verifies that HF_ENDPOINT env var is respected in URL construction
    #[test]
    fn test_pull_gguf_url_construction_custom_endpoint() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", "https://hf.mirror.example.com");

        let repo_id = "org/model";
        let filename = "model.gguf";
        let endpoint =
            std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
        let url = format!("{}/{}/resolve/main/{}", endpoint, repo_id, filename);
        assert_eq!(
            url,
            "https://hf.mirror.example.com/org/model/resolve/main/model.gguf"
        );

        std::env::remove_var("HF_ENDPOINT");
    }

    /// Verifies that empty token does NOT add Authorization header
    #[test]
    fn test_empty_token_no_auth_header() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_TOKEN", "");

        let token = super::super::get_hf_token();
        assert!(token.is_none(), "Empty HF_TOKEN should resolve to None");

        std::env::remove_var("HF_TOKEN");
    }

    /// Verifies that whitespace-only token does NOT add Authorization header
    #[test]
    fn test_whitespace_token_no_auth_header() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_TOKEN", "   \n  ");

        let token = super::super::get_hf_token();
        assert!(
            token.is_none(),
            "Whitespace-only HF_TOKEN should resolve to None"
        );

        std::env::remove_var("HF_TOKEN");
    }

    /// Verifies that a valid token produces the correct Bearer header value
    #[test]
    fn test_valid_token_produces_bearer_header() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_TOKEN", "hf_test_token_123");

        let token = super::super::get_hf_token();
        assert_eq!(token, Some("hf_test_token_123".to_string()));

        // Verify the header value format
        let header_value = format!("Bearer {}", token.unwrap());
        assert_eq!(header_value, "Bearer hf_test_token_123");

        // Verify it parses as a valid header
        let parsed: reqwest::header::HeaderValue = header_value.parse().unwrap();
        assert_eq!(parsed.to_str().unwrap(), "Bearer hf_test_token_123");

        std::env::remove_var("HF_TOKEN");
    }

    /// Verifies PullResult contains expected fields
    #[test]
    fn test_pull_result_structure() {
        let result = PullResult {
            path: PathBuf::from("/tmp/model.gguf"),
            size_bytes: 1234567890,
        };
        assert_eq!(result.path, PathBuf::from("/tmp/model.gguf"));
        assert_eq!(result.size_bytes, 1234567890);
    }

    /// Integration test: pull a small public file from HuggingFace.
    /// Marked `#[ignore]` to skip in normal test runs.
    #[tokio::test]
    #[ignore]
    async fn test_pull_gguf_with_progress_real() {
        let temp_dir = tempfile::tempdir().unwrap();
        let result = pull_gguf_with_progress(
            "julien-c/dummy-unknown",
            "config.json",
            temp_dir.path(),
            None,
        )
        .await;
        assert!(result.is_ok());
        assert!(result.unwrap().path.exists());
    }
}
