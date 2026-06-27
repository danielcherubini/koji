use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::backends::BackendManager;
use crate::proxy::process::{
    check_health, configure_process_group, force_kill_process, force_kill_process_group,
    is_process_alive, is_process_group_alive, kill_process, kill_process_group,
};
use crate::proxy::types::{ModelState, ProxyState};
use anyhow::{Context, Result};

impl ProxyState {
    /// Load a TTS backend (Kokoro-FastAPI) by spawning its uvicorn server.
    ///
    /// This method opens the backend registry, looks up the requested backend,
    /// derives paths from its install directory, finds a free port, and spawns
    /// the Kokoro-FastAPI uvicorn process with appropriate environment variables.
    /// It then performs a health check (polling every 2s, timeout 60s) before
    /// transitioning the model state to Ready.
    pub async fn load_tts_backend(&self, backend_name: &str) -> Result<String> {
        debug!("Loading TTS backend: {}", backend_name);

        // Open manager and look up backend by name
        let base_dir =
            crate::config::Config::base_dir().with_context(|| "Failed to get config directory")?;
        let mgr =
            BackendManager::open(&base_dir).with_context(|| "Failed to open backend manager")?;

        // Discover variant dynamically - TTS backends typically only have one variant
        let variants = mgr
            .list_versions(backend_name, None)
            .with_context(|| format!("Failed to list versions for '{}'", backend_name))?
            .ok_or_else(|| anyhow::anyhow!("Backend '{}' not installed", backend_name))?;

        let variant = variants
            .first()
            .map(|v| v.gpu_variant.clone())
            .unwrap_or_else(|| "cpu".to_string());

        let info = mgr
            .get_active(backend_name, &variant)
            .with_context(|| format!("Backend '{}' not found in manager", backend_name))?
            .ok_or_else(|| anyhow::anyhow!("Backend '{}' not installed", backend_name))?;

        // Derive paths from BackendInfo.path (base_dir = backends/tts_kokoro/).
        // The repo root is the kokoro-fastapi subdirectory, and venv is a sibling.
        let base_path = info.path.as_path();
        let repo_root = base_path.join("kokoro-fastapi");
        let venv_dir = base_path.join("venv");
        let python_bin = venv_dir.join("bin").join("python");

        // Atomically check if already loaded and reserve if not
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get(backend_name) {
                if state.is_ready() || matches!(state, ModelState::Starting { .. }) {
                    debug!("TTS backend '{}' already loaded/starting", backend_name);
                    return Ok(backend_name.to_string());
                }
            }

            // Reserve with Starting state
            models.insert(
                backend_name.to_string(),
                ModelState::Starting {
                    model_name: backend_name.to_string(),
                    backend: info.name.clone(),
                    backend_url: String::new(),
                    backend_pid: 0,
                    last_accessed: Instant::now(),
                    start_time: Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                },
            );
        }

        // Find a free port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let backend_url = format!("http://127.0.0.1:{}", port);
        let health_url = format!("http://127.0.0.1:{}/health", port);

        info!("Starting Kokoro-FastAPI TTS backend on port {}", port);

        // Spawn the uvicorn server process
        let mut child = tokio::process::Command::new(&python_bin);
        configure_process_group(&mut child);
        child
            .args([
                "-m",
                "uvicorn",
                "api.src.main:app",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .current_dir(&repo_root)
            .env("PYTHONPATH", &repo_root)
            .env("MODEL_DIR", "api/src/models")
            .env("VOICES_DIR", "api/src/voices/v1_0");

        let mut child = child.spawn().with_context(|| {
            format!(
                "Failed to spawn Kokoro-FastAPI process: {}",
                python_bin.display()
            )
        })?;

        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Failed to get PID for Kokoro-FastAPI"))?;
        info!("Kokoro-FastAPI started (pid: {:?})", pid);

        // Update the PID in the Starting state so cleanup paths can find it
        {
            let mut models = self.models.write().await;
            if let Some(ModelState::Starting { backend_pid, .. }) = models.get_mut(backend_name) {
                *backend_pid = pid;
            }
        }

        // Spawn a reaper task so the child process is waited on
        let reaper_backend = backend_name.to_string();
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    debug!(
                        "Kokoro-FastAPI process {} for backend '{}' exited with {}",
                        pid, reaper_backend, status
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to wait on Kokoro-FastAPI process {} for backend '{}': {}",
                        pid, reaper_backend, e
                    );
                }
            }
        });

        // Health check: poll every 500ms, single success is enough.
        let timeout = Duration::from_secs(self.config.read().await.proxy.startup_timeout_secs);
        let start = Instant::now();
        let mut health_ok = false;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                warn!(
                    "Startup health check timeout for TTS backend '{}' after {}s, killing process group",
                    backend_name, timeout.as_secs()
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
                    debug!("Health check passed for TTS backend '{}'", backend_name);
                    health_ok = true;
                    break;
                }
            }
        }

        if !health_ok {
            let mut models = self.models.write().await;
            models.remove(backend_name);
            self.inference_stats.send_modify(|map| {
                map.remove(backend_name);
            });
            return Err(anyhow::anyhow!(
                "Kokoro-FastAPI failed to start for backend '{}' (timeout after {}s)",
                backend_name,
                timeout.as_secs()
            ));
        }

        // Update to Ready state
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get_mut(backend_name) {
                if let ModelState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    model_name,
                    ..
                } = state
                {
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = ModelState::Ready {
                        model_name: model_name.clone(),
                        backend: info.name.clone(),
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

        info!("TTS backend '{}' loaded successfully", backend_name);
        self.metrics
            .models_loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(backend_name.to_string())
    }

    /// Unload a TTS backend by stopping its subprocess.
    ///
    /// Sends SIGTERM for graceful shutdown, waits up to 5s, then SIGKILL if needed.
    pub async fn unload_tts_backend(&self, backend_name: &str) -> Result<()> {
        debug!("Unloading TTS backend: {}", backend_name);

        let state = self
            .get_model_state(backend_name)
            .await
            .with_context(|| format!("TTS backend '{}' not loaded", backend_name))?;

        if !matches!(
            state,
            ModelState::Ready { .. } | ModelState::Unloading { .. }
        ) {
            return Err(anyhow::anyhow!(
                "TTS backend '{}' is not ready (state: {:?})",
                backend_name,
                state
            ));
        }

        let pid = match &state {
            ModelState::Ready { backend_pid, .. } => *backend_pid,
            ModelState::Unloading { backend_pid, .. } => *backend_pid,
            _ => unreachable!("already checked above"),
        };

        info!("Stopping Kokoro-FastAPI (pid: {})", pid);

        // Send SIGTERM for graceful shutdown
        let _ = kill_process(pid).await;

        // Wait up to 5 seconds for the process to exit, polling every 250ms
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !is_process_alive(pid) {
                debug!("Kokoro-FastAPI exited gracefully");
                break;
            }
            if Instant::now() >= deadline {
                warn!("Kokoro-FastAPI did not exit after SIGTERM, sending SIGKILL",);
                let _ = force_kill_process(pid).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                break;
            }
        }

        // Remove from models
        self.models.write().await.remove(backend_name);

        info!("TTS backend '{}' unloaded", backend_name);
        self.metrics
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Check if a TTS backend is loaded and ready.
    ///
    /// Returns the backend name if found in Ready state, None otherwise.
    pub async fn get_tts_server(&self, backend_name: &str) -> Option<String> {
        let models = self.models.read().await;
        if let Some(state) = models.get(backend_name) {
            if state.is_ready() {
                return Some(backend_name.to_string());
            }
        }
        None
    }
}
