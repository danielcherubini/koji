//! Container lifecycle: spawn, stop, logs, inspect.
//!
//! This module provides the core container lifecycle functions that build and execute
//! docker commands. It handles path rewriting for model directories, volume resolution,
//! group GID resolution, and full container management (spawn, stop, remove, logs, inspect).

use super::{DockerConfig, DockerVolume};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// A spawned Docker container.
#[derive(Debug)]
pub struct DockerContainer {
    /// Human-readable container name.
    pub name: String,
    /// Docker-assigned container ID (full or short hash).
    pub id: String,
    /// PID of the container process on the host.
    pub pid: u32,
}

/// Parsed output from `docker inspect`.
#[derive(Debug, Deserialize)]
pub struct DockerInspect {
    #[serde(rename = "State")]
    pub state: InspectState,
    #[serde(rename = "NetworkSettings", default)]
    pub network: InspectNetwork,
}

/// The `State` block from docker inspect.
#[derive(Debug, Deserialize)]
pub struct InspectState {
    pub running: Option<bool>,
    #[serde(rename = "Pid")]
    pub pid: Option<u64>,
}

/// The `NetworkSettings` block from docker inspect.
#[derive(Debug, Deserialize, Default)]
pub struct InspectNetwork {
    #[serde(rename = "Ports", default)]
    pub ports: Option<serde_json::Value>,
}

// ─── Path Rewriting ──────────────────────────────────────────────

/// Rewrite host paths in args to container paths when they fall under `models_dir`.
///
/// Only paths under `models_dir` are rewritten. Paths referencing other mounted
/// directories (e.g., additional Docker volumes) are not supported and will error.
///
/// For each arg:
/// - Split form `"--flag /abs/path"` → if value is under `models_dir`, rewrite to `{container_model_path}/{relative}`
/// - Joined form `"--flag=/abs/path"` → split on first `=`, rewrite value if under `models_dir`
/// - Strip surrounding quotes (from shlex quoting) before matching paths
/// - Absolute paths outside `models_dir` that are already container paths (under
///   `container_model_path`, e.g. `--chat-template '/models/...'`) → pass through unchanged
/// - Absolute paths outside both `models_dir` and any container mount → Error
/// - Non-path args (flags without path values) → pass through unchanged
pub fn rewrite_args_for_container(
    args: &[String],
    models_dir: &Path,
    container_model_path: &str,
) -> Result<Vec<String>> {
    let mut result = Vec::with_capacity(args.len());

    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        // Strip surrounding quotes for matching purposes
        let unquoted = arg.trim_matches('"').trim_matches('\'');

        // Check for joined form: --flag=/path
        if let Some(eq_pos) = unquoted.find('=') {
            let flag = &unquoted[..eq_pos];
            let value = &unquoted[eq_pos + 1..];

            // Only rewrite if this looks like a path flag (contains a '/' in the value after '=')
            if value.starts_with('/') && flag.starts_with("--") {
                if let Some(rewritten) =
                    maybe_rewrite_path(value, models_dir, container_model_path)?
                {
                    result.push(format!("{}={}", flag, rewritten));
                    continue;
                }
            }
            // Not a path arg or not under models_dir — pass through original
            result.push(arg.clone());
            continue;
        }

        // Check for split form: --flag value
        if arg.starts_with("--") {
            if let Some(next) = iter.peek() {
                let next_str = *next;
                let next_unquoted = next_str.trim_matches('"').trim_matches('\'');

                // Only rewrite if the next arg looks like a path (starts with /)
                if next_unquoted.starts_with('/') {
                    if let Some(rewritten) =
                        maybe_rewrite_path(next_unquoted, models_dir, container_model_path)?
                    {
                        iter.next();
                        result.push(arg.clone());
                        result.push(rewritten);
                        continue;
                    }
                }
            }

            // Not a path-flag pair — pass through original flag arg
            result.push(arg.clone());
            continue;
        }

        // Bare positional absolute path (e.g. vLLM's model path as first arg).
        // Rewrite paths under models_dir to the container path.
        if unquoted.starts_with('/') {
            if let Some(rewritten) = maybe_rewrite_path(unquoted, models_dir, container_model_path)?
            {
                result.push(rewritten);
                continue;
            }
        }

        // Non-path arg — pass through unchanged
        result.push(arg.clone());
    }

    Ok(result)
}

/// Try to rewrite a path: if it's under `models_dir`, return `{container_path}/{relative}`.
/// If it's already a container path (under `container_model_path`) or a relative path,
/// return `Ok(None)` to pass it through unchanged. Returns an error if the path starts
/// with `/` but is neither under `models_dir` nor an existing container mount path.
fn maybe_rewrite_path(
    path: &str,
    models_dir: &Path,
    container_model_path: &str,
) -> Result<Option<String>> {
    let p = Path::new(path);

    // Check if the path is under models_dir
    if let Ok(relative) = p.strip_prefix(models_dir) {
        let rewritten = format!(
            "{}/{}",
            container_model_path.trim_end_matches('/'),
            relative.display()
        );
        return Ok(Some(rewritten));
    }

    // Already a container path — it lives under the model mount's container path
    // (e.g. `--chat-template '/models/templates/chat.jinja'`). The file is mounted
    // there, so pass it through unchanged instead of rejecting it as a host path.
    let container_prefix = container_model_path.trim_end_matches('/');
    if p.starts_with(container_prefix) {
        return Ok(None);
    }

    // Path starts with '/' but is not under models_dir or a container path — error
    if path.starts_with('/') {
        return Err(anyhow!(
            "Path '{}' is outside the models directory '{}' and cannot be mounted",
            path,
            models_dir.display()
        ));
    }

    // Relative path — no rewrite needed
    Ok(None)
}

// ─── Volume Resolution ───────────────────────────────────────────

/// Resolve volume mounts for a docker run command.
///
/// Substitutes `{{MODEL_DIR}}` → `models_dir` in `host_path`. Validates that all
/// host paths exist on the filesystem. Returns `["host:container:ro" ...]` format.
pub fn resolve_volumes(config: &DockerConfig, models_dir: &Path) -> Result<Vec<String>> {
    let mut volumes = Vec::new();

    // Model mount first
    volumes.push(format_volume(&config.model_mount, models_dir)?);

    // Additional volumes
    for vol in &config.volumes {
        volumes.push(format_volume(vol, models_dir)?);
    }

    Ok(volumes)
}

/// Format a single volume mount string: `host:container[:ro]`.
fn format_volume(vol: &DockerVolume, models_dir: &Path) -> Result<String> {
    let host = vol.host_path.replace(
        "{{MODEL_DIR}}",
        models_dir
            .to_str()
            .ok_or_else(|| anyhow!("models_dir contains invalid UTF-8"))?,
    );

    // Validate host path exists
    let host_path = Path::new(&host);
    if !host_path.exists() {
        return Err(anyhow!(
            "Host path does not exist for volume mount: {}",
            host
        ));
    }

    let mut result = format!("{}:{}", host, vol.container_path);
    if vol.read_only {
        result.push_str(":ro");
    }
    Ok(result)
}

// ─── Group Resolution ────────────────────────────────────────────

/// Resolve group names to GIDs using `getent group <name>`.
///
/// Skips silently (with a warning) if a group is not found.
pub async fn resolve_group_gids(group_names: &[String]) -> Vec<String> {
    let mut gids = Vec::with_capacity(group_names.len());

    for name in group_names {
        match getent_gid(name).await {
            Ok(gid) => {
                gids.push(gid);
            }
            Err(e) => {
                eprintln!("Warning: could not resolve group '{}': {}", name, e);
            }
        }
    }

    gids
}

/// Look up the GID for a group name via `getent group <name>`.
async fn getent_gid(name: &str) -> Result<String> {
    let output = Command::new("getent")
        .arg("group")
        .arg(name)
        .output()
        .await?;

    if !output.status.success() {
        return Err(anyhow!("group '{}' not found", name));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Format: group_name:password:GID:members
    let parts: Vec<&str> = stdout.split(':').collect();
    if parts.len() >= 3 {
        Ok(parts[2].to_string())
    } else {
        Err(anyhow!(
            "unexpected getent output for '{}': {}",
            name,
            stdout.trim()
        ))
    }
}

// ─── Container Lifecycle ─────────────────────────────────────────

/// Spawn a Docker container with the given configuration.
///
/// Builds and executes a `docker run` command with all flags from the config.
/// Returns a `DockerContainer` with name, ID, and PID.
pub async fn spawn_container(
    backend_name: &str,
    config: &DockerConfig,
    host_port: u16,
    args: Vec<String>,
    env_vars: Vec<String>,
    models_dir: &Path,
) -> Result<DockerContainer> {
    let container_name = format!("tama-{}", backend_name);

    // Build the docker run command
    let mut cmd = Command::new("docker");
    cmd.arg("run");

    // Detached mode
    cmd.arg("-d");

    // Container name
    cmd.arg("--name").arg(&container_name);

    // Managed label for reconciliation filtering
    cmd.arg("--label").arg("tama.managed=true");

    // Port mapping: host_port -> container_port (loopback only)
    cmd.arg("-p")
        .arg(format!("127.0.0.1:{}:{}", host_port, config.container_port));

    // Volumes
    let volumes = resolve_volumes(config, models_dir)?;
    for vol in &volumes {
        cmd.arg("-v").arg(vol);
    }

    // Devices
    for device in &config.devices {
        cmd.arg("--device").arg(device);
    }

    // GPUs
    if let Some(gpus) = &config.gpus {
        cmd.arg("--gpus").arg(gpus);
    }

    // Shared memory size
    if let Some(shm) = &config.shm_size {
        cmd.arg("--shm-size").arg(shm);
    }

    // Capabilities
    for cap in &config.cap_adds {
        cmd.arg("--cap-add").arg(cap);
    }

    // Security options
    for opt in &config.security_opts {
        cmd.arg("--security-opt").arg(opt);
    }

    // Group adds (resolved GIDs)
    let gids = resolve_group_gids(&config.group_adds).await;
    for gid in &gids {
        cmd.arg("--group-add").arg(gid);
    }

    // Environment variables
    for env in &env_vars {
        cmd.arg("-e").arg(env);
    }

    // Image and args
    cmd.arg(&config.image);
    for arg in &args {
        cmd.arg(arg);
    }

    let output = cmd.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "docker run failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }

    // Parse container ID from stdout
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Inspect to get PID
    let inspect = inspect_container(&container_name).await?;
    let pid = inspect
        .and_then(|i| i.state.pid.map(|p| p as u32))
        .unwrap_or(0);

    Ok(DockerContainer {
        name: container_name,
        id,
        pid,
    })
}

/// Stop a Docker container. Tolerates "No such container".
pub async fn stop_container(name: &str) -> Result<()> {
    let output = Command::new("docker")
        .arg("stop")
        .arg("-t")
        .arg("5")
        .arg(name)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such container") || stderr.contains("not found") {
            return Ok(());
        }
        return Err(anyhow!("docker stop failed: {}", stderr.trim()));
    }

    Ok(())
}

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

/// Stream logs from a container. Returns the child process for reading stdout/stderr.
pub async fn logs_stream(container_id: &str, since_epoch: u64) -> Result<tokio::process::Child> {
    let child = Command::new("docker")
        .arg("logs")
        .arg("-f")
        .arg("--since")
        .arg(since_epoch.to_string())
        .arg(container_id)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    Ok(child)
}

/// Inspect a container and return parsed state information.
///
/// Returns `None` if the container does not exist.
pub async fn inspect_container(name: &str) -> Result<Option<DockerInspect>> {
    let output = Command::new("docker")
        .arg("inspect")
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such container") || stderr.contains("not found") {
            return Ok(None);
        }
        return Err(anyhow!("docker inspect failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let inspected: Vec<DockerInspect> = serde_json::from_str(&stdout)
        .map_err(|e| anyhow!("failed to parse docker inspect JSON: {}", e))?;

    Ok(inspected.into_iter().next())
}

// ─── Tests ───────────────────────────────────────────────────────

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
        let fixture_src = format!(
            "{}/tests/fixtures/fake-docker.sh",
            env!("CARGO_MANIFEST_DIR")
        );
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

    // ─── rewrite_args_for_container tests ────────────────────────

    #[test]
    fn test_rewrite_split_form_under_models_dir() {
        let models_dir = Path::new("/models");
        let args = vec!["--model".to_string(), "/models/gguf/model.gguf".to_string()];
        let result = rewrite_args_for_container(&args, models_dir, "/container-models").unwrap();
        assert_eq!(result, vec!["--model", "/container-models/gguf/model.gguf"]);
    }

    #[test]
    fn test_rewrite_joined_form_under_models_dir() {
        let models_dir = Path::new("/models");
        let args = vec!["--model=/models/gguf/model.gguf".to_string()];
        let result = rewrite_args_for_container(&args, models_dir, "/container-models").unwrap();
        assert_eq!(result, vec!["--model=/container-models/gguf/model.gguf"]);
    }

    #[test]
    fn test_rewrite_bare_positional_path_under_models_dir() {
        // vLLM passes the model path as the first positional arg (no flag prefix).
        let models_dir = Path::new("/mnt/models");
        let args = vec!["/mnt/models/Qwen/Qwen3.6-27B-FP8".to_string()];
        let result = rewrite_args_for_container(&args, models_dir, "/models").unwrap();
        assert_eq!(result, vec!["/models/Qwen/Qwen3.6-27B-FP8"]);
    }

    #[test]
    fn test_rewrite_non_path_arg_passthrough() {
        let models_dir = Path::new("/models");
        let args = vec![
            "--help".to_string(),
            "--threads".to_string(),
            "4".to_string(),
            "--model".to_string(),
            "/models/gguf/model.gguf".to_string(),
        ];
        let result = rewrite_args_for_container(&args, models_dir, "/container-models").unwrap();
        assert_eq!(result[0], "--help");
        assert_eq!(result[1], "--threads");
        assert_eq!(result[2], "4");
        assert_eq!(result[3], "--model");
        assert_eq!(result[4], "/container-models/gguf/model.gguf");
    }

    #[test]
    fn test_rewrite_container_path_passthrough() {
        // A path that already lives inside the container (under container_model_path)
        // is a valid container path and must be passed through unchanged rather
        // than rejected as "outside the models directory".
        // e.g. --chat-template '/models/templates/...' where /models is the mount.
        let models_dir = Path::new("/mnt/models");
        let args = vec![
            "--chat-template".to_string(),
            "/models/templates/froggeric/Qwen-Fixed-Chat-Templates/chat_template.jinja".to_string(),
        ];
        let result = rewrite_args_for_container(&args, models_dir, "/models").unwrap();
        assert_eq!(
            result,
            vec![
                "--chat-template",
                "/models/templates/froggeric/Qwen-Fixed-Chat-Templates/chat_template.jinja"
            ]
        );
    }

    #[test]
    fn test_rewrite_path_outside_models_dir_error() {
        let models_dir = Path::new("/models");
        let args = vec!["--config".to_string(), "/etc/config.yaml".to_string()];
        let result = rewrite_args_for_container(&args, models_dir, "/container-models");
        assert!(result.is_err());
    }

    #[test]
    fn test_rewrite_joined_form_outside_models_dir_error() {
        let models_dir = Path::new("/models");
        let args = vec!["--config=/etc/config.yaml".to_string()];
        let result = rewrite_args_for_container(&args, models_dir, "/container-models");
        assert!(result.is_err());
    }

    #[test]
    fn test_rewrite_strips_quotes() {
        let models_dir = Path::new("/models");
        // Simulate shlex-quoted path
        let args = vec![
            "--model".to_string(),
            "\"/models/gguf/model.gguf\"".to_string(),
        ];
        let result = rewrite_args_for_container(&args, models_dir, "/container-models").unwrap();
        assert_eq!(result, vec!["--model", "/container-models/gguf/model.gguf"]);
    }

    #[test]
    fn test_rewrite_no_rewrite_needed() {
        // Path that doesn't start with / — should pass through without error
        let models_dir = Path::new("/models");
        let args = vec!["--model".to_string(), "relative/path".to_string()];
        let result = rewrite_args_for_container(&args, models_dir, "/container-models").unwrap();
        assert_eq!(result, vec!["--model", "relative/path"]);
    }

    // ─── resolve_volumes tests ───────────────────────────────────

    #[test]
    fn test_resolve_volumes_valid() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("models");
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();

        let config = DockerConfig {
            image: "test-image:latest".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: models_dir.to_str().unwrap().to_string(),
                container_path: "/models".to_string(),
                read_only: true,
            },
            volumes: vec![DockerVolume {
                host_path: data_dir.to_str().unwrap().to_string(),
                container_path: "/data".to_string(),
                read_only: false,
            }],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };

        let volumes = resolve_volumes(&config, &models_dir).unwrap();
        assert_eq!(volumes.len(), 2);
        assert!(volumes[0].contains("/models"));
        assert!(volumes[0].ends_with(":ro"));
        assert!(volumes[1].contains("/data"));
        assert!(!volumes[1].contains(":ro"));
    }

    #[test]
    fn test_resolve_volumes_missing_host_path_error() {
        let config = DockerConfig {
            image: "test-image:latest".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: "/nonexistent/path".to_string(),
                container_path: "/models".to_string(),
                read_only: false,
            },
            volumes: vec![],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };

        let result = resolve_volumes(&config, Path::new("/models"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_resolve_volumes_model_dir_substitution() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("shared-models");
        std::fs::create_dir_all(&models_dir).unwrap();

        let config = DockerConfig {
            image: "test-image:latest".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: "{{MODEL_DIR}}".to_string(),
                container_path: "/models".to_string(),
                read_only: true,
            },
            volumes: vec![],
            devices: vec![],
            gpus: None,
            shm_size: None,
            cap_adds: vec![],
            security_opts: vec![],
            group_adds: vec![],
        };

        let volumes = resolve_volumes(&config, &models_dir).unwrap();
        assert_eq!(volumes.len(), 1);
        assert!(volumes[0].starts_with(models_dir.to_str().unwrap()));
        assert!(volumes[0].contains(":/models:ro"));
    }

    // ─── resolve_group_gids tests ────────────────────────────────

    #[tokio::test]
    async fn test_resolve_group_gids_present_group() {
        // "root" group always exists on Linux systems — GID 0.
        let gids = resolve_group_gids(&["root".to_string()]).await;
        assert_eq!(gids, vec!["0"]);
    }

    #[tokio::test]
    async fn test_resolve_group_gids_missing_group_skipped() {
        let gids = resolve_group_gids(&["nonexistent_group_xyz".to_string()]).await;
        assert!(gids.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_group_gids_mixed() {
        // root exists, nonexistent doesn't — should only return root's GID
        let gids =
            resolve_group_gids(&["root".to_string(), "nonexistent_group_xyz".to_string()]).await;
        assert_eq!(gids, vec!["0"]);
    }

    // ─── Container lifecycle tests (require fake-docker.sh updates) ─

    #[tokio::test]
    async fn test_stop_container_no_such_container_ok() {
        let _guard = setup_fake_docker();
        // This will fail because fake-docker doesn't have stop yet — placeholder
        // When fake-docker supports stop, this should pass
        let result = stop_container("nonexistent-container").await;
        assert!(
            result.is_ok(),
            "stop_container should tolerate missing container"
        );
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

    #[tokio::test]
    async fn test_inspect_container_no_such_container_returns_none() {
        let _guard = setup_fake_docker();
        let result = inspect_container("nonexistent-container").await;
        assert!(
            result.is_ok(),
            "inspect should succeed (return None) for missing container, got: {:?}",
            result
        );
        assert!(
            result.unwrap().is_none(),
            "inspect should return None for missing container"
        );
    }

    #[tokio::test]
    async fn test_spawn_container_builds_correct_command() {
        let _guard = setup_fake_docker();

        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("models");
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::create_dir_all(&data_dir).unwrap();

        let config = DockerConfig {
            image: "stilldeadcode/vllm-radiance:0.5.8".to_string(),
            container_port: 8000,
            model_mount: DockerVolume {
                host_path: models_dir.to_str().unwrap().to_string(),
                container_path: "/models".to_string(),
                read_only: true,
            },
            volumes: vec![DockerVolume {
                host_path: data_dir.to_str().unwrap().to_string(),
                container_path: "/data".to_string(),
                read_only: false,
            }],
            devices: vec!["/dev/nvidia0".to_string()],
            gpus: Some("all".to_string()),
            shm_size: Some("2G".to_string()),
            cap_adds: vec!["SYS_PTRACE".to_string()],
            security_opts: vec!["no-new-privileges".to_string()],
            group_adds: vec!["docker".to_string()], // docker group usually exists on Linux
        };

        let args = vec![
            "--model".to_string(),
            models_dir
                .join("gguf/model.gguf")
                .to_str()
                .unwrap()
                .to_string(),
            "--threads".to_string(),
            "4".to_string(),
        ];
        let env_vars = vec!["HF_HUB_ENABLE_HF_TRANSFER=1".to_string()];

        // Rewrite paths for container
        let rewritten_args =
            rewrite_args_for_container(&args, &models_dir, "/container-models").unwrap();

        // Verify path rewriting worked
        let expected_path = "/container-models/gguf/model.gguf".to_string();
        assert_eq!(rewritten_args.len(), 4);
        assert_eq!(rewritten_args[0], "--model");
        assert_eq!(rewritten_args[1], expected_path);
        assert_eq!(rewritten_args[2], "--threads");
        assert_eq!(rewritten_args[3], "4");

        // Now spawn the container (will use fake docker)
        let result = spawn_container(
            "test-backend",
            &config,
            18910,
            rewritten_args,
            env_vars,
            &models_dir,
        )
        .await;

        let container = result.unwrap();
        assert_eq!(container.name, "tama-test-backend");
        assert!(!container.id.is_empty());
    }
}
