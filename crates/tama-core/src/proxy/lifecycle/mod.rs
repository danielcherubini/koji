use anyhow::{Context, Result};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use super::lifecycle::traits::HealthChecker;
use super::types::{BackendState, ProxyState};
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
    /// Load a model by starting its backend process.
    pub async fn load_model<H: HealthChecker>(
        &self,
        model_name: &str,
        model_toml: Option<&crate::models::ModelToml>,
        _health_checker: &H,
    ) -> Result<String> {
        debug!("Loading model: {}", model_name);

        let config = self.config.read().await.clone();

        // Resolve the backend name for this model
        let model_configs = self.model_configs.read().await;
        let backends = config.resolve_backends_for_model(&model_configs, model_name);
        let backend_name = backends
            .first()
            .map(|(name, _, _)| name.clone())
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve backend for model {}", model_name))?;

        // Get backend and backend config from config
        let (model_config, backend_config) =
            config.resolve_backend(&model_configs, &backend_name)?;

        // Atomically check if already loaded and reserve if not (single write lock)
        {
            let mut models = self.models.write().await;
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
                },
            );
        }

        // Open BackendManager for path resolution and default args.
        let manager = self
            .db_dir
            .as_ref()
            .and_then(|dir| crate::backends::BackendManager::open(dir).ok())
            .unwrap_or_else(|| {
                crate::backends::BackendManager::open_in_memory()
                    .expect("in-memory BackendManager must always open")
            });

        // Resolve the backend binary path: DB takes priority, config.path is fallback.
        let backend_path = config.resolve_backend_path(
            &model_config.backend,
            model_config.gpu_variant.as_ref(),
            &manager,
        )?;

        // Find a free port for this backend.
        // Note: there is a small race window between dropping the listener and the
        // backend binding to the port. This is an accepted trade-off for local use;
        // in practice port collisions are extremely rare.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener); // Free the port for the backend to use

        // Resolve effective gpu_device: model config > model TOML default
        let effective_gpu_device = resolve_gpu_device(
            model_config.gpu_device.clone(),
            model_toml.and_then(|toml| toml.model.default_gpu_device.clone()),
        );

        // Build a modified backend config with the resolved gpu_device.
        // This ensures build_full_args() sees the effective value.
        let model_config = if effective_gpu_device.is_some() && model_config.gpu_device.is_none() {
            let mut modified = model_config.clone();
            modified.gpu_device = effective_gpu_device;
            modified
        } else {
            model_config.clone()
        };

        // Build full args (including -m, -c, -ngl from model card) and override host/port
        let gpu_variant = model_config
            .gpu_variant
            .clone()
            .unwrap_or(crate::gpu::GpuVariant::CpuOnly);
        let default_args =
            manager.get_default_args(&model_config.backend, gpu_variant.variant_folder());
        let mut args =
            config.build_full_args(&model_config, backend_config, None, &default_args)?;

        tracing::debug!(
            gpu = %model_config.gpu_device.as_deref().unwrap_or("default"),
            "Loading model '{}' with backend '{}'",
            model_name,
            model_config.backend
        );

        override_arg(&mut args, "--host", "127.0.0.1");
        override_arg(&mut args, "--port", &port.to_string());

        let health_url = format!("http://127.0.0.1:{}/health", port);
        let backend_url = format!("http://127.0.0.1:{}", port);

        info!(
            "Starting backend '{}' for backend '{}' (model '{}')",
            model_config.backend, backend_name, model_name
        );

        // Resolve logs directory for backend log file
        let logs_dir = self.config.read().await.logs_dir().ok();

        let mut child = tokio::process::Command::new(&backend_path);
        crate::process::configure_backend_command(&mut child, &backend_path);
        // Inject GPU isolation env var (ROCR/CUDA/GGML_VK_VISIBLE_DEVICES)
        // keyed off the backend's gpu_variant. Uses positional indexes.
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

        // Apply default env vars from backend config (e.g. RADV_PERFTEST=nogttspill)
        let default_env =
            manager.get_default_env(&model_config.backend, gpu_variant.variant_folder());
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

        // Update the PID in the Starting state so cleanup paths can find it
        {
            let mut models = self.models.write().await;
            if let Some(BackendState::Starting { backend_pid, .. }) = models.get_mut(&backend_name)
            {
                *backend_pid = pid;
            }
        }

        // Get the backend log stream for SSE broadcasting — use same key as
        // the dashboard constructs: {backend}_{backend_name}.
        let log_key = format!("{}_{}", model_config.backend, backend_name);
        let log_stream = self.backend_logs.get_or_create(&log_key).await;

        // Open log file for this backend instance — include backend name so
        // multiple models on the same backend get separate log files.
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
                    // std::sync::Mutex is safe here: the critical section (lock + write + unlock)
                    // is fully synchronous and takes microseconds. No .await is held while locked.
                    // A tokio::sync::Mutex would add unnecessary overhead for this pattern.
                    let _ = f.lock().map(|mut fw| {
                        let _ = writeln!(fw, "{line}");
                    });
                }
            });
        });

        // Track spawned tasks (stdout reader, stderr reader, reaper) for clean
        // cancellation on unload. The per-line push_line tasks are too short-lived
        // to track individually.
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

        // Spawn a reaper task so the child process is waited on and doesn't become a zombie
        let reaper_backend = backend_name.clone();
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
            .insert(backend_name.clone(), model_tasks);

        // Wait for health check to pass — single success is enough.
        let timeout = Duration::from_secs(self.config.read().await.proxy.startup_timeout_secs);
        let start = Instant::now();
        let mut health_ok = false;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                warn!(
                    "Startup health check timeout for backend '{}' after {}s, killing process group",
                    backend_name,
                    timeout.as_secs()
                );
                // Kill entire process group, not just parent
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
            // Abort orphan task readers and reaper (JoinSet was already inserted above)
            if let Some(mut tasks) = self.model_tasks.write().await.remove(&backend_name) {
                tasks.abort_all();
            }
            // Clean up the Starting entry so future load_model calls don't short-circuit
            let mut models = self.models.write().await;
            models.remove(&backend_name);
            self.inference_stats.send_modify(|map| {
                map.remove(&backend_name);
            });
            return Err(anyhow::anyhow!(
                "Backend '{}' failed to start for backend '{}' (timeout after {}s)",
                model_config.backend,
                backend_name,
                timeout.as_secs()
            ));
        }

        // Update the loaded model state to Ready, reusing the existing
        // consecutive_failures Arc so external holders keep observing updates.
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get_mut(&backend_name) {
                if let BackendState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    ..
                } = state
                {
                    // Reset the counter on successful start, reuse the Arc
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
                    };
                }
            }
        }

        // Write to DB after model is ready (best-effort)
        if let Some(mgr) = self.model_mgr() {
            let _ = mgr.insert_active(
                &backend_name,
                model_name,
                &model_config.backend,
                pid as i64,
                port as i64,
                &backend_url,
            );
        }

        info!("Backend '{}' loaded successfully", backend_name);
        self.metrics
            .models_loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(backend_name)
    }

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
        let model_configs = self.model_configs.read().await;
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
    ///
    /// `target_gpu_device` is the GPU the new model will be loaded onto.
    /// Only models on the same GPU (matching `Some(x) == Some(x)` or both `None`)
    /// count against the limit and are candidates for eviction.
    ///
    /// This method atomically transitions a Ready model to Unloading (holding
    /// the write lock for only microseconds), then releases the lock before
    /// calling `unload_model()` (which can take up to 5 seconds). This design
    /// prevents both lock contention and race conditions.
    pub async fn evict_lru_if_needed(
        &self,
        target_gpu_device: Option<String>,
    ) -> Result<Option<String>> {
        let config = self.config.read().await;
        let max = config.proxy.max_loaded_models;

        // 0 = unlimited (feature disabled)
        if max == 0 {
            return Ok(None);
        }

        // Collect all Ready backend names AND non-inference backend names while
        // holding the write lock. Non-inference backends (TTS, compaction) are
        // NOT in model_configs (DB), so we must check the runtime `models` map.
        let models = self.models.write().await;
        let ready_backends: Vec<String> = models
            .iter()
            .filter(|(_, s)| matches!(s, BackendState::Ready { .. }))
            .map(|(name, _)| name.clone())
            .collect();

        // Collect names of non-inference backends (TTS, compaction) from the
        // runtime models map. These are NOT in model_configs, so checking only
        // model_configs would miss them (e.g. compaction).
        let non_inference_backends: std::collections::HashSet<String> = models
            .iter()
            .filter(|(_, s)| s.is_non_inference_backend())
            .map(|(name, _)| name.clone())
            .collect();

        // Release the write lock before reading model_configs (avoids deadlock).
        drop(models);

        // Only count LLM (non-TTS, non-compaction) models on the same GPU
        // against the limit.
        let model_configs = self.model_configs.read().await;
        let llm_count = ready_backends
            .iter()
            .filter(|backend_name| {
                // Skip non-inference backends (TTS, compaction)
                if model_configs
                    .get(backend_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    || non_inference_backends.contains(backend_name.as_str())
                {
                    return false;
                }
                // Only count models on the same GPU device
                let model_gpu = model_configs
                    .get(backend_name.as_str())
                    .and_then(|mc| mc.gpu_device.as_ref());
                model_gpu == target_gpu_device.as_ref()
            })
            .count();

        if llm_count < max as usize {
            return Ok(None);
        }

        // Find LRU Ready model among LLM (non-TTS, non-compaction) models
        // on the same GPU only.
        let mut models = self.models.write().await;
        let lru_name = ready_backends
            .iter()
            .filter(|backend_name| {
                // Skip non-inference backends (TTS, compaction)
                if model_configs
                    .get(backend_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    || non_inference_backends.contains(backend_name.as_str())
                {
                    return false;
                }
                // Only consider models on the same GPU device
                let model_gpu = model_configs
                    .get(backend_name.as_str())
                    .and_then(|mc| mc.gpu_device.as_ref());
                model_gpu == target_gpu_device.as_ref()
            })
            .filter_map(|backend_name| models.get(backend_name).map(|s| (backend_name, s)))
            .min_by_key(|(_, s)| s.last_accessed())
            .map(|(name, _)| name.to_string());

        // Atomically transition Ready → Unloading
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
                    };
                }
            }
        }

        drop(models); // Release lock BEFORE calling unload_model (can take 5s)

        if let Some(name) = lru_name {
            self.unload_model(&name).await?;
            Ok(Some(name))
        } else {
            // All models are non-Ready (Starting/Failed/Unloading) — can't evict
            Ok(None)
        }
    }

    /// Unload a backend by stopping its backend process.
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

        let (_backend, pid) = match &state {
            BackendState::Ready {
                backend,
                backend_pid,
                ..
            }
            | BackendState::Unloading {
                backend,
                backend_pid,
                ..
            } => (backend.clone(), *backend_pid),
            _ => {
                return Err(anyhow::anyhow!(
                    "Backend '{}' entered unexpected state during unload (state: {:?})",
                    backend_name,
                    state
                ));
            }
        };

        let gpu_info: String = self
            .model_configs
            .read()
            .await
            .get(backend_name)
            .and_then(|mc| mc.gpu_device.clone())
            .unwrap_or_else(|| "default".to_string());
        info!(gpu = %gpu_info, "Stopping backend '{}'", backend_name);

        // Send SIGTERM for graceful shutdown
        info!("Sending SIGTERM to backend process {}", pid);
        let _ = kill_process(pid).await;

        // Wait up to 5 seconds for the process to exit, polling every 250ms
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
                // Brief wait for SIGKILL to take effect
                tokio::time::sleep(Duration::from_millis(500)).await;
                break;
            }
        }

        // Cancel and join any spawned tasks for this backend (stdout/stderr readers, reaper)
        if let Some(mut tasks) = self.model_tasks.write().await.remove(backend_name) {
            tasks.abort_all();
            // Wait for all tasks to finish (they should exit quickly after abort)
            while tasks.join_next().await.is_some() {}
        }

        // Remove from models
        let mut models = self.models.write().await;
        models.remove(backend_name);

        // Clear stale inference stats for this backend
        self.inference_stats.send_modify(|map| {
            map.remove(backend_name);
        });

        // Write to DB after model is unloaded (best-effort)
        if let Some(mgr) = self.model_mgr() {
            let _ = mgr.remove_active(backend_name);
        }

        info!(gpu = %gpu_info, "Backend '{}' unloaded", backend_name);
        self.metrics
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
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
