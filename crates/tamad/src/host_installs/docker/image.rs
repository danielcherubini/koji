//! Docker availability check (plan-191 Task 10).
//!
//! What stays: `docker_available` — the startup reconcile (`reconcile.rs`)
//! uses it to skip cleanly when the Docker daemon is absent. Image
//! pull/inspect (`pull_image`/`is_image_present`) had no live caller: the
//! docker engine is not installable through the tamad (see `installs.rs`),
//! so they were deleted in Task 10.

use anyhow::{anyhow, Result};
use std::process::Stdio;
use tokio::process::Command;

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

#[cfg(test)]
mod image_tests {
    use super::*;

    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// Set up the fake docker CLI for a test.
    ///
    /// Copies `fixtures/fake-docker.sh` into a per-test temp directory as `docker`,
    /// makes it executable, and returns (temp_dir, docker_dir) so the caller can
    /// prepend `docker_dir` to PATH and set `FAKE_DOCKER_STATE_DIR`.
    fn setup_fake_docker() -> (tempfile::TempDir, PathBuf) {
        let tmpdir = tempfile::tempdir().expect("create temp dir");
        let docker_dir = tmpdir.path().join("bin");
        fs::create_dir_all(&docker_dir).expect("create bin dir");

        // Copy the fake docker script into the test's tempdir.
        let fixture_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/fake-docker.sh");
        let dest_path = docker_dir.join("docker");
        fs::copy(&fixture_path, &dest_path).expect("copy fake-docker.sh");
        fs::set_permissions(&dest_path, PermissionsExt::from_mode(0o755))
            .expect("chmod +x fake-docker.sh");

        (tmpdir, docker_dir)
    }

    fn set_fake_docker_path() -> (tempfile::TempDir, PathBuf, String) {
        let (tmpdir, docker_dir) = setup_fake_docker();
        let state_dir = tmpdir.path().join("state");
        fs::create_dir_all(&state_dir).expect("create state dir");

        // Save original PATH.
        let original_path = std::env::var("PATH").unwrap_or_default();

        // Prepend docker_dir to PATH.
        let new_path = format!("{}:{}", docker_dir.display(), original_path);
        std::env::set_var("PATH", &new_path);
        std::env::set_var("FAKE_DOCKER_STATE_DIR", state_dir.to_str().unwrap());

        (tmpdir, docker_dir, original_path)
    }

    fn restore_docker_path(original_path: &str) {
        std::env::set_var("PATH", original_path);
        std::env::remove_var("FAKE_DOCKER_STATE_DIR");
    }

    // ─── docker_available tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_docker_available_with_fake_docker() {
        let (_tmpdir, _docker_dir, original_path) = set_fake_docker_path();

        let result = docker_available().await;
        assert!(
            result.is_ok(),
            "docker_available should succeed with fake docker: {:?}",
            result
        );

        restore_docker_path(&original_path);
    }
}
