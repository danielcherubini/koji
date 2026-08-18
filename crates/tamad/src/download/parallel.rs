use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use futures_util::TryStreamExt;
use indicatif::ProgressBar;
use reqwest::header::HeaderMap;
use reqwest::Client;
use tokio::io::AsyncWriteExt;

use super::{exponential_backoff, ProgressCallback, MAX_RETRIES};

/// Pull a file using parallel HTTP Range requests.
#[allow(clippy::too_many_arguments)]
pub async fn pull_parallel(
    client: &Client,
    url: &str,
    dest: &Path,
    total_size: u64,
    num_connections: usize,
    pb: &ProgressBar,
    progress_callback: Option<&ProgressCallback>,
    headers: Option<&HeaderMap>,
) -> anyhow::Result<()> {
    if num_connections == 0 {
        anyhow::bail!("num_connections must be > 0");
    }
    if total_size < num_connections as u64 {
        anyhow::bail!(
            "total_size ({}) must be >= num_connections ({})",
            total_size,
            num_connections
        );
    }
    // Build temp file paths
    let tmp_dir = dest.parent().unwrap_or(Path::new("."));
    let dest_filename = dest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Destination path has no file name: {:?}", dest))?
        .to_string_lossy();
    let tmp_paths: Vec<PathBuf> = (0..num_connections)
        .map(|i| tmp_dir.join(format!(".{}.part{}", dest_filename, i)))
        .collect();

    // Shared atomic counter for tracking total progress across all chunks
    let total_pulled = Arc::new(AtomicU64::new(0));

    // Spawn a task to poll progress and call the callback
    let progress_handle = if let Some(callback) = progress_callback {
        let callback = callback.clone();
        let total_pulled = total_pulled.clone();
        let pb_clone = pb.clone();
        Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let pulled = total_pulled.load(Ordering::Relaxed);
                pb_clone.set_position(pulled);
                callback(pulled, total_size);
            }
        }))
    } else {
        None
    };

    // Pull each chunk to a temp file
    let mut handles = Vec::new();

    for (i, tmp_path) in tmp_paths.iter().enumerate().take(num_connections) {
        let (start, end) = super::calculate_chunk_ranges(total_size, num_connections)[i];

        let client = client.clone();
        let url = url.to_string();
        let tmp_path = tmp_path.clone();
        let pb = pb.clone();
        let total_pulled = total_pulled.clone();
        let headers = headers.cloned();

        let handle = tokio::spawn(async move {
            pull_chunk_with_retry(
                &client,
                &url,
                &tmp_path,
                start,
                end,
                i,
                &pb,
                Some(&total_pulled),
                &headers.unwrap_or_default(),
            )
            .await?;
            Ok::<PathBuf, anyhow::Error>(tmp_path)
        });

        handles.push(handle);
    }

    // Wait for all chunks — clean up on any failure
    let mut first_error: Option<anyhow::Error> = None;

    for handle in handles {
        match handle.await {
            Ok(Ok(_path)) => {}
            Ok(Err(e)) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e.into());
                }
            }
        }
    }

    // Stop the progress polling task
    if let Some(handle) = progress_handle {
        handle.abort();
    }

    // If any chunk failed, clean up all temp files and bail
    if let Some(err) = first_error {
        cleanup_temp_files(&tmp_paths).await;
        return Err(err);
    }

    // Reassemble chunks into final file in index order (using tmp_paths which
    // are ordered by chunk index, not completion order)
    let mut dest_file = tokio::fs::File::create(dest).await?;
    for tmp_path in &tmp_paths {
        let mut chunk_file = tokio::fs::File::open(tmp_path).await?;
        tokio::io::copy(&mut chunk_file, &mut dest_file).await?;
        tokio::fs::remove_file(tmp_path).await.ok();
    }
    dest_file.flush().await?;

    Ok(())
}

/// Pull a single chunk with retry and exponential backoff.
#[allow(clippy::too_many_arguments)]
async fn pull_chunk_with_retry(
    client: &Client,
    url: &str,
    tmp_path: &Path,
    start: u64,
    end: u64,
    chunk_index: usize,
    pb: &ProgressBar,
    total_pulled: Option<&AtomicU64>,
    headers: &HeaderMap,
) -> anyhow::Result<()> {
    let expected_size = end - start + 1;
    let mut attempt = 0u32;

    loop {
        attempt += 1;

        let range = format!("bytes={}-{}", start, end);
        let request = client
            .get(url)
            .header("Range", &range)
            .headers(headers.clone());
        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) if attempt <= MAX_RETRIES => {
                tracing::warn!(
                    "  Chunk {} failed (attempt {}/{}), retrying... ({})",
                    chunk_index,
                    attempt,
                    MAX_RETRIES,
                    e
                );
                tokio::time::sleep(exponential_backoff(attempt)).await;
                continue;
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Range request failed for chunk {}", chunk_index));
            }
        };

        // Validate we got 206 Partial Content
        let status = resp.status().as_u16();
        if status != 206 {
            if attempt <= MAX_RETRIES {
                tracing::warn!(
                    "  Chunk {} got status {} (expected 206), retrying ({}/{})...",
                    chunk_index,
                    status,
                    attempt,
                    MAX_RETRIES
                );
                tokio::time::sleep(exponential_backoff(attempt)).await;
                continue;
            }
            anyhow::bail!(
                "Chunk {} got status {} instead of 206 Partial Content",
                chunk_index,
                status
            );
        }

        let mut stream = resp.bytes_stream();
        let mut file = tokio::fs::File::create(tmp_path).await?;
        let mut chunk_pulled: u64 = 0;
        let mut stream_failed = false;

        loop {
            match stream.try_next().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk).await?;
                    let len = chunk.len() as u64;
                    chunk_pulled += len;
                    pb.inc(len);
                    if let Some(counter) = total_pulled {
                        counter.fetch_add(len, Ordering::Relaxed);
                    }
                }
                Ok(None) => break,
                Err(_e) => {
                    stream_failed = true;
                    break;
                }
            }
        }

        file.flush().await?;

        if stream_failed {
            if attempt > MAX_RETRIES {
                anyhow::bail!(
                    "Chunk {} stream failed after {} retries",
                    chunk_index,
                    MAX_RETRIES
                );
            }
            pb.dec(chunk_pulled);
            tokio::time::sleep(exponential_backoff(attempt)).await;
            continue;
        }

        // Verify chunk size
        if chunk_pulled != expected_size {
            if attempt <= MAX_RETRIES {
                tracing::warn!(
                    "  Chunk {} short read ({}/{} bytes), retrying ({}/{})...",
                    chunk_index,
                    chunk_pulled,
                    expected_size,
                    attempt,
                    MAX_RETRIES
                );
                pb.dec(chunk_pulled);
                tokio::time::sleep(exponential_backoff(attempt)).await;
                continue;
            }
            anyhow::bail!(
                "Chunk {} incomplete: got {} of {} bytes",
                chunk_index,
                chunk_pulled,
                expected_size
            );
        }

        break;
    }

    Ok(())
}

/// Best-effort cleanup of temp chunk files.
async fn cleanup_temp_files(paths: &[PathBuf]) {
    for path in paths {
        tokio::fs::remove_file(path).await.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::{ProgressBar, ProgressStyle};
    use reqwest::Client;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Verify that `exponential_backoff` returns durations within expected bounds
    /// for a range of attempt values.
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

    /// Verify that `pull_parallel` correctly reassembles chunks from a wiremock
    /// server returning 206 Partial Content responses.
    #[tokio::test]
    async fn test_pull_parallel_happy_path() {
        let server = MockServer::start().await;

        // Total size: 100 bytes, 2 connections → 50 bytes each
        let total_size: u64 = 100;
        let num_connections = 2;

        // Wiremock matches most-recently-mounted mocks first. Mount the more specific
        // Range header mocks AFTER the fallback so they take precedence.
        Mock::given(method("GET"))
            .and(path("/test.bin"))
            .respond_with(
                ResponseTemplate::new(206)
                    .set_body_string("AB")
                    .append_header("Content-Range", "bytes 0-1/100")
                    .append_header("Content-Length", "2"),
            )
            .mount(&server)
            .await;

        // Chunk 0: Range bytes=0-49 → 50 'a' characters
        Mock::given(method("GET"))
            .and(path("/test.bin"))
            .and(header("Range", "bytes=0-49"))
            .respond_with(
                ResponseTemplate::new(206)
                    .set_body_string("a".repeat(50))
                    .append_header("Content-Range", "bytes 0-49/100")
                    .append_header("Content-Length", "50"),
            )
            .with_priority(1) // highest priority
            .mount(&server)
            .await;

        // Chunk 1: Range bytes=50-99 → 50 'b' characters
        Mock::given(method("GET"))
            .and(path("/test.bin"))
            .and(header("Range", "bytes=50-99"))
            .respond_with(
                ResponseTemplate::new(206)
                    .set_body_string("b".repeat(50))
                    .append_header("Content-Range", "bytes 50-99/100")
                    .append_header("Content-Length", "50"),
            )
            .with_priority(1) // highest priority
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

        let result = pull_parallel(
            &client,
            &format!("{}/test.bin", server.uri()),
            &dest,
            total_size,
            num_connections,
            &pb,
            None::<&ProgressCallback>,
            None::<&HeaderMap>,
        )
        .await;

        // The parallel download should succeed and reassemble a 100-byte file.
        assert!(result.is_ok(), "pull_parallel should succeed: {:?}", result);
        let content = tokio::fs::read(&dest).await.expect("file should exist");
        assert_eq!(content.len(), 100);
    }

    /// Verify that a short chunk (incomplete body) errors after MAX_RETRIES.
    #[tokio::test]
    async fn test_pull_parallel_short_chunk_errors_after_retries() {
        let server = MockServer::start().await;

        // Total size: 100 bytes, but server only sends 30 bytes
        let total_size: u64 = 100;
        let num_connections = 1;

        Mock::given(method("GET"))
            .and(path("/test.bin"))
            .respond_with(
                ResponseTemplate::new(206)
                    .set_body_string("X".repeat(30))
                    .append_header("Content-Range", "bytes 0-29/100")
                    .append_header("Content-Length", "30"),
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

        let result = pull_parallel(
            &client,
            &format!("{}/test.bin", server.uri()),
            &dest,
            total_size,
            num_connections,
            &pb,
            None::<&ProgressCallback>,
            None::<&HeaderMap>,
        )
        .await;

        assert!(result.is_err(), "pull_parallel should fail for short chunk");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("incomplete") || err_msg.contains("short read"),
            "Error should mention incomplete/short, got: {}",
            err_msg
        );
    }

    /// Verify that `pull_parallel` rejects invalid arguments.
    #[tokio::test]
    async fn test_pull_parallel_rejects_bad_args() {
        let client = Client::new();
        let temp_dir = tempfile::tempdir().unwrap();
        let dest = temp_dir.path().join("test.bin");

        let pb = ProgressBar::new(100);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
                .expect("valid template"),
        );

        // num_connections = 0 should error
        let result = pull_parallel(
            &client,
            "http://example.com/test.bin",
            &dest,
            100,
            0, // num_connections = 0
            &pb,
            None::<&ProgressCallback>,
            None::<&HeaderMap>,
        )
        .await;
        assert!(
            result.is_err(),
            "num_connections=0 should error: {:?}",
            result
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must be > 0"),
            "Error should mention > 0, got: {}",
            err_msg
        );

        // total_size < num_connections should error
        let result = pull_parallel(
            &client,
            "http://example.com/test.bin",
            &dest,
            5,  // total_size = 5
            10, // num_connections = 10 > total_size
            &pb,
            None::<&ProgressCallback>,
            None::<&HeaderMap>,
        )
        .await;
        assert!(result.is_err(), "total_size < num_connections should error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("must be >="),
            "Error should mention must be >=, got: {}",
            err_msg
        );
    }
}
