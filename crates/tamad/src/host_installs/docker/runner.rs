//! Docker container execution (plan-191 Task 10 restore).
//!
//! The tamad spawns *native host binaries* for most backends. Docker-backed
//! engines (e.g. `stilldeadcode/vllm-radiance`, which is a vLLM container)
//! are recovered here: pull the image if missing, then `docker run` with
//! the volume/device/shm/capability config shipped from the proxy in
//! `LoadModelRequest.docker_config_json`. The proxy owns the central DB, so
//! it sends the `DockerConfig` down; the tamad owns the host and executes.
//!
//! Containers use deterministic names (`tama-{model_name}`) with a managed
//! label so the startup reconcile can reap stragglers (plan-080 / ADR-0010).

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tokio::process::Command;

use super::runtime::ContainerRuntime;
use tama_core::installations::{DockerConfig, DockerVolume};

/// A spawned Docker container.
#[derive(Debug)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub struct DockerInspect {
    #[serde(rename = "Id")]
    pub id: Option<String>,
    #[serde(rename = "State")]
    pub state: InspectState,
    #[serde(rename = "NetworkSettings", default)]
    pub network: InspectNetwork,
}

/// The `State` block from docker inspect.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct InspectState {
    pub running: Option<bool>,
    #[serde(rename = "Pid")]
    pub pid: Option<u64>,
}

/// The `NetworkSettings` block from docker inspect.
#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct InspectNetwork {
    #[serde(rename = "Ports", default)]
    pub ports: Option<serde_json::Value>,
}

// ─── Path Rewriting ──────────────────────────────────────────────

/// Rewrite host paths in args to container paths when they fall under the
/// models dir (mounted at `container_model_path` inside the container).
///
/// Only paths under the models_dir are rewritten. Non-model absolute paths
/// are rejected (they can't be reached inside the container); relative
/// paths and plain flags pass through unchanged.
pub fn rewrite_args_for_container(
    args: &[String],
    models_dir: &Path,
    container_model_path: &str,
) -> Result<Vec<String>> {
    let mut result = Vec::with_capacity(args.len());

    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let unquoted = arg.trim_matches('"').trim_matches('\'');

        if let Some(eq_pos) = unquoted.find('=') {
            let flag = &unquoted[..eq_pos];
            let value = &unquoted[eq_pos + 1..];
            if value.starts_with('/') && flag.starts_with("--") {
                match maybe_rewrite_path(value, models_dir, container_model_path)? {
                    Some(rewritten) => {
                        result.push(format!("{}={}", flag, rewritten));
                        continue;
                    }
                    None => {
                        result.push(arg.clone());
                        continue;
                    }
                }
            }
            result.push(arg.clone());
            continue;
        }

        if arg.starts_with('-') {
            if let Some(next) = iter.peek() {
                let next_str = *next;
                let next_unquoted = next_str.trim_matches('"').trim_matches('\'');
                if next_unquoted.starts_with('/') {
                    match maybe_rewrite_path(next_unquoted, models_dir, container_model_path)? {
                        Some(rewritten) => {
                            iter.next();
                            result.push(arg.clone());
                            result.push(rewritten);
                            continue;
                        }
                        None => {
                            result.push(arg.clone());
                            continue;
                        }
                    }
                }
            }
        }

        // Check for positional path arg (e.g. vLLM model path `/models/org/repo`)
        if unquoted.starts_with('/') {
            if let Some(rewritten) = maybe_rewrite_path(unquoted, models_dir, container_model_path)?
            {
                result.push(rewritten);
                continue;
            }
        }

        result.push(arg.clone());
    }

    Ok(result)
}

/// Rewrite a host path under `models_dir` into the container path.
/// If the path is already under `container_model_path` or is another
/// container-internal path, it passes through untouched.
fn maybe_rewrite_path(
    path: &str,
    models_dir: &Path,
    container_model_path: &str,
) -> Result<Option<String>> {
    let p = Path::new(path);
    if let Ok(relative) = p.strip_prefix(models_dir) {
        let rewritten = format!(
            "{}/{}",
            container_model_path.trim_end_matches('/'),
            relative.display()
        );
        return Ok(Some(rewritten));
    }
    // Path doesn't match host models_dir — leave as-is for container-internal paths
    Ok(None)
}

// ─── Volume Resolution ───────────────────────────────────────────

/// Resolve the volume mounts for a `docker run`.
///
/// Substitutes `{{MODEL_DIR}}` -> `models_dir` in `host_path`, validates
/// the host paths exist, and returns `host:container[:ro]` mount specs.
pub fn resolve_volumes(config: &DockerConfig, models_dir: &Path) -> Result<Vec<String>> {
    let mut volumes = Vec::new();
    volumes.push(format_volume(&config.model_mount, models_dir)?);
    for vol in &config.volumes {
        volumes.push(format_volume(vol, models_dir)?);
    }
    Ok(volumes)
}

fn format_volume(vol: &DockerVolume, models_dir: &Path) -> Result<String> {
    let host = vol.host_path.replace(
        "{{MODEL_DIR}}",
        models_dir
            .to_str()
            .ok_or_else(|| anyhow!("models_dir contains invalid UTF-8"))?,
    );
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

/// Resolve group names to GIDs (skipping missing groups with a warning).
pub async fn resolve_group_gids(group_names: &[String]) -> Vec<String> {
    // Dirt-simple: map the well-known groups directly, else getent.
    let mut found = Vec::with_capacity(group_names.len());
    for name in group_names {
        match getent_gid(name).await {
            Ok(gid) => found.push(gid),
            Err(e) => {
                tracing::warn!(group = %name, error = %e, "could not resolve group GID");
            }
        }
    }
    found
}

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

// ─── Image Management ────────────────────────────────────────────

/// Whether a Docker image is already present locally.
pub async fn is_image_present(runtime: ContainerRuntime, image: &str) -> Result<bool> {
    let output = Command::new(runtime.command())
        .arg("image")
        .arg("inspect")
        .arg(image)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if output.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such image") || stderr.contains("not found") {
        return Ok(false);
    }
    Err(anyhow!("docker image inspect failed: {}", stderr.trim()))
}

/// Pull a Docker image (blocking, bounded by `timeout_secs`).
pub async fn pull_image(runtime: ContainerRuntime, image: &str, timeout_secs: u64) -> Result<()> {
    let mut child = Command::new(runtime.command())
        .arg("pull")
        .arg(image)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let result = tokio::select! {
        _ = tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)) => {
            let _ = child.kill().await;
            Err(anyhow!("docker pull timed out after {} seconds", timeout_secs))
        }
        r = child.wait() => {
            if r?.success() {
                Ok(())
            } else {
                Err(anyhow!("docker pull failed for '{}'", image))
            }
        }
    };
    result
}

// ─── Container Lifecycle ─────────────────────────────────────────

/// Deterministic container name for a model (used by spawn, stop, remove).
pub fn container_name_for(model_name: &str) -> String {
    format!("tama-{}", model_name)
}

/// Spawn a Docker container with the given configuration.
///
/// Builds and executes `docker run` with all flags from the config. Returns
/// a `DockerContainer` (name, id, pid).
pub async fn spawn_container(
    runtime: ContainerRuntime,
    model_name: &str,
    config: &DockerConfig,
    host_port: u16,
    args: Vec<String>,
    env_vars: &[String],
    models_dir: &Path,
) -> Result<DockerContainer> {
    let container_name = container_name_for(model_name);

    // Clean up any existing container with this name first
    let _ = remove_container(runtime, &container_name).await;

    let mut cmd = Command::new(runtime.command());
    cmd.arg("run");
    cmd.arg("-d");
    cmd.arg("--name").arg(&container_name);
    cmd.arg("--label").arg("tama.managed=true");
    cmd.arg("-p")
        .arg(format!("127.0.0.1:{}:{}", host_port, config.container_port));

    let volumes = resolve_volumes(config, models_dir)?;
    for vol in &volumes {
        cmd.arg("-v").arg(vol);
    }
    for device in &config.devices {
        cmd.arg("--device").arg(device);
    }
    if let Some(gpus) = &config.gpus {
        cmd.arg("--gpus").arg(gpus);
    }
    if let Some(shm) = &config.shm_size {
        cmd.arg("--shm-size").arg(shm);
    }
    for cap in &config.cap_adds {
        cmd.arg("--cap-add").arg(cap);
    }
    for opt in &config.security_opts {
        cmd.arg("--security-opt").arg(opt);
    }
    let gids = resolve_group_gids(&config.group_adds).await;
    for gid in &gids {
        cmd.arg("--group-add").arg(gid);
    }
    for env in env_vars {
        cmd.arg("-e").arg(env);
    }

    cmd.arg(&config.image);
    for arg in args {
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

    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Best-effort cleanup for every hard error raised AFTER a successful
    // run below: the freshly created container is live with a bound host
    // port, so leaking it would pin the port until the daemon reaps it.
    // The host PID must be present AND non-zero: `kill_process_group(0)`
    // would signal tamad's OWN process group (kill(-0) == kill(0)), so a
    // missing/zero inspect PID is a hard error, never a silent 0.
    let inspected = match inspect_container(runtime, &id).await {
        Ok(v) => v,
        Err(e) => {
            let _ = stop_container(runtime, &id).await;
            let _ = remove_container(runtime, &id).await;
            return Err(e.context("inspecting spawned container for its host PID"));
        }
    };
    let inspected = match inspected {
        Some(v) => v,
        None => {
            let _ = stop_container(runtime, &id).await;
            let _ = remove_container(runtime, &id).await;
            anyhow::bail!("container '{container_name}' vanished immediately after run");
        }
    };
    let pid = inspected.state.pid.map(|p| p as u32).filter(|p| *p != 0);
    let Some(pid) = pid else {
        let _ = stop_container(runtime, &id).await;
        let _ = remove_container(runtime, &id).await;
        anyhow::bail!("docker did not report a usable host PID for container '{container_name}'");
    };

    Ok(DockerContainer {
        name: container_name,
        id,
        pid,
    })
}

/// Stop a Docker container. Tolerates "No such container".
pub async fn stop_container(runtime: ContainerRuntime, name: &str) -> Result<()> {
    let output = Command::new(runtime.command())
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

/// Inspect a container, returning parsed state (None when absent).
pub async fn inspect_container(
    runtime: ContainerRuntime,
    name: &str,
) -> Result<Option<DockerInspect>> {
    let output = Command::new(runtime.command())
        .arg("inspect")
        .arg(name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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

/// Remove a Docker container. Tolerates "No such container".
pub async fn remove_container(runtime: ContainerRuntime, name: &str) -> Result<()> {
    let output = Command::new(runtime.command())
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

// ─── Container Logs ────────────────────────────────────────────

/// Build the `docker logs` argument vector for tailing a container's
/// engine log. Extracted so the exact CLI can be unit-tested without
/// executing docker.
pub fn logs_tail_args(container_name: &str, max_lines: usize) -> Vec<String> {
    vec![
        "logs".to_string(),
        "--tail".to_string(),
        max_lines.to_string(),
        container_name.to_string(),
    ]
}

/// Build the `docker logs` argument vector for FOLLOWING a container's
/// engine log (plan-195 task 6): `-f` blocks until EOF, `-t` prefixes
/// each line with its RFC3339 capture time (parsed by the tail
/// supervisor, `crates/tamad/src/push/tails.rs`). New alongside the
/// one-shot `logs_tail_args` above; the one-shot legacy form keeps its
/// shape (without `-t`) — no behavioral cosmetic difference there: the
/// legacy `Logs` RPC is unchanged.
pub fn logs_follow_args(container_name: &str) -> Vec<String> {
    vec![
        "logs".to_string(),
        "-f".to_string(),
        "-t".to_string(),
        container_name.to_string(),
    ]
}

/// Tail the last `max_lines` lines of a container's logs.
///
/// Runs `docker logs --tail <n> <name>`. A non-zero exit
/// (e.g. "No such container") is an error carrying docker's stderr so the
/// caller can log what actually happened; the server's `logs` handler
/// degrades that to an empty stream.
pub async fn tail_container_logs(
    runtime: ContainerRuntime,
    container_name: &str,
    max_lines: usize,
) -> Result<Vec<String>> {
    let output = Command::new(runtime.command())
        .args(logs_tail_args(container_name, max_lines))
        .output()
        .await
        .with_context(|| format!("failed to spawn `docker logs` for '{container_name}'"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "docker logs failed for '{}': {}",
            container_name,
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::server::test_support::{
        fake_docker_config_for_tests, guarded_fake_docker, restore_docker_path,
    };

    #[tokio::test]
    async fn test_remove_container_no_such_container_ok() {
        let (_tmpdir, _dir, original, _guard) = guarded_fake_docker().await;
        let result = remove_container(ContainerRuntime::default(), "nonexistent-container").await;
        restore_docker_path(&original);
        assert!(result.is_ok(), "remove should tolerate missing container");
    }

    #[tokio::test]
    async fn test_stop_container_no_such_container_ok() {
        let (_tmpdir, _dir, original, _guard) = guarded_fake_docker().await;
        let result = stop_container(ContainerRuntime::default(), "nonexistent-container").await;
        restore_docker_path(&original);
        assert!(result.is_ok(), "stop should tolerate missing container");
    }

    #[tokio::test]
    async fn test_inspect_container_no_such_container_returns_none() {
        let (_tmpdir, _dir, original, _guard) = guarded_fake_docker().await;
        let result = inspect_container(ContainerRuntime::default(), "nonexistent-container").await;
        restore_docker_path(&original);
        assert!(
            result.is_ok(),
            "inspect should succeed for missing container"
        );
        assert!(result.unwrap().is_none(), "missing container -> None");
    }

    /// The exact `docker logs` CLI: --tail with the line count, then the
    /// container name (last arg).
    #[test]
    fn test_logs_tail_args_vector() {
        assert_eq!(
            logs_tail_args("tama-model-a", 200),
            vec!["logs", "--tail", "200", "tama-model-a"]
        );
    }

    /// The follow form (plan-195 task 6) is the streaming tail used by
    /// the engine-tail supervisor: `-f -t <container>`.
    #[test]
    fn test_logs_follow_args_vector() {
        assert_eq!(
            logs_follow_args("tama-model-a"),
            vec!["logs", "-f", "-t", "tama-model-a"]
        );
    }

    /// Tailing a running (fake) container returns its log lines in order.
    #[tokio::test]
    async fn test_tail_container_logs_returns_lines() {
        let (_tmpdir, _dir, original, _guard) = guarded_fake_docker().await;

        // Create the container via the same spawn path the lifecycle uses.
        let models_dir = tempfile::tempdir().unwrap();
        let config = fake_docker_config_for_tests();
        let container = spawn_container(
            ContainerRuntime::default(),
            "model-a",
            &config,
            18099,
            vec![],
            &[],
            models_dir.path(),
        )
        .await
        .expect("fake container spawn");
        assert_eq!(container.name, "tama-model-a");

        let lines = tail_container_logs(ContainerRuntime::default(), &container.name, 200)
            .await
            .expect("tail must succeed for a live container");
        restore_docker_path(&original);

        assert_eq!(
            lines,
            vec![
                "[fake-docker] Container tama-model-a logs",
                "[fake-docker] Starting backend on port 8000",
                "[fake-docker] Model loaded successfully",
            ]
        );
    }

    /// Tailing a container that was never created is an error (not an
    /// empty success) — the server's logs handler turns this into an
    /// empty stream.
    #[tokio::test]
    async fn test_tail_container_logs_missing_container_errors() {
        let (_tmpdir, _dir, original, _guard) = guarded_fake_docker().await;
        let err = tail_container_logs(ContainerRuntime::default(), "tama-ghost", 200)
            .await
            .expect_err("missing container must error");
        restore_docker_path(&original);
        assert!(
            err.to_string().contains("No such container"),
            "stderr context must be preserved: {err}"
        );
    }

    #[tokio::test]
    async fn test_resolve_group_gids_root() {
        let gids = resolve_group_gids(&["root".to_string()]).await;
        assert_eq!(gids, vec!["0"]);
    }

    #[tokio::test]
    async fn test_resolve_group_gids_missing_skipped() {
        let gids = resolve_group_gids(&["nonexistent_group_xyz".to_string()]).await;
        assert!(gids.is_empty());
    }

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
    fn test_rewrite_container_internal_path_passthrough() {
        let models_dir = Path::new("/mnt/models");
        let args = vec![
            "--chat-template".to_string(),
            "/models/templates/chat_template.jinja".to_string(),
        ];
        let result = rewrite_args_for_container(&args, models_dir, "/models").unwrap();
        assert_eq!(
            result,
            vec!["--chat-template", "/models/templates/chat_template.jinja"]
        );
    }

    #[test]
    fn test_rewrite_positional_model_path_under_models_dir() {
        let models_dir = Path::new("/mnt/models");
        let args = vec![
            "/mnt/models/Qwen/Qwen3.8-27B-FP8".to_string(),
            "--port".to_string(),
            "8000".to_string(),
        ];
        let result = rewrite_args_for_container(&args, models_dir, "/models").unwrap();
        assert_eq!(
            result,
            vec!["/models/Qwen/Qwen3.8-27B-FP8", "--port", "8000"]
        );
    }

    #[test]
    fn test_resolve_volumes_model_dir_substitution() {
        let temp_dir = tempfile::tempdir().unwrap();
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
}
