//! Trait abstractions for backend lifecycle operations.
//!
//! This module defines traits that abstract over process management, health
//! checking, port allocation, and process existence checks. These traits
//! enable dependency injection for testing while providing default
//! implementations that delegate to the real production functions.
//!
//! # Design
//!
//! - `HealthChecker` — abstracts HTTP health endpoint checks
//! - `ProcessSpawner` — abstracts process spawning and process group management
//! - `PortAllocator` — abstracts ephemeral TCP port allocation
//! - `ProcessChecker` — abstracts process existence checks (PID and process group)
//!
//! Default implementations delegate to `crate::process` module functions,
//! so production code is unchanged when using `()` as the type parameter.
//!
//! Mock implementations are provided for testing (e.g., `MockHealthChecker`).
//!
//! # Dependencies
//!
//! Uses `async-trait` for async trait methods. This adds heap allocation for
//! vtables but is confined to test infrastructure — production code uses `()`
//! default impls with zero overhead.

use std::path::Path;

use anyhow::Result;

use crate::process;

/// Represents a spawned process.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SpawnedProcess {
    /// PID of the spawned process.
    pub pid: u32,
}

/// Check the health of a backend by making a request to its health endpoint.
///
/// Returns `true` if the health endpoint responds with a success status
/// code, `false` otherwise.
#[async_trait::async_trait]
pub trait HealthChecker: Send + Sync {
    /// Check the health of a backend at the given URL.
    ///
    /// `timeout` is an optional timeout in milliseconds. If `None`, uses
    /// the default timeout.
    async fn check_health(&self, url: &str, timeout: Option<u64>) -> bool;
}

/// Spawn and manage backend processes.
///
/// This trait abstracts over process creation, graceful shutdown, and
/// forceful termination of process groups.
#[async_trait::async_trait]
#[allow(dead_code)]
pub trait ProcessSpawner: Send + Sync {
    /// Spawn a new backend process with the given command, arguments,
    /// environment variables, and working directory.
    ///
    /// Returns the spawned process (with its PID) on success.
    async fn spawn(
        &self,
        cmd: &str,
        args: &[String],
        env: &[(&str, String)],
        cwd: Option<&Path>,
    ) -> Result<SpawnedProcess>;

    /// Send SIGTERM to a process group for graceful shutdown.
    async fn kill_process_group(&self, pid: u32) -> Result<()>;

    /// Send SIGKILL to a process group for forceful termination.
    async fn force_kill_process_group(&self, pid: u32) -> Result<()>;
}

/// Allocate an ephemeral TCP port.
#[allow(dead_code)]
pub trait PortAllocator: Send + Sync {
    /// Allocate a free TCP port.
    ///
    /// Returns the port number. The caller is responsible for binding
    /// to the port and then releasing the listener.
    fn allocate_port(&self) -> Result<u16>;
}

/// Check if a process or process group is still alive.
#[allow(dead_code)]
pub trait ProcessChecker: Send + Sync {
    /// Check if a process with the given PID is alive.
    fn is_process_alive(&self, pid: u32) -> bool;

    /// Check if a process group leader (by PID) is still alive.
    fn is_process_group_alive(&self, pid: u32) -> bool;
}

// ─── Default implementations ───────────────────────────────────────────

/// Default (unit) implementation that delegates to the real `process` module.
///
/// Using `()` as the type parameter for trait bounds keeps production code
/// unchanged — callers simply use the default behavior.
#[async_trait::async_trait]
impl HealthChecker for () {
    async fn check_health(&self, url: &str, timeout: Option<u64>) -> bool {
        process::check_health(url, timeout)
            .await
            .map(|resp| resp.status().is_success())
            .unwrap_or(false)
    }
}

#[async_trait::async_trait]
impl ProcessSpawner for () {
    async fn spawn(
        &self,
        cmd: &str,
        args: &[String],
        env: &[(&str, String)],
        cwd: Option<&Path>,
    ) -> Result<SpawnedProcess> {
        let mut child = tokio::process::Command::new(cmd);
        child.args(args);
        for (key, value) in env {
            child.env(key, value);
        }
        if let Some(cwd) = cwd {
            child.current_dir(cwd);
        }
        let child = child.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Failed to get PID for spawned process '{}'", cmd))?;
        Ok(SpawnedProcess { pid })
    }

    async fn kill_process_group(&self, pid: u32) -> Result<()> {
        process::kill_process_group(pid).await
    }

    async fn force_kill_process_group(&self, pid: u32) -> Result<()> {
        process::force_kill_process_group(pid).await
    }
}

impl PortAllocator for () {
    fn allocate_port(&self) -> Result<u16> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        Ok(port)
    }
}

impl ProcessChecker for () {
    fn is_process_alive(&self, pid: u32) -> bool {
        process::is_process_alive(pid)
    }

    fn is_process_group_alive(&self, pid: u32) -> bool {
        process::is_process_group_alive(pid)
    }
}

// ─── Mock implementations ──────────────────────────────────────────────

/// A mock health checker that returns configurable responses.
///
/// Use `set_response` to configure what the mock should return for all
/// subsequent calls.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct MockHealthChecker {
    response: std::sync::Arc<std::sync::Mutex<bool>>,
}

#[allow(dead_code)]
impl MockHealthChecker {
    /// Create a new mock health checker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mock to return the given response.
    pub fn set_response(&self, response: bool) {
        *self.response.lock().unwrap() = response;
    }

    /// Reset the mock to return `false` (unhealthy).
    pub fn reset(&self) {
        self.set_response(false);
    }
}

#[async_trait::async_trait]
impl HealthChecker for MockHealthChecker {
    async fn check_health(&self, _url: &str, _timeout: Option<u64>) -> bool {
        *self.response.lock().unwrap()
    }
}

/// A mock process spawner that tracks spawn calls and returns configurable PIDs.
/// Reserved for future lifecycle tests that need to mock process spawning.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct MockProcessSpawner {
    pub spawn_count: std::sync::Arc<std::sync::atomic::AtomicU32>,
    pub return_pid: std::sync::Arc<std::sync::Mutex<u32>>,
    pub kill_errors: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    pub fail_spawn: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[allow(dead_code)]
impl MockProcessSpawner {
    /// Create a new mock process spawner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the PID to return on the next spawn.
    pub fn set_return_pid(&self, pid: u32) {
        *self.return_pid.lock().unwrap() = pid;
    }

    /// Reset the spawn counter and PID.
    pub fn reset(&self) {
        self.spawn_count
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.set_return_pid(12345);
    }

    /// Mark a PID as having a kill error on the next kill attempt.
    pub fn expect_kill_error(&self, pid: u32) {
        self.kill_errors.lock().unwrap().push(pid);
    }

    /// Configure the mock to fail the next (and every subsequent) spawn.
    pub fn set_fail_spawn(&self, fail: bool) {
        self.fail_spawn
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl ProcessSpawner for MockProcessSpawner {
    async fn spawn(
        &self,
        cmd: &str,
        _args: &[String],
        _env: &[(&str, String)],
        _cwd: Option<&Path>,
    ) -> Result<SpawnedProcess> {
        self.spawn_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_spawn.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(anyhow::anyhow!("Mock spawn error for '{}'", cmd));
        }
        let pid = *self.return_pid.lock().unwrap();
        Ok(SpawnedProcess { pid })
    }

    async fn kill_process_group(&self, pid: u32) -> Result<()> {
        if self.kill_errors.lock().unwrap().contains(&pid) {
            return Err(anyhow::anyhow!("Mock kill error for PID {}", pid));
        }
        Ok(())
    }

    async fn force_kill_process_group(&self, pid: u32) -> Result<()> {
        if self.kill_errors.lock().unwrap().contains(&pid) {
            return Err(anyhow::anyhow!("Mock force kill error for PID {}", pid));
        }
        Ok(())
    }
}

/// A mock port allocator that returns a configurable port.
/// Reserved for future lifecycle tests that need to mock port allocation.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct MockPortAllocator {
    pub port: std::sync::Arc<std::sync::Mutex<u16>>,
}

#[allow(dead_code)]
impl MockPortAllocator {
    /// Create a new mock port allocator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the port to return.
    pub fn set_port(&self, port: u16) {
        *self.port.lock().unwrap() = port;
    }
}

impl PortAllocator for MockPortAllocator {
    fn allocate_port(&self) -> Result<u16> {
        Ok(*self.port.lock().unwrap())
    }
}

/// A mock process checker that returns configurable answers.
/// Reserved for future lifecycle tests that need to mock process existence checks.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct MockProcessChecker {
    pub alive: std::sync::Arc<std::sync::Mutex<bool>>,
}

#[allow(dead_code)]
impl MockProcessChecker {
    /// Create a new mock process checker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the mock to report processes as alive.
    pub fn set_alive(&self, alive: bool) {
        *self.alive.lock().unwrap() = alive;
    }

    /// Reset to report processes as dead.
    pub fn reset(&self) {
        self.set_alive(false);
    }
}

impl ProcessChecker for MockProcessChecker {
    fn is_process_alive(&self, _pid: u32) -> bool {
        *self.alive.lock().unwrap()
    }

    fn is_process_group_alive(&self, _pid: u32) -> bool {
        *self.alive.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that MockHealthChecker returns the configured response.
    #[tokio::test]
    async fn test_mock_health_checker_returns_configured_response() {
        let mock = MockHealthChecker::new();

        mock.set_response(true);
        assert!(
            mock.check_health("http://localhost:8080/health", None)
                .await
        );

        mock.set_response(false);
        assert!(
            !mock
                .check_health("http://localhost:8080/health", None)
                .await
        );
    }

    /// Test that MockHealthChecker with timeout option works.
    #[tokio::test]
    async fn test_mock_health_checker_with_timeout() {
        let mock = MockHealthChecker::new();
        mock.set_response(true);
        assert!(
            mock.check_health("http://localhost:8080/health", Some(5))
                .await
        );
    }

    /// Test that MockProcessSpawner tracks spawn count.
    #[tokio::test]
    async fn test_mock_process_spawner_tracks_spawns() {
        let mock = MockProcessSpawner::new();
        mock.set_return_pid(9999);

        let result = mock
            .spawn(
                "test-cmd",
                &["arg1".to_string(), "arg2".to_string()],
                &[("KEY", "value".to_string())],
                None,
            )
            .await;

        assert!(result.is_ok());
        let proc = result.unwrap();
        assert_eq!(proc.pid, 9999);
        assert_eq!(
            mock.spawn_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    /// Test that MockProcessSpawner returns configured PID.
    #[tokio::test]
    async fn test_mock_process_spawner_returns_pid() {
        let mock = MockProcessSpawner::new();
        mock.set_return_pid(42);

        let result = mock.spawn("cmd", &[], &[], None).await.unwrap();
        assert_eq!(result.pid, 42);
    }

    /// Test that MockProcessSpawner kill_process_group succeeds.
    #[tokio::test]
    async fn test_mock_process_spawner_kill_succeeds() {
        let mock = MockProcessSpawner::new();
        let result = mock.kill_process_group(1234).await;
        assert!(result.is_ok());
    }

    /// Test that MockProcessSpawner returns error when configured.
    #[tokio::test]
    async fn test_mock_process_spawner_kill_error() {
        let mock = MockProcessSpawner::new();
        mock.expect_kill_error(5678);
        let result = mock.kill_process_group(5678).await;
        assert!(result.is_err());
    }

    /// Test that MockProcessChecker reports alive correctly.
    #[tokio::test]
    async fn test_mock_process_checker_alive() {
        let mock = MockProcessChecker::new();
        mock.set_alive(true);
        assert!(mock.is_process_alive(1234));
        assert!(mock.is_process_group_alive(1234));
    }

    /// Test that MockProcessChecker reports dead correctly.
    #[tokio::test]
    async fn test_mock_process_checker_dead() {
        let mock = MockProcessChecker::new();
        mock.set_alive(false);
        assert!(!mock.is_process_alive(1234));
        assert!(!mock.is_process_group_alive(1234));
    }

    /// Test that MockPortAllocator returns configured port.
    #[test]
    fn test_mock_port_allocator_returns_port() {
        let mock = MockPortAllocator::new();
        mock.set_port(9876);
        let port = mock.allocate_port().unwrap();
        assert_eq!(port, 9876);
    }

    /// Test that default () impl of HealthChecker returns false for unreachable URL.
    #[tokio::test]
    async fn test_default_health_checker_returns_false_for_unreachable() {
        let checker: () = ();
        // Use a port that's unlikely to be in use
        let result = checker.check_health("http://127.0.0.1:1", Some(100)).await;
        assert!(!result);
    }

    /// Test that SpawnedProcess derives Debug and Clone.
    #[test]
    fn test_spawned_process_debug_clone() {
        let proc = SpawnedProcess { pid: 42 };
        let debug_str = format!("{:?}", proc);
        assert!(debug_str.contains("42"));

        let cloned = proc.clone();
        assert_eq!(cloned.pid, 42);
    }

    /// Test that MockHealthChecker reset sets response to false.
    #[tokio::test]
    async fn test_mock_health_checker_reset() {
        let mock = MockHealthChecker::new();
        mock.set_response(true);
        assert!(
            mock.check_health("http://localhost:8080/health", None)
                .await
        );

        mock.reset();
        assert!(
            !mock
                .check_health("http://localhost:8080/health", None)
                .await
        );
    }

    /// Test that MockProcessChecker reset sets alive to false.
    #[test]
    fn test_mock_process_checker_reset() {
        let mock = MockProcessChecker::new();
        mock.set_alive(true);
        assert!(mock.is_process_alive(1));

        mock.reset();
        assert!(!mock.is_process_alive(1));
    }
}
