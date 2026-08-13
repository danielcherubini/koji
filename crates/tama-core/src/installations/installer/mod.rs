mod download;
pub use download::download_with_client;

mod extract;
mod prebuilt;
mod source;
mod urls;

pub use extract::{extract_archive, find_backend_binary};
pub use prebuilt::prepare_target_dir;
pub use urls::get_prebuilt_url;

use anyhow::Result;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::Arc;

use super::types::{InstallationSource, InstallationType};
use super::ProgressSink;

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub backend_type: InstallationType,
    pub source: InstallationSource,
    pub target_dir: PathBuf,
    /// GPU variant string (e.g. "cpu", "cuda", "rocm").
    /// Used for path computation, download URL resolution, and registry metadata.
    pub gpu_variant: String,
    /// When true, skip the target directory existence check.
    /// Used by the update path where the directory already exists.
    pub allow_overwrite: bool,
}

/// Emit a log line through the progress sink, or println if no sink is provided.
pub(crate) fn emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>) {
    let line = line.into();
    match sink {
        Some(s) => s.log(&line),
        None => tracing::info!("{line}"),
    }
}

/// Emit an error through the progress sink AND tracing.
///
/// Always writes to `tracing::error!` for server-side observability, then
/// additionally sends the line to the progress sink when one is available.
pub(crate) fn emit_error(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>) {
    let line = line.into();
    tracing::error!("{line}");
    if let Some(s) = sink {
        s.log(&line);
    }
}

/// Main entry point for installing a backend with progress tracking.
///
/// Clones `source` from `options` before matching so that `options` fields
/// remain accessible inside each arm.
///
/// When `client` is `Some`, it is used for prebuilt downloads (enabling connection
/// pooling across multiple downloads). When `None`, a new client is created per download.
pub async fn install_installation_with_progress(
    options: InstallOptions,
    progress: Option<Arc<dyn ProgressSink>>,
    client: Option<&Client>,
) -> Result<PathBuf> {
    let source = options.source.clone();
    match source {
        InstallationSource::Prebuilt { version } => {
            // Resolve "latest" to an actual release tag before constructing the download URL.
            // GitHub releases do not support "latest" as a path segment in asset URLs.
            let resolved = if version.eq_ignore_ascii_case("latest") {
                tracing::info!(
                    target: "tama_core::backends::installer",
                    "Resolving 'latest' version tag for {:?}",
                    options.backend_type
                );
                let tag = crate::installations::updater::check_latest_version(
                    &options.backend_type,
                    None,
                    None,
                )
                .await?;
                tracing::info!(
                    target: "tama_core::backends::installer",
                    "Resolved 'latest' -> {}",
                    tag
                );
                tag
            } else {
                version
            };
            prebuilt::install_prebuilt(&options, &resolved, progress.as_ref(), client).await
        }
        InstallationSource::SourceCode {
            version,
            git_url,
            commit,
        } => {
            source::install_from_source(
                &options,
                &version,
                &git_url,
                commit.as_deref(),
                progress.as_ref(),
            )
            .await
        }
    }
}

/// Main entry point for installing a backend (no progress tracking).
///
/// This is a thin wrapper around `install_installation_with_progress` that passes `None`
/// for the progress sink and client, preserving the original CLI behavior.
pub async fn install_installation(options: InstallOptions) -> Result<PathBuf> {
    install_installation_with_progress(options, None, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installations::ProgressSink;
    use std::sync::{Arc, Mutex};

    /// A mock progress sink that collects lines into a Vec for testing.
    struct MockSink {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl MockSink {
        fn new() -> Self {
            Self {
                lines: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_lines(&self) -> Vec<String> {
            self.lines.lock().unwrap().clone()
        }
    }

    impl ProgressSink for MockSink {
        fn log(&self, line: &str) {
            self.lines.lock().unwrap().push(line.to_string());
        }
        fn result(&self, _json: &str) {}
    }

    /// Test that InstallOptions still derives Debug (smoke test guard).
    #[test]
    fn test_install_options_debug_assertion() {
        fn _assert<T: std::fmt::Debug>() {}
        _assert::<InstallOptions>();
    }

    /// Test that emit_error always writes to tracing AND sends to sink when present.
    #[test]
    fn test_emit_error_dual_write() {
        let sink = Arc::new(MockSink::new());
        let progress: Option<Arc<dyn ProgressSink>> = Some(sink.clone());

        super::emit_error(progress.as_ref(), "test error line");

        // Sink should have received the line (dual-write)
        let lines = sink.get_lines();
        assert!(
            lines.contains(&"test error line".to_string()),
            "Sink should have received the error line"
        );
    }

    /// Test that emit routes to sink when Some, println when None.
    #[test]
    fn test_emit_routes_to_sink() {
        let sink = Arc::new(MockSink::new());
        let progress: Option<Arc<dyn ProgressSink>> = Some(sink.clone());

        // Test the sink path - the sink should have received the line
        super::emit(progress.as_ref(), "test line from sink");

        let lines = sink.get_lines();
        assert!(
            lines.contains(&"test line from sink".to_string()),
            "Sink should have received the line"
        );
    }
}
