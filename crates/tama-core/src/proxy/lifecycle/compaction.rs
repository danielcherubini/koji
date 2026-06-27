use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::proxy::process::{
    check_health, configure_process_group, force_kill_process_group, is_process_group_alive,
    kill_process_group,
};
use crate::proxy::types::{ModelState, ProxyState};
use anyhow::{Context, Result};

impl ProxyState {
    /// Load the compaction backend by spawning the embedded Python server via `uv run`.
    ///
    /// Uses the model registry lifecycle (Starting → Ready/Failed) for state tracking.
    /// Follows the Kokoro TTS pattern for registry registration and state transitions.
    pub async fn load_compaction_backend(&self) -> Result<()> {
        // 1. Read config (scoped read lock)
        let compaction = {
            let config = self.config.read().await;
            config.compaction.clone()
        };

        // 2. Check enabled
        if !compaction.enabled {
            return Err(anyhow::anyhow!("Compaction is not enabled in config"));
        }

        // 3. Fast path — already starting or ready
        {
            let models = self.models.read().await;
            if let Some(state) = models.get("compaction") {
                if state.is_ready() || matches!(state, ModelState::Starting { .. }) {
                    debug!("Compaction backend already loaded/starting");
                    return Ok(());
                }
            }
        }

        // 4. Extract embedded files
        let base_dir =
            crate::config::Config::base_dir().with_context(|| "Failed to get config directory")?;
        let server_dir = crate::compaction_server::get_server_dir(&base_dir)
            .with_context(|| "Failed to get compaction server directory")?;

        // 5. Resolve entrypoint
        let server_path = crate::compaction_server::get_server_entrypoint(&compaction, &base_dir)
            .with_context(|| "Failed to resolve compaction server entrypoint")?;

        // 6. Determine port — honor config port, auto-assign otherwise
        let port = if let Some(p) = compaction.port {
            p
        } else {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .with_context(|| "Failed to bind TcpListener for port assignment")?;
            let p = listener.local_addr()?.port();
            drop(listener); // Free the port for the backend
            p
        };

        // 7. Register in model registry
        {
            let mut models = self.models.write().await;
            models.insert(
                "compaction".to_string(),
                ModelState::Starting {
                    model_name: "compaction".to_string(),
                    backend: "compaction".to_string(),
                    backend_url: String::new(),
                    backend_pid: 0,
                    last_accessed: Instant::now(),
                    start_time: Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                },
            );
        }

        // 8. Derive uvicorn target from entrypoint filename
        let module_name = server_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();
        let uvicorn_target = format!("{}:app", module_name);

        let backend_url = format!("http://127.0.0.1:{}", port);
        let health_url = format!("{}/health", backend_url);

        info!("Starting compaction backend on port {}", port);

        // 9. Spawn via `uv run` (uses project venv so deps are available)
        let mut child = tokio::process::Command::new("uv");
        configure_process_group(&mut child);
        child
            .arg("run")
            .arg("--project")
            .arg(&server_dir)
            .arg("uvicorn")
            .arg(&uvicorn_target)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .env("COMPACTION_PORT", port.to_string())
            .env("COMPACTION_DEVICE", &compaction.device)
            .current_dir(&server_dir);

        let mut child = child.spawn().with_context(|| {
            "Failed to spawn compaction server via uv run (install with: pipx install uv)"
                .to_string()
        })?;

        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Failed to get PID for compaction server"))?;

        // 10. Update PID in Starting state
        {
            let mut models = self.models.write().await;
            if let Some(ModelState::Starting { backend_pid, .. }) = models.get_mut("compaction") {
                *backend_pid = pid;
            }
        }

        // 11. Spawn reaper task
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    debug!("Compaction server process {} exited with {}", pid, status);
                }
                Err(e) => {
                    warn!("Failed to wait on compaction server process {}: {}", pid, e);
                }
            }
        });

        // 12. Health poll loop — single success is enough.
        let timeout = Duration::from_secs(self.config.read().await.proxy.startup_timeout_secs);
        let start = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                warn!(
                    "Startup health check timeout for compaction backend after {}s, killing process group",
                    timeout.as_secs()
                );
                let _ = kill_process_group(pid).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
                if is_process_group_alive(pid) {
                    let _ = force_kill_process_group(pid).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                // Set Failed state
                let mut models = self.models.write().await;
                models.insert(
                    "compaction".to_string(),
                    ModelState::Failed {
                        model_name: "compaction".to_string(),
                        backend: "compaction".to_string(),
                        error: format!("Startup timeout after {}s", timeout.as_secs()),
                    },
                );
                return Err(anyhow::anyhow!(
                    "Compaction backend failed to start (timeout after {}s)",
                    timeout.as_secs()
                ));
            }

            if let Ok(response) = check_health(&health_url, Some(5)).await {
                if response.status().is_success() {
                    debug!("Health check passed for compaction backend");
                    break;
                }
            }
        }

        // 13. Transition to Ready (always reached — timeout returns early)
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get_mut("compaction") {
                if let ModelState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    ..
                } = state
                {
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = ModelState::Ready {
                        model_name: "compaction".to_string(),
                        backend: "compaction".to_string(),
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
            info!("Compaction backend loaded successfully on {}", backend_url);
        }

        Ok(())
    }
}
