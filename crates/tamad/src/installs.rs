//! Backend install/update/remove execution (plan-191 Task 7).
//!
//! The backend binaries (llama.cpp builds, Kokoro TTS, ...) live on the
//! *tamad's* host, so the actual download/build executes here — rooted at
//! `<data-dir>/install` — while the proxy keeps the central DB as the
//! system of record. Long operations run as jobs ([`run_install`] /
//! [`run_update`] report through the [`JobHandle`] and end with a result
//! JSON the proxy persists into `installation_configs`); removal is
//! synchronous on the tamad.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::host_installs::installer::{install_installation_with_progress, InstallOptions};
use tama_core::installations::get_backend_install_path;
use tama_core::installations::types::InstallationSource;
use tama_core::installations::types::InstallationType;
use tama_core::installations::ProgressSink;
use tama_core::tamad::{InstallProviderRequest, UpdateProviderRequest};

use crate::jobs::JobHandle;
use crate::lifecycle::TamadLifecycle;
use crate::process_table::ProcessTable;
use crate::state::TamadState;

/// Future returned by an [`Installer::run`] call (borrowing the spec).
pub type RunFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf>> + Send + 'a>>;

/// The pinned Kokoro-FastAPI tag (mirrors the proxy-side registration).
const KOKORO_FASTAPI_TAG: &str = tama_core::installations::tts_kokoro::paths::KOKORO_FASTAPI_TAG;
/// The Kokoro-FastAPI repo URL (mirrors the proxy-side registration).
const KOKORO_FASTAPI_URL: &str = tama_core::installations::tts_kokoro::paths::KOKORO_FASTAPI_URL;

/// Terminal result payload of an install/update job.
///
/// The proxy persists the installation config row from this JSON — the
/// tamad itself holds no database (ADR-0010).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResult {
    pub installed: bool,
    /// The version string the backend was installed at (the versioned
    /// directory name — e.g. a release tag, or the Kokoro FastAPI tag).
    pub version: String,
    /// Installed binary path (Kokoro: its base directory) ON THIS HOST.
    pub path: String,
}

/// Whether a backend type can be installed on a tamad host.
fn is_installable(backend_type: &InstallationType) -> bool {
    matches!(
        backend_type,
        InstallationType::LlamaCpp | InstallationType::IkLlama | InstallationType::TtsKokoro
    )
}

/// Executor for the install/update download-build (plan-191 Task 7).
///
/// Dependency-injection seam: production uses [`TamadInstaller`] (the real
/// `tama_core::installations` code), unit tests use a stub that writes a
/// marker file instead of touching the network.
pub trait Installer: Send + Sync {
    /// Install the backend described by `spec` into `spec.target_dir`,
    /// streaming progress lines to `sink`.
    ///
    /// Returns the installed binary path (Kokoro: its base directory).
    fn run<'a>(&'a self, spec: &'a InstallSpec, sink: Arc<dyn ProgressSink>) -> RunFuture<'a>;
}

/// Full install description resolved from an RPC request.
#[derive(Debug, Clone)]
pub struct InstallSpec {
    /// Backend name (DB key; the install-dir key is the backend type).
    pub name: String,
    pub backend_type: InstallationType,
    /// GPU variant folder (e.g. "cpu", "cuda").
    pub gpu_variant: String,
    /// Version string used as the versioned directory name and in the
    /// result JSON.
    pub version: String,
    pub source: InstallationSource,
    /// Overwrite an existing version directory (update path / `force`).
    pub allow_overwrite: bool,
    /// The versioned target directory under the tamad's install root.
    pub target_dir: PathBuf,
}

/// Resolve an `InstallProvider` RPC request into an install spec rooted at
/// `install_root` (the tamad's `<data-dir>/install`).
///
/// - unknown/non-host engines → error
/// - `version` empty → "latest" (TTS Kokoro ignores the request version
///   and always installs the pinned tag)
/// - `git_url` empty → prebuilt download; non-empty → source build
/// - TTS Kokoro is always a source install into its pinned tag directory
pub fn spec_from_install(req: &InstallProviderRequest, install_root: &Path) -> Result<InstallSpec> {
    let backend_type = InstallationType::from_str(&req.engine).map_err(|e| anyhow!("{e}"))?;
    if !is_installable(&backend_type) {
        bail!(
            "engine '{}' cannot be installed on a tamad host (supported: llama_cpp, ik_llama, tts_kokoro)",
            req.engine
        );
    }

    let (version, gpu_variant, source) = if matches!(backend_type, InstallationType::TtsKokoro) {
        (
            KOKORO_FASTAPI_TAG.to_string(),
            "cpu".to_string(),
            InstallationSource::SourceCode {
                version: KOKORO_FASTAPI_TAG.to_string(),
                git_url: KOKORO_FASTAPI_URL.to_string(),
                commit: None,
            },
        )
    } else {
        let version = if req.version.trim().is_empty() {
            "latest".to_string()
        } else {
            req.version.trim().to_string()
        };
        let gpu_variant = if req.gpu_variant.trim().is_empty() {
            "cpu".to_string()
        } else {
            req.gpu_variant.trim().to_string()
        };
        let source = if req.git_url.trim().is_empty() {
            InstallationSource::Prebuilt {
                version: version.clone(),
            }
        } else {
            InstallationSource::SourceCode {
                version: version.clone(),
                git_url: req.git_url.trim().to_string(),
                commit: None,
            }
        };
        (version, gpu_variant, source)
    };

    let target_dir = get_backend_install_path(install_root, &backend_type, &gpu_variant, &version);
    Ok(InstallSpec {
        name: if req.name.trim().is_empty() {
            backend_type.to_string()
        } else {
            req.name.trim().to_string()
        },
        backend_type,
        gpu_variant,
        version,
        source,
        allow_overwrite: req.force,
        target_dir,
    })
}

/// Resolve an `UpdateProvider` RPC request into an install spec rooted at
/// `install_root`.
///
/// Like install, but the version is mandatory (the proxy resolves "latest"
/// to a concrete tag first) and the version directory is always
/// overwritable (updates install alongside the old version).
pub fn spec_from_update(req: &UpdateProviderRequest, install_root: &Path) -> Result<InstallSpec> {
    if req.version.trim().is_empty() {
        bail!("update version must not be empty");
    }
    let backend_type = InstallationType::from_str(&req.engine).map_err(|e| anyhow!("{e}"))?;
    if !is_installable(&backend_type) {
        bail!(
            "engine '{}' cannot be updated on a tamad host (supported: llama_cpp, ik_llama, tts_kokoro)",
            req.engine
        );
    }
    if matches!(backend_type, InstallationType::TtsKokoro) {
        bail!("tts_kokoro cannot be updated via this flow (pinned release)");
    }

    let version = req.version.trim().to_string();
    let gpu_variant = if req.gpu_variant.trim().is_empty() {
        "cpu".to_string()
    } else {
        req.gpu_variant.trim().to_string()
    };
    let source = if req.git_url.trim().is_empty() {
        InstallationSource::Prebuilt {
            version: version.clone(),
        }
    } else {
        InstallationSource::SourceCode {
            version: version.clone(),
            git_url: req.git_url.trim().to_string(),
            commit: None,
        }
    };

    let target_dir = get_backend_install_path(install_root, &backend_type, &gpu_variant, &version);
    Ok(InstallSpec {
        name: if req.name.trim().is_empty() {
            backend_type.to_string()
        } else {
            req.name.trim().to_string()
        },
        backend_type,
        gpu_variant,
        version,
        source,
        allow_overwrite: true,
        target_dir,
    })
}

/// ProgressSink that forwards installer output lines to the job handle.
///
/// Install progress is message-only (progress 0 — the installer reports
/// lines, not fractions).
struct JobLineSink {
    handle: JobHandle,
}

impl ProgressSink for JobLineSink {
    fn log(&self, line: &str) {
        if !line.trim().is_empty() {
            self.handle.report(0, line);
        }
    }

    fn result(&self, _json: &str) {}
}

/// Production installer: the host download/build code from
/// `host_installs` (moved from `tama_core::installations` in plan-191 Task 10),
/// rooted at the spec's target directory.
pub struct TamadInstaller;

impl Installer for TamadInstaller {
    fn run<'a>(&'a self, spec: &'a InstallSpec, sink: Arc<dyn ProgressSink>) -> RunFuture<'a> {
        Box::pin(async move {
            match &spec.backend_type {
                // Kokoro TTS: pinned git clone + venv + model files under
                // the versioned base dir (no prebuilt path exists).
                InstallationType::TtsKokoro => {
                    crate::host_installs::kokoro::install_kokoro_fastapi(&spec.target_dir, &sink)
                        .await
                        .with_context(|| {
                            format!(
                                "kokoro fastapi install failed at {}",
                                spec.target_dir.display()
                            )
                        })?;
                    Ok(spec.target_dir.clone())
                }
                _ => {
                    let options = InstallOptions {
                        backend_type: spec.backend_type.clone(),
                        source: spec.source.clone(),
                        target_dir: spec.target_dir.clone(),
                        gpu_variant: spec.gpu_variant.clone(),
                        allow_overwrite: spec.allow_overwrite,
                    };
                    install_installation_with_progress(options, Some(sink), None).await
                }
            }
        })
    }
}

/// Run an already-resolved install spec to completion as a job.
///
/// Streams installer output through the job handle and returns the result
/// JSON on success (the job registry resolves the job from this return).
pub async fn run_spec(
    spec: &InstallSpec,
    handle: JobHandle,
    installer: &dyn Installer,
) -> Result<String> {
    let sink = Arc::new(JobLineSink {
        handle: handle.clone(),
    });
    handle.report(0, &format!("installing {} {}", spec.name, spec.version));

    // The install is user-cancellable (proxy relays `CancelJob`): select so
    // a cancel drops the installer work. Note: spawned subprocesses (e.g.
    // the smoke test) are not individually killed on drop — the install
    // dir is left as-is and the job records `cancelled`.
    let run_fut = installer.run(spec, sink);
    tokio::pin!(run_fut);
    let path = match tokio::select! {
        r = run_fut => r,
        _ = handle.cancelled() => anyhow::bail!("install cancelled"),
    } {
        Ok(p) => p,
        // `{e:#}` renders the full error chain — the job registry stores a
        // plain `to_string()`, so the context must carry the root cause.
        Err(e) => bail!("install of '{}' failed: {e:#}", spec.name),
    };
    handle.report(0, &format!("installed {} at {}", spec.name, path.display()));

    let result = InstallResult {
        installed: true,
        version: spec.version.clone(),
        path: path.to_string_lossy().to_string(),
    };
    serde_json::to_string(&result).context("serializing install result")
}

/// Production entry point for the `InstallProvider` RPC (kind "install")
/// with the given executor (the service injects its configured installer —
/// [`TamadInstaller`] in production, a stub in tests).
pub async fn run_install_with(
    req: &InstallProviderRequest,
    state: &TamadState,
    handle: JobHandle,
    installer: &dyn Installer,
) -> Result<String> {
    let spec = spec_from_install(req, &state.install_dir())?;
    run_spec(&spec, handle, installer).await
}

/// Production entry point for the `UpdateProvider` RPC (kind "update")
/// with the given executor.
pub async fn run_update_with(
    req: &UpdateProviderRequest,
    state: &TamadState,
    handle: JobHandle,
    installer: &dyn Installer,
) -> Result<String> {
    let spec = spec_from_update(req, &state.install_dir())?;
    run_spec(&spec, handle, installer).await
}

/// Kill all backend processes owned by the given backend name(s) via the
/// lifecycle (SIGTERM → SIGKILL of the process group).
///
/// Returns the model names that were unloaded. Unknown/no processes is not
/// an error (removal is idempotent).
pub async fn kill_backend_processes(
    table: &ProcessTable,
    lifecycle: &TamadLifecycle,
    backend_names: &[String],
) -> Vec<String> {
    let entries = table.list().await;
    let mut unloaded = Vec::new();
    for entry in entries {
        if !backend_names.iter().any(|n| n == &entry.provider_name) {
            continue;
        }
        match lifecycle.unload(&entry.model_name).await {
            Ok(()) => unloaded.push(entry.model_name),
            Err(e) => {
                tracing::warn!(
                    model = %entry.model_name,
                    error = %e,
                    "failed to kill backend process during removal"
                );
            }
        }
    }
    unloaded
}

/// Delete the backend's versioned install directory entries under
/// `install_root`.
///
/// - `gpu_variant = None` → all variants of the backend
/// - `version = None` → all versions of the (variant) scope
/// - missing directories are fine (idempotent removal)
///
/// Refuses to delete anything outside `install_root` (defense in depth —
/// same policy as the proxy's `safe_remove_installation`).
pub fn remove_install_dirs(
    install_root: &Path,
    engine: &str,
    gpu_variant: Option<&str>,
    version: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let canonical_root = std::fs::canonicalize(install_root).with_context(|| {
        format!(
            "install root '{}' does not exist or is not accessible",
            install_root.display()
        )
    })?;

    let mut target = install_root.join(engine);
    if let Some(v) = gpu_variant.filter(|v| !v.trim().is_empty()) {
        target = target.join(v);
    }
    if let Some(ver) = version.filter(|v| !v.trim().is_empty()) {
        target = target.join(ver);
    }

    if !target.exists() {
        return Ok(Vec::new());
    }

    // Guard: only ever delete inside the install root.
    let canonical_target = std::fs::canonicalize(&target)
        .with_context(|| format!("canonicalizing {}", target.display()))?;
    if !canonical_target.starts_with(&canonical_root) {
        bail!(
            "install path '{}' is outside the managed install directory; remove manually",
            target.display()
        );
    }

    std::fs::remove_dir_all(&target)
        .with_context(|| format!("failed to remove install directory: {}", target.display()))?;
    tracing::info!(path = %target.display(), "install directory removed");
    Ok(vec![target])
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub installer: waits for a gate (so tests can subscribe to the job
    /// broadcast first), writes a marker binary into the target dir, and
    /// streams two progress lines.
    #[derive(Clone)]
    struct StubInstaller {
        gate: Arc<tokio::sync::Notify>,
    }

    impl Installer for StubInstaller {
        fn run<'a>(&'a self, spec: &'a InstallSpec, sink: Arc<dyn ProgressSink>) -> RunFuture<'a> {
            Box::pin(async move {
                self.gate.notified().await;
                sink.log("stub-line-1");
                tokio::fs::create_dir_all(&spec.target_dir).await?;
                let bin = spec.target_dir.join("llama-server");
                tokio::fs::write(&bin, b"#!/bin/sh\necho stub").await?;
                sink.log("stub-line-2");
                Ok(bin)
            })
        }
    }

    /// Stub installer that always fails.
    struct FailingInstaller;

    impl Installer for FailingInstaller {
        fn run<'a>(
            &'a self,
            _spec: &'a InstallSpec,
            _sink: Arc<dyn ProgressSink>,
        ) -> RunFuture<'a> {
            Box::pin(async { Err(anyhow!("synthetic installer failure")) })
        }
    }

    fn install_req(
        engine: &str,
        version: &str,
        variant: &str,
        git_url: &str,
    ) -> InstallProviderRequest {
        InstallProviderRequest {
            name: engine.to_string(),
            engine: engine.to_string(),
            version: version.to_string(),
            gpu_variant: variant.to_string(),
            force: false,
            git_url: git_url.to_string(),
        }
    }

    /// Spec resolution: prebuilt target dir under the install root,
    /// "latest" default, force → allow_overwrite.
    #[test]
    fn test_spec_from_install_prebuilt() {
        let root = Path::new("/tmp/tamad-install");
        let spec = spec_from_install(&install_req("llama_cpp", "b100", "cpu", ""), root).unwrap();
        assert_eq!(
            spec.target_dir,
            PathBuf::from("/tmp/tamad-install/llama_cpp/cpu/b100")
        );
        assert_eq!(spec.version, "b100");
        assert!(!spec.allow_overwrite);
        assert!(matches!(spec.source, InstallationSource::Prebuilt { .. }));
        assert_eq!(spec.name, "llama_cpp");
    }

    /// Spec resolution: source build from git_url; empty version → latest.
    #[test]
    fn test_spec_from_install_source_latest() {
        let root = Path::new("/tmp/tamad-install");
        let req = install_req("ik_llama", "", "rocm", "https://example.com/repo.git");
        let spec = spec_from_install(&req, root).unwrap();
        assert_eq!(spec.version, "latest");
        assert_eq!(
            spec.target_dir,
            PathBuf::from("/tmp/tamad-install/ik_llama/rocm/latest")
        );
        match &spec.source {
            InstallationSource::SourceCode {
                version,
                git_url,
                commit,
            } => {
                assert_eq!(version, "latest");
                assert_eq!(git_url, "https://example.com/repo.git");
                assert!(commit.is_none());
            }
            other => panic!("expected source build, got {other:?}"),
        }
    }

    /// Spec resolution: TTS Kokoro ignores the request version/variant and
    /// pins the FastAPI tag (mirrors the proxy's registration).
    #[test]
    fn test_spec_from_install_tts_pins_tag() {
        let root = Path::new("/tmp/tamad-install");
        let req = install_req("tts_kokoro", "whatever", "cpu", "");
        let spec = spec_from_install(&req, root).unwrap();
        assert_eq!(spec.version, KOKORO_FASTAPI_TAG);
        assert_eq!(spec.gpu_variant, "cpu");
        assert_eq!(
            spec.target_dir,
            PathBuf::from(format!(
                "/tmp/tamad-install/tts_kokoro/cpu/{KOKORO_FASTAPI_TAG}"
            ))
        );
        match &spec.source {
            InstallationSource::SourceCode { git_url, .. } => {
                assert_eq!(git_url, KOKORO_FASTAPI_URL);
            }
            other => panic!("expected source build, got {other:?}"),
        }
    }

    /// Spec resolution: non-installable and unknown engines are rejected.
    #[test]
    fn test_spec_from_install_rejects_bad_engine() {
        let root = Path::new("/tmp/tamad-install");
        assert!(
            spec_from_install(&install_req("docker", "1.0", "cpu", ""), root).is_err(),
            "docker is not a host-installable engine"
        );
        assert!(spec_from_install(&install_req("nope", "1.0", "cpu", ""), root).is_err());
    }

    /// Spec resolution: update requires a version and always overwrites.
    #[test]
    fn test_spec_from_update() {
        let root = Path::new("/tmp/tamad-install");
        let req = UpdateProviderRequest {
            name: "llama_cpp".into(),
            version: String::new(),
            engine: "llama_cpp".into(),
            gpu_variant: "cpu".into(),
            git_url: String::new(),
        };
        assert!(
            spec_from_update(&req, root).is_err(),
            "version is mandatory"
        );

        let req = UpdateProviderRequest {
            name: "llama_cpp".into(),
            version: "b9123".into(),
            engine: "llama_cpp".into(),
            gpu_variant: "cuda".into(),
            git_url: "https://example.com/repo.git".into(),
        };
        let spec = spec_from_update(&req, root).unwrap();
        assert_eq!(spec.version, "b9123");
        assert!(spec.allow_overwrite, "updates always overwrite");
        assert_eq!(
            spec.target_dir,
            PathBuf::from("/tmp/tamad-install/llama_cpp/cuda/b9123")
        );
    }

    /// Job lifecycle with a stub installer: running events carry the
    /// installer lines, the terminal job returns the result JSON, and the
    /// marker file lands in the versioned target dir.
    #[tokio::test]
    async fn test_run_spec_job_lifecycle_with_stub_installer() {
        let root = tempfile::tempdir().unwrap();
        let req = install_req("llama_cpp", "b100", "cpu", "");
        let spec = spec_from_install(&req, root.path()).unwrap();

        let gate = Arc::new(tokio::sync::Notify::new());
        let gate_for_runner = Arc::clone(&gate);
        let registry = crate::jobs::JobRegistry::new();
        let id = registry
            .start("install", move |h| {
                let spec = spec.clone();
                let stub = StubInstaller {
                    gate: gate_for_runner,
                };
                Box::pin(async move { run_spec(&spec, h, &stub).await })
            })
            .await;

        // Subscribe before the runner proceeds (the gate holds it).
        let (mut rx, _history) = registry.subscribe(&id).expect("job exists after start");
        gate.notify_one();

        let mut lines = Vec::new();
        // Terminal event's result JSON (the `None` init is overwritten on
        // the terminal event before it is ever read).
        #[allow(unused_assignments)]
        let mut result_json: Option<String> = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "job did not reach a terminal event"
            );
            match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
                .await
                .expect("channel must stay open")
            {
                Ok(ev) if ev.job_id == id => {
                    if ev.status == "running" {
                        lines.push(ev.message);
                    } else {
                        assert_eq!(ev.status, "succeeded");
                        result_json = Some(ev.result_json);
                        break;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("channel closed early")
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    panic!("receiver lagged by {n}")
                }
            }
        }

        assert!(
            lines.iter().any(|l| l == "stub-line-1") && lines.iter().any(|l| l == "stub-line-2"),
            "installer lines must be relayed as running events: {lines:?}"
        );

        let rj = result_json.expect("terminal event carried the result JSON");
        assert!(!rj.is_empty(), "result JSON must not be empty");
        let result: InstallResult = serde_json::from_str(&rj).expect("result must be valid JSON");
        assert!(result.installed);
        assert_eq!(result.version, "b100");
        let expected_bin = root.path().join("llama_cpp/cpu/b100/llama-server");
        assert_eq!(PathBuf::from(&result.path), expected_bin);
        assert!(expected_bin.exists(), "marker binary must exist");

        let job = registry.get(&id).expect("job retained");
        assert!(job.is_terminal());
    }

    /// A failing installer marks the job failed with a contextual error.
    #[tokio::test]
    async fn test_run_spec_installer_failure() {
        let root = tempfile::tempdir().unwrap();
        let req = install_req("llama_cpp", "b100", "cpu", "");
        let spec = spec_from_install(&req, root.path()).unwrap();

        let registry = crate::jobs::JobRegistry::new();
        let id = registry
            .start("install", move |h| {
                let spec = spec.clone();
                Box::pin(async move { run_spec(&spec, h, &FailingInstaller).await })
            })
            .await;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let job = loop {
            let job = registry.get(&id).expect("job exists");
            if job.is_terminal() {
                break job;
            }
            assert!(std::time::Instant::now() < deadline, "job did not finish");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        let err = job.error.unwrap_or_default();
        assert!(err.contains("synthetic installer failure"), "got: {err}");
        assert!(err.contains("install of 'llama_cpp' failed"), "got: {err}");
    }

    /// Result JSON round-trips through serde (the proxy deserializes it).
    #[test]
    fn test_install_result_json_roundtrip() {
        let r = InstallResult {
            installed: true,
            version: "b9123".into(),
            path: "/x/llama_cpp/cpu/b9123/llama-server".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: InstallResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, "b9123");
        assert_eq!(back.path, "/x/llama_cpp/cpu/b9123/llama-server");
        assert!(back.installed);
    }

    /// remove_install_dirs: whole backend, single variant, single version;
    /// missing directories are no-ops.
    #[test]
    fn test_remove_install_dirs_scopes() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("llama_cpp");
        for p in ["cpu/b100", "cpu/b200", "cuda/c1"] {
            std::fs::create_dir_all(base.join(p)).unwrap();
        }

        // Single version only.
        let removed =
            remove_install_dirs(root.path(), "llama_cpp", Some("cpu"), Some("b100")).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(!base.join("cpu/b100").exists());
        assert!(base.join("cpu/b200").exists());

        // Whole variant.
        remove_install_dirs(root.path(), "llama_cpp", Some("cpu"), None).unwrap();
        assert!(!base.join("cpu").exists());
        assert!(base.join("cuda/c1").exists());

        // Whole backend.
        remove_install_dirs(root.path(), "llama_cpp", None, None).unwrap();
        assert!(!base.exists());

        // Missing scope → empty, no error.
        assert!(
            remove_install_dirs(root.path(), "llama_cpp", Some("cpu"), None)
                .unwrap()
                .is_empty()
        );
    }

    /// kill_backend_processes unloads exactly the entries matching the
    /// backend name; other providers are left running.
    #[tokio::test]
    async fn test_kill_backend_processes() {
        let (state, _dir) = crate::server::test_support::test_state();
        let table = Arc::new(ProcessTable::default());
        let lifecycle = TamadLifecycle::new(Arc::clone(&table), Arc::clone(&state));

        // Two real sleeper processes via the lifecycle (proper process
        // groups, so unload can kill them), different provider names.
        let resp1 = lifecycle
            .load(&fake_req("m1", "llama_cpp"))
            .await
            .expect("load m1");
        let resp2 = lifecycle
            .load(&fake_req("m2", "vllm"))
            .await
            .expect("load m2");
        let pid1 = resp1.pid;
        let pid2 = resp2.pid;

        let unloaded = kill_backend_processes(&table, &lifecycle, &["llama_cpp".to_string()]).await;
        assert_eq!(unloaded, vec!["m1".to_string()]);

        // m1 dead, m2 alive, table holds only m2.
        for _ in 0..40 {
            if !crate::process::is_process_alive(pid1 as u32) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        assert!(
            !crate::process::is_process_alive(pid1 as u32),
            "m1 must be dead"
        );
        assert!(
            crate::process::is_process_alive(pid2 as u32),
            "m2 must survive"
        );
        assert!(table.get("m1").await.is_none());
        assert!(table.get("m2").await.is_some());

        // Cleanup m2.
        let _ = lifecycle.unload("m2").await;
    }

    /// A trivial load request: `sleep 30`, no health check, one provider.
    fn fake_req(model_name: &str, provider_name: &str) -> tama_core::tamad::LoadModelRequest {
        tama_core::tamad::LoadModelRequest {
            provider_name: provider_name.to_string(),
            model_path: String::new(),
            gpu_variant: "cpu".to_string(),
            params: std::collections::HashMap::new(),
            model_name: model_name.to_string(),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 30".to_string()],
            env: std::collections::HashMap::new(),
            health_url: String::new(),
            health_timeout_ms: 0,
            gpu_device: String::new(),
        }
    }
}
