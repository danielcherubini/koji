//! Startup reconciliation: reap managed containers left behind from crashed Tama instances.
//!
//! On proxy startup, this module runs `docker ps` to find any containers with the
//! `tama.managed=true` label and removes them with `docker rm -f`. This prevents
//! stale containers from being adopted as native backends by `cleanup_stale_processes`.
//!
//! If Docker is unavailable (daemon not reachable), a warning is logged and `Ok(())`
//! is returned — startup is never blocked.

use super::image::docker_available;
use super::runner;
use anyhow::Result;

/// Reap any managed containers left behind from crashed Tama instances.
///
/// 1. Check if Docker is available via `docker info`. If unavailable, log a warning and return `Ok(())`.
/// 2. Run `docker ps -a --filter label=tama.managed=true` to list managed containers.
/// 3. For each container found, call `docker rm -f {id}` to remove it.
pub async fn startup_reconcile() -> Result<()> {
    // Check if Docker is available — if not, skip reconciliation (don't block startup).
    if docker_available().await.is_err() {
        tracing::warn!("Docker not available, skipping startup reconciliation");
        return Ok(());
    }

    let output = tokio::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            "label=tama.managed=true",
            "--format",
            "{{.ID}} {{.Names}}",
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("docker ps failed during reconciliation: {}", stderr.trim());
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut removed_count = 0;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() == 2 {
            let _id = parts[0];
            let name = parts[1];
            match runner::remove_container(name).await {
                Ok(()) => {
                    tracing::info!("Reaped stale managed container: {}", name);
                    removed_count += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to remove managed container '{}': {}", name, e);
                }
            }
        }
    }

    if removed_count > 0 {
        tracing::info!(
            "Startup reconciliation: removed {} stale managed container(s)",
            removed_count
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    /// Set up a fake Docker CLI on PATH for testing, using the project's fake-docker.sh fixture.
    fn setup_fake_docker() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let fixture = temp_dir.path().join("docker");
        let fixture_src = format!(
            "{}/tests/fixtures/fake-docker.sh",
            env!("CARGO_MANIFEST_DIR")
        );
        fs::copy(&fixture_src, &fixture)
            .unwrap_or_else(|_| panic!("copy fake-docker.sh from {}", fixture_src));
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755)).expect("chmod +x");

        let original_path = env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", temp_dir.path().display(), original_path);
        env::set_var("PATH", &new_path);
        env::set_var(
            "FAKE_DOCKER_STATE_DIR",
            temp_dir.path().join("docker-state"),
        );

        temp_dir
    }

    /// Build a JSON state file for a managed container.
    fn build_managed_state(id: &str, name: &str) -> String {
        let mut s = String::from("{\"Id\": \"");
        s.push_str(id);
        s.push_str("\", \"Name\": \"");
        s.push_str(name);
        s.push_str("\", \"State\": {\"Running\": true, \"Pid\": 12345}, ");
        s.push_str("\"HostConfig\": {\"Labels\": {\"tama.managed\": \"true\"}}");
        s.push('}');
        s
    }

    /// Build a JSON state file for a non-managed container.
    fn build_non_managed_state(id: &str, name: &str) -> String {
        let mut s = String::from("{\"Id\": \"");
        s.push_str(id);
        s.push_str("\", \"Name\": \"");
        s.push_str(name);
        s.push_str("\", \"State\": {\"Running\": true, \"Pid\": 12345}}");
        s
    }

    /// Create a fake container state file and return its ID.
    fn create_container(
        containers_dir: &std::path::Path,
        id: &str,
        name: &str,
        managed: bool,
    ) -> String {
        // Ensure the containers directory exists (fake-docker.sh creates it lazily).
        fs::create_dir_all(containers_dir).expect("create containers dir");
        let state_content = if managed {
            build_managed_state(id, name)
        } else {
            build_non_managed_state(id, name)
        };
        let state_file = containers_dir.join(id);
        fs::write(&state_file, &state_content).expect("write container state");
        id.to_string()
    }

    #[tokio::test]
    async fn test_startup_reconcile_removes_managed_containers() {
        let _guard = setup_fake_docker();

        // Get the state directory from env var set by setup_fake_docker.
        let state_dir =
            env::var("FAKE_DOCKER_STATE_DIR").expect("FAKE_DOCKER_STATE_DIR must be set");
        let containers_dir = std::path::Path::new(&state_dir).join("containers");

        // Create two managed containers and one non-managed container.
        let managed1_id = create_container(&containers_dir, "abc001", "tama-llama", true);
        let managed2_id = create_container(&containers_dir, "abc002", "tama-vllm", true);
        let non_managed_id = create_container(&containers_dir, "xyz001", "other-container", false);

        // Verify containers exist before reconciliation.
        assert!(
            containers_dir.join(&managed1_id).exists(),
            "managed1 should exist before reconcile"
        );
        assert!(
            containers_dir.join(&managed2_id).exists(),
            "managed2 should exist before reconcile"
        );
        assert!(
            containers_dir.join(&non_managed_id).exists(),
            "non-managed should exist before reconcile"
        );

        // Run reconciliation.
        startup_reconcile().await.expect("reconcile should succeed");

        // Verify managed containers were removed, non-managed was left alone.
        assert!(
            !containers_dir.join(&managed1_id).exists(),
            "managed container 'tama-llama' should be removed"
        );
        assert!(
            !containers_dir.join(&managed2_id).exists(),
            "managed container 'tama-vllm' should be removed"
        );
        assert!(
            containers_dir.join(&non_managed_id).exists(),
            "non-managed container should NOT be removed"
        );
    }

    #[tokio::test]
    async fn test_startup_reconcile_no_managed_containers_is_noop() {
        let _guard = setup_fake_docker();

        // Only create a non-managed container.
        let state_dir =
            env::var("FAKE_DOCKER_STATE_DIR").expect("FAKE_DOCKER_STATE_DIR must be set");
        let containers_dir = std::path::Path::new(&state_dir).join("containers");
        create_container(&containers_dir, "xyz001", "other-container", false);

        // Should succeed without errors.
        startup_reconcile()
            .await
            .expect("reconcile should succeed with no managed containers");

        // Non-managed container still exists.
        let count = fs::read_dir(&containers_dir)
            .expect("read containers dir")
            .count();
        assert_eq!(count, 1, "non-managed container should remain");
    }

    #[tokio::test]
    async fn test_startup_reconcile_empty_containers_dir_is_noop() {
        let _guard = setup_fake_docker();

        // No containers at all — should succeed without errors.
        startup_reconcile()
            .await
            .expect("reconcile should succeed with no containers");
    }

    #[tokio::test]
    async fn test_startup_reconcile_docker_unavailable_returns_ok() {
        // Save original PATH and replace with one that has no docker at all.
        let original_path = env::var("PATH").unwrap_or_default();
        env::set_var("PATH", "/nonexistent");

        let result = startup_reconcile().await;

        // Restore PATH.
        env::set_var("PATH", &original_path);

        // Should return Ok (not block startup).
        assert!(
            result.is_ok(),
            "startup_reconcile should return Ok when docker is unavailable, got: {:?}",
            result
        );
    }
}
