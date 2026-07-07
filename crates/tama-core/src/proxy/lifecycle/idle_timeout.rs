use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::proxy::process::{
    check_health, force_kill_process_group, is_process_alive, is_process_group_alive,
    kill_process_group,
};
use crate::proxy::types::{BackendState, ProxyState};

impl ProxyState {
    /// Check if any server has been idle for longer than the timeout.
    ///
    /// Also performs process health monitoring:
    /// - Detects dead PIDs in Ready models and confirms via health endpoint
    /// - Transitions stuck Starting models to Failed
    /// - Auto-restarts dead models (respecting max_restarts and restart_delay_ms)
    /// - Cleans up Failed models
    pub async fn check_idle_timeouts(&self) -> Vec<String> {
        let now = Instant::now();
        let mut to_unload = Vec::new();
        let mut failed_to_remove = Vec::new();
        // (backend_name, model_name, backend, restart_count, pid, backend_url)
        let mut dead_pid_candidates: Vec<(String, String, String, u32, u32, String)> = Vec::new();
        // (backend_name, model_name, backend, start_time, pid)
        let mut stuck_starting_servers: Vec<(String, String, String, Instant, u32)> = Vec::new();

        let (auto_unload, idle_timeout_secs, startup_timeout_secs, max_restarts, restart_delay_ms) = {
            let cfg = self.config.read().await;
            (
                cfg.proxy.auto_unload,
                cfg.proxy.idle_timeout_secs,
                cfg.proxy.startup_timeout_secs,
                cfg.supervisor.max_restarts,
                cfg.supervisor.restart_delay_ms,
            )
        };

        let idle_timeout = Duration::from_secs(idle_timeout_secs);
        let startup_timeout = Duration::from_secs(startup_timeout_secs);

        // === PHASE 1: Collect candidates under read lock (fast only) ===
        let models = self.models.read().await;
        for (backend_name, state) in models.iter() {
            // Check Starting state first (including TTS — they can also get stuck)
            if let BackendState::Starting { start_time, .. } = state {
                if now.saturating_duration_since(*start_time) > startup_timeout {
                    warn!(
                        "Server '{}' stuck in Starting for {}s (timeout: {}s)",
                        backend_name,
                        now.saturating_duration_since(*start_time).as_secs(),
                        startup_timeout_secs,
                    );
                    stuck_starting_servers.push((
                        backend_name.clone(),
                        state.model_name().to_string(),
                        state.backend().to_string(),
                        *start_time,
                        state.backend_pid().unwrap_or(0),
                    ));
                }
                continue;
            }

            // Skip Unloading — already being handled
            if matches!(state, BackendState::Unloading { .. }) {
                continue;
            }

            // Skip non-inference backends (TTS, compaction) for Ready checks (separate lifecycle)
            // Starting states for all backends were already checked above
            if state.is_non_inference_backend() {
                continue;
            }

            // Ready models — check PID liveness (fast syscall, OK under lock)
            if let BackendState::Ready {
                backend_pid,
                restart_count,
                ..
            } = state
            {
                let pid = *backend_pid;
                if !is_process_alive(pid) {
                    dead_pid_candidates.push((
                        backend_name.clone(),
                        state.model_name().to_string(),
                        state.backend().to_string(),
                        *restart_count,
                        pid,
                        state
                            .backend_url()
                            .map(|u| u.to_string())
                            .unwrap_or_default(),
                    ));
                    continue; // Skip idle check — process is dead
                }

                // Process alive — check idle timeout (existing logic)
                if let Some(last) = state.last_accessed() {
                    let idle_duration = now.saturating_duration_since(last);
                    if auto_unload && idle_duration > idle_timeout {
                        warn!(
                            "Server '{}' idle for {}s (timeout: {}s)",
                            backend_name,
                            idle_duration.as_secs(),
                            idle_timeout_secs
                        );
                        to_unload.push(backend_name.clone());
                    }
                }
            }

            // Failed models — mark for cleanup
            if matches!(state, BackendState::Failed { .. }) {
                warn!(
                    "Server '{}' in Failed state, marking for cleanup",
                    backend_name
                );
                failed_to_remove.push(backend_name.clone());
            }
        }
        drop(models); // Release read lock

        // === PHASE 2: Health confirmation (outside lock) ===
        // (backend_name, model_name, backend, restart_count, pid)
        let mut confirmed_dead: Vec<(String, String, String, u32, u32)> = Vec::new();
        for (backend_name, model_name, backend, restart_count, pid, backend_url) in
            dead_pid_candidates
        {
            let health_url = format!("{}/health", backend_url);
            let still_dead = match check_health(&health_url, Some(5)).await {
                Ok(resp) => !resp.status().is_success(),
                Err(_) => true,
            };

            if still_dead {
                info!(
                    "Server '{}' confirmed dead (pid {}, restart_count: {}/{})",
                    backend_name, pid, restart_count, max_restarts
                );
                confirmed_dead.push((backend_name, model_name, backend, restart_count, pid));
            } else {
                debug!(
                    "Server '{}' PID {} reused, health endpoint responds",
                    backend_name, pid
                );
            }
        }

        // === PHASE 3: Mutations ===

        // Remove Failed models
        if !failed_to_remove.is_empty() {
            let mut models = self.models.write().await;
            for backend_name in &failed_to_remove {
                models.remove(backend_name);
                self.inference_stats.send_modify(|map| {
                    map.remove(backend_name);
                });
                info!("Removed failed server '{}' from model map", backend_name);
            }
        }

        // Handle stuck Starting — transition to Failed and kill orphaned process groups
        if !stuck_starting_servers.is_empty() {
            let mut pids_to_clean: Vec<(String, u32)> = Vec::new();
            {
                let mut models = self.models.write().await;
                for (backend_name, model_name, backend, observed_start, observed_pid) in
                    &stuck_starting_servers
                {
                    // Revalidate: only transition if still in Starting state with matching start_time
                    // (could have become Ready between Phase 1 and Phase 3)
                    if let Some(existing) = models.get(backend_name) {
                        let still_starting = matches!(existing, BackendState::Starting { start_time, .. } if start_time == observed_start);
                        if !still_starting {
                            debug!(
                                "Server '{}' state or start_time changed, skipping stuck transition",
                                backend_name
                            );
                            continue;
                        }
                    }
                    models.insert(
                        backend_name.clone(),
                        BackendState::Failed {
                            model_name: model_name.clone(),
                            backend: backend.clone(),
                            error: format!(
                                "Stuck in Starting state for {}s — backend failed to initialize",
                                startup_timeout_secs
                            ),
                        },
                    );
                    warn!(
                        "Transitioned '{}' to Failed (stuck in Starting)",
                        backend_name
                    );
                    pids_to_clean.push((backend_name.clone(), *observed_pid));
                }
            }
            // Kill orphaned process groups outside the write lock
            for (backend_name, pid) in pids_to_clean {
                if pid > 0 {
                    warn!(
                        "Killing orphaned process group {} for stuck server '{}'",
                        pid, backend_name
                    );
                    let _ = kill_process_group(pid).await;
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    if is_process_group_alive(pid) {
                        let _ = force_kill_process_group(pid).await;
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }

        // Handle dead Ready servers — clean up + insert Failed or spawn restart
        if !confirmed_dead.is_empty() {
            // Remove + insert Failed under SAME lock — no race
            // Revalidate state under lock to avoid TOCTOU with forward_request()
            let mut to_restart: Vec<(String, String, u32)> = Vec::new();
            let mut removed_servers: Vec<String> = Vec::new();
            {
                let mut models = self.models.write().await;
                for (backend_name, model_name, backend, restart_count, observed_pid) in
                    &confirmed_dead
                {
                    // Revalidate: only act if still Ready with matching PID
                    // (could have been replaced by forward_request() auto-load)
                    let pid_matches = models.get(backend_name).and_then(|s| match s {
                        BackendState::Ready { backend_pid, .. } => {
                            if backend_pid == observed_pid {
                                Some(true)
                            } else {
                                // Different PID — process was replaced, skip
                                None
                            }
                        }
                        BackendState::Starting { .. } => {
                            // Already being restarted by another path, skip
                            None
                        }
                        _ => None, // Failed, Unloading, or absent — skip
                    });

                    if pid_matches.unwrap_or(false) {
                        models.remove(backend_name);
                        self.inference_stats.send_modify(|map| {
                            map.remove(backend_name);
                        });
                        removed_servers.push(backend_name.clone());
                        if *restart_count >= max_restarts {
                            models.insert(
                                backend_name.clone(),
                                BackendState::Failed {
                                    model_name: model_name.clone(),
                                    backend: backend.clone(),
                                    error: format!(
                                        "Exceeded maximum restart attempts ({}) — manual intervention required",
                                        max_restarts
                                    ),
                                },
                            );
                            warn!(
                                "Server '{}' exceeded max restarts ({}/{})",
                                backend_name, restart_count, max_restarts
                            );
                        } else {
                            to_restart.push((
                                backend_name.clone(),
                                model_name.clone(),
                                *restart_count,
                            ));
                        }
                    } else {
                        debug!(
                            "Server '{}' state changed during health check, skipping cleanup",
                            backend_name
                        );
                    }
                }
            }
            // Clean DB — remove ALL dead entries so cleanup_stale_processes()
            // doesn't rediscover them, regardless of whether they'll be restarted
            if let Some(mgr) = self.model_mgr() {
                for backend_name in &removed_servers {
                    let _ = mgr.remove_active(backend_name);
                }
            }

            // Spawn restart tasks (no locks)
            for (backend_name, model_name, restart_count) in &to_restart {
                let new_restart_count = restart_count + 1;
                info!(
                    "Auto-restarting '{}' (model '{}', attempt {}/{})",
                    backend_name, model_name, new_restart_count, max_restarts
                );

                let state = self.clone();
                let sn = backend_name.clone();
                let mn = model_name.clone();
                let rdc = new_restart_count;
                let delay_ms = restart_delay_ms;
                // Total timeout: delay + startup_timeout_secs. Prevents a stuck
                // restart from holding resources forever (also keeps tests from
                // hanging when there's no real backend to load).
                let total_timeout = Duration::from_millis(delay_ms) + startup_timeout;
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    match tokio::time::timeout(total_timeout, state.load_model(&mn, None)).await {
                        Ok(Ok(_)) => {
                            let mut models = state.models.write().await;
                            if let Some(BackendState::Ready {
                                restart_count: rc, ..
                            }) = models.get_mut(&sn)
                            {
                                *rc = rdc;
                            }
                            info!("Auto-restart succeeded for '{}' (model '{}')", sn, mn);
                        }
                        Ok(Err(e)) => {
                            warn!("Auto-restart failed for '{}' (model '{}'): {}", sn, mn, e);
                        }
                        Err(_) => {
                            warn!(
                                "Auto-restart timed out for '{}' (model '{}') after {:?}",
                                sn, mn, total_timeout
                            );
                        }
                    }
                });
            }
        }

        // Unload idle models (existing logic)
        for backend_name in &to_unload {
            if let Err(e) = self.unload_model(backend_name).await {
                warn!("Failed to unload '{}': {}", backend_name, e);
            }
        }

        // Build return value
        let mut cleaned = Vec::new();
        cleaned.extend(failed_to_remove);
        cleaned.extend(
            stuck_starting_servers
                .iter()
                .map(|(n, _, _, _, _)| n.clone()),
        );
        cleaned.extend(confirmed_dead.iter().map(|(n, _, _, _, _)| n.clone()));
        cleaned.extend(to_unload);
        cleaned
    }
}
