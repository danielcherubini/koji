use anyhow::{Context, Result};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;
use tracing::{debug, info, warn};

use super::process::{
    check_health, configure_process_group, force_kill_process, force_kill_process_group,
    is_process_alive, is_process_group_alive, kill_process, kill_process_group, override_arg,
};
use super::types::{ModelState, ProxyState};
use crate::logging;

mod compaction;
mod idle_timeout;
mod tts;

impl ProxyState {
    /// Load a model by starting its backend process.
    pub async fn load_model(
        &self,
        model_name: &str,
        model_card: Option<&crate::models::card::ModelCard>,
    ) -> Result<String> {
        debug!("Loading model: {}", model_name);

        let config = self.config.read().await.clone();

        // Resolve the server name for this model
        let model_configs = self.model_configs.read().await;
        let servers = config.resolve_servers_for_model(&model_configs, model_name);
        let server_name = servers
            .first()
            .map(|(name, _, _)| name.clone())
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve server for model {}", model_name))?;

        // Get server and backend config from config
        let (server_config, backend_config) =
            config.resolve_server(&model_configs, &server_name)?;

        // Atomically check if already loaded and reserve if not (single write lock)
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get(&server_name) {
                if state.is_ready() || matches!(state, ModelState::Starting { .. }) {
                    debug!(
                        "Server '{}' already loaded/starting for model '{}'",
                        server_name, model_name
                    );
                    return Ok(server_name);
                }
            }

            // Reserve this server with Starting state
            models.insert(
                server_name.clone(),
                ModelState::Starting {
                    model_name: model_name.to_string(),
                    backend: server_config.backend.clone(),
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
            &server_config.backend,
            server_config.gpu_variant.as_deref(),
            &manager,
        )?;

        // Find a free port for this backend.
        // Note: there is a small race window between dropping the listener and the
        // backend binding to the port. This is an accepted trade-off for local use;
        // in practice port collisions are extremely rare.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener); // Free the port for the backend to use

        // Resolve effective gpu_device: model config > model card default
        let effective_gpu_device = _resolve_gpu_device(
            server_config.gpu_device.clone(),
            model_card.and_then(|card| card.model.default_gpu_device.clone()),
        );

        // Build a modified server config with the resolved gpu_device.
        // This ensures build_full_args() sees the effective value.
        let server_config = if effective_gpu_device.is_some() && server_config.gpu_device.is_none()
        {
            let mut modified = server_config.clone();
            modified.gpu_device = effective_gpu_device;
            modified
        } else {
            server_config.clone()
        };

        // Build full args (including -m, -c, -ngl from model card) and override host/port
        let gpu_variant = server_config.gpu_variant.as_deref().unwrap_or("cpu");
        let default_args = manager.get_default_args(&server_config.backend, gpu_variant);
        let mut args =
            config.build_full_args(&server_config, backend_config, None, &default_args)?;

        // Map position-based GPU device (e.g. "GPU0") to backend device name
        // (e.g. "CUDA0", "ROCm0", "Vulkan0") via --list-devices discovery.
        if let Some(mapped_device) =
            resolve_gpu_device_to_backend_name(&backend_path, &server_config.gpu_device)
                .ok()
                .flatten()
        {
            override_arg(&mut args, "--device", &mapped_device);
        }

        override_arg(&mut args, "--host", "127.0.0.1");
        override_arg(&mut args, "--port", &port.to_string());

        let health_url = format!("http://127.0.0.1:{}/health", port);
        let backend_url = format!("http://127.0.0.1:{}", port);

        info!(
            "Starting backend '{}' for server '{}' (model '{}')",
            server_config.backend, server_name, model_name
        );

        // Resolve logs directory for backend log file
        let logs_dir = self.config.read().await.logs_dir().ok();

        let mut child = tokio::process::Command::new(&backend_path);
        crate::process::configure_backend_command(&mut child, &backend_path);
        configure_process_group(&mut child);
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
                server_config.backend
            )
        })?;

        let pid = child.id().ok_or_else(|| {
            anyhow::anyhow!("Failed to get PID for backend '{}'", server_config.backend)
        })?;
        info!(
            "Backend '{}' started for server '{}' (pid: {:?})",
            server_config.backend, server_name, pid
        );

        // Update the PID in the Starting state so cleanup paths can find it
        {
            let mut models = self.models.write().await;
            if let Some(ModelState::Starting { backend_pid, .. }) = models.get_mut(&server_name) {
                *backend_pid = pid;
            }
        }

        // Get the backend log stream for SSE broadcasting — use same key as
        // the dashboard constructs: {backend}_{server_name}.
        let log_key = format!("{}_{}", server_config.backend, server_name);
        let log_stream = self.backend_logs.get_or_create(&log_key).await;

        // Open log file for this backend instance — include server name so
        // multiple models on the same backend get separate log files.
        let log_name = format!("{}_{}", server_config.backend, server_name);
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

        // Stream stdout
        if let Some(stdout) = child.stdout.take() {
            let push = push_line.clone();
            tokio::spawn(async move {
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
            tokio::spawn(async move {
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    push(line);
                }
            });
        }

        // Spawn a reaper task so the child process is waited on and doesn't become a zombie
        let reaper_server = server_name.clone();
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    debug!(
                        "Backend process {} for server '{}' exited with {}",
                        pid, reaper_server, status
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to wait on backend process {} for server '{}': {}",
                        pid, reaper_server, e
                    );
                }
            }
        });

        // Wait for health check to pass — single success is enough.
        let timeout = Duration::from_secs(self.config.read().await.proxy.startup_timeout_secs);
        let start = Instant::now();
        let mut health_ok = false;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                warn!(
                    "Startup health check timeout for server '{}' after {}s, killing process group",
                    server_name,
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

            if let Ok(response) = check_health(&health_url, Some(5)).await {
                if response.status().is_success() {
                    debug!("Health check passed for server '{}'", server_name);
                    health_ok = true;
                    break;
                }
            }
        }

        if !health_ok {
            // Clean up the Starting entry so future load_model calls don't short-circuit
            let mut models = self.models.write().await;
            models.remove(&server_name);
            return Err(anyhow::anyhow!(
                "Backend '{}' failed to start for server '{}' (timeout after {}s)",
                server_config.backend,
                server_name,
                timeout.as_secs()
            ));
        }

        // Update the loaded model state to Ready, reusing the existing
        // consecutive_failures Arc so external holders keep observing updates.
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get_mut(&server_name) {
                if let ModelState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    ..
                } = state
                {
                    // Reset the counter on successful start, reuse the Arc
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = ModelState::Ready {
                        model_name: model_name.to_string(),
                        backend: server_config.backend.clone(),
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
                &server_name,
                model_name,
                &server_config.backend,
                pid as i64,
                port as i64,
                &backend_url,
            );
        }

        info!("Server '{}' loaded successfully", server_name);
        self.metrics
            .models_loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(server_name)
    }

    /// Evict the least-recently-used Ready model if the proxy is at capacity.
    ///
    /// This method atomically transitions a Ready model to Unloading (holding
    /// the write lock for only microseconds), then releases the lock before
    /// calling `unload_model()` (which can take up to 5 seconds). This design
    /// prevents both lock contention and race conditions.
    pub async fn evict_lru_if_needed(&self) -> Result<Option<String>> {
        let config = self.config.read().await;
        let max = config.proxy.max_loaded_models;

        // 0 = unlimited (feature disabled)
        if max == 0 {
            return Ok(None);
        }

        // Collect all Ready server names AND non-inference server names while
        // holding the write lock. Non-inference backends (TTS, compaction) are
        // NOT in model_configs (DB), so we must check the runtime `models` map.
        let models = self.models.write().await;
        let ready_servers: Vec<String> = models
            .iter()
            .filter(|(_, s)| matches!(s, ModelState::Ready { .. }))
            .map(|(name, _)| name.clone())
            .collect();

        // Collect names of non-inference backends (TTS, compaction) from the
        // runtime models map. These are NOT in model_configs, so checking only
        // model_configs would miss them (e.g. compaction).
        let non_inference_servers: std::collections::HashSet<String> = models
            .iter()
            .filter(|(_, s)| s.is_non_inference_backend())
            .map(|(name, _)| name.clone())
            .collect();

        // Release the write lock before reading model_configs (avoids deadlock).
        drop(models);

        // Only count LLM (non-TTS, non-compaction) models against the limit.
        let model_configs = self.model_configs.read().await;
        let llm_count = ready_servers
            .iter()
            .filter(|server_name| {
                !model_configs
                    .get(server_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    && !non_inference_servers.contains(server_name.as_str())
            })
            .count();

        if llm_count < max as usize {
            return Ok(None);
        }

        // Find LRU Ready model among LLM (non-TTS, non-compaction) models only.
        let mut models = self.models.write().await;
        let lru_name = ready_servers
            .iter()
            .filter(|server_name| {
                !model_configs
                    .get(server_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_") || mc.backend == "compaction")
                    && !non_inference_servers.contains(server_name.as_str())
            })
            .filter_map(|server_name| models.get(server_name).map(|s| (server_name, s)))
            .min_by_key(|(_, s)| s.last_accessed())
            .map(|(name, _)| name.to_string());

        // Atomically transition Ready → Unloading
        if let Some(ref name) = lru_name {
            if let Some(state) = models.get_mut(name) {
                if let ModelState::Ready {
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
                    *state = ModelState::Unloading {
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

    /// Unload a server by stopping its backend process.
    pub async fn unload_model(&self, server_name: &str) -> Result<()> {
        debug!("Unloading server: {}", server_name);

        let state = self
            .get_model_state(server_name)
            .await
            .with_context(|| format!("Server '{}' not loaded", server_name))?;

        if !matches!(
            state,
            ModelState::Ready { .. } | ModelState::Unloading { .. }
        ) {
            return Err(anyhow::anyhow!(
                "Server '{}' is not ready (state: {:?})",
                server_name,
                state
            ));
        }

        let (backend_name, pid) = match &state {
            ModelState::Ready {
                backend,
                backend_pid,
                ..
            }
            | ModelState::Unloading {
                backend,
                backend_pid,
                ..
            } => (backend.clone(), *backend_pid),
            _ => unreachable!("already checked above"),
        };

        info!(
            "Stopping backend '{}' for server '{}'",
            backend_name, server_name
        );

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

        // Remove from models
        let mut models = self.models.write().await;
        models.remove(server_name);

        // Write to DB after model is unloaded (best-effort)
        if let Some(mgr) = self.model_mgr() {
            let _ = mgr.remove_active(server_name);
        }

        info!("Server '{}' unloaded", server_name);
        self.metrics
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

/// Resolve the effective GPU device from the fallback chain:
/// model config > model card default.
fn _resolve_gpu_device(config: Option<String>, card_default: Option<String>) -> Option<String> {
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

/// Map a position-based GPU device ID (e.g. "GPU0") to the backend's actual
/// device name (e.g. "CUDA0", "ROCm0", "Vulkan0") by discovering devices
/// via `<backend-binary> --list-devices`.
///
/// Returns None if:
/// - `gpu_device` is None or not in "GPU_N" format
/// - Device discovery fails
/// - The requested index is out of range
fn resolve_gpu_device_to_backend_name(
    backend_path: &std::path::Path,
    gpu_device: &Option<String>,
) -> Result<Option<String>> {
    let Some(device) = gpu_device else {
        return Ok(None);
    };
    let device = device.trim();

    // Only map position-based IDs (GPU0, GPU1, ...). Pass through raw names.
    let Some(stripped) = device.strip_prefix("GPU") else {
        // Not a position-based ID — pass through as-is (e.g. "CUDA0").
        return Ok(Some(device.to_string()));
    };
    let Ok(index) = stripped.parse::<usize>() else {
        return Ok(Some(device.to_string()));
    };

    // Discover devices via backend binary.
    let devices = crate::gpu::discover_devices_via_binary(backend_path)?;
    let count = devices.len();

    // Find the device at the requested index.
    let Some(device_info) = devices.into_iter().nth(index) else {
        tracing::warn!(
            "GPU{} requested but only {} device(s) discovered",
            index,
            count
        );
        return Ok(None);
    };

    Ok(Some(device_info.device_id))
}

#[cfg(test)]
mod tests;
