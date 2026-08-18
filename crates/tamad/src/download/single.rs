use anyhow::Context;
use futures_util::TryStreamExt;
use indicatif::ProgressBar;
use reqwest::header::HeaderMap;
use reqwest::Client;
use std::path::Path;
use tokio::io::AsyncWriteExt;

use super::{exponential_backoff, ProgressCallback, MAX_RETRIES};

/// Pull a file using a single HTTP stream with retry support.
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
                    "  Pull failed (attempt {}/{}), retrying... ({})",
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
            anyhow::bail!("Pull failed with status {}", status);
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
            anyhow::bail!("Pull failed with status {}", status);
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

        // Pull complete
        break;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::{ProgressBar, ProgressStyle};
    use reqwest::Client;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Verify that `exponential_backoff` returns durations within expected bounds
    /// for a range of attempt values (same as parallel tests).
    #[test]
    fn test_exponential_backoff_bounds() {
        // Attempt 0: base = 300 + 0² + jitter(0..=500) = 300..=800
        for _ in 0..20 {
            let dur = exponential_backoff(0);
            let ms = dur.as_millis();
            assert!(
                (300..=800).contains(&ms),
                "attempt 0: {}ms not in [300, 800]",
                ms
            );
        }

        // Attempt 1: base = 300 + 1² + jitter(0..=500) = 301..=801
        for _ in 0..20 {
            let dur = exponential_backoff(1);
            let ms = dur.as_millis();
            assert!(
                (301..=801).contains(&ms),
                "attempt 1: {}ms not in [301, 801]",
                ms
            );
        }

        // Attempt 3: base = 300 + 9 + jitter(0..=500) = 309..=809
        for _ in 0..20 {
            let dur = exponential_backoff(3);
            let ms = dur.as_millis();
            assert!(
                (309..=809).contains(&ms),
                "attempt 3: {}ms not in [309, 809]",
                ms
            );
        }

        // Attempt 10: base = 300 + 100 + jitter(0..=500) = 400..=900
        for _ in 0..20 {
            let dur = exponential_backoff(10);
            let ms = dur.as_millis();
            assert!(
                (400..=900).contains(&ms),
                "attempt 10: {}ms not in [400, 900]",
                ms
            );
        }

        // Attempt 100: base = 300 + 10000 + jitter → capped at 10_000
        for _ in 0..5 {
            let dur = exponential_backoff(100);
            assert_eq!(dur.as_millis(), 10_000);
        }
    }

    /// Verify that `pull_single` correctly downloads a file via wiremock.
    #[tokio::test]
    async fn test_pull_single_happy_path() {
        let server = MockServer::start().await;

        let body = "Hello, World! This is a test file for pull_single.".to_string();
        let total_size = body.len() as u64;

        Mock::given(method("GET"))
            .and(path("/test.bin"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(&body)
                    .append_header("Content-Length", total_size.to_string().as_str()),
            )
            .mount(&server)
            .await;

        let client = Client::new();
        let temp_dir = tempfile::tempdir().unwrap();
        let dest = temp_dir.path().join("test.bin");

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .expect("valid template"),
        );

        pull_single(
            &client,
            &format!("{}/test.bin", server.uri()),
            &dest,
            total_size,
            &pb,
            None::<&ProgressCallback>,
            None::<&HeaderMap>,
        )
        .await
        .expect("pull_single should succeed");

        // Verify file content
        let content = tokio::fs::read_to_string(&dest)
            .await
            .expect("file should exist");
        assert_eq!(content, body);
    }

    /// Verify that `pull_single` returns an error after MAX_RETRIES on a permanent failure.
    #[tokio::test]
    async fn test_pull_single_permanent_failure_errors() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test.bin"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = Client::new();
        let temp_dir = tempfile::tempdir().unwrap();
        let dest = temp_dir.path().join("test.bin");

        let pb = ProgressBar::new(100);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .expect("valid template"),
        );

        let result = pull_single(
            &client,
            &format!("{}/test.bin", server.uri()),
            &dest,
            100,
            &pb,
            None::<&ProgressCallback>,
            None::<&HeaderMap>,
        )
        .await;

        assert!(result.is_err(), "pull_single should fail after retries");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("status") || err_msg.contains("500"),
            "Error should mention status code, got: {}",
            err_msg
        );
    }
}
