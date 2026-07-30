use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::traits::{HealthChecker, PortAllocator, ProcessSpawner};
use crate::proxy::types::{BackendState, ProxyState};
use anyhow::{Context, Result};

impl ProxyState {
    /// Load the compaction backend by spawning the embedded Python server via `uv run`.
    ///
    /// Uses the model registry lifecycle (Starting → Ready/Failed) for state tracking.
    /// Follows the Kokoro TTS pattern for registry registration and state transitions.
    pub async fn load_compaction_backend<H: HealthChecker, S: ProcessSpawner, P: PortAllocator>(
        &self,
        health_checker: &H,
        spawner: &S,
        port_allocator: &P,
    ) -> Result<()> {
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
            let models = self.registry.models.read().await;
            if let Some(state) = models.get("compaction") {
                if state.is_ready() || matches!(state, BackendState::Starting { .. }) {
                    debug!("Compaction backend already loaded/starting");
                    return Ok(());
                }
            }
        }

        // 4. Resolve base directory from self.db_dir first, fall back to Config::base_dir()
        let base_dir = match self.db_dir.clone() {
            Some(dir) => dir,
            None => crate::config::Config::base_dir()
                .with_context(|| "Failed to get config directory")?,
        };
        let server_dir = crate::compaction_server::get_server_dir(&base_dir)
            .with_context(|| "Failed to get compaction server directory")?;

        // 5. Resolve entrypoint
        let server_path = crate::compaction_server::get_server_entrypoint(&compaction, &base_dir)
            .with_context(|| "Failed to resolve compaction server entrypoint")?;

        // 6. Determine port — honor config port, allocate via trait otherwise
        let port = if let Some(p) = compaction.port {
            p
        } else {
            port_allocator
                .allocate_port()
                .with_context(|| "Failed to allocate port for compaction backend")?
        };

        // 7. Register in model registry (Starting reservation)
        {
            let mut models = self.registry.models.write().await;
            models.insert(
                "compaction".to_string(),
                BackendState::Starting {
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
        let args: Vec<String> = vec![
            "run".into(),
            "--project".into(),
            server_dir.to_string_lossy().into_owned(),
            "uvicorn".into(),
            uvicorn_target.clone(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
        ];
        let env: Vec<(&str, String)> = vec![
            ("COMPACTION_PORT", port.to_string()),
            ("COMPACTION_DEVICE", compaction.device.as_str().to_string()),
        ];
        let spawned = match spawner.spawn("uv", &args, &env, Some(&server_dir)).await {
            Ok(s) => s,
            Err(e) => {
                // Clean up the Starting reservation on spawn failure
                let mut models = self.registry.models.write().await;
                models.remove("compaction");
                drop(models);
                self.metrics.modify_inference_stats(|map| {
                    map.remove("compaction");
                });
                return Err(e).with_context(|| {
                    "Failed to spawn compaction server via uv run (install with: pipx install uv)"
                });
            }
        };

        let pid = spawned.pid;

        // 10. Update PID in Starting state
        {
            let mut models = self.registry.models.write().await;
            if let Some(BackendState::Starting { backend_pid, .. }) = models.get_mut("compaction") {
                *backend_pid = pid;
            }
        }

        // 11. Health poll loop — single success is enough.
        let timeout = Duration::from_secs(self.config.read().await.proxy.startup_timeout_secs);
        let start = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                warn!(
                    "Startup health check timeout for compaction backend after {}s, killing process group",
                    timeout.as_secs()
                );
                let _ = spawner.kill_process_group(pid).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
                if crate::process::is_process_group_alive(pid) {
                    let _ = spawner.force_kill_process_group(pid).await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                // Set Failed state
                let mut models = self.registry.models.write().await;
                models.insert(
                    "compaction".to_string(),
                    BackendState::Failed {
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

            if health_checker.check_health(&health_url, Some(5)).await {
                debug!("Health check passed for compaction backend");
                break;
            }
        }

        // 12. Transition to Ready (always reached — timeout returns early)
        {
            let mut models = self.registry.models.write().await;
            if let Some(state) = models.get_mut("compaction") {
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
