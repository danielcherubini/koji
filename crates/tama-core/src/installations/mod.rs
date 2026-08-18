//! Backend installation management (plan-191 Task 10 split).
//!
//! Stays here (proxy-side): the `InstallationManager` (central Postgres rows,
//! single-writer rule), shared types (`InstallationType`, `InstallationSource`,
//! `DockerConfig`, ...), the DB migration, prebuilt URL construction, and the
//! network update *checkers*.
//!
//! Moved to the tamad crate (`host_installs`): download/build execution,
//! docker container execution, and the Kokoro TTS install — the proxy
//! never downloads or builds backends (ADR-0010).

pub mod installer;
pub mod log_stream;
pub mod manager;
pub mod migration;
pub mod tts_kokoro;
pub mod types;
pub mod updater;

pub use installer::urls::get_prebuilt_url;
pub use manager::{InstallationManager, InstallationOption};
pub use types::{
    DockerConfig, DockerVolume, InstallationInfo, InstallationSource, InstallationType,
};
pub use updater::{
    check_installation_updates, check_latest_version, has_update, supports_update_check,
    UpdateCheck,
};

use std::path::{Path, PathBuf};

use crate::config::Config;
use anyhow::{Context, Result};

/// Trait for logging progress during backend installation.
pub trait ProgressSink: Send + Sync {
    fn log(&self, line: &str);

    /// Called with benchmark results as JSON when a benchmark completes.
    fn result(&self, json: &str);
}

/// A no-op implementation of ProgressSink for use when no progress tracking is needed.
pub struct NullSink;

impl ProgressSink for NullSink {
    fn log(&self, _line: &str) {}
    fn result(&self, _json: &str) {}
}

/// Returns the backends directory path: `<config_dir>/backends`.
/// Creates the directory if it doesn't exist.
pub fn backends_dir() -> Result<PathBuf> {
    let base_dir = Config::base_dir()?;
    let backends_dir = base_dir.join("backends");
    std::fs::create_dir_all(&backends_dir).with_context(|| {
        format!(
            "Failed to create backends directory: {}",
            backends_dir.display()
        )
    })?;
    Ok(backends_dir)
}

/// Compute the installation directory for a backend given its type, GPU variant, and version.
/// Returns: `backends_dir / backend_type / gpu_variant / version`
pub fn get_backend_install_path(
    backends_dir: &Path,
    backend_type: &InstallationType,
    gpu_variant: &str,
    version: &str,
) -> PathBuf {
    backends_dir
        .join(backend_type.to_string())
        .join(gpu_variant)
        .join(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_backend_install_path_llama_cpp_cpu() {
        let base = Path::new("/tmp/backends");
        let path = get_backend_install_path(base, &InstallationType::LlamaCpp, "cpu", "b8407");
        assert_eq!(path, PathBuf::from("/tmp/backends/llama_cpp/cpu/b8407"));
    }

    #[test]
    fn test_get_backend_install_path_llama_cpp_cuda() {
        let base = Path::new("/tmp/backends");
        let path = get_backend_install_path(base, &InstallationType::LlamaCpp, "cuda", "b9000");
        assert_eq!(path, PathBuf::from("/tmp/backends/llama_cpp/cuda/b9000"));
    }

    #[test]
    fn test_get_backend_install_path_ik_llama_rocm() {
        let base = Path::new("/tmp/backends");
        let path = get_backend_install_path(base, &InstallationType::IkLlama, "rocm", "main");
        assert_eq!(path, PathBuf::from("/tmp/backends/ik_llama/rocm/main"));
    }

    #[test]
    fn test_get_backend_install_path_tts_kokoro() {
        let base = Path::new("/tmp/backends");
        let path = get_backend_install_path(base, &InstallationType::TtsKokoro, "cpu", "v0.2.4");
        assert_eq!(path, PathBuf::from("/tmp/backends/tts_kokoro/cpu/v0.2.4"));
    }

    #[test]
    fn test_get_backend_install_path_custom() {
        let base = Path::new("/tmp/backends");
        let path = get_backend_install_path(base, &InstallationType::Custom, "cpu", "1.0.0");
        assert_eq!(path, PathBuf::from("/tmp/backends/custom/cpu/1.0.0"));
    }

    #[test]
    fn test_backends_dir_returns_config_subdir() {
        let path = backends_dir().expect("backends_dir() should succeed");
        assert!(
            path.ends_with("backends"),
            "backends_dir() should return a path ending in 'backends', got: {:?}",
            path
        );
        assert!(
            path.exists(),
            "backends_dir() should create the directory if missing"
        );
    }

    #[test]
    fn test_progress_sink_trait() {
        // NullSink should implement ProgressSink
        let sink: NullSink = NullSink;
        sink.log("test line"); // Should not panic
    }
}
