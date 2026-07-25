use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::config::HealthCheck;
use crate::logging;

/// Configure a Command to find companion shared libraries alongside the binary.
///
/// Sets the working directory and LD_LIBRARY_PATH (on Unix) to the binary's
/// parent directory so that .so/.dylib/.dll files in the same location are
/// found at runtime. Call this before spawning any backend process.
///
/// Generic over the Command type so it works with both `tokio::process::Command`
/// (for live backend launches) and `std::process::Command` (for one-off
/// subprocess probes like `--list-devices` discovery).
pub fn configure_backend_command(cmd: &mut impl BackendCommand, binary_path: &Path) {
    if let Some(parent) = binary_path.parent().filter(|p| p.is_dir()) {
        cmd.current_dir(parent);
        #[cfg(unix)]
        {
            let existing = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
            let parent_str = parent.to_string_lossy();
            let new_path = if existing.is_empty() {
                parent_str.into_owned()
            } else {
                format!("{}:{}", parent_str, existing)
            };
            cmd.env("LD_LIBRARY_PATH", new_path);
        }
    }
    // Silence unused warning on non-unix targets.
    let _ = binary_path;
}

/// Abstraction over the small slice of a `Command` builder that
/// [`configure_backend_command`] needs. Implemented for both
/// `tokio::process::Command` and `std::process::Command`.
pub trait BackendCommand {
    fn current_dir(&mut self, dir: &Path);
    fn env<K: AsRef<std::ffi::OsStr>, V: AsRef<std::ffi::OsStr>>(&mut self, key: K, value: V);
}

impl BackendCommand for Command {
    fn current_dir(&mut self, dir: &Path) {
        Command::current_dir(self, dir);
    }
    fn env<K: AsRef<std::ffi::OsStr>, V: AsRef<std::ffi::OsStr>>(&mut self, key: K, value: V) {
        Command::env(self, key, value);
    }
}

impl BackendCommand for std::process::Command {
    fn current_dir(&mut self, dir: &Path) {
        std::process::Command::current_dir(self, dir);
    }
    fn env<K: AsRef<std::ffi::OsStr>, V: AsRef<std::ffi::OsStr>>(&mut self, key: K, value: V) {
        std::process::Command::env(self, key, value);
    }
}

#[derive(Debug, Clone)]
pub enum ProcessEvent {
    Started,
    Ready,
    Output(String),
    Crashed(String),
    Restarting {
        attempt: u32,
        max: u32,
    },
    Stopped,
    HealthCheck {
        alive: bool,
        healthy: bool,
        uptime_secs: u64,
        restarts: u32,
    },
}

pub struct ProcessSupervisor {
    exe_path: String,
    args: Vec<String>,
    health_check: HealthCheck,
    max_restarts: u32,
    restart_delay_ms: u64,
    log_dir: Option<std::path::PathBuf>,
    /// Optional (env_var_name, value) for driver-level GPU isolation,
    /// resolved from the model's `gpu_device` before constructing the supervisor.
    gpu_env: Option<(String, String)>,
}

impl ProcessSupervisor {
    pub fn new(
        exe_path: String,
        args: Vec<String>,
        health_check: HealthCheck,
        max_restarts: u32,
        restart_delay_ms: u64,
    ) -> Self {
        Self {
            exe_path,
            args,
            health_check,
            max_restarts,
            restart_delay_ms,
            log_dir: None,
            gpu_env: None,
        }
    }

    pub fn with_log_dir(mut self, log_dir: std::path::PathBuf) -> Self {
        self.log_dir = Some(log_dir);
        self
    }

    /// Set the GPU isolation env var applied at spawn.
    pub fn with_gpu_env(mut self, env: Option<(String, String)>) -> Self {
        self.gpu_env = env;
        self
    }

    /// Run the supervisor. Listens for shutdown on `shutdown_rx`.
    /// If `shutdown_rx` is None, listens for ctrl-c instead.
    pub async fn run(
        &self,
        tx: mpsc::UnboundedSender<ProcessEvent>,
        mut shutdown_rx: Option<mpsc::Receiver<()>>,
    ) -> Result<()> {
        let mut restart_count: u32 = 0;

        loop {
            let exe = std::path::Path::new(&self.exe_path);
            let mut cmd = Command::new(exe);
            cmd.args(&self.args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);

            configure_backend_command(&mut cmd, exe);

            // Apply GPU isolation env var if configured.
            if let Some((ref k, ref v)) = self.gpu_env {
                cmd.env(k, v);
            }

            let mut child = cmd
                .spawn()
                .with_context(|| format!("Failed to spawn: {}", self.exe_path))?;

            let start_time = std::time::Instant::now();
            tx.send(ProcessEvent::Started).ok();

            // Open log file if log_dir is set
            let log_file = if let Some(ref log_dir) = self.log_dir {
                if let Ok(f) = logging::open_log(log_dir, "default") {
                    Some(Arc::new(Mutex::new(f)))
                } else {
                    None
                }
            } else {
                None
            };

            // Stream stdout
            let stdout = child.stdout.take();
            let tx_out = tx.clone();
            let log_file_out = log_file.clone();
            let stdout_handle = tokio::spawn(async move {
                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tx_out.send(ProcessEvent::Output(line.clone())).ok();
                        if let Some(ref f) = log_file_out {
                            let _ = f.lock().unwrap().write_all((line + "\n").as_bytes());
                        }
                    }
                }
            });

            // Stream stderr
            let stderr = child.stderr.take();
            let tx_err = tx.clone();
            let log_file_err = log_file.clone();
            let stderr_handle = tokio::spawn(async move {
                if let Some(stderr) = stderr {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tx_err.send(ProcessEvent::Output(line.clone())).ok();
                        if let Some(ref f) = log_file_err {
                            let _ = f.lock().unwrap().write_all((line + "\n").as_bytes());
                        }
                    }
                }
            });

            // Health check loop
            let interval_ms = self.health_check.interval_ms.unwrap_or(5000).max(1);
            let timeout_ms = self.health_check.timeout_ms.unwrap_or(3000).max(1);
            let mut health_interval = interval(Duration::from_millis(interval_ms));
            let mut backend_ready = false;
            let timeout = Duration::from_millis(timeout_ms);
            let http_client = reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_default();

            enum ExitReason {
                ProcessExited(std::io::Result<std::process::ExitStatus>),
                Shutdown,
            }

            let exit_reason = loop {
                tokio::select! {
                    status = child.wait() => {
                        break ExitReason::ProcessExited(status);
                    }
                    _ = health_interval.tick() => {
                        let alive = child.try_wait().map(|s| s.is_none()).unwrap_or(false);
                        let healthy = if !alive {
                            false
                        } else if let Some(url) = &self.health_check.url {
                            http_client.get(url).send().await
                                .map(|r| r.status().is_success())
                                .unwrap_or(false)
                        } else {
                            alive
                        };

                        if healthy && !backend_ready {
                            backend_ready = true;
                            tx.send(ProcessEvent::Ready).ok();
                        }

                        tx.send(ProcessEvent::HealthCheck {
                            alive,
                            healthy,
                            uptime_secs: start_time.elapsed().as_secs(),
                            restarts: restart_count,
                        }).ok();
                    }
                    _ = async {
                        match &mut shutdown_rx {
                            Some(rx) => { rx.recv().await; },
                            None => { tokio::signal::ctrl_c().await.ok(); },
                        }
                    } => {
                        break ExitReason::Shutdown;
                    }
                }
            };

            // Clean up child and stream tasks
            stdout_handle.abort();
            stderr_handle.abort();

            match exit_reason {
                ExitReason::Shutdown => {
                    tracing::info!("Shutdown signal received, killing child process");
                    child.kill().await.ok();
                    // Wait for it to actually exit
                    child.wait().await.ok();
                    tx.send(ProcessEvent::Stopped).ok();
                    return Ok(());
                }
                ExitReason::ProcessExited(status) => match status {
                    Ok(s) => {
                        let msg = format!("Process exited with {}", s);
                        tx.send(ProcessEvent::Crashed(msg)).ok();
                    }
                    Err(e) => {
                        let msg = format!("Process error: {}", e);
                        tx.send(ProcessEvent::Crashed(msg)).ok();
                    }
                },
            }

            restart_count += 1;
            if restart_count > self.max_restarts {
                tracing::error!("Max restarts ({}) exceeded, giving up", self.max_restarts);
                tx.send(ProcessEvent::Stopped).ok();
                return Ok(());
            }

            tx.send(ProcessEvent::Restarting {
                attempt: restart_count,
                max: self.max_restarts,
            })
            .ok();

            tokio::time::sleep(Duration::from_millis(self.restart_delay_ms)).await;
        }
    }
}

/// Override a CLI flag's value in an argument list (e.g. --host, --port).
/// If the flag exists, replaces its value. If not, appends the flag and value.
pub fn override_arg(args: &mut Vec<String>, flag: &str, value: &str) {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        if pos + 1 < args.len() {
            args[pos + 1] = value.to_string();
        } else {
            args.push(value.to_string());
        }
    } else {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

/// Check if a process is still alive by PID.
/// Uses `kill(pid, 0)` — POSIX-portable across Linux/macOS/BSD.
pub fn is_process_alive(pid: u32) -> bool {
    // POSIX-portable: kill(pid, 0) checks process existence without
    // sending a signal. Returns 0 if alive, -1 with ESRCH if not.
    // EPERM means the process exists but we lack permission to signal it.
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    // Check errno: ESRCH = no such process, EPERM = exists but no permission
    let err = std::io::Error::last_os_error();
    err.raw_os_error() == Some(libc::EPERM)
}

/// Kill a process by PID. Sends SIGTERM for graceful shutdown.
pub async fn kill_process(pid: u32) -> Result<()> {
    let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        return Err(anyhow!("Failed to send SIGTERM to PID {}: {}", pid, err));
    }
    Ok(())
}

/// Forcefully kill a process by PID (sends SIGKILL).
pub async fn force_kill_process(pid: u32) -> Result<()> {
    let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        return Err(anyhow!("Failed to send SIGKILL to PID {}: {}", pid, err));
    }
    Ok(())
}

/// Configure a child process to be spawned in its own process group.
/// Uses process_group(0) to create a new session.
/// Call this before spawning any backend process.
pub fn configure_process_group(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

/// Send SIGTERM to an entire process group.
/// Negative PID in kill() targets the process group.
pub async fn kill_process_group(pid: u32) -> Result<()> {
    // SAFETY: libc::kill with a negative PID targets the entire process group.
    // The PID was obtained from a successfully spawned child process and is guaranteed > 0.
    // SIGTERM is a standard POSIX signal. The call cannot access invalid memory.
    let ret = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGTERM) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH = no such process group, which is fine (already dead)
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(anyhow::anyhow!(
                "Failed to send SIGTERM to process group {}: {}",
                pid,
                err
            ));
        }
    }
    Ok(())
}

/// Send SIGKILL to an entire process group.
pub async fn force_kill_process_group(pid: u32) -> Result<()> {
    // SAFETY: libc::kill with a negative PID targets the entire process group.
    // The PID was obtained from a successfully spawned child process and is guaranteed > 0.
    // SIGKILL is a standard POSIX signal. The call cannot access invalid memory.
    let ret = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(anyhow::anyhow!(
                "Failed to send SIGKILL to process group {}: {}",
                pid,
                err
            ));
        }
    }
    Ok(())
}

/// Check if a process group leader (by PID) is still alive.
/// If the leader is dead, the group is effectively dead.
pub fn is_process_group_alive(pid: u32) -> bool {
    is_process_alive(pid)
}

/// Check the health of a backend by making a request to its health endpoint.
pub async fn check_health(url: &str, timeout: Option<u64>) -> Result<reqwest::Response> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout.unwrap_or(10)))
        .build()?;
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to check health: {}", url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command as TokioCommand;

    #[test]
    fn test_process_supervisor_gpu_env_defaults_none() {
        let supervisor = ProcessSupervisor::new(
            "test_exe".to_string(),
            vec![],
            HealthCheck::default(),
            3,
            1000,
        );
        assert!(supervisor.gpu_env.is_none());
    }

    #[test]
    fn test_process_supervisor_with_gpu_env_sets_value() {
        let supervisor = ProcessSupervisor::new(
            "test_exe".to_string(),
            vec![],
            HealthCheck::default(),
            3,
            1000,
        )
        .with_gpu_env(Some((
            "CUDA_VISIBLE_DEVICES".to_string(),
            "GPU-abc123".to_string(),
        )));
        assert_eq!(
            supervisor.gpu_env,
            Some(("CUDA_VISIBLE_DEVICES".to_string(), "GPU-abc123".to_string()))
        );
    }

    #[test]
    fn test_process_supervisor_with_gpu_env_none() {
        let supervisor = ProcessSupervisor::new(
            "test_exe".to_string(),
            vec![],
            HealthCheck::default(),
            3,
            1000,
        )
        .with_gpu_env(None);
        assert!(supervisor.gpu_env.is_none());
    }

    #[tokio::test]
    async fn test_kill_process_group_nonexistent_pid_returns_ok() {
        // Use a PID that definitely doesn't exist.
        // ESRCH should be handled gracefully.
        let result = kill_process_group(99999999).await;
        assert!(
            result.is_ok(),
            "ESRCH should be treated as OK: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_force_kill_process_group_nonexistent_pid_returns_ok() {
        // Same for SIGKILL variant.
        let result = force_kill_process_group(99999999).await;
        assert!(
            result.is_ok(),
            "ESRCH should be treated as OK: {:?}",
            result
        );
    }

    #[allow(unused_imports)]
    #[tokio::test]
    async fn test_process_group_kills_children() {
        use std::os::unix::process::CommandExt;
        use std::time::Duration;
        let mut child = TokioCommand::new("/bin/sh");
        child.process_group(0);
        child.arg("-c").arg("sleep 100 & exit 0");
        let mut child = child.spawn().unwrap();
        let pid = child.id().unwrap();

        // Give the child time to fork
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Kill the process group
        kill_process_group(pid)
            .await
            .expect("kill_process_group should succeed");

        // Wait briefly for signals to propagate
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The parent shell exited on its own, but the child (sleep 100) should be killed.
        let _ = child.wait().await;

        // Verify: check that no "sleep 100" process is still running.
        // We use pgrep to find any sleep processes started recently.
        // If the process group kill worked, there should be no orphan.
        let pgrep = std::process::Command::new("pgrep")
            .args(["-f", "sleep 100"])
            .output()
            .ok();
        let orphans = pgrep
            .as_ref()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().len())
            .unwrap_or(0);
        assert!(
            orphans == 0,
            "Expected no orphan 'sleep 100' processes, found {}",
            orphans
        );
    }
}
