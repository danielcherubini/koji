use std::path::PathBuf;

use anyhow::{Context, Result};
use hf_hub::api::tokio::{Api, ApiBuilder};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

mod parallel;
mod single;

pub mod api;
pub mod download;
pub mod metadata;
pub mod quant;

use std::path::Path;
use std::sync::Arc;

use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::HeaderMap;
use reqwest::Client;

const MIN_CHUNK_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB
const MAX_RETRIES: u32 = 3;

/// Callback type for reporting pull progress.
/// Called with (bytes_pulled, total_bytes).
pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// Parse the Content-Length header from raw headers, bypassing reqwest's
/// Response::content_length() which returns Some(0) for HEAD requests (known bug).
pub fn parse_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Calculate the number of connections to use for parallel pull,
/// based on total file size and minimum chunk size.
pub fn calculate_connections(total_size: u64, max_connections: usize) -> usize {
    if total_size <= MIN_CHUNK_SIZE {
        return 1;
    }
    let suggested = (total_size / MIN_CHUNK_SIZE) as usize;
    suggested.min(max_connections).max(1)
}

/// Calculate chunk ranges for parallel pull.
/// Returns a vector of (start, end) byte ranges for each chunk.
pub fn calculate_chunk_ranges(total_size: u64, num_chunks: usize) -> Vec<(u64, u64)> {
    if num_chunks == 0 || total_size == 0 {
        return vec![];
    }
    let chunk_size = total_size / num_chunks as u64;
    (0..num_chunks)
        .map(|i| {
            let start = i as u64 * chunk_size;
            let end = if i == num_chunks - 1 {
                total_size.saturating_sub(1)
            } else {
                (i as u64 + 1) * chunk_size - 1
            };
            (start, end)
        })
        .collect()
}

/// Calculate the expected size of a single chunk given total size and number of chunks.
pub fn chunk_size_for(total_size: u64, num_chunks: usize) -> u64 {
    if num_chunks == 0 || total_size == 0 {
        return 0;
    }
    total_size / num_chunks as u64
}

/// Pull a file using parallel HTTP Range requests.
/// Falls back to single-stream if Range is not supported.
/// Skips pull if the destination already exists with matching size.
pub async fn pull_chunked(
    client: &Client,
    url: &str,
    dest: &Path,
    connections: usize,
    headers: Option<&HeaderMap>,
) -> Result<u64> {
    pull_chunked_with_progress(client, url, dest, connections, None, headers).await
}

/// Pull a file using parallel HTTP Range requests with progress callback.
/// Falls back to single-stream if Range is not supported.
/// Skips pull if the destination already exists with matching size.
///
/// The progress callback is called periodically with (bytes_pulled, total_bytes).
/// This is useful for reporting progress to external consumers (e.g., SSE streams).
pub async fn pull_chunked_with_progress(
    client: &Client,
    url: &str,
    dest: &Path,
    connections: usize,
    progress_callback: Option<ProgressCallback>,
    headers: Option<&HeaderMap>,
) -> Result<u64> {
    // HEAD request to get Content-Length and check Range support
    let head = client
        .head(url)
        .headers(headers.cloned().unwrap_or_default())
        .send()
        .await
        .with_context(|| format!("HEAD request failed for {}", url))?;

    if !head.status().is_success() {
        anyhow::bail!("HEAD request returned HTTP {}: {}", head.status(), url);
    }

    let total_size = parse_content_length(head.headers())
        .context("Server did not return a valid Content-Length")?;

    if total_size == 0 {
        anyhow::bail!("Server reported Content-Length of 0 for {}", url);
    }

    // Skip pull if file already exists with matching size
    if dest.exists() {
        if let Ok(meta) = tokio::fs::metadata(dest).await {
            if meta.len() == total_size {
                return Ok(total_size);
            }
        }
    }

    let accept_ranges = head
        .headers()
        .get("accept-ranges")
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
        .unwrap_or("none");

    let use_chunked = accept_ranges != "none" && total_size > MIN_CHUNK_SIZE;
    let num_connections = if use_chunked {
        connections
            .min((total_size / MIN_CHUNK_SIZE) as usize)
            .max(1)
    } else {
        1
    };

    let pb = ProgressBar::new(total_size);
    let template = "{spinner:.green} [{elapsed_precise}] \
                    [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})";
    pb.set_style(
        ProgressStyle::default_bar()
            .template(template)
            .context("Invalid progress bar template")?
            .progress_chars("=>-"),
    );

    // Wrap the callback to also update the progress bar
    let callback_for_bar = if let Some(cb) = progress_callback.clone() {
        let pb_clone = pb.clone();
        Some(Arc::new(move |pulled: u64, total: u64| {
            pb_clone.set_position(pulled);
            cb(pulled, total);
        }) as ProgressCallback)
    } else {
        None
    };

    let result = if num_connections == 1 {
        single::pull_single(
            client,
            url,
            dest,
            total_size,
            &pb,
            callback_for_bar.as_ref(),
            headers,
        )
        .await
    } else {
        parallel::pull_parallel(
            client,
            url,
            dest,
            total_size,
            num_connections,
            &pb,
            callback_for_bar.as_ref(),
            headers,
        )
        .await
    };

    pb.finish_and_clear();
    result?;
    Ok(total_size)
}

#[cfg(test)]
mod chunk_tests {
    use super::*;

    // ── parse_content_length tests ────────────────────────────────────────

    #[test]
    fn test_parse_content_length_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "12345".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(12345));
    }

    #[test]
    fn test_parse_content_length_large_value() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "999999999999".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(999999999999));
    }

    #[test]
    fn test_parse_content_length_missing() {
        let headers = HeaderMap::new();
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_non_numeric() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "abc".parse().unwrap());
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_zero() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "0".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(0));
    }

    #[test]
    fn test_parse_content_length_negative_string() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "-1".parse().unwrap());
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_with_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "  512  ".parse().unwrap());
        // to_str() preserves whitespace, parse::<u64>() fails on whitespace
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_case_insensitive_header() {
        let mut headers = HeaderMap::new();
        // HTTP headers are case-insensitive
        headers.insert("Content-Length", "4096".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(4096));
    }

    #[test]
    fn test_parse_content_length_multiple_values() {
        let mut headers = HeaderMap::new();
        headers.append("content-length", "100".parse().unwrap());
        headers.append("content-length", "200".parse().unwrap());
        // and_then takes the first value
        assert_eq!(parse_content_length(&headers), Some(100));
    }

    // ── calculate_connections tests ───────────────────────────────────────

    #[test]
    fn test_calculate_connections_small_file() {
        // File smaller than MIN_CHUNK_SIZE (5 MiB)
        assert_eq!(calculate_connections(1024, 4), 1);
        assert_eq!(calculate_connections(MIN_CHUNK_SIZE - 1, 8), 1);
    }

    #[test]
    fn test_calculate_connections_exact_chunk_size() {
        // File exactly MIN_CHUNK_SIZE should use 1 connection
        assert_eq!(calculate_connections(MIN_CHUNK_SIZE, 4), 1);
    }

    #[test]
    fn test_calculate_connections_multiple_chunks() {
        // 10 MiB file / 5 MiB chunk = 2 connections
        assert_eq!(calculate_connections(10 * 1024 * 1024, 4), 2);
        // 20 MiB file with max 2 connections
        assert_eq!(calculate_connections(20 * 1024 * 1024, 2), 2);
    }

    #[test]
    fn test_calculate_connections_capped_by_max() {
        // Large file but max connections limits it
        assert_eq!(calculate_connections(100 * 1024 * 1024, 3), 3);
    }

    #[test]
    fn test_calculate_connections_minimum_one() {
        // Even with max=1, should return at least 1 for large files
        assert_eq!(calculate_connections(100 * 1024 * 1024, 1), 1);
    }

    #[test]
    fn test_calculate_connections_zero_max() {
        // With max_connections=0, should still return at least 1
        assert_eq!(calculate_connections(100 * 1024 * 1024, 0), 1);
    }

    #[test]
    fn test_calculate_connections_zero_size() {
        // Zero-size file should return 1 (not 0)
        assert_eq!(calculate_connections(0, 8), 1);
    }

    // ── calculate_chunk_ranges tests ──────────────────────────────────────

    #[test]
    fn test_calculate_chunk_ranges_single_chunk() {
        let ranges = calculate_chunk_ranges(1000, 1);
        assert_eq!(ranges, vec![(0, 999)]);
    }

    #[test]
    fn test_calculate_chunk_ranges_even_split() {
        // 100 bytes split into 4 chunks = 25 bytes each
        let ranges = calculate_chunk_ranges(100, 4);
        assert_eq!(ranges, vec![(0, 24), (25, 49), (50, 74), (75, 99)]);
    }

    #[test]
    fn test_calculate_chunk_ranges_uneven_split() {
        // 100 bytes split into 3 chunks = 33 bytes each, last chunk gets remainder
        let ranges = calculate_chunk_ranges(100, 3);
        assert_eq!(ranges[0], (0, 32));
        assert_eq!(ranges[1], (33, 65));
        // Last chunk covers remaining bytes
        assert_eq!(ranges[2].0, 66);
        assert_eq!(ranges[2].1, 99);
    }

    #[test]
    fn test_calculate_chunk_ranges_zero_size() {
        let ranges = calculate_chunk_ranges(0, 4);
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_calculate_chunk_ranges_zero_chunks() {
        let ranges = calculate_chunk_ranges(1000, 0);
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_calculate_chunk_ranges_covers_full_range() {
        // Verify that all ranges together cover [0, total_size - 1]
        let total_size = 1024;
        let num_chunks = 7;
        let ranges = calculate_chunk_ranges(total_size, num_chunks);

        assert_eq!(ranges.len(), num_chunks);
        assert_eq!(ranges[0].0, 0);
        assert_eq!(ranges[num_chunks - 1].1, total_size - 1);

        // Verify no gaps between consecutive ranges
        for i in 0..(num_chunks - 1) {
            assert_eq!(ranges[i + 1].0, ranges[i].1 + 1);
        }
    }

    #[test]
    fn test_calculate_chunk_ranges_two_chunks() {
        let ranges = calculate_chunk_ranges(10, 2);
        assert_eq!(ranges, vec![(0, 4), (5, 9)]);
    }

    // ── chunk_size_for tests ──────────────────────────────────────────────

    #[test]
    fn test_chunk_size_for_even_split() {
        assert_eq!(chunk_size_for(1000, 4), 250);
        assert_eq!(chunk_size_for(100, 10), 10);
    }

    #[test]
    fn test_chunk_size_for_uneven_split() {
        // 100 / 3 = 33 (integer division)
        assert_eq!(chunk_size_for(100, 3), 33);
    }

    #[test]
    fn test_chunk_size_for_single_chunk() {
        assert_eq!(chunk_size_for(1000, 1), 1000);
    }

    #[test]
    fn test_chunk_size_for_zero_values() {
        assert_eq!(chunk_size_for(0, 5), 0);
        assert_eq!(chunk_size_for(100, 0), 0);
    }
}

// TODO(2026-06-27): Consider storing Api in ProxyState instead of global static.
// This is tracked in the Code Quality Backlog in docs/plans/README.md.
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
}

// ── Re-exports from sub-modules ──────────────────────────────────────────────

pub use super::gguf::GgufMetadata;
pub use api::{
    fetch_blob_metadata, fetch_hf_metadata, fetch_model_pipeline_tag, group_sharded_quants,
    infer_modalities_from_pipeline, list_gguf_files, parse_blob_siblings, GroupedQuant,
};
pub use download::{pull_gguf_with_progress, PullResult};
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
