use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;

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

/// Wait until the process group led by `pid` is gone (10s deadline).
///
/// Used after a SIGKILL escalation to make sure the group is reaped —
/// a dead-but-unreaped leader would still answer `kill(pid, 0)`.
pub async fn wait_group_dead(pid: u32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while is_process_group_alive(pid) {
        if Instant::now() >= deadline {
            bail!("process group {pid} still alive after 10s")
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
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
        use std::time::{Duration, Instant};
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
