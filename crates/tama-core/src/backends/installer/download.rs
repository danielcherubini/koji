use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::backends::ProgressSink;
use crate::models::pull::parse_content_length;

/// Maximum number of download retries.
const MAX_RETRIES: u32 = 3;
/// Base backoff delay for retry attempts (1s, 2s, 4s).
const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Download a file from a URL to a destination path with retry logic.
///
/// Retries up to `MAX_RETRIES` times on network errors and 5xx responses,
/// with exponential backoff (1s, 2s, 4s).
///
/// When `progress.is_some()`, skips `indicatif` and emits throttled progress lines
/// via the sink (format: "downloaded {hsz_done} / {hsz_total} ({pct}%)").
///
/// When `progress.is_none()`, preserves the existing `indicatif` TTY bar behavior.
///
/// After the stream completes, verifies downloaded bytes match Content-Length when known.
pub async fn download_with_client(
    url: &str,
    dest: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
    client: Option<&Client>,
) -> Result<()> {
    let client = match client {
        Some(c) => c,
        None => &Client::builder()
            .user_agent("tama-backend-manager")
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(30))
            .build()?,
    };

    let mut last_error = None;
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let multiplier: u32 = 1 << (attempt - 1); // 1, 2, 4
            let backoff = BASE_BACKOFF * multiplier;
            tracing::info!(
                attempt,
                max_retries = MAX_RETRIES,
                url,
                "Retrying download after {:?} backoff",
                backoff
            );
            tokio::time::sleep(backoff).await;
        }

        match perform_download(client, url, dest, progress).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Check if this is a retryable error (network error or 5xx)
                let is_retryable = is_retryable_error(&e);
                if !is_retryable || attempt == MAX_RETRIES {
                    tracing::warn!(attempt, url, error = %e, "Download failed after {} attempts", attempt + 1);
                    return Err(e);
                }
                last_error = Some(e);
            }
        }
    }

    // Should not reach here, but just in case
    Err(last_error.unwrap_or_else(|| anyhow!("Download failed after {} retries", MAX_RETRIES)))
}

/// Check if an error is retryable (network errors and 5xx responses).
fn is_retryable_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string().to_lowercase();
    // Network-level errors
    if msg.contains("connection")
        || msg.contains("timeout")
        || msg.contains("dns")
        || msg.contains("refused")
        || msg.contains("reset")
        || msg.contains("tls")
        || msg.contains("certificate")
        || msg.contains("closed")
    {
        return true;
    }
    // HTTP 5xx errors
    if msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("501")
        || msg.contains("status: 5")
    {
        return true;
    }
    false
}

/// Perform a single download attempt.
pub async fn perform_download(
    client: &Client,
    url: &str,
    dest: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download from {}", url))?;

    let status = response.status();
    if !status.is_success() {
        // For 5xx responses, return a retryable error
        if status.as_u16() >= 500 {
            return Err(anyhow!(
                "Download failed with server error {}: {}",
                status,
                status.canonical_reason().unwrap_or("unknown")
            ));
        }
        return Err(anyhow!("Download failed with status: {}", status));
    }

    let total_size = parse_content_length(response.headers()).unwrap_or(0);

    if let Some(sink) = progress {
        // Web path: no indicatif, emit throttled progress lines
        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();
        let mut last_emit = tokio::time::Instant::now();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            // Throttle emissions to ~1 per 250ms
            if downloaded % (1024 * 1024) < chunk.len() as u64
                || last_emit.elapsed() >= Duration::from_millis(250)
            {
                let pct = if total_size > 0 {
                    (downloaded as f64 / total_size as f64) * 100.0
                } else {
                    0.0
                };
                let done_mb = downloaded as f64 / 1_048_576.0;
                let total_mb = total_size as f64 / 1_048_576.0;
                let msg = format!(
                    "downloaded {:.1} MiB / {:.1} MiB ({:.0}%)",
                    done_mb, total_mb, pct
                );
                sink.log(&msg);
                last_emit = tokio::time::Instant::now();
            }
        }

        // Verify downloaded bytes match Content-Length when known
        if total_size > 0 && downloaded != total_size {
            return Err(anyhow!(
                "Download incomplete: expected {} bytes but got {}",
                total_size,
                downloaded
            ));
        }

        // Flush to ensure all data is written to disk before returning
        file.flush().await?;
        Ok(())
    } else {
        // CLI path: preserve indicatif TTY bar
        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        // Verify downloaded bytes match Content-Length when known
        if total_size > 0 && downloaded != total_size {
            return Err(anyhow!(
                "Download incomplete: expected {} bytes but got {}",
                total_size,
                downloaded
            ));
        }

        // Flush to ensure all data is written to disk before returning
        file.flush().await?;

        pb.finish_and_clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_retryable_error_network() {
        let err = anyhow::anyhow!("connection refused");
        assert!(is_retryable_error(&err));

        let err = anyhow::anyhow!("timeout exceeded");
        assert!(is_retryable_error(&err));

        let err = anyhow::anyhow!("dns resolution failed");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_5xx() {
        let err = anyhow::anyhow!("Download failed with status: 500 Internal Server Error");
        assert!(is_retryable_error(&err));

        let err = anyhow::anyhow!("Download failed with server error 503: Service Unavailable");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_not_retryable() {
        let err = anyhow::anyhow!("Download failed with status: 404 Not Found");
        assert!(!is_retryable_error(&err));

        let err = anyhow::anyhow!("Download failed with status: 401 Unauthorized");
        assert!(!is_retryable_error(&err));

        let err = anyhow::anyhow!("file not found");
        assert!(!is_retryable_error(&err));
    }
}
