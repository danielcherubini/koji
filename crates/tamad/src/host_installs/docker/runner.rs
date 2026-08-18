//! Docker container execution (plan-191 Task 10).
//!
//! What stays: `remove_container` — used by the startup reconcile
//! (`reconcile.rs`) to reap `tama.managed=true` containers left behind on
//! this host. The full container runner (spawn/stop/inspect/rewrite) had
//! no live caller: the tamad's load path spawns host binaries directly and
//! the install spec rejects the docker engine (the legacy proxy never
//! spawned docker backends either — every load set `is_docker: false`), so
//! the rest was deleted in Task 10.

use tokio::process::Command;

use anyhow::{anyhow, Result};

/// Remove a Docker container. Tolerates "No such container".
pub async fn remove_container(name: &str) -> Result<()> {
    let output = Command::new("docker")
        .arg("rm")
        .arg("-f")
        .arg(name)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such container") || stderr.contains("not found") {
            return Ok(());
        }
        return Err(anyhow!("docker rm failed: {}", stderr.trim()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    /// Helper: set up a fake docker on PATH.
    fn setup_fake_docker() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let fixture = temp_dir.path().join("docker");
        let fixture_src = format!("{}/fixtures/fake-docker.sh", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(&fixture_src, &fixture)
            .unwrap_or_else(|_| panic!("copy fake-docker.sh from {}", fixture_src));
        std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x");

        let original_path = env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", temp_dir.path().display(), original_path);
        env::set_var("PATH", &new_path);
        env::set_var(
            "FAKE_DOCKER_STATE_DIR",
            temp_dir.path().join("docker-state"),
        );

        temp_dir
    }

    #[tokio::test]
    async fn test_remove_container_no_such_container_ok() {
        let _guard = setup_fake_docker();
        let result = remove_container("nonexistent-container").await;
        assert!(
            result.is_ok(),
            "remove_container should tolerate missing container"
        );
    }
}
