use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::traits::{HealthChecker, PortAllocator, ProcessSpawner};
use crate::backends::BackendManager;
use crate::process::{force_kill_process, is_process_alive, kill_process};
use crate::proxy::types::{BackendState, ProxyState};
use anyhow::{Context, Result};

impl ProxyState {
    /// Load a TTS backend (Kokoro-FastAPI) by spawning its uvicorn server.
    ///
    /// This method opens the backend registry, looks up the requested backend,
    /// derives paths from its install directory, finds a free port, and spawns
    /// the Kokoro-FastAPI uvicorn process with appropriate environment variables.
    /// It then performs a health check (polling every 500ms, timeout configurable)
    /// before transitioning the model state to Ready.
    pub async fn load_tts_backend<H: HealthChecker, S: ProcessSpawner, P: PortAllocator>(
        &self,
        backend_name: &str,
        health_checker: &H,
        spawner: &S,
        port_allocator: &P,
    ) -> Result<String> {
        debug!("Loading TTS backend: {}", backend_name);

        // Resolve base directory from self.db_dir first, fall back to Config::base_dir()
        let base_dir = match self.db_dir.clone() {
            Some(dir) => dir,
            None => crate::config::Config::base_dir()
                .with_context(|| "Failed to get config directory")?,
        };
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
            let mut models = self.registry.models.write().await;
            if let Some(state) = models.get(backend_name) {
                if state.is_ready() || matches!(state, BackendState::Starting { .. }) {
                    debug!("TTS backend '{}' already loaded/starting", backend_name);
                    return Ok(backend_name.to_string());
                }
            }

            // Reserve with Starting state
            models.insert(
                backend_name.to_string(),
                BackendState::Starting {
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

        // Allocate port via trait
        let port = port_allocator.allocate_port()?;

        let backend_url = format!("http://127.0.0.1:{}", port);
        let health_url = format!("http://127.0.0.1:{}/health", port);

        info!("Starting Kokoro-FastAPI TTS backend on port {}", port);

        // Spawn the uvicorn server process via trait
        let args: Vec<String> = vec![
            "-m".into(),
            "uvicorn".into(),
            "api.src.main:app".into(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
        ];
        let env: Vec<(&str, String)> = vec![
            ("PYTHONPATH", repo_root.to_string_lossy().into_owned()),
            ("MODEL_DIR", "api/src/models".into()),
            ("VOICES_DIR", "api/src/voices/v1_0".into()),
        ];
        let spawned = match spawner
            .spawn(
                &python_bin.to_string_lossy(),
                &args,
                &env,
                Some(repo_root.as_path()),
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                // Clean up the Starting reservation on spawn failure
                let mut models = self.registry.models.write().await;
                models.remove(backend_name);
                drop(models);
                self.metrics.modify_inference_stats(|map| {
                    map.remove(backend_name);
                });
                return Err(e).with_context(|| {
                    format!(
                        "Failed to spawn Kokoro-FastAPI process: {}",
                        python_bin.display()
                    )
                });
            }
        };

        let pid = spawned.pid;
        info!("Kokoro-FastAPI started (pid: {:?})", pid);

        // Update the PID in the Starting state so cleanup paths can find it
        {
            let mut models = self.registry.models.write().await;
            if let Some(BackendState::Starting { backend_pid, .. }) = models.get_mut(backend_name) {
                *backend_pid = pid;
            }
        }

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
                let _ = spawner.kill_process_group(pid).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
                if crate::process::is_process_group_alive(pid) {
                    warn!("Process group {} still alive, sending SIGKILL", pid);
                    let _ = spawner.force_kill_process_group(pid).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                break;
            }

            if health_checker.check_health(&health_url, Some(5)).await {
                debug!("Health check passed for TTS backend '{}'", backend_name);
                health_ok = true;
                break;
            }
        }

        if !health_ok {
            let mut models = self.registry.models.write().await;
            models.remove(backend_name);
            self.metrics.modify_inference_stats(|map| {
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
            let mut models = self.registry.models.write().await;
            if let Some(state) = models.get_mut(backend_name) {
                if let BackendState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    model_name,
                    ..
                } = state
                {
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = BackendState::Ready {
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
            .counters
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
            BackendState::Ready { .. } | BackendState::Unloading { .. }
        ) {
            return Err(anyhow::anyhow!(
                "TTS backend '{}' is not ready (state: {:?})",
                backend_name,
                state
            ));
        }

        let pid = match &state {
            BackendState::Ready { backend_pid, .. } => *backend_pid,
            BackendState::Unloading { backend_pid, .. } => *backend_pid,
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
        self.registry.models.write().await.remove(backend_name);

        info!("TTS backend '{}' unloaded", backend_name);
        self.metrics
            .counters
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}
