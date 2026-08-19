//! Pull execution on the tamad (plan-191 Task 6).
//!
//! Model weights land on the *tamad's* disk, so the download runs here. The
//! proxy keeps the queue and progress UI (the `pull_queue` DB/SSE pipeline);
//! it dispatches `PullModel`, streams `StreamJob` events, and persists the
//! registry rows from the terminal event's result JSON.
//!
//! Two execution paths:
//! - **GGUF** (`repo_pull == false`): chunked HTTP download via
//!   [`crate::download::pull_chunked_with_progress`], then the
//!   disk-side verification half (SHA-256 + GGUF/transformers header parse)
//!   that used to run proxy-side in `run_verification`.
//! - **Whole repo** (`repo_pull == true`): the `hf` CLI as a tracked
//!   subprocess (the same helpers the proxy's repo-pull jobs use).
//!
//! The HF token is only ever used to build auth headers / the child's
//! `HF_TOKEN` env — it is never logged.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use tama_core::config::QuantKind;
use tama_core::models::gguf::GgufMetadata;
use tama_core::models::pull::hf_cli::{scan_dir_bytes, stderr_tail_str};
use tama_core::models::pull::{
    determine_primary_shard, lookup_blob_metadata, TamadGgufPullResult, TamadPulledFile,
    TamadRepoPullResult,
};

use crate::download::{
    check_hf_binary, hf_auth_headers_with_token, hf_resolve_url, pull_chunked_with_progress,
    spawn_hf_download, start_stderr_reader, ProgressCallback,
};
use tama_core::models::transformers::TransformersMetadata;
use tama_core::models::verify::sha256_file;
use tama_core::models::{is_safe_relative_path, is_valid_repo_id, repo_path};
use tama_core::tamad::PullModelRequest;

use crate::jobs::JobHandle;

/// Name of the `hf` CLI binary. Overridable via `TAMA_TEST_HF_BINARY` for
/// tests (a fake `hf` script in a temp dir placed on `PATH`).
fn hf_binary_name() -> String {
    std::env::var("TAMA_TEST_HF_BINARY").unwrap_or_else(|_| "hf".to_string())
}

/// Execute a `PullModel` request on this host (plan-191 Task 6).
///
/// Runs as the body of a [`JobRegistry`](crate::jobs::JobRegistry) job;
/// `handle.report` maps to `StreamJob` progress events. Returns the terminal
/// result JSON (GGUF: files + hashes + metadata; repo: dir + ok).
///
/// Errors fail the job; the proxy relay turns the failure into the queue
/// item's error.
pub async fn run_pull(
    req: &PullModelRequest,
    models_dir: &Path,
    hf_token: &str,
    handle: JobHandle,
) -> Result<String> {
    // Validate inputs before touching disk (path-traversal safe).
    if !is_valid_repo_id(&req.repo_id) {
        return Err(anyhow!("invalid repo_id: '{}'", req.repo_id));
    }
    if req.repo_pull {
        return run_repo_pull(req, models_dir, hf_token, handle).await;
    }
    if req.quants.is_empty() {
        return Err(anyhow!(
            "no files requested for GGUF pull of '{}'",
            req.repo_id
        ));
    }
    for filename in &req.quants {
        if !is_safe_relative_path(filename) {
            return Err(anyhow!("unsafe filename: '{filename}'"));
        }
    }
    run_gguf_pull(req, models_dir, hf_token, handle).await
}

/// Resolve the destination directory for a pull.
///
/// When the request carries a `dest_dir` (the proxy always does), the host
/// writes there directly so the file lands at the exact path the proxy
/// expects — regardless of the host's own `models_dir` config. `dest_dir`
/// must be absolute and free of `..` components. Otherwise the host
/// composes its own two-level layout (`models_dir/org/repo`).
fn resolve_dest_dir(models_dir: &Path, req: &PullModelRequest) -> Result<PathBuf> {
    let dest = req.dest_dir.trim();
    if dest.is_empty() {
        return Ok(repo_path(models_dir, &req.repo_id));
    }
    let path = PathBuf::from(dest);
    if !path.is_absolute() {
        bail!("dest_dir must be an absolute path: '{dest}'");
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("dest_dir must not contain '..': '{dest}'");
    }
    Ok(path)
}

/// GGUF path: chunked download + disk-side verification + metadata parse.
async fn run_gguf_pull(
    req: &PullModelRequest,
    models_dir: &Path,
    hf_token: &str,
    handle: JobHandle,
) -> Result<String> {
    // Same two-level layout as the proxy used to write (or the explicit
    // dest_dir from the request).
    let dest_dir = resolve_dest_dir(models_dir, req)?;
    tokio::fs::create_dir_all(&dest_dir)
        .await
        .with_context(|| format!("creating dest dir {}", dest_dir.display()))?;

    // Upstream blob metadata (best-effort; same call the proxy's
    // run_verification used). The blobs API is a metadata endpoint, not a
    // download.
    let blobs = match tokio::select! {
        r = lookup_blob_metadata(&req.repo_id) => r,
        _ = handle.cancelled() => anyhow::bail!("pull cancelled"),
    } {
        Ok(b) => Some(b),
        Err(e) => {
            if e.to_string() == "pull cancelled" {
                anyhow::bail!("pull cancelled");
            }
            tracing::warn!(repo = %req.repo_id, error = %e, "HF blob metadata lookup failed; verification will be best-effort");
            None
        }
    };

    // Reuse one client for all files (HTTP/2 keep-alive).
    let client = reqwest::Client::builder()
        .http2_keep_alive_timeout(Duration::from_secs(15))
        .build()
        .context("building HTTP client")?;
    // Headers carry the user's token — never log them.
    let headers = hf_auth_headers_with_token(hf_token);

    let mut files = Vec::with_capacity(req.quants.len());
    let mut gguf_metadata: Option<GgufMetadata> = None;
    let mut transformers_metadata: Option<TransformersMetadata> = None;

    for filename in &req.quants {
        let dest_path = dest_dir.join(filename);
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating parent dir {}", parent.display()))?;
        }

        // Download with progress mapped onto the job events.
        handle.report(0, &format!("Downloading {filename}"));
        let url = hf_resolve_url(&req.repo_id, filename);
        let handle_clone = handle.clone();
        let filename_for_cb = filename.clone();
        let callback: ProgressCallback = Arc::new(move |pulled: u64, total: u64| {
            let pct = (pulled
                .saturating_mul(100)
                .checked_div(total)
                .unwrap_or(0)
                .min(99)) as i32;
            handle_clone.report_with_bytes(
                pct,
                &format!("Downloading {filename_for_cb}"),
                pulled as i64,
                total as i64,
            );
        });
        let size = tokio::select! {
            r = pull_chunked_with_progress(
                &client,
                &url,
                &dest_path,
                8, // max connections
                Some(callback),
                Some(&headers),
            ) => r,
            _ = handle.cancelled() => {
                anyhow::bail!("pull cancelled")
            }
        }
        .with_context(|| format!("download of '{filename}' failed"))?;
        handle.report(100, "Verifying...");
        // Carry the final byte counts onto the terminal-bound events so the
        // relay can compute totals even if it only sees the last events.
        handle.report_with_bytes(100, "Verifying...", size as i64, size as i64);

        // Disk-side verification (moved from the proxy's run_verification):
        // SHA-256 of the local file compared to the upstream LFS hash.
        // On mismatch the file is deleted so no corrupt data lingers.
        let expected_sha = blobs
            .as_ref()
            .and_then(|b| b.get(filename).and_then(|i| i.lfs_sha256.clone()));
        let is_primary_shard = match blobs.as_ref() {
            Some(b) => determine_primary_shard(filename, b),
            None => !filename.contains('/') || filename.contains("00001-of"),
        };
        let hash_src = dest_path.clone();
        let sha_fut = tokio::task::spawn_blocking(move || sha256_file(&hash_src, |_| {}).ok());
        let actual_sha = tokio::select! {
            r = sha_fut => r.unwrap_or(None),
            _ = handle.cancelled() => anyhow::bail!("pull cancelled"),
        };
        if actual_sha.is_none() {
            tracing::warn!(path = %dest_path.display(), "hashing failed");
        }
        let (verified, verify_error) = match (expected_sha.as_deref(), actual_sha.as_deref()) {
            (None, _) => (true, None),
            (Some(_), None) => (false, Some("hash error: failed to read file".to_string())),
            (Some(exp), Some(act)) if act.eq_ignore_ascii_case(exp) => (true, None),
            (Some(exp), Some(act)) => (
                false,
                Some(format!(
                    "hash mismatch: expected {} got {}",
                    exp.chars().take(10).collect::<String>(),
                    act.chars().take(10).collect::<String>()
                )),
            ),
        };
        if !verified {
            tokio::fs::remove_file(&dest_path).await.ok();
            let err = verify_error
                .clone()
                .unwrap_or_else(|| "verification failed".into());
            tracing::error!(file = %filename, error = %err, "pull verification failed — file deleted");
            return Err(anyhow!("verification failed for '{filename}': {err}"));
        }

        // Metadata parse (soft failure — don't fail the pull), same skip
        // rules as the proxy: mmproj/MTP files are not LLM headers.
        let skip_gguf_parse = matches!(
            QuantKind::from_filename(filename),
            QuantKind::Mmproj | QuantKind::Mtp
        );
        if !skip_gguf_parse && gguf_metadata.is_none() {
            match tama_core::models::gguf::parse_gguf_metadata(&dest_path) {
                Ok(meta) => {
                    tracing::info!(file = %filename, architecture = ?meta.architecture, "GGUF metadata parsed");
                    gguf_metadata = Some(meta);
                }
                Err(e) => {
                    tracing::debug!(file = %filename, error = %e, "GGUF metadata parse failed");
                }
            }
        }
        if !skip_gguf_parse
            && gguf_metadata.is_none()
            && transformers_metadata.is_none()
            && dest_dir.join("config.json").exists()
        {
            match tama_core::models::transformers::parse_transformers_metadata(&dest_dir) {
                Ok(meta) => {
                    tracing::info!(dir = %dest_dir.display(), architectures = ?meta.architectures, "transformers metadata parsed");
                    transformers_metadata = Some(meta);
                }
                Err(e) => {
                    tracing::warn!(dir = %dest_dir.display(), error = %e, "transformers metadata parse failed");
                }
            }
        }

        files.push(TamadPulledFile {
            path: filename.clone(),
            size,
            sha256: actual_sha,
            expected_sha,
            verified,
            verify_error,
            is_primary_shard,
        });
    }

    Ok(serde_json::to_string(&TamadGgufPullResult {
        dir: dest_dir.to_string_lossy().to_string(),
        files,
        gguf_metadata,
        transformers_metadata,
    })?)
}

/// Whole-repo path: the `hf` CLI as a tracked subprocess with progress.
async fn run_repo_pull(
    req: &PullModelRequest,
    models_dir: &Path,
    hf_token: &str,
    handle: JobHandle,
) -> Result<String> {
    let dest = resolve_dest_dir(models_dir, req)?;
    tokio::fs::create_dir_all(&dest)
        .await
        .with_context(|| format!("creating dest dir {}", dest.display()))?;

    // The host must have the `hf` CLI (the proxy's repo-pull path checks
    // the same way).
    check_hf_binary().await.map_err(anyhow::Error::msg)?;

    // The token goes to the child via the HF_TOKEN env — never argv, never
    // logs.
    let token = hf_token.trim();
    let mut child = spawn_hf_download(
        &hf_binary_name(),
        &req.repo_id,
        &dest,
        if token.is_empty() { None } else { Some(token) },
    )
    .await
    .map_err(anyhow::Error::msg)?;

    let sink = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let stderr = child
        .stderr
        .take()
        .context("hf child stderr must be piped")?;
    let reader = start_stderr_reader(stderr, sink.clone());

    handle.report(0, "Starting hf download");
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Drain the stderr tail before reporting the outcome.
                reader.await.ok();
                let tail = stderr_tail_str(&sink).await.unwrap_or_default();
                if status.success() {
                    handle.report(100, "Repo pull complete");
                    return Ok(serde_json::to_string(&TamadRepoPullResult {
                        dir: dest.to_string_lossy().to_string(),
                        ok: true,
                    })?);
                }
                let detail = if tail.is_empty() {
                    format!("hf download exited with code {status}")
                } else {
                    format!("hf download exited with code {status}: {tail}")
                };
                tracing::error!(repo = %req.repo_id, "hf download failed: {detail}");
                return Err(anyhow!("{detail}"));
            }
            Ok(None) => {
                let bytes = scan_dir_bytes(&dest);
                handle.report(
                    0,
                    &format!("Repo pull in progress: {bytes} bytes downloaded"),
                );
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    _ = handle.cancelled() => {
                        // Cancelled (proxy relayed `CancelJob`): kill the
                        // hf child so no download lingers on the host.
                        reader.abort();
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        anyhow::bail!("repo pull cancelled")
                    }
                }
            }
            Err(e) => {
                reader.abort();
                return Err(anyhow!("waiting on hf child failed: {e}"));
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    /// Serializes env-mutating tests (PATH/HF_ENDPOINT/HF_TOKEN) — the
    /// shared guard is in server::test_support.
    use crate::server::test_support::ENV_GUARD;

    /// Write an executable `hf` stub into `bin_dir`. The stub:
    /// - writes its argv to `<dest>/.argv_check`
    /// - writes `HF_TOKEN` to `<dest>/.hf_token_check`
    /// - emits a stderr line
    /// - writes a marker file to `<dest>` (so scan_dir_bytes > 0)
    /// - exits with the given code.
    fn write_hf_stub(bin_dir: &Path, exit_code: u8, stderr_line: &str) {
        let script = "#!/bin/sh\n"
            .to_string()
            + "if [ \"$1\" = \"--version\" ]; then echo 'hf, version 99.9.9'; exit 0; fi\n"
            + "dest=\"\"\n"
            + "prev=\"\"\n"
            + "for a in \"$@\"; do\n"
            + "  if [ \"$prev\" = \"--local-dir\" ]; then dest=\"$a\"; fi\n"
            + "  prev=\"$a\"\n"
            + "done\n"
            + "mkdir -p \"$dest\"\n"
            + "printf '%s\\n' \"$@\" > \"$dest/.argv_check\"\n"
            + "if [ -n \"$HF_TOKEN\" ]; then printf '%s' \"$HF_TOKEN\" > \"$dest/.hf_token_check\"; fi\n"
            + &format!("echo \"{stderr_line}\" >&2\n")
            + "echo \"fake-repo-weights\" > \"$dest/config.json\"\n"
            + "sleep 0.3\n"
            + &format!("exit {exit_code}\n");
        std::fs::write(bin_dir.join("hf"), script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(bin_dir.join("hf")).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(bin_dir.join("hf"), p).unwrap();
        }
    }

    /// Point `PATH` (and `TAMA_TEST_HF_BINARY` stays default) at `bin_dir`.
    /// Caller must hold ENV_GUARD (briefly) and restore PATH afterwards.
    fn prepend_to_path(bin_dir: &Path) -> Option<String> {
        let old = std::env::var("PATH").ok();
        let new = match old.as_deref() {
            Some(p) => format!("{}:{}", bin_dir.display(), p),
            None => bin_dir.to_string_lossy().to_string(),
        };
        std::env::set_var("PATH", new);
        old
    }

    fn repo_request(repo_id: &str, filenames: &[&str]) -> PullModelRequest {
        PullModelRequest {
            repo_id: repo_id.to_string(),
            quants: filenames.iter().map(|s| s.to_string()).collect(),
            model_name: String::new(),
            backend: "llama_cpp".to_string(),
            hf_token: String::new(),
            repo_pull: false,
            dest_dir: String::new(),
        }
    }

    async fn wait_terminal(registry: &Arc<crate::jobs::JobRegistry>, id: &str) -> crate::jobs::Job {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let job = registry.get(id).expect("job must exist");
            if job.is_terminal() {
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "job did not finish in time (status={})",
                job.status
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Repo pull against a fake `hf` binary: the result JSON carries the
    /// destination dir, the stub observes the token via env (never argv),
    /// and progress events are observed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_run_pull_repo_pull_with_fake_hf() {
        let bin_dir = tempfile::tempdir().unwrap();
        write_hf_stub(bin_dir.path(), 0, "downloading repo...");
        let old_path = {
            let _g = ENV_GUARD.lock().unwrap();
            prepend_to_path(bin_dir.path())
        };

        let models_dir = tempfile::tempdir().unwrap();
        let models_dir_path = models_dir.path().to_path_buf();
        let registry = crate::jobs::JobRegistry::new();
        let req = PullModelRequest {
            repo_id: "org/fake-repo".to_string(),
            quants: vec![],
            model_name: String::new(),
            backend: "llama_cpp".to_string(),
            hf_token: "hf_secret_token_123".to_string(),
            repo_pull: true,
            dest_dir: String::new(),
        };

        let hf_token = req.hf_token.clone();
        let job_dir = models_dir_path.clone();
        let job_id = registry
            .start("pull", move |handle| {
                Box::pin(async move { run_pull(&req, &job_dir, &hf_token, handle).await })
            })
            .await;

        // Observe progress: the stub sleeps 0.3s, so a subscriber that
        // attaches right after start catches at least one running event.
        let (mut rx, _history) = registry.subscribe(&job_id).expect("job exists");
        let mut saw_running = false;
        let mut events = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && events < 10 {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(ev)) if ev.job_id == job_id => {
                    events += 1;
                    if ev.status == "running" {
                        saw_running = true;
                        if ev.message.contains("bytes downloaded") {
                            break;
                        }
                    }
                    if ev.status == "succeeded" || ev.status == "failed" {
                        break;
                    }
                }
                _ => {}
            }
        }

        let job = wait_terminal(&registry, &job_id).await;
        assert_eq!(
            job.status,
            crate::jobs::STATUS_SUCCEEDED,
            "repo pull must succeed, error: {:?}",
            job.error
        );
        assert!(saw_running, "must observe running progress events");

        let result: TamadRepoPullResult =
            serde_json::from_str(&job.result_json.expect("terminal result json")).unwrap();
        assert!(result.ok);
        assert_eq!(
            Path::new(&result.dir),
            models_dir.path().join("org/fake-repo").as_path()
        );

        // The stub wrote the marker file into the destination dir.
        let dest = models_dir.path().join("org/fake-repo");
        assert!(dest.join("config.json").exists());

        // Token delivered via HF_TOKEN env, never via argv.
        let token_check = std::fs::read_to_string(dest.join(".hf_token_check")).unwrap();
        assert_eq!(token_check, "hf_secret_token_123");
        let argv = std::fs::read_to_string(dest.join(".argv_check")).unwrap();
        assert!(
            !argv.contains("hf_secret_token_123"),
            "token must never appear in argv"
        );

        {
            let _g = ENV_GUARD.lock().unwrap();
            std::env::set_var("PATH", old_path.expect("PATH was set"));
        }
    }

    /// A failing `hf` child fails the job with the stderr tail.
    #[tokio::test]
    async fn test_run_pull_repo_pull_failure_carries_stderr() {
        let bin_dir = tempfile::tempdir().unwrap();
        let old_path = {
            let _g = ENV_GUARD.lock().unwrap();
            prepend_to_path(bin_dir.path())
        };

        write_hf_stub(bin_dir.path(), 1, "Repo org/fake-repo not found");

        let models_dir = tempfile::tempdir().unwrap();
        let models_dir_path = models_dir.path().to_path_buf();
        let registry = crate::jobs::JobRegistry::new();
        let req = PullModelRequest {
            repo_id: "org/fake-repo".to_string(),
            quants: vec![],
            model_name: String::new(),
            backend: "llama_cpp".to_string(),
            hf_token: String::new(),
            repo_pull: true,
            dest_dir: String::new(),
        };

        let job_dir = models_dir_path.clone();
        let job_id = registry
            .start("pull", move |handle| {
                Box::pin(async move { run_pull(&req, &job_dir, "", handle).await })
            })
            .await;

        let job = wait_terminal(&registry, &job_id).await;
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        assert!(
            job.error
                .as_deref()
                .unwrap_or_default()
                .contains("Repo org/fake-repo not found"),
            "stderr tail must be in the error, got: {:?}",
            job.error
        );

        {
            let _g = ENV_GUARD.lock().unwrap();
            std::env::set_var("PATH", old_path.expect("PATH was set"));
        }
    }

    /// Invalid inputs are rejected before any disk access.
    #[tokio::test]
    async fn test_run_pull_rejects_invalid_inputs() {
        let registry = crate::jobs::JobRegistry::new();
        let models_dir = tempfile::tempdir().unwrap();
        let models_dir_path = models_dir.path().to_path_buf();

        // Bad repo_id.
        let bad_repo = {
            let models_dir = models_dir_path.clone();
            registry
                .start("pull", move |handle| {
                    let req = repo_request("../evil", &["x.gguf"]);
                    Box::pin(async move { run_pull(&req, &models_dir, "", handle).await })
                })
                .await
        };
        let job = wait_terminal(&registry, &bad_repo).await;
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        assert!(job
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("invalid repo_id"));

        // Unsafe filename (traversal).
        let bad_file = {
            let models_dir = models_dir_path.clone();
            registry
                .start("pull", move |handle| {
                    let req = repo_request("org/repo", &["../evil.gguf"]);
                    Box::pin(async move { run_pull(&req, &models_dir, "", handle).await })
                })
                .await
        };
        let job = wait_terminal(&registry, &bad_file).await;
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        assert!(job
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("unsafe filename"));

        // No files requested (GGUF path).
        let no_files = {
            let models_dir = models_dir_path.clone();
            registry
                .start("pull", move |handle| {
                    let req = repo_request("org/repo", &[]);
                    Box::pin(async move { run_pull(&req, &models_dir, "", handle).await })
                })
                .await
        };
        let job = wait_terminal(&registry, &no_files).await;
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        assert!(job
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("no files requested"));
    }

    /// GGUF path against a local file server standing in for HF (wiremock):
    /// the file lands in `<models_dir>/<org>/<repo>`, the result JSON carries
    /// the precomputed SHA-256, verification passes against the blobs-API
    /// hash, and the request carries the per-pull token header.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_run_pull_gguf_against_local_file_server() {
        use sha2::{Digest, Sha256};

        let body: Vec<u8> = b"fake but consistent gguf bytes for the tamad pull test"
            .iter()
            .copied()
            .cycle()
            .take(128 * 1024)
            .collect();
        let sha = format!("{:x}", Sha256::digest(&body));
        const FILE: &str = "fake-model-Q4_K_M.gguf";
        const REPO: &str = "test/repo";

        let server = wiremock::MockServer::start().await;

        // Point HF_ENDPOINT at the mock server (the download runs on a
        // spawned job task; nextest's process-per-test isolation keeps this
        // from leaking into other tests).
        {
            let _g = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
            std::env::remove_var("HF_TOKEN");
        }

        // 1. Blobs API — upstream LFS hash (verification compares to this).
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/models/test/repo"))
            .and(wiremock::matchers::query_param("blobs", "true"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "siblings": [{
                        "rfilename": FILE,
                        "blobId": "b1",
                        "size": body.len(),
                        "lfs": { "sha256": sha }
                    }]
                })),
            )
            .expect(1..)
            .mount(&server)
            .await;

        // 2. HEAD — content length.
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .and(wiremock::matchers::path(format!(
                "/test/repo/resolve/main/{FILE}"
            )))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-length", body.len().to_string())
                    .insert_header("accept-ranges", "bytes"),
            )
            .mount(&server)
            .await;

        // 3. GET — the file body; must carry the per-pull token header.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!(
                "/test/repo/resolve/main/{FILE}"
            )))
            .and(wiremock::matchers::header(
                "authorization",
                "Bearer hf_pull_token_999",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let models_dir = tempfile::tempdir().unwrap();
        let models_dir_path = models_dir.path().to_path_buf();
        let registry = crate::jobs::JobRegistry::new();
        let req = PullModelRequest {
            repo_id: REPO.to_string(),
            quants: vec![FILE.to_string()],
            model_name: String::new(),
            backend: "llama_cpp".to_string(),
            hf_token: "hf_pull_token_999".to_string(),
            repo_pull: false,
            dest_dir: String::new(),
        };

        let hf_token = req.hf_token.clone();
        let job_dir = models_dir_path.clone();
        let job_id = registry
            .start("pull", move |handle| {
                Box::pin(async move { run_pull(&req, &job_dir, &hf_token, handle).await })
            })
            .await;

        let job = wait_terminal(&registry, &job_id).await;
        assert_eq!(
            job.status,
            crate::jobs::STATUS_SUCCEEDED,
            "GGUF pull must succeed, error: {:?}",
            job.error
        );

        // The file must land in models_dir/org/repo with the exact bytes.
        let dest = models_dir.path().join("test").join("repo").join(FILE);
        assert!(
            dest.exists(),
            "downloaded file must exist at {}",
            dest.display()
        );
        let downloaded = std::fs::read(&dest).unwrap();
        assert_eq!(downloaded, body);

        let result: TamadGgufPullResult =
            serde_json::from_str(&job.result_json.expect("terminal result json")).unwrap();
        assert_eq!(
            Path::new(&result.dir),
            models_dir.path().join("test/repo").as_path()
        );
        assert_eq!(result.files.len(), 1);
        let f = &result.files[0];
        assert_eq!(f.path, FILE);
        assert_eq!(f.size, body.len() as u64);
        // The verification hash in the result JSON matches the precomputed
        // SHA-256 (and the upstream LFS hash).
        assert_eq!(f.sha256.as_deref(), Some(sha.as_str()));
        assert_eq!(f.expected_sha.as_deref(), Some(sha.as_str()));
        assert!(f.verified);
        assert!(f.is_primary_shard);
        // Not a real GGUF — the metadata parse soft-fails to None.
        assert!(result.gguf_metadata.is_none());
        assert!(result.transformers_metadata.is_none());

        {
            let _g = ENV_GUARD.lock().unwrap();
            std::env::remove_var("HF_ENDPOINT");
        }
    }

    /// GGUF pull with a hash mismatch: the job fails and the corrupt file is
    /// deleted (same semantics as the proxy's run_verification).
    #[tokio::test]
    async fn test_run_pull_gguf_hash_mismatch_fails_and_deletes() {
        use sha2::{Digest, Sha256};

        let body: Vec<u8> = b"corrupt gguf bytes".to_vec();
        // Upstream hash of DIFFERENT content.
        let sha = format!("{:x}", Sha256::digest(b"different content"));
        const FILE: &str = "fake-model-Q4_K_M.gguf";
        const REPO: &str = "test/repo-mismatch";

        let server = wiremock::MockServer::start().await;
        {
            let _g = ENV_GUARD.lock().unwrap();
            std::env::set_var("HF_ENDPOINT", server.uri());
        }

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!("/api/models/{REPO}")))
            .and(wiremock::matchers::query_param("blobs", "true"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "siblings": [{
                        "rfilename": FILE,
                        "blobId": "b1",
                        "size": body.len(),
                        "lfs": { "sha256": sha }
                    }]
                })),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .and(wiremock::matchers::path(format!(
                "/{REPO}/resolve/main/{FILE}"
            )))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-length", body.len().to_string()),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!(
                "/{REPO}/resolve/main/{FILE}"
            )))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let models_dir = tempfile::tempdir().unwrap();
        let models_dir_path = models_dir.path().to_path_buf();
        let registry = crate::jobs::JobRegistry::new();
        let req = repo_request(REPO, &[FILE]);

        let job_dir = models_dir_path.clone();
        let job_id = registry
            .start("pull", move |handle| {
                Box::pin(async move { run_pull(&req, &job_dir, "", handle).await })
            })
            .await;

        let job = wait_terminal(&registry, &job_id).await;
        assert_eq!(job.status, crate::jobs::STATUS_FAILED);
        assert!(
            job.error
                .as_deref()
                .unwrap_or_default()
                .contains("hash mismatch"),
            "got: {:?}",
            job.error
        );
        // The corrupt file must be deleted — no corrupt data lingers.
        let dest = models_dir.path().join(REPO).join(FILE);
        assert!(!dest.exists(), "corrupt file must be deleted");

        {
            let _g = ENV_GUARD.lock().unwrap();
            std::env::remove_var("HF_ENDPOINT");
        }
    }
}
