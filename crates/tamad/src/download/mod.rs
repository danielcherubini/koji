//! Download execution for model weights (plan-191 Task 10; ADR-0010).
//!
//! The proxy never downloads: this module (tamad-side) owns the chunked
//! HTTP Range GGUF downloader, the HF URL/auth helpers it needs, and the
//! `hf` CLI spawn helpers for whole-repo pulls. Moved from
//! `tama_core::models::pull` (the chunked engine, `parallel`, `single`,
//! and the `hf` CLI spawn parts of `hf_cli`).
//!
//! What stays in `tama_core::models::pull`: the HF *metadata* API (wizard
//! endpoints), shared DTOs (`HfModelMetadata`, `RepoGgufListing`, ...),
//! `get_hf_token` (read for the gRPC token hand-off), and the pure
//! helpers (`scan_dir_bytes`, `hf_endpoint`).

pub mod hf;
mod parallel;
mod single;

pub use hf::{check_hf_binary, spawn_hf_download, start_stderr_reader};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Client;
use std::path::Path;
use std::sync::Arc;

const MIN_CHUNK_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB
const MAX_RETRIES: u32 = 3;

/// Callback type for reporting pull progress.
/// Called with (bytes_pulled, total_bytes).
pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// HuggingFace base URL, honoring the `HF_ENDPOINT` env var (mirror
/// support) — shared with the proxy's metadata endpoints.
fn hf_endpoint() -> String {
    tama_core::models::pull::hf_endpoint()
}

/// `{endpoint}/{repo_id}/resolve/main/{filename}`
pub fn hf_resolve_url(repo_id: &str, filename: &str) -> String {
    format!("{}/{}/resolve/main/{}", hf_endpoint(), repo_id, filename)
}

/// Authorization headers for HF requests from an explicit token.
///
/// Used by the tamad download path (plan-191 Task 6), which receives the
/// user's per-pull token over gRPC rather than reading the local
/// environment. Empty/whitespace-only tokens produce no header. The token
/// is only ever placed in an `Authorization` header — never logged.
pub fn hf_auth_headers_with_token(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let trimmed = token.trim();
    if !trimmed.is_empty() {
        if let Ok(value) = format!("Bearer {}", trimmed).parse::<HeaderValue>() {
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
    }
    headers
}

/// Parse the Content-Length header from raw headers, bypassing reqwest's
/// Response::content_length() which returns Some(0) for HEAD requests (known bug).
pub fn parse_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Random jitter in milliseconds (0..=500), adapted from hf_transfer.
fn jitter() -> u64 {
    rand::rng().random_range(0..=500)
}

/// Exponential backoff with jitter, adapted from hf_transfer.
/// Base: 300ms, max: 10000ms.
pub(super) fn exponential_backoff(attempt: u32) -> std::time::Duration {
    let base = 300 + (attempt as u64).pow(2) + jitter();
    std::time::Duration::from_millis(base.min(10_000))
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
        calculate_connections(total_size, connections)
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

    /// Serializes env-var mutating tests in this module.
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    // ── exponential_backoff (engine) tests ────────────────────────────────

    /// Verifies that `exponential_backoff` returns durations within expected bounds.
    /// Attempt 0 → between 300ms and 800ms inclusive (base 300 + 0² + jitter 0..=500).
    /// Attempt 100 → exactly 10_000ms (capped).
    #[test]
    fn test_exponential_backoff_bounds() {
        // Attempt 0: base = 300 + 0² + jitter(0..=500) = 300..=800
        for _ in 0..20 {
            let dur = super::exponential_backoff(0);
            let ms = dur.as_millis();
            assert!(
                (300..=800).contains(&ms),
                "attempt 0: {}ms not in [300, 800]",
                ms
            );
        }

        // Attempt 100: base = 300 + 100² + jitter = 100300 + jitter → capped at 10_000
        for _ in 0..5 {
            let dur = super::exponential_backoff(100);
            assert_eq!(dur.as_millis(), 10_000);
        }
    }

    // ── URL / auth helper tests ───────────────────────────────────────────

    /// `hf_resolve_url` uses the default endpoint when `HF_ENDPOINT` is not set.
    #[test]
    fn test_hf_resolve_url_default_endpoint() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::remove_var("HF_ENDPOINT");

        let url = hf_resolve_url("org/model", "model.gguf");
        assert_eq!(
            url,
            "https://huggingface.co/org/model/resolve/main/model.gguf"
        );
    }

    /// `hf_resolve_url` honours `HF_ENDPOINT` for mirror support.
    #[test]
    fn test_hf_resolve_url_custom_endpoint() {
        let _guard = ENV_GUARD.lock().unwrap();
        std::env::set_var("HF_ENDPOINT", "https://hf.mirror.example.com");

        let url = hf_resolve_url("org/model", "model.gguf");
        assert_eq!(
            url,
            "https://hf.mirror.example.com/org/model/resolve/main/model.gguf"
        );

        std::env::remove_var("HF_ENDPOINT");
    }

    /// `hf_auth_headers_with_token` builds a Bearer header from an explicit
    /// token (the pull path receives the token over gRPC).
    #[test]
    fn test_hf_auth_headers_with_token_valid() {
        let headers = hf_auth_headers_with_token("hf_explicit_token");
        assert_eq!(
            headers
                .get(reqwest::header::AUTHORIZATION)
                .map(|v| v.to_str().unwrap()),
            Some("Bearer hf_explicit_token")
        );
    }

    /// Empty/whitespace-only explicit tokens produce no header.
    #[test]
    fn test_hf_auth_headers_with_token_empty_omits_header() {
        assert!(hf_auth_headers_with_token("")
            .get(reqwest::header::AUTHORIZATION)
            .is_none());
        assert!(hf_auth_headers_with_token("   ")
            .get(reqwest::header::AUTHORIZATION)
            .is_none());
    }
}
