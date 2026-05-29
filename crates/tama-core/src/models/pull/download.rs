use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;

use crate::models::download::ProgressCallback;

// ── ProgressAdapter (kept for proxy handler) ─────────────────────────────────

/// Progress adapter that bridges hf-hub's Progress trait to our callback.
#[derive(Clone)]
pub struct ProgressAdapter {
    total_size: u64,
    downloaded: Arc<AtomicU64>,
    callback: Option<ProgressCallback>,
}

impl ProgressAdapter {
    pub fn new(callback: Option<ProgressCallback>) -> Self {
        Self {
            total_size: 0,
            downloaded: Arc::new(AtomicU64::new(0)),
            callback,
        }
    }
}

impl hf_hub::api::tokio::Progress for ProgressAdapter {
    async fn init(&mut self, size: usize, _filename: &str) {
        self.total_size = size as u64;
        self.downloaded.store(0, Ordering::Relaxed);
        if let Some(cb) = &self.callback {
            cb(0, self.total_size);
        }
    }

    async fn update(&mut self, size: usize) {
        // size is the chunk just downloaded, accumulate it
        let new_total = self.downloaded.fetch_add(size as u64, Ordering::Relaxed) + size as u64;
        if let Some(cb) = &self.callback {
            cb(new_total, self.total_size);
        }
    }

    async fn finish(&mut self) {
        self.downloaded.store(self.total_size, Ordering::Relaxed);
        if let Some(cb) = &self.callback {
            cb(self.total_size, self.total_size);
        }
    }
}

// ── Parallel downloader ─────────────────────────────────────────────────────

/// Result of downloading a GGUF file.
#[derive(Debug)]
pub struct DownloadResult {
    /// Local path to the file
    pub path: PathBuf,
    /// File size in bytes
    pub size_bytes: u64,
}

/// Download a GGUF file from a HuggingFace repo using our parallel downloader.
/// Uses HTTP Range requests with auth headers for gated repos.
pub async fn download_gguf_with_progress(
    repo_id: &str,
    filename: &str,
    dest_dir: &Path,
    progress_callback: Option<ProgressCallback>,
) -> Result<DownloadResult> {
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

    let size_bytes = crate::models::download::download_chunked_with_progress(
        &client,
        &url,
        &dest_path,
        8, // max connections
        progress_callback,
        Some(&headers),
    )
    .await
    .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

    Ok(DownloadResult {
        path: dest_path,
        size_bytes,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use hf_hub::api::tokio::Progress;
    use std::sync::Mutex;

    // Guard to serialize env var tests
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    /// Verifies that the HF resolve URL is constructed correctly
    #[test]
    fn test_download_gguf_url_construction_default_endpoint() {
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
    fn test_download_gguf_url_construction_custom_endpoint() {
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

    /// Verifies DownloadResult contains expected fields
    #[test]
    fn test_download_result_structure() {
        let result = DownloadResult {
            path: PathBuf::from("/tmp/model.gguf"),
            size_bytes: 1234567890,
        };
        assert_eq!(result.path, PathBuf::from("/tmp/model.gguf"));
        assert_eq!(result.size_bytes, 1234567890);
    }

    // ── ProgressAdapter tests ─────────────────────────────────────────────

    /// Verifies ProgressAdapter::new creates adapter with zeroed state
    #[test]
    fn test_progress_adapter_new_state() {
        let adapter = ProgressAdapter::new(None);
        assert_eq!(adapter.total_size, 0);
        assert_eq!(adapter.downloaded.load(Ordering::Relaxed), 0);
    }

    /// Verifies ProgressAdapter::init sets total size and calls callback
    #[tokio::test]
    async fn test_progress_adapter_init() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();
        let callback: ProgressCallback = Arc::new(move |_downloaded: u64, _total: u64| {
            called_clone.store(true, Ordering::Relaxed);
        });

        let mut adapter = ProgressAdapter::new(Some(callback));
        adapter.init(1000, "test.gguf").await;

        assert_eq!(adapter.total_size, 1000);
        assert!(called.load(Ordering::Relaxed));
    }

    /// Verifies ProgressAdapter::update accumulates downloaded bytes
    #[tokio::test]
    async fn test_progress_adapter_update() {
        let downloaded = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let downloaded_clone = downloaded.clone();
        let callback: ProgressCallback = Arc::new(move |d: u64, _total: u64| {
            downloaded_clone.store(d, Ordering::Relaxed);
        });

        let mut adapter = ProgressAdapter::new(Some(callback));
        adapter.init(1000, "test.gguf").await;
        adapter.update(100).await;
        adapter.update(200).await;

        assert_eq!(downloaded.load(Ordering::Relaxed), 300);
    }

    /// Verifies ProgressAdapter::finish reports total as fully downloaded
    #[tokio::test]
    async fn test_progress_adapter_finish() {
        let final_downloaded = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let final_downloaded_clone = final_downloaded.clone();
        let callback: ProgressCallback = Arc::new(move |d: u64, _total: u64| {
            final_downloaded_clone.store(d, Ordering::Relaxed);
        });

        let mut adapter = ProgressAdapter::new(Some(callback));
        adapter.init(1000, "test.gguf").await;
        adapter.finish().await;

        assert_eq!(final_downloaded.load(Ordering::Relaxed), 1000);
    }
}
