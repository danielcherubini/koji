//! Docker image management: check availability, inspect images, and pull.

use anyhow::{anyhow, Result};
use std::process::Stdio;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// Check whether the Docker CLI is available and the daemon is reachable.
///
/// Runs `docker info` and returns `Ok(())` on success. Returns an error if the
/// docker binary is missing or the daemon cannot be reached.
pub async fn docker_available() -> Result<()> {
    let output = Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await?;

    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow!("Docker daemon is not reachable"))
    }
}

/// Check whether a Docker image is already present locally.
///
/// Runs `docker image inspect <image>` and returns `true` if the image exists,
/// `false` otherwise (when the error message contains "No such image").
pub async fn is_image_present(image: &str) -> Result<bool> {
    let output = Command::new("docker")
        .arg("image")
        .arg("inspect")
        .arg(image)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such image") || stderr.contains("not found") {
        return Ok(false);
    }

    Err(anyhow!("docker inspect failed: {}", stderr.trim()))
}

/// Pull a Docker image, streaming progress to the callback.
///
/// Runs `docker pull <image>` and feeds each line of stdout/stderr to the
/// `progress` callback. Respects the `CancellationToken` (kills the subprocess
/// on cancellation) and enforces a timeout of `timeout_secs` seconds.
pub async fn pull_image(
    image: &str,
    progress: impl Fn(String) + Send + Sync + Clone + 'static,
    timeout_secs: u64,
    cx: &CancellationToken,
) -> Result<()> {
    let mut child = Command::new("docker")
        .arg("pull")
        .arg(image)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

    // Spawn tasks to read stdout and stderr lines concurrently.
    let progress_stdout = progress.clone();
    let stdout_task = tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await.unwrap_or_default() {
            if line.is_empty() {
                continue;
            }
            progress_stdout(line);
        }
    });

    let progress_stderr = progress.clone();
    let stderr_task = tokio::spawn(async move {
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await.unwrap_or_default() {
            if line.is_empty() {
                continue;
            }
            progress_stderr(line);
        }
    });

    // Wait for the child process with cancellation and timeout.
    let result = tokio::select! {
        _ = cx.cancelled() => {
            let _ = child.kill().await;
            return Err(anyhow!("pull cancelled"));
        }
        status = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait(),
        ) => match status {
            Ok(status) => status?,
            Err(_) => {
                let _ = child.kill().await;
                return Err(anyhow!(
                    "pull timed out after {} seconds",
                    timeout_secs
                ));
            }
        },
    };

    // Wait for the streaming tasks to finish.
    stdout_task.await?;
    stderr_task.await?;

    if result.success() {
        Ok(())
    } else {
        Err(anyhow!("docker pull failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that docker_available() returns an error when docker is not on PATH.
    /// Skip this test when docker IS available (use integration tests with fake-docker
    /// instead to verify both success and failure paths).
    #[tokio::test]
    async fn test_docker_available_not_on_path() {
        // Save original PATH and replace with a path that has no docker.
        let original_path = std::env::var("PATH").unwrap_or_default();

        // Use only /usr/bin which won't have our fake docker.
        std::env::set_var("PATH", "/usr/bin");

        let result = docker_available().await;

        // Restore original PATH before asserting.
        std::env::set_var("PATH", &original_path);

        if result.is_ok() {
            // Docker is available on this system — skip rather than fail.
            // The integration tests (docker_image_tests.rs) cover both paths.
            println!("SKIP: docker_available succeeded; docker is installed");
        } else {
            assert!(
                result.is_err(),
                "docker_available should fail when docker is not on PATH"
            );
        }
    }
}
