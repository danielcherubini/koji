use anyhow::Context;
use futures_util::TryStreamExt;
use indicatif::ProgressBar;
use reqwest::header::HeaderMap;
use reqwest::Client;
use std::path::Path;
use tokio::io::AsyncWriteExt;

use super::{exponential_backoff, ProgressCallback, MAX_RETRIES};

/// Download a file using a single HTTP stream with retry support.
pub async fn pull_single(
    client: &Client,
    url: &str,
    dest: &Path,
    total_size: u64,
    pb: &ProgressBar,
    progress_callback: Option<&ProgressCallback>,
    headers: Option<&HeaderMap>,
) -> anyhow::Result<()> {
    let headers = headers.cloned().unwrap_or_default();
    let mut attempt = 0u32;
    let mut pulled: u64 = 0;

    loop {
        attempt += 1;

        let mut request = client.get(url).headers(headers.clone());
        if pulled > 0 {
            request = request.header("Range", format!("bytes={}-", pulled));
        }

        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) if attempt <= MAX_RETRIES => {
                tracing::warn!(
                    "  Download failed (attempt {}/{}), retrying... ({})",
                    attempt,
                    MAX_RETRIES,
                    e
                );
                tokio::time::sleep(exponential_backoff(attempt)).await;
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        // Validate status code
        let status = resp.status().as_u16();
        if pulled > 0 && status != 206 {
            // Only bail immediately for permanent mismatch (un-ranged 200)
            if status == 200 {
                anyhow::bail!(
                    "Expected 206 Partial Content for resumed pull, got {}",
                    status
                );
            }
            // Retry transient errors (429/5xx)
            if attempt <= MAX_RETRIES {
                tracing::warn!(
                    "  Server returned {}, retrying ({}/{})...",
                    status,
                    attempt,
                    MAX_RETRIES
                );
                tokio::time::sleep(exponential_backoff(attempt)).await;
                continue;
            }
            anyhow::bail!("Download failed with status {}", status);
        }
        if pulled == 0 && !resp.status().is_success() {
            if attempt <= MAX_RETRIES {
                tracing::warn!(
                    "  Server returned {}, retrying ({}/{})...",
                    status,
                    attempt,
                    MAX_RETRIES
                );
                tokio::time::sleep(exponential_backoff(attempt)).await;
                continue;
            }
            anyhow::bail!("Download failed with status {}", status);
        }

        let mut file = if pulled > 0 {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(dest)
                .await
                .with_context(|| format!("Failed to open {} for append", dest.display()))?
        } else {
            tokio::fs::File::create(dest)
                .await
                .with_context(|| format!("Failed to create {}", dest.display()))?
        };

        let mut stream = resp.bytes_stream();
        let mut stream_failed = false;

        loop {
            match stream.try_next().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk)
                        .await
                        .with_context(|| format!("Failed to write to {}", dest.display()))?;
                    pulled += chunk.len() as u64;
                    pb.set_position(pulled);
                    if let Some(cb) = progress_callback {
                        cb(pulled, total_size);
                    }
                }
                Ok(None) => break,
                Err(_e) => {
                    if attempt <= MAX_RETRIES {
                        tracing::warn!(
                            "  Stream interrupted at {:.1} MiB (attempt {}/{}), retrying... ({})",
                            pulled as f64 / 1_048_576.0,
                            attempt,
                            MAX_RETRIES,
                            _e
                        );
                        // Keep progress bar at current position for retry
                        pb.set_position(pulled);
                        stream_failed = true;
                        break;
                    }
                    return Err(_e.into());
                }
            }
        }

        file.flush().await?;

        if stream_failed {
            tokio::time::sleep(exponential_backoff(attempt)).await;
            continue;
        }

        // Download complete
        break;
    }

    Ok(())
}
