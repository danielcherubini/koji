use anyhow::{Context, Result};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::io::AsyncBufReadExt;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use super::lifecycle::traits::HealthChecker;
use super::types::{BackendState, ProxyState};
use crate::installations::docker::runner as docker_runner;
use crate::installations::docker::{
    docker_available, is_image_present, pull_image, remove_container, rewrite_args_for_container,
    spawn_container, stop_container,
};
use crate::logging;
use crate::process::{
    configure_process_group, force_kill_process, force_kill_process_group, is_process_alive,
    is_process_group_alive, kill_process, kill_process_group, override_arg,
};

/// Ensure a model is loaded and return its backend name.
///
/// This encapsulates the shared flow used by multiple handlers:
/// resolve alias → get available backend → evict LRU if needed → get model card
/// → load model → update last_accessed.
///
/// Callers provide an `on_load_error` closure to handle the case where loading
/// fails (e.g., returning an error response or falling back to another backend).
/// The closure receives the resolved model name and the error, and returns the
/// fallback backend name (or an error if no fallback is possible).
pub async fn ensure_model_loaded(
    state: &Arc<ProxyState>,
    model_name: &str,
    on_load_error: impl FnOnce(&str, anyhow::Error) -> Result<String> + Send,
) -> Result<String> {
    // Resolve alias before routing
    let resolved_model = state.resolve_alias(model_name).await;

    // Check if model has a provider_name that resolves to a remote provider.
    // When set, this overrides the `backend` field for routing.
    let provider_name: Option<String> = {
        let model_configs = state.registry.model_configs.read().await;
        model_configs
            .get(&resolved_model)
            .and_then(|c| c.provider_name.clone())
    };

    if let Some(ref name) = provider_name {
        if let Some(provider) = state.get_provider(name).await {
            if provider.provider_type.is_remote() {
                // Return sentinel indicating remote provider
                return Ok(format!("remote:{}", provider.id));
            }
        }
    }

    let backend_name = match state.get_available_backend_for_model(&resolved_model).await {
        Some(name) => name,
        None => {
            let model_toml = state.get_model_toml(&resolved_model).await;
            let target_gpu = state
                .resolve_model_gpu_device(&resolved_model, model_toml.as_ref())
                .await;
            let _ = state.evict_lru_if_needed(target_gpu).await;
            match state
                .load_model(&resolved_model, model_toml.as_ref(), &())
                .await
            {
                Ok(s) => s,
                Err(e) => on_load_error(&resolved_model, e)?,
            }
        }
    };

    state.update_last_accessed(&backend_name).await;
    Ok(backend_name)
}

mod compaction;
mod idle_timeout;
mod traits;
mod tts;

impl ProxyState {
    // ─── Public API ────────────────────────────────────────────────

    /// Load a model by starting its backend process.
    ///
    /// If the active backend installation has a `docker_config`, delegates to
    /// the docker path (preflight → pull → reserve → spawn container → health).
    /// Otherwise follows the native process-spawn path.
    pub async fn load_model<H: HealthChecker>(
        &self,
        model_name: &str,
        model_toml: Option<&crate::models::ModelToml>,
        _health_checker: &H,
    ) -> Result<String> {
        debug!("Loading model: {}", model_name);

        let config = self.config.read().await.clone();

        // Resolve the backend name for this model
        let model_configs = self.registry.model_configs.read().await;
        let backends = config.resolve_backends_for_model(&model_configs, model_name);
        let backend_name = backends
            .first()
            .map(|(name, _, _)| name.clone())
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve backend for model {}", model_name))?;

        // Get backend and backend config from config
        let config_for_resolve = config.clone();
        let (model_config, backend_config) =
            config_for_resolve.resolve_backend(&model_configs, &backend_name)?;

        // Resolve the effective GPU device from model config > model card default.
        let effective_gpu_device = resolve_gpu_device(
            model_config.gpu_device.clone(),
            model_toml.and_then(|toml| toml.model.default_gpu_device.clone()),
        );

        // Build a modified model_config with the resolved GPU device.
        let mut model_config = model_config.clone();
        model_config.gpu_device = effective_gpu_device;

        // Open InstallationManager for path resolution and default args.
        // Postgres-pool based (plan-190 Task 8); None when no pool is configured.
        let manager = self
            .db_pool()
            .map(crate::installations::InstallationManager::new);

        // Resolve the backend binary path: DB takes priority, config.path is fallback.
        let backend_path = config
            .resolve_backend_path(
                &model_config.backend,
                model_config.gpu_variant.as_ref(),
                manager.as_ref(),
            )
            .await?;

        // Resolve gpu_variant (reuse same fallback logic as resolve_backend_path)
        let gpu_variant = model_config
            .gpu_variant
            .clone()
            .unwrap_or(crate::gpu::GpuVariant::CpuOnly);

        // Get active installation to check for docker_config.
        // Note: use the backend *name* (e.g. "vllm"), not the model config key,
        // since provider_installations rows are keyed by backend name + gpu_variant.
        let active = match &manager {
            Some(m) => {
                m.get_active(&model_config.backend, gpu_variant.variant_folder())
                    .await?
            }
            None => None,
        };
        let docker_config = active.and_then(|a| a.docker_config);

        // Atomically check if already loaded and reserve if not (single write lock).
        {
            let mut models = self.registry.models.write().await;

            if let Some(state) = models.get(&backend_name) {
                if state.is_ready() || matches!(state, BackendState::Starting { .. }) {
                    debug!(
                        "Backend '{}' already loaded/starting for model '{}'",
                        backend_name, model_name
                    );
                    return Ok(backend_name);
                }
            }

            // Reserve this backend with Starting state
            models.insert(
                backend_name.clone(),
                BackendState::Starting {
                    model_name: model_name.to_string(),
                    backend: model_config.backend.clone(),
                    backend_url: String::new(),
                    backend_pid: 0,
                    last_accessed: Instant::now(),
                    start_time: Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    is_docker: docker_config.is_some(),
                },
            );
        } // models write lock dropped

        if let Some(docker_cfg) = docker_config {
            // Build args now (before entering async context with docker path)
            let default_args = match &manager {
                Some(m) => {
                    m.get_default_args(&model_config.backend, gpu_variant.variant_folder())
                        .await
                }
                None => Vec::new(),
            };
            let args =
                config.build_full_args(&model_config, backend_config, None, &default_args)?;

            // Build env vars for the container from the backend's default_env,
            // plus auto-inject HF_TOKEN so vLLM can download gated models.
            let mut env_vars = match &manager {
                Some(m) => {
                    m.get_default_env(&model_config.backend, gpu_variant.variant_folder())
                        .await
                }
                None => Vec::new(),
            };
            let has_hf_token = env_vars
                .iter()
                .any(|e| e.split_once('=').map(|(k, _)| k) == Some("HF_TOKEN"));
            if !has_hf_token {
                if let Some(token) = crate::models::pull::get_hf_token() {
                    env_vars.push(format!("HF_TOKEN={}", token));
                }
            }

            self.load_model_docker(
                config,
                model_config.clone(),
                backend_config.clone(),
                args,
                env_vars,
                gpu_variant,
                model_name,
                &backend_name,
                docker_cfg,
                _health_checker,
            )
            .await
        } else {
            self.load_model_native(
                config,
                model_config.clone(),
                backend_config.clone(),
                model_name,
                backend_path.to_path_buf(),
                &backend_name,
                gpu_variant,
                _health_checker,
            )
            .await
        }
    }

    // ─── Docker path ───────────────────────────────────────────────

    /// Load a model using the docker backend path.
    ///
    /// Flow: preflight (docker_available) → pull image → reserve Starting →
    /// spawn container → health check → Ready.
    #[allow(clippy::too_many_arguments)]
    async fn load_model_docker<H: HealthChecker>(
        &self,
        config: crate::config::Config,
        model_config: crate::config::ModelConfig,
        _backend_config: crate::config::BackendConfig,
        args: Vec<String>,
        env_vars: Vec<String>,
        _gpu_variant: crate::gpu::GpuVariant,
        model_name: &str,
        backend_name: &str,
        docker_cfg: crate::installations::docker::DockerConfig,
        _health_checker: &H,
    ) -> Result<String> {
        // ── Step A — Preflight + Pull (BEFORE Starting reservation) ────
        info!(
            "Docker preflight for backend '{}' (model '{}')",
            backend_name, model_name
        );

        // Check docker availability
        docker_available()
            .await
            .with_context(|| format!("Docker is not available for backend '{}'", backend_name))?;

        // Verify or pull the image (concurrent pulls are idempotent at docker layer)
        let is_present = is_image_present(&docker_cfg.image).await?;
        if !is_present {
            info!("Pulling docker image: {}", docker_cfg.image);
            let progress = |line: String| {
                debug!("docker pull: {}", line);
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            // Use a generous pull timeout: 6x the startup timeout (default ~1800s)
            let pull_timeout_secs = config.proxy.startup_timeout_secs.saturating_mul(6);
            pull_image(&docker_cfg.image, progress, pull_timeout_secs, &cancel)
                .await
                .with_context(|| format!("Failed to pull docker image '{}'", docker_cfg.image))?;
            info!("Docker image pulled successfully: {}", docker_cfg.image);
        } else {
            debug!("Docker image already present: {}", docker_cfg.image);
        }

        // ── Step B — Starting reservation (is_docker=true) ─────────────
        // Already reserved in load_model before this call.

        // Check if cancel-load removed the Starting entry during pull.
        // If so, bail out to avoid spawning an orphaned container.
        {
            let models = self.registry.models.read().await;
            if !models.contains_key(backend_name) {
                return Err(anyhow::anyhow!(
                    "Load cancelled: Starting entry removed during pull for backend '{}'",
                    backend_name
                ));
            }
        }

        // ── Step C — Spawn container + health check ────────────────────
        let timeout = Duration::from_secs(config.proxy.startup_timeout_secs);

        // Find a free host port (retry up to 3x on collision)
        let host_port = find_free_port_with_retry(3).await?;

        // Resolve volumes (substitute {{MODEL_DIR}} → models_dir, validate paths)
        let models_dir = config.models_dir()?;

        // Rewrite model path in args for container (both split and joined forms)
        let container_model_path = docker_cfg.model_mount.container_path.as_str();
        let rewritten_args = rewrite_args_for_container(&args, &models_dir, container_model_path)?;

        // Override host/port args for container networking
        let mut container_args = rewritten_args.clone();
        override_arg(&mut container_args, "--host", "0.0.0.0");
        override_arg(
            &mut container_args,
            "--port",
            &docker_cfg.container_port.to_string(),
        );

        // Best-effort cleanup of any leftover container
        let container_name = format!("tama-{}", backend_name);
        let _ = remove_container(&container_name).await;

        // Spawn the container
        info!(
            "Spawning docker container '{}' for backend '{}' (model '{}')",
            container_name, backend_name, model_name
        );

        let container = match spawn_container(
            backend_name,
            &docker_cfg,
            host_port,
            container_args,
            env_vars,
            &models_dir,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                // Clean up Starting entry on spawn failure
                let mut models = self.registry.models.write().await;
                models.remove(backend_name);
                self.metrics.modify_inference_stats(|map| {
                    map.remove(backend_name);
                });
                // Best-effort remove any partial container
                let _ = remove_container(&container_name).await;
                return Err(e).with_context(|| {
                    format!(
                        "Failed to spawn docker container for backend '{}'",
                        backend_name
                    )
                });
            }
        };

        info!(
            "Docker container '{}' started (pid: {}, id: {})",
            container.name, container.pid, container.id
        );

        // Update the PID in the Starting state
        {
            let mut models = self.registry.models.write().await;
            if let Some(BackendState::Starting { backend_pid, .. }) = models.get_mut(backend_name) {
                *backend_pid = container.pid;
            }
        }

        // Start log streaming (capture timestamp immediately before docker run)
        let _spawn_timestamp = UNIX_EPOCH.elapsed().unwrap_or_default().as_secs();
        self.start_docker_log_stream(&container_name, backend_name)
            .await;

        // ── Step D — Health check ──────────────────────────────────────
        let health_url = format!("http://127.0.0.1:{}/health", host_port);
        let backend_url = format!("http://127.0.0.1:{}", host_port);

        let start = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                warn!(
                    "Startup health check timeout for docker backend '{}' after {}s",
                    backend_name,
                    timeout.as_secs()
                );
                // Docker stop + cleanup on timeout
                let _ = stop_container(&container_name).await;
                let _ = remove_container(&container_name).await;
                // Cancel log streaming task
                self.cancel_log_task(backend_name).await;
                // Clean up Starting entry
                let mut models = self.registry.models.write().await;
                models.remove(backend_name);
                self.metrics.modify_inference_stats(|map| {
                    map.remove(backend_name);
                });
                return Err(anyhow::anyhow!(
                    "Docker backend '{}' failed to start (timeout after {}s)",
                    backend_name,
                    timeout.as_secs()
                ));
            }

            if _health_checker.check_health(&health_url, Some(5)).await {
                debug!("Health check passed for docker backend '{}'", backend_name);
                break;
            }
        }

        // ── Step E — Update to Ready state ─────────────────────────────
        {
            let mut models = self.registry.models.write().await;
            if let Some(state) = models.get_mut(backend_name) {
                if let BackendState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    ..
                } = state
                {
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = BackendState::Ready {
                        model_name: model_name.to_string(),
                        backend: model_config.backend.clone(),
                        backend_pid: container.pid,
                        backend_url: backend_url.clone(),
                        load_time: std::time::SystemTime::now(),
                        last_accessed: Instant::now(),
                        consecutive_failures: cf,
                        failure_timestamp: ft,
                        restart_count: 0,
                        is_docker: true,
                    };
                }
            }
        }

        // Write to DB after model is ready (best-effort)
        if let Some(pool) = self.db_pool() {
            let _ = crate::db::queries::insert_active_model(
                &pool,
                backend_name,
                model_name,
                &model_config.backend,
                container.pid as i64,
                host_port as i64,
                &backend_url,
            )
            .await;
        }

        info!("Docker backend '{}' loaded successfully", backend_name);
        self.metrics
            .counters
            .models_loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(backend_name.to_string())
    }

    /// Start log streaming from a docker container using `docker logs -f --since`.
    async fn start_docker_log_stream(&self, container_name: &str, backend_name: &str) {
        let log_key = format!("docker_{}", backend_name);
        let log_stream = self.backend_logs.get_or_create(&log_key).await;

        // Open log file for this backend instance
        let logs_dir = self.config.read().await.logs_dir().ok();
        let log_name = format!("docker_{}", backend_name);
        let log_file = logs_dir
            .as_ref()
            .and_then(|dir| logging::open_log(dir, &log_name).ok());
        let log_file_arc = log_file.map(|f| Arc::new(Mutex::new(f)));

        // Spawn the docker logs streaming task and register it for cancellation.
        // Note: we spawn the future directly into a JoinSet (not via tokio::spawn)
        // so the JoinSet gets type JoinSet<()> instead of JoinSet<Result<(), JoinError>>.
        let container_id = container_name.to_string();
        let push_line = Arc::clone(&log_stream);
        let file_arc = log_file_arc.clone();

        let backend_name_docker = backend_name.to_string();
        let log_future = async move {
            let since_epoch = UNIX_EPOCH.elapsed().unwrap_or_default().as_secs();
            match docker_runner::logs_stream(&container_id, since_epoch).await {
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        let push = push_line.clone();
                        let file = file_arc.clone();
                        tokio::spawn(async move {
                            let reader = tokio::io::BufReader::new(stdout);
                            let mut lines = reader.lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                let _ = push.push(line.clone()).await;
                                if let Some(ref f) = file {
                                    let _ = f.lock().map(|mut fw| {
                                        let _ = writeln!(fw, "{line}");
                                    });
                                }
                            }
                        });
                    }
                    let _ = child.wait().await;
                }
                Err(e) => {
                    warn!(
                        "Failed to start docker log stream for '{}': {}",
                        container_id, e
                    );
                }
            }
        };
        let mut docker_log_set = JoinSet::new();
        docker_log_set.spawn(log_future);
        self.model_tasks
            .write()
            .await
            .insert(backend_name_docker, docker_log_set);
    }

    /// Cancel the log streaming task for a backend.
    async fn cancel_log_task(&self, backend_name: &str) {
        if let Some(mut tasks) = self.model_tasks.write().await.remove(backend_name) {
            tasks.abort_all();
        }
    }

    // ─── Native path ────────────────────────────────────────────────

    /// Load a model using the native process-spawn path.
    #[allow(clippy::too_many_arguments)]
    async fn load_model_native<H: HealthChecker>(
        &self,
        config: crate::config::Config,
        model_config: crate::config::ModelConfig,
        backend_config: crate::config::BackendConfig,
        model_name: &str,
        backend_path: std::path::PathBuf,
        backend_name: &str,
        gpu_variant: crate::gpu::GpuVariant,
        _health_checker: &H,
    ) -> Result<String> {
        // Create InstallationManager internally (doesn't borrow across await points)
        let manager = self
            .db_pool()
            .map(crate::installations::InstallationManager::new);

        // Find a free port for this backend.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener);

        tracing::debug!(
            gpu = %model_config.gpu_device.as_deref().unwrap_or("default"),
            "Loading model '{}' with backend '{}'",
            model_name,
            model_config.backend
        );

        // Build full args and override host/port
        let default_args = match &manager {
            Some(m) => {
                m.get_default_args(&model_config.backend, gpu_variant.variant_folder())
                    .await
            }
            None => Vec::new(),
        };
        let mut args =
            config.build_full_args(&model_config, &backend_config, None, &default_args)?;

        override_arg(&mut args, "--host", "127.0.0.1");
        override_arg(&mut args, "--port", &port.to_string());

        let health_url = format!("http://127.0.0.1:{}/health", port);
        let backend_url = format!("http://127.0.0.1:{}", port);

        info!(
            "Starting backend '{}' for backend '{}' (model '{}')",
            model_config.backend, backend_name, model_name
        );

        // Resolve logs directory for backend log file
        let logs_dir = config.logs_dir().ok();

        let mut child = tokio::process::Command::new(backend_path.as_path());
        crate::process::configure_backend_command(&mut child, backend_path.as_path());

        // Inject GPU isolation env var (ROCR/CUDA/GGML_VK_VISIBLE_DEVICES)
        if !matches!(gpu_variant, crate::gpu::GpuVariant::CpuOnly) {
            if let Some(ref device) = model_config.gpu_device {
                match crate::gpu::env::resolve_gpu_env(device, &gpu_variant) {
                    Some((name, value)) => {
                        info!(
                            "GPU isolation: setting {}={} for device {} (variant {})",
                            name,
                            value,
                            device,
                            gpu_variant.variant_folder()
                        );
                        child.env(&name, &value);
                    }
                    None => {
                        warn!(
                            "GPU isolation: could not resolve device '{}' \
                             (variant {}); no env var set",
                            device,
                            gpu_variant.variant_folder()
                        );
                    }
                }
            }
        }
        configure_process_group(&mut child);

        // Apply default env vars from backend config
        let default_env = match &manager {
            Some(m) => {
                m.get_default_env(&model_config.backend, gpu_variant.variant_folder())
                    .await
            }
            None => Vec::new(),
        };
        for env_var in &default_env {
            if let Some((key, value)) = env_var.split_once('=') {
                info!("Applying env var: {}={}", key, value);
                child.env(key, value);
            } else if !env_var.is_empty() {
                warn!("Skipping malformed env var (missing '='): {}", env_var);
            }
        }

        child
            .args(&args)
            .env("MODEL_NAME", model_name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        info!(
            "Executing backend: {} {}",
            backend_path.display(),
            args.join(" ")
        );

        let mut child = child.spawn().with_context(|| {
            format!(
                "Failed to execute backend process '{}'",
                model_config.backend
            )
        })?;

        let pid = child.id().ok_or_else(|| {
            anyhow::anyhow!("Failed to get PID for backend '{}'", model_config.backend)
        })?;
        info!(
            "Backend '{}' started for backend '{}' (pid: {:?})",
            model_config.backend, backend_name, pid
        );

        // Update the PID in the Starting state
        {
            let mut models = self.registry.models.write().await;
            if let Some(BackendState::Starting { backend_pid, .. }) = models.get_mut(backend_name) {
                *backend_pid = pid;
            }
        }

        // Get the backend log stream for SSE broadcasting
        let log_key = format!("{}_{}", model_config.backend, backend_name);
        let log_stream = self.backend_logs.get_or_create(&log_key).await;

        // Open log file for this backend instance
        let log_name = format!("{}_{}", model_config.backend, backend_name);
        let log_file = logs_dir
            .as_ref()
            .and_then(|dir| logging::open_log(dir, &log_name).ok());
        let log_file_arc = log_file.map(|f| Arc::new(Mutex::new(f)));

        // Helper to push a line: broadcast + write to file.
        let push_line = Arc::new(move |line: String| {
            let stream = log_stream.clone();
            let file = log_file_arc.clone();
            tokio::spawn(async move {
                let _ = stream.push(line.clone()).await;
                if let Some(ref f) = file {
                    let _ = f.lock().map(|mut fw| {
                        let _ = writeln!(fw, "{line}");
                    });
                }
            });
        });

        // Track spawned tasks for clean cancellation on unload.
        let mut model_tasks = JoinSet::new();

        // Stream stdout
        if let Some(stdout) = child.stdout.take() {
            let push = push_line.clone();
            model_tasks.spawn(async move {
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    push(line);
                }
            });
        }

        // Stream stderr
        if let Some(stderr) = child.stderr.take() {
            let push = push_line.clone();
            model_tasks.spawn(async move {
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    push(line);
                }
            });
        }

        // Spawn a reaper task so the child process is waited on
        let reaper_backend = backend_name.to_string();
        model_tasks.spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    debug!(
                        "Backend process {} for backend '{}' exited with {}",
                        pid, reaper_backend, status
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to wait on backend process {} for backend '{}': {}",
                        pid, reaper_backend, e
                    );
                }
            }
        });

        // Register the JoinSet so unload_model can cancel these tasks
        self.model_tasks
            .write()
            .await
            .insert(backend_name.to_string(), model_tasks);

        // Wait for health check to pass
        let timeout = Duration::from_secs(config.proxy.startup_timeout_secs);
        let start = Instant::now();
        let mut health_ok = false;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                warn!(
                    "Startup health check timeout for backend '{}' after {}s, killing process group",
                    backend_name, timeout.as_secs()
                );
                let _ = kill_process_group(pid).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
                if is_process_group_alive(pid) {
                    warn!("Process group {} still alive, sending SIGKILL", pid);
                    let _ = force_kill_process_group(pid).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                break;
            }

            if _health_checker.check_health(&health_url, Some(5)).await {
                debug!("Health check passed for backend '{}'", backend_name);
                health_ok = true;
                break;
            }
        }

        if !health_ok {
            // Abort orphan task readers and reaper
            if let Some(mut tasks) = self.model_tasks.write().await.remove(backend_name) {
                tasks.abort_all();
            }
            // Clean up the Starting entry
            let mut models = self.registry.models.write().await;
            models.remove(backend_name);
            self.metrics.modify_inference_stats(|map| {
                map.remove(backend_name);
            });
            return Err(anyhow::anyhow!(
                "Backend '{}' failed to start for backend '{}' (timeout after {}s)",
                model_config.backend,
                backend_name,
                timeout.as_secs()
            ));
        }

        // Update the loaded model state to Ready
        {
            let mut models = self.registry.models.write().await;
            if let Some(state) = models.get_mut(backend_name) {
                if let BackendState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    ..
                } = state
                {
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = BackendState::Ready {
                        model_name: model_name.to_string(),
                        backend: model_config.backend.clone(),
                        backend_pid: pid,
                        backend_url: backend_url.clone(),
                        load_time: std::time::SystemTime::now(),
                        last_accessed: Instant::now(),
                        consecutive_failures: cf,
                        failure_timestamp: ft,
                        restart_count: 0,
                        is_docker: false,
                    };
                }
            }
        }

        // Write to DB after model is ready (best-effort)
        if let Some(pool) = self.db_pool() {
            let _ = crate::db::queries::insert_active_model(
                &pool,
                backend_name,
                model_name,
                &model_config.backend,
                pid as i64,
                port as i64,
                &backend_url,
            )
            .await;
        }

        info!("Backend '{}' loaded successfully", backend_name);
        self.metrics
            .counters
            .models_loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(backend_name.to_string())
    }

    // ─── Other public methods ──────────────────────────────────────

    /// Resolve the effective GPU device for a model.
    ///
    /// Uses the same resolution logic as `load_model`: checks the backend config's
    /// `gpu_device` first, then falls back to the model card's `default_gpu_device`.
    /// Returns `None` if the model cannot be routed to any backend.
    pub async fn resolve_model_gpu_device(
        &self,
        model_name: &str,
        model_toml: Option<&crate::models::ModelToml>,
    ) -> Option<String> {
        let config = self.config.read().await.clone();
        let model_configs = self.registry.model_configs.read().await;
        let backends = config.resolve_backends_for_model(&model_configs, model_name);
        let backend_name = backends.first().map(|(name, _, _)| name.clone())?;
        let (model_config, _) = config.resolve_backend(&model_configs, &backend_name).ok()?;
        resolve_gpu_device(
            model_config.gpu_device.clone(),
            model_toml.and_then(|toml| toml.model.default_gpu_device.clone()),
        )
    }

    /// Evict the least-recently-used Ready model on the target GPU if the proxy
    /// is at capacity for that device.
    pub async fn evict_lru_if_needed(
        &self,
        target_gpu_device: Option<String>,
    ) -> Result<Option<String>> {
        let config = self.config.read().await;
        let max = config.proxy.max_loaded_models;

        if max == 0 {
            return Ok(None);
        }

        let models = self.registry.models.write().await;
        let ready_backends: Vec<String> = models
            .iter()
            .filter(|(_, s)| matches!(s, BackendState::Ready { .. }))
            .map(|(name, _)| name.clone())
            .collect();

        let non_inference_backends: std::collections::HashSet<String> = models
            .iter()
            .filter(|(_, s)| s.is_non_inference_backend())
            .map(|(name, _)| name.clone())
            .collect();

        drop(models);

        let model_configs = self.registry.model_configs.read().await;
        let llm_count = ready_backends
            .iter()
            .filter(|backend_name| {
                if model_configs
                    .get(backend_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    || non_inference_backends.contains(backend_name.as_str())
                {
                    return false;
                }
                let model_gpu = model_configs
                    .get(backend_name.as_str())
                    .and_then(|mc| mc.gpu_device.as_ref());
                model_gpu == target_gpu_device.as_ref()
            })
            .count();

        if llm_count < max as usize {
            return Ok(None);
        }

        let mut models = self.registry.models.write().await;
        let lru_name = ready_backends
            .iter()
            .filter(|backend_name| {
                if model_configs
                    .get(backend_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    || non_inference_backends.contains(backend_name.as_str())
                {
                    return false;
                }
                let model_gpu = model_configs
                    .get(backend_name.as_str())
                    .and_then(|mc| mc.gpu_device.as_ref());
                model_gpu == target_gpu_device.as_ref()
            })
            .filter_map(|backend_name| models.get(backend_name).map(|s| (backend_name, s)))
            .min_by_key(|(_, s)| s.last_accessed())
            .map(|(name, _)| name.to_string());

        if let Some(ref name) = lru_name {
            if let Some(state) = models.get_mut(name) {
                if let BackendState::Ready {
                    model_name,
                    backend,
                    backend_pid,
                    backend_url,
                    last_accessed,
                    consecutive_failures,
                    failure_timestamp,
                    restart_count,
                    load_time: _,
                    is_docker,
                } = std::mem::take(state)
                {
                    *state = BackendState::Unloading {
                        model_name,
                        backend,
                        backend_pid,
                        backend_url,
                        last_accessed,
                        consecutive_failures,
                        failure_timestamp,
                        restart_count,
                        is_docker,
                    };
                }
            }
        }

        drop(models);

        if let Some(name) = lru_name {
            self.unload_model(&name).await?;
            Ok(Some(name))
        } else {
            Ok(None)
        }
    }

    /// Unload a backend by stopping its process or container.
    pub async fn unload_model(&self, backend_name: &str) -> Result<()> {
        debug!("Unloading backend: {}", backend_name);

        let state = self
            .get_model_state(backend_name)
            .await
            .with_context(|| format!("Backend '{}' not loaded", backend_name))?;

        if !matches!(
            state,
            BackendState::Ready { .. } | BackendState::Unloading { .. }
        ) {
            return Err(anyhow::anyhow!(
                "Backend '{}' is not ready (state: {:?})",
                backend_name,
                state
            ));
        }

        let (_backend, pid, is_docker) = match &state {
            BackendState::Ready {
                backend,
                backend_pid,
                is_docker,
                ..
            }
            | BackendState::Unloading {
                backend,
                backend_pid,
                is_docker,
                ..
            } => (backend.clone(), *backend_pid, *is_docker),
            _ => {
                return Err(anyhow::anyhow!(
                    "Backend '{}' entered unexpected state during unload (state: {:?})",
                    backend_name,
                    state
                ));
            }
        };

        let gpu_info: String = self
            .registry
            .model_configs
            .read()
            .await
            .get(backend_name)
            .and_then(|mc| mc.gpu_device.clone())
            .unwrap_or_else(|| "default".to_string());

        if is_docker {
            // Docker path: stop container → rm → cleanup
            info!(gpu = %gpu_info, "Stopping docker container for backend '{}'", backend_name);
            let container_name = format!("tama-{}", backend_name);
            // Stop with timeout (tolerates missing container)
            let _ = stop_container(&container_name).await;
            // Remove the container (tolerates missing container)
            let _ = remove_container(&container_name).await;
            // Cancel log streaming task
            self.cancel_log_task(backend_name).await;
        } else {
            // Native path: SIGTERM → wait → SIGKILL if needed
            info!(gpu = %gpu_info, "Stopping backend '{}'", backend_name);
            info!("Sending SIGTERM to backend process {}", pid);
            let _ = kill_process(pid).await;

            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                if !is_process_alive(pid) {
                    debug!("Backend process {} exited gracefully", pid);
                    break;
                }
                if Instant::now() >= deadline {
                    warn!(
                        "Backend process {} did not exit after SIGTERM, sending SIGKILL",
                        pid
                    );
                    let _ = force_kill_process(pid).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    break;
                }
            }
        }

        // Cancel and join any spawned tasks for this backend
        if let Some(mut tasks) = self.model_tasks.write().await.remove(backend_name) {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }

        // Remove from models
        let mut models = self.registry.models.write().await;
        models.remove(backend_name);

        // Clear stale inference stats
        self.metrics.modify_inference_stats(|map| {
            map.remove(backend_name);
        });

        // Write to DB after model is unloaded (best-effort)
        if let Some(pool) = self.db_pool() {
            let _ = crate::db::queries::remove_active_model(&pool, backend_name).await;
        }

        info!(gpu = %gpu_info, "Backend '{}' unloaded", backend_name);
        self.metrics
            .counters
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

/// Find a free port by binding to 0.0.0.0:0 and releasing the listener.
async fn find_free_port() -> Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Find a free port with retry. Tries up to `max_attempts` times on port collision.
async fn find_free_port_with_retry(max_attempts: u32) -> Result<u16> {
    for attempt in 1..=max_attempts {
        match find_free_port().await {
            Ok(port) => return Ok(port),
            Err(e) if attempt < max_attempts => {
                debug!("Port collision on attempt {}, retrying: {}", attempt, e);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => return Err(e),
        }
    }
    // Should not reach here if max_attempts >= 1
    find_free_port().await
}

/// Resolve the effective GPU device from the fallback chain:
/// model config > model card default.
fn resolve_gpu_device(config: Option<String>, card_default: Option<String>) -> Option<String> {
    let normalize = |s: Option<String>| {
        s.and_then(|v| {
            let t = v.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        })
    };
    normalize(config).or_else(|| normalize(card_default))
}

#[cfg(test)]
mod tests;
