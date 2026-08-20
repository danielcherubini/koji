//! Tamad-side lifecycle: spawn/health/unload/restart of backend processes
//! (plan-191 Task 5).
//!
//! Thin orchestrator over the local `process` module (plan-191 Task 10: moved
//! Tamad is a dumb executor (ADR-0010): it spawns whatever fully-resolved
//! launch spec the proxy sends in `LoadModelRequest`, health-polls it, and
//! records the process in the in-memory [`ProcessTable`]. No database, no
//! model registry.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use tracing::{debug, info, warn};

use crate::process::{
    configure_backend_command, configure_process_group, force_kill_process_group,
    is_process_group_alive, kill_process_group, wait_group_dead,
};
use tama_core::tamad::{LoadModelRequest, LoadModelResponse, ProcessInfo, ProviderInfo};

use crate::process_table::{ProcessEntry, ProcessTable};
use crate::state::TamadState;

/// Env key the proxy uses to carry the *proxy's* models directory in the
/// launch spec. The tamad rewrites any arg that references it (a path
/// prefix under the proxy's models dir) to its own `models_dir` so the
/// same spec works when proxy and tamad host the weights in different
/// places. The key is stripped before spawning.
pub const PROXY_MODELS_DIR_ENV: &str = "TAMA_MODELS_DIR";

/// Tamad-side lifecycle over the process table.
pub struct TamadLifecycle {
    /// In-memory table of spawned backend processes.
    pub table: Arc<ProcessTable>,
    /// Runtime state (models_dir for path remapping).
    pub state: Arc<TamadState>,
}

impl TamadLifecycle {
    /// Create a lifecycle backed by `table` and `state`.
    pub fn new(table: Arc<ProcessTable>, state: Arc<TamadState>) -> Self {
        Self { table, state }
    }

    /// Spawn the backend described by `req`, health-poll until success or
    /// timeout, and record the process in the table.
    ///
    /// - `health_timeout_ms == 0` or empty `health_url` → the process is
    ///   considered ready immediately (no health polling).
    /// - `provider_name == "compaction"` → the proxy ships the generic
    ///   `uv run uvicorn ...` shape and this tamad injects its own embedded
    ///   server directory (`--project`), because the Python source is
    ///   bundled in this binary (plan-191 Task 10).
    /// - On timeout the process group is killed and a `failed` entry is
    ///   recorded before the error is returned.
    pub async fn load(&self, req: &LoadModelRequest) -> Result<LoadModelResponse> {
        let (args, env) = self.resolve_launch(req).await?;

        // Docker-backed engines (e.g. vLLM-radiance): the proxy shipped a
        // DockerConfig in `docker_config_json`; spawn a container instead of
        // a host binary (plan-080 style runner restored in tamad).
        if !req.docker_config_json.is_empty() {
            return self.load_container(req, args, env).await;
        }

        info!(
            model = %req.model_name,
            command = %req.command,
            "spawning backend process"
        );

        let mut command = tokio::process::Command::new(&req.command);
        command.args(&args);
        for (key, value) in &env {
            command.env(key, value);
        }
        // Same isolation as the proxy's former native path: companion .so
        // resolution next to the binary + own process group (so unload can
        // SIGTERM the whole tree).
        let binary_path = std::path::PathBuf::from(&req.command);
        configure_backend_command(&mut command, binary_path.as_path());
        configure_process_group(&mut command);
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to spawn backend '{}' for model '{}'",
                req.command, req.model_name
            )
        })?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow!("failed to get PID for model '{}'", req.model_name))?;
        // Reap task: wait for the child to exit and mark the entry failed.
        // The tamad owns the process, so it must reap it — otherwise a
        // crashed backend lingers as a zombie that still answers
        // kill(pid, 0), and the proxy's reconciler would never see it
        // as dead (and never restart it).
        {
            let table = Arc::clone(&self.table);
            let model_name = req.model_name.clone();
            tokio::spawn(async move {
                let _ = child.wait().await;
                table.mark_failed(&model_name, pid).await;
            });
        }

        info!(model = %req.model_name, pid, "backend process spawned");

        // Health polling.
        let timeout = Duration::from_millis(req.health_timeout_ms.max(0) as u64);
        let healthy = if req.health_url.is_empty() || req.health_timeout_ms == 0 {
            // No health check requested → immediately ready.
            true
        } else {
            self.wait_for_health(&req.health_url, timeout).await
        };

        if !healthy {
            warn!(
                model = %req.model_name,
                timeout_ms = timeout.as_millis() as u64,
                "backend failed to become healthy; killing process group"
            );
            let _ = kill_process_group(pid).await;
            tokio::time::sleep(Duration::from_millis(250)).await;
            if is_process_group_alive(pid) {
                let _ = force_kill_process_group(pid).await;
            }
            // Record the failed attempt (status "failed") so the stats
            // stream shows it until the next load replaces it.
            self.table
                .insert(ProcessEntry {
                    model_name: req.model_name.clone(),
                    provider_name: req.provider_name.clone(),
                    pid,
                    endpoint_url: Self::endpoint_from_health_url(&req.health_url),
                    status: "failed".to_string(),
                    started_at: Instant::now(),
                    spec: req.clone(),
                })
                .await;
            return Err(anyhow!(
                "backend '{}' for model '{}' failed to become healthy within {}ms",
                req.provider_name,
                req.model_name,
                req.health_timeout_ms
            ));
        }

        let endpoint_url = Self::endpoint_from_health_url(&req.health_url);
        self.table
            .insert(ProcessEntry {
                model_name: req.model_name.clone(),
                provider_name: req.provider_name.clone(),
                pid,
                endpoint_url: endpoint_url.clone(),
                status: "ready".to_string(),
                started_at: Instant::now(),
                spec: req.clone(),
            })
            .await;

        Ok(LoadModelResponse {
            endpoint_url,
            pid: pid as i32,
            status: "ready".to_string(),
        })
    }

    /// Spawn a Docker-backed backend (container) and health-poll it to ready.
    ///
    /// The proxy ships a serialized [`DockerConfig`] in `req.docker_config_json`
    /// (the tamad owns no DB). We pull the image if missing, rewrite the
    /// already path-remapped args to the container's mounted model dir, then
    /// `docker run` with the mount/device/shm/capability config. On timeout or
    /// health failure the container is stopped+removed and a `failed` entry is
    /// recorded in the table.
    async fn load_container(
        &self,
        req: &LoadModelRequest,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<LoadModelResponse> {
        let config =
            serde_json::from_str::<tama_core::installations::DockerConfig>(&req.docker_config_json)
                .map_err(|e| anyhow!("invalid docker_config_json: {}", e))?;

        // Image presence: pull on first load (images can be large — allow a
        // generous timeout). Fail when the host genuinely can't fetch it.
        if !crate::host_installs::docker::runner::is_image_present(&config.image).await? {
            info!(
                model = %req.model_name,
                image = %config.image,
                "pulling docker image"
            );
            crate::host_installs::docker::runner::pull_image(&config.image, 1800)
                .await
                .with_context(|| format!("pulling docker image '{}'", config.image))?;
        }

        let local_models = self.state.models_dir.clone();
        let container_models = config.model_mount.container_path.clone();
        let container_args = crate::host_installs::docker::runner::rewrite_args_for_container(
            &args,
            &local_models,
            &container_models,
        )?;

        // Host-side port: the proxy aliases it into args (`--port <n>`) and
        // the health URL (`http://127.0.0.1:<n>/health`). Reuse the health URL
        // port so what we spawn maps to what the proxy health-checks.
        let host_port =
            Self::port_from_health_url(&req.health_url).unwrap_or(config.container_port);

        let env_strs: Vec<String> = env
            .into_iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        info!(
            model = %req.model_name,
            image = %config.image,
            host_port,
            container_port = config.container_port,
            "spawning backend container"
        );

        let container = crate::host_installs::docker::runner::spawn_container(
            &req.model_name,
            &config,
            host_port,
            container_args,
            &env_strs,
            &local_models,
        )
        .await
        .with_context(|| {
            format!(
                "failed to spawn container '{}' for model '{}'",
                config.image, req.model_name
            )
        })?;

        let pid = container.pid;

        // Health polling (same contract as the native path).
        let timeout = Duration::from_millis(req.health_timeout_ms.max(0) as u64);
        let healthy = if req.health_url.is_empty() || req.health_timeout_ms == 0 {
            true
        } else {
            self.wait_for_health(&req.health_url, timeout).await
        };

        if !healthy {
            warn!(
                model = %req.model_name,
                "container failed to become healthy; tearing down"
            );
            let _ = crate::host_installs::docker::runner::stop_container(&container.name).await;
            let _ = crate::host_installs::docker::runner::remove_container(&container.name).await;
            self.table
                .insert(ProcessEntry {
                    model_name: req.model_name.clone(),
                    provider_name: req.provider_name.clone(),
                    pid,
                    endpoint_url: Self::endpoint_from_health_url(&req.health_url),
                    status: "failed".to_string(),
                    started_at: Instant::now(),
                    spec: req.clone(),
                })
                .await;
            return Err(anyhow!(
                "container for model '{}' failed to become healthy within {}ms",
                req.model_name,
                req.health_timeout_ms
            ));
        }

        let endpoint_url = Self::endpoint_from_health_url(&req.health_url);
        self.table
            .insert(ProcessEntry {
                model_name: req.model_name.clone(),
                provider_name: req.provider_name.clone(),
                pid,
                endpoint_url: endpoint_url.clone(),
                status: "ready".to_string(),
                started_at: Instant::now(),
                spec: req.clone(),
            })
            .await;

        Ok(LoadModelResponse {
            endpoint_url,
            pid: pid as i32,
            status: "ready".to_string(),
        })
    }

    /// Extract the host-side port from a health URL like
    /// `http://127.0.0.1:8080/health`. Falls back to None.
    fn port_from_health_url(url: &str) -> Option<u16> {
        if url.is_empty() {
            return None;
        }
        url::Url::parse(url).ok().and_then(|u| u.port())
    }

    /// Kill the process group for `model_name` and remove the entry.
    ///
    /// Returns an error when the model is unknown to this tamad.
    pub async fn unload(&self, model_name: &str) -> Result<()> {
        let entry = self
            .table
            .remove(model_name)
            .await
            .ok_or_else(|| anyhow!("model '{}' is not loaded on this tamad", model_name))?;

        info!(model = %model_name, pid = entry.pid, "unloading backend process");

        // Docker backend: the "pid" is the container's host process. Kill it
        // and also stop+remove the managed container so it doesn't linger or
        // auto-restart.
        if !entry.spec.docker_config_json.is_empty() {
            let _ = kill_process_group(entry.pid).await;
            let name = format!("tama-{}", model_name);
            let _ = crate::host_installs::docker::runner::stop_container(&name).await;
            let _ = crate::host_installs::docker::runner::remove_container(&name).await;
            info!(model = %model_name, "docker backend container unloaded");
            return Ok(());
        }

        let _ = kill_process_group(entry.pid).await;

        // SIGTERM → wait up to 5s → SIGKILL.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !is_process_group_alive(entry.pid) {
                break;
            }
            if Instant::now() >= deadline {
                warn!(model = %model_name, pid = entry.pid, "SIGTERM ignored; sending SIGKILL");
                let _ = force_kill_process_group(entry.pid).await;
                // Reap it properly — a lingering zombie would keep
                // answering `kill(pid, 0)`.
                if let Err(e) = wait_group_dead(entry.pid).await {
                    warn!(model = %model_name, pid = entry.pid, error = %e, "group not fully reaped after SIGKILL");
                }
                break;
            }
        }

        info!(model = %model_name, "backend process unloaded");
        Ok(())
    }

    /// Kill every loaded backend (their whole process groups): SIGTERM
    /// each, grace, SIGKILL escalation, then drop the entries — the
    /// in-memory inventory must not outlive the daemon (plan-191
    /// follow-up A: a SIGTERM to tamad leaves no orphaned backends).
    pub async fn kill_all(&self) -> Result<()> {
        let entries = self.table.list().await;
        for entry in entries {
            info!(
                model = %entry.model_name,
                pid = entry.pid,
                "kill_all: stopping backend process group"
            );
            // Reuse the graceful per-model path (SIGTERM → 5s → SIGKILL
            // → entry removal). One model's stall must not block the
            // rest — errors are logged, not returned.
            if let Err(e) = self.unload(&entry.model_name).await {
                warn!(
                    model = %entry.model_name,
                    error = %e,
                    "kill_all: unload failed; continuing with other backends"
                );
            }
        }
        Ok(())
    }

    /// Unload then re-load using the stored launch spec (the original
    /// `LoadModelRequest` that started the process).
    pub async fn restart(&self, model_name: &str) -> Result<LoadModelResponse> {
        let entry = self
            .table
            .get(model_name)
            .await
            .ok_or_else(|| anyhow!("model '{}' is not loaded on this tamad", model_name))?;
        let spec = entry.spec.clone();
        self.unload(model_name).await?;
        self.load(&spec).await
    }

    /// Group table entries by provider name.
    ///
    /// `engine`/`version`/`gpu_variant`/`status` are empty/"unknown": the
    /// tamad has no database — the proxy's DB is the source of truth for
    /// those fields.
    pub async fn list(&self) -> Vec<ProviderInfo> {
        let mut by_provider: std::collections::BTreeMap<String, Vec<ProcessInfo>> =
            std::collections::BTreeMap::new();
        for entry in self.table.list().await {
            let info = ProcessInfo {
                model_name: entry.model_name.clone(),
                provider_name: entry.provider_name.clone(),
                pid: entry.pid as i32,
                alive: entry.status != "failed" && crate::process::is_process_alive(entry.pid),
                endpoint_url: entry.endpoint_url.clone(),
                status: entry.status.clone(),
            };
            by_provider
                .entry(entry.provider_name)
                .or_default()
                .push(info);
        }
        by_provider
            .into_iter()
            .map(|(name, loaded_models)| ProviderInfo {
                name,
                engine: String::new(),
                version: String::new(),
                status: "unknown".to_string(),
                gpu_variant: String::new(),
                loaded_models,
            })
            .collect()
    }

    /// Health-poll `url` (200–399) every 500ms until `timeout`.
    async fn wait_for_health(&self, url: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            if start.elapsed() >= timeout {
                return false;
            }
            let healthy = crate::process::check_health(url, Some(5))
                .await
                .map(|resp| resp.status().is_success())
                .unwrap_or(false);
            if healthy {
                debug!(url, "health check passed");
                return true;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Resolve the spawn args/env for a request:
    /// 1. remap model paths from the proxy's models dir (carried in
    ///    `PROXY_MODELS_DIR_ENV`) to the tamad's own `models_dir`,
    /// 2. strip the helper env key before spawning,
    /// 3. for `provider_name == "compaction"`, inject the embedded compaction
    ///    server directory (`--project` after `run`).
    async fn resolve_launch(
        &self,
        req: &LoadModelRequest,
    ) -> Result<(Vec<String>, Vec<(String, String)>)> {
        let mut env: Vec<(String, String)> = Vec::new();
        let mut proxy_models_dir: Option<String> = None;
        for (key, value) in &req.env {
            if key == PROXY_MODELS_DIR_ENV {
                proxy_models_dir = Some(value.clone());
            } else {
                env.push((key.clone(), value.clone()));
            }
        }

        // GPU isolation env vars are resolved on THIS HOST against this
        // daemon's own hardware (the proxy sends the configured device +
        // variant — ADR-0010: it never samples local hardware). Explicit
        // entries from the installation's `default_env` win over the
        // resolved vendor var.
        let device = req.gpu_device.trim();
        if !device.is_empty() {
            if let Ok(variant) = req.gpu_variant.parse::<tama_core::gpu::GpuVariant>() {
                let dev = device.to_string();
                match tokio::task::spawn_blocking(move || {
                    crate::gpu::env::resolve_gpu_env(&dev, &variant)
                })
                .await
                {
                    Ok(Some((key, value))) => {
                        if !env.iter().any(|(k, _)| k == &key) {
                            info!(
                                model = %req.model_name,
                                env = %key,
                                value = %value,
                                "resolved GPU isolation env on this host"
                            );
                            env.push((key, value));
                        }
                    }
                    Ok(None) => {
                        debug!(
                            model = %req.model_name,
                            device,
                            variant = %req.gpu_variant,
                            "no GPU env var for this device/variant (no matching local GPU)"
                        );
                    }
                    Err(e) => {
                        warn!(
                            model = %req.model_name,
                            error = %e,
                            "GPU env resolution panicked; launching without isolation env"
                        );
                    }
                }
            } else {
                warn!(
                    model = %req.model_name,
                    variant = %req.gpu_variant,
                    "unknown gpu_variant; skipping GPU env resolution"
                );
            }
        }

        let mut args = req.args.clone();
        if let Some(ref proxy_dir) = proxy_models_dir {
            let local_dir = self.state.models_dir.to_string_lossy().to_string();
            if proxy_dir.as_str() != local_dir {
                let prefix = proxy_dir.trim_end_matches('/').to_string() + "/";
                args = args
                    .into_iter()
                    .map(|arg| Self::remap_path_prefix(&arg, &prefix, &local_dir))
                    .collect();
                info!(
                    "remapped model paths from '{}' to '{}'",
                    proxy_dir, local_dir
                );
            }
        }

        // Compaction: the Python server is embedded in this binary — inject
        // the `--project` dir the proxy cannot know about.
        if req.provider_name == "compaction" {
            let server_dir = crate::compaction_server::get_server_dir(&self.state.data_dir)
                .with_context(|| "resolving embedded compaction server dir")?;
            let project = server_dir.to_string_lossy().into_owned();
            let mut new_args = Vec::with_capacity(args.len() + 1);
            let mut injected = false;
            for arg in args.into_iter() {
                if !injected && arg == "run" {
                    new_args.push(arg);
                    new_args.push("--project".to_string());
                    new_args.push(project.clone());
                    injected = true;
                } else {
                    new_args.push(arg);
                }
            }
            if !injected {
                new_args.insert(1, "--project".to_string());
                new_args.insert(2, project);
            }
            args = new_args;
        }

        Ok((args, env))
    }

    /// Rewrite an arg that is a path (optionally shell-quoted) under the
    /// proxy's models dir to the local equivalent. Non-paths pass through.
    fn remap_path_prefix(arg: &str, prefix: &str, local_dir: &str) -> String {
        let (quoted, inner) =
            if let Some(stripped) = arg.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
                (true, stripped)
            } else if let Some(stripped) = arg.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                (true, stripped)
            } else {
                (false, arg)
            };

        if let Some(rel) = inner.strip_prefix(prefix) {
            let remapped = if rel.is_empty() {
                local_dir.to_string()
            } else {
                format!("{}/{}", local_dir.trim_end_matches('/'), rel)
            };
            if quoted {
                format!("'{}'", remapped)
            } else {
                remapped
            }
        } else {
            arg.to_string()
        }
    }

    /// Derive the base endpoint URL from the health URL (strip the path).
    fn endpoint_from_health_url(health_url: &str) -> String {
        url::Url::parse(health_url)
            .ok()
            .map(|mut u| {
                u.set_path("");
                u.set_query(None);
                u.set_fragment(None);
                u.to_string().trim_end_matches('/').to_string()
            })
            .unwrap_or_else(|| health_url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_support::test_state;

    fn make_req(
        model: &str,
        command: &str,
        args: &[&str],
        health_timeout_ms: i64,
    ) -> LoadModelRequest {
        let env = std::collections::HashMap::new();
        let health_url = if health_timeout_ms > 0 {
            format!("http://127.0.0.1:59{}/health", model.len() % 100)
        } else {
            String::new()
        };
        LoadModelRequest {
            provider_name: "llama_cpp".to_string(),
            model_path: format!("owner/repo/{}.gguf", model),
            gpu_variant: "cpu".to_string(),
            params: std::collections::HashMap::new(),
            model_name: model.to_string(),
            command: command.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env,
            health_url,
            health_timeout_ms,
            gpu_device: String::new(),
            docker_config_json: String::new(),
        }
    }

    /// load (health skipped) → alive process in the table; restart → new
    /// pid; unload → entry gone and process dead.
    #[tokio::test]
    async fn test_load_restart_unload() {
        let (state, _dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let lc = TamadLifecycle::new(Arc::clone(&table), Arc::clone(&state));

        // health_timeout_ms = 0 → immediately ready, no health polling.
        let resp = lc
            .load(&make_req("sleepy", "sh", &["-c", "sleep 30"], 0))
            .await
            .expect("load should succeed");
        assert_ne!(resp.pid, 0);
        assert_eq!(resp.status, "ready");

        let entry = table.get("sleepy").await.expect("entry recorded");
        assert_eq!(entry.status, "ready");
        assert!(
            crate::process::is_process_alive(entry.pid),
            "spawned process must be alive"
        );
        assert_eq!(entry.pid as i32, resp.pid);

        // Restart → new pid, old process gone.
        let old_pid = entry.pid;
        let resp2 = lc.restart("sleepy").await.expect("restart should succeed");
        assert_ne!(resp2.pid, old_pid as i32, "restart must spawn a new pid");
        // Old process should be dead (poll briefly).
        for _ in 0..40 {
            if !crate::process::is_process_alive(old_pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(
            !crate::process::is_process_alive(old_pid),
            "old process must be dead after restart"
        );
        let new_pid = table.get("sleepy").await.expect("entry replaced").pid;
        assert_eq!(new_pid as i32, resp2.pid);
        assert!(crate::process::is_process_alive(new_pid));

        // Unload → entry gone, process dead.
        lc.unload("sleepy").await.expect("unload should succeed");
        assert!(table.get("sleepy").await.is_none(), "entry removed");
        for _ in 0..40 {
            if !crate::process::is_process_alive(new_pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(
            !crate::process::is_process_alive(new_pid),
            "process must be dead after unload"
        );
    }

    /// unload of an unknown model fails.
    #[tokio::test]
    async fn test_unload_unknown_fails() {
        let (state, _dir) = test_state();
        let lc = TamadLifecycle::new(Arc::new(ProcessTable::default()), Arc::clone(&state));
        assert!(lc.unload("nope").await.is_err());
    }

    /// restart of an unknown model fails.
    #[tokio::test]
    async fn test_restart_unknown_fails() {
        let (state, _dir) = test_state();
        let lc = TamadLifecycle::new(Arc::new(ProcessTable::default()), Arc::clone(&state));
        assert!(lc.restart("nope").await.is_err());
    }

    /// Health polling: a local listener answering 200 → ready.
    #[tokio::test]
    async fn test_load_with_health_check() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 512];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await;
                });
            }
        });

        let (state, _dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let lc = TamadLifecycle::new(Arc::clone(&table), Arc::clone(&state));

        let mut req = make_req("healthy", "sh", &["-c", "sleep 30"], 0);
        req.health_url = format!("http://127.0.0.1:{port}/health");
        req.health_timeout_ms = 10_000;
        let resp = lc.load(&req).await.expect("health load should succeed");
        assert_eq!(resp.status, "ready");
        assert_eq!(resp.endpoint_url, format!("http://127.0.0.1:{port}"));

        lc.unload("healthy").await.ok();
    }

    /// Health timeout: unreachable health URL → Err + failed entry, process
    /// killed.
    #[tokio::test]
    async fn test_load_health_timeout() {
        let (state, _dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let lc = TamadLifecycle::new(Arc::clone(&table), Arc::clone(&state));

        let mut req = make_req("unhealthy", "sh", &["-c", "sleep 30"], 0);
        // A port nothing listens on.
        req.health_url = "http://127.0.0.1:1/health".to_string();
        req.health_timeout_ms = 1_500;
        let err = lc.load(&req).await.expect_err("must time out");
        assert!(err.to_string().contains("failed to become healthy"));

        let entry = table.get("unhealthy").await.expect("failed entry recorded");
        assert_eq!(entry.status, "failed");
        // Process must have been killed.
        for _ in 0..40 {
            if !crate::process::is_process_alive(entry.pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(!crate::process::is_process_alive(entry.pid));
    }

    /// A backend that exits on its own is reaped by the tamad and marked
    /// "failed" in the table (the reap task is the authoritative liveness
    /// signal — a zombie pid would otherwise read as alive via kill(pid,0)
    /// and the proxy's reconciler would never restart it).
    #[tokio::test]
    async fn test_load_marks_failed_when_backend_crashes() {
        let (state, _dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let lc = TamadLifecycle::new(Arc::clone(&table), Arc::clone(&state));

        // No health check: load succeeds immediately, then the process
        // exits on its own.
        let resp = lc
            .load(&make_req("crashy", "sh", &["-c", "exit 1"], 0))
            .await
            .expect("load should succeed");
        assert_eq!(resp.status, "ready");

        // Poll until the reap task has marked the entry failed.
        for _ in 0..40 {
            if let Some(entry) = table.get("crashy").await {
                if entry.status == "failed" {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let entry = table.get("crashy").await.expect("entry kept");
        assert_eq!(
            entry.status, "failed",
            "crashed backend must be marked failed"
        );

        // The snapshot reports it dead (reconciler will re-load it).
        let snap = table.snapshot().await;
        let p = snap
            .iter()
            .find(|p| p.model_name == "crashy")
            .expect("crashy in snapshot");
        assert!(!p.alive, "crashed backend must be reported dead");
    }

    /// list() groups entries by provider_name with empty engine/version.
    #[tokio::test]
    async fn test_list_groups_by_provider() {
        let (state, _dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let lc = TamadLifecycle::new(Arc::clone(&table), Arc::clone(&state));

        let mut req_a = make_req("alpha", "sh", &["-c", "sleep 30"], 0);
        req_a.provider_name = "llama_cpp".to_string();
        let mut req_b = make_req("beta", "sh", &["-c", "sleep 30"], 0);
        req_b.provider_name = "vllm".to_string();
        lc.load(&req_a).await.unwrap();
        lc.load(&req_b).await.unwrap();

        let providers = lc.list().await;
        assert_eq!(providers.len(), 2);
        let llama = providers
            .iter()
            .find(|p| p.name == "llama_cpp")
            .expect("llama_cpp group");
        assert_eq!(llama.loaded_models.len(), 1);
        assert_eq!(llama.loaded_models[0].model_name, "alpha");
        assert!(llama.engine.is_empty());
        assert_eq!(llama.status, "unknown");
        let vllm = providers
            .iter()
            .find(|p| p.name == "vllm")
            .expect("vllm group");
        assert_eq!(vllm.loaded_models[0].model_name, "beta");

        let _ = lc.unload("alpha").await;
        let _ = lc.unload("beta").await;
    }

    /// Model paths under the proxy's models dir are remapped to the
    /// tamad's own models dir via PROXY_MODELS_DIR_ENV.
    #[tokio::test]
    async fn test_models_dir_remap() {
        let (state, _dir) = test_state();
        let lc = TamadLifecycle::new(Arc::new(ProcessTable::default()), Arc::clone(&state));

        let proxy_dir = "/srv/proxy/models";
        let local_dir = state.models_dir.to_string_lossy().to_string();
        assert_ne!(proxy_dir, local_dir, "fixture must differ from local dir");

        let mut req = make_req(
            "pathy",
            "sh",
            &[
                "-c",
                "sleep 30",
                "-m",
                "/srv/proxy/models/owner/repo/m.gguf",
            ],
            0,
        );
        req.env
            .insert(PROXY_MODELS_DIR_ENV.to_string(), proxy_dir.to_string());

        let (args, env) = lc.resolve_launch(&req).await.unwrap();
        // The -m value was remapped to the local models dir.
        let mut it = args.iter();
        let m_pos = it.position(|a| a == "-m").expect("-m flag present");
        let remapped = &args[m_pos + 1];
        assert_eq!(
            remapped,
            &format!("{local_dir}/owner/repo/m.gguf"),
            "path must be remapped"
        );
        // The helper env key was stripped.
        assert!(
            !env.iter().any(|(k, _)| k == PROXY_MODELS_DIR_ENV),
            "PROXY_MODELS_DIR_ENV must not be passed to the process"
        );
        // A non-path arg is untouched.
        assert!(args.contains(&"sleep 30".to_string()));
    }

    /// Paths already under the local models dir are left untouched.
    #[test]
    fn test_remap_path_prefix() {
        assert_eq!(
            TamadLifecycle::remap_path_prefix(
                "/srv/models/a/b.gguf",
                "/srv/models/",
                "/local/models"
            ),
            "/local/models/a/b.gguf"
        );
        // Quoted path.
        assert_eq!(
            TamadLifecycle::remap_path_prefix(
                "'/srv/models/a b/c.gguf'",
                "/srv/models/",
                "/local/models"
            ),
            "'/local/models/a b/c.gguf'"
        );
        // Non-path passthrough.
        assert_eq!(
            TamadLifecycle::remap_path_prefix("-ngl", "/srv/models/", "/local/models"),
            "-ngl"
        );
        assert_eq!(
            TamadLifecycle::remap_path_prefix("99", "/srv/models/", "/local/models"),
            "99"
        );
    }

    /// `resolve_launch` GPU-env wiring: the isolation env vars are resolved
    /// on this host (ADR-0010) — on a GPU-less host the resolution must not
    /// fail and no vendor env appears; explicit `default_env` entries are
    /// always preserved; the cpu variant never gains a vendor env. The
    /// device→env mapping itself is covered in `gpu::env` tests.
    #[tokio::test]
    async fn test_resolve_launch_gpu_env_wiring() {
        let (state, _dir) = test_state();
        let lc = TamadLifecycle::new(Arc::new(ProcessTable::default()), state);

        // GPU device + cuda variant with an explicit env entry.
        let mut req = make_req("gpu-env-a", "/bin/true", &[], 0);
        req.gpu_device = "GPU0".to_string();
        req.gpu_variant = "cuda".to_string();
        req.env.insert("MY_ENV".to_string(), "1".to_string());
        let (_args, env) = lc.resolve_launch(&req).await.expect("resolve must succeed");
        let env: std::collections::BTreeMap<String, String> = env.into_iter().collect();
        assert_eq!(env.get("MY_ENV"), Some(&"1".to_string()));
        if crate::gpu::system::detect_gpu_devices().is_empty() {
            assert!(
                !env.contains_key("CUDA_VISIBLE_DEVICES"),
                "no local GPU → no isolation env var"
            );
        } else {
            assert!(
                env.contains_key("CUDA_VISIBLE_DEVICES"),
                "CUDA host with device set must gain the isolation env var"
            );
        }

        // CPU variant on a device: never a vendor env var.
        let mut req = make_req("gpu-env-b", "/bin/true", &[], 0);
        req.gpu_device = "GPU0".to_string();
        req.gpu_variant = "cpu".to_string();
        let (_args, env) = lc.resolve_launch(&req).await.unwrap();
        for key in env.iter().map(|(k, _)| k.as_str()) {
            assert!(
                !key.to_uppercase().contains("VISIBLE"),
                "cpu variant must not gain a vendor env var, got {key}"
            );
        }

        // Unknown variant folder: warn path, still resolves.
        let mut req = make_req("gpu-env-c", "/bin/true", &[], 0);
        req.gpu_device = "GPU0".to_string();
        req.gpu_variant = "not-a-variant".to_string();
        assert!(lc.resolve_launch(&req).await.is_ok());
    }

    /// `kill_all` must terminate EVERY loaded backend's process group —
    /// including grandchildren of the spawned leader (the orphan case:
    /// tamad dies while backends run, plan-191 follow-up A).
    #[tokio::test]
    #[cfg(unix)]
    async fn test_kill_all_kills_every_backend_group() {
        use crate::process::{is_process_group_alive, wait_group_dead};

        let (state, _dir) = test_state();
        let table = Arc::new(ProcessTable::default());
        let lc = TamadLifecycle::new(Arc::clone(&table), state);

        // Child (sh) + grandchild (sleep) — the grandchild keeps running
        // even after sh exits; only a *group* kill removes it.
        let req = make_req("ghost-1", "/bin/sh", &["-c", "sleep 120"], 0);
        lc.load(&req).await.expect("load ghost-1");
        let req = make_req("ghost-2", "/bin/sh", &["-c", "sleep 120"], 0);
        lc.load(&req).await.expect("load ghost-2");

        let entries = table.list().await;
        assert_eq!(entries.len(), 2, "two backends loaded before kill_all");
        let pids: Vec<u32> = entries.iter().map(|e| e.pid).collect();
        for p in &pids {
            assert!(
                is_process_group_alive(*p),
                "group leader {p} alive before kill"
            );
        }

        lc.kill_all().await.expect("kill_all succeeds");

        // Every group (leader + grandchildren in the group) must be gone.
        for p in &pids {
            wait_group_dead(*p).await.expect("group reaped");
            assert!(
                !is_process_group_alive(*p),
                "group {p} must be dead after kill_all"
            );
        }
        assert!(table.list().await.is_empty(), "table cleared by kill_all");
    }
}
