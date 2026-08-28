//! Tamad-hosted backend execution relay (plan-191 Task 7).
//!
//! Install/update execution happens on the *tamad* of the provider that
//! owns the backend (ADR-0010). This module resolves that tamad, dispatches
//! the job, and bridges tamad `StreamJob` events into the web `JobManager` —
//! so the jobs API (`GET /tama/v1/backends/jobs/:id`) and SSE UX are
//! unchanged from the old local execution. On a succeeded terminal, the DB
//! writer (always the proxy) persists the installation config rows from the
//! job's result JSON.
//!
//! Fail-loud policy: no local fallback. A dispatch/relay failure fails the
//! job with an actionable error.

use std::sync::Arc;
use std::time::Duration;

use tama_core::installations::types::InstallationSource;
use tama_core::installations::{InstallationInfo, InstallationManager, InstallationType};
use tama_core::proxy::ProxyState;
use tama_core::tamad::pool::TamadHandle;
use tama_core::updates::UpdateChecker;

use crate::web_types::{Job, JobManager, JobStatus};

/// How long the relay waits for the next tamad job event before declaring
/// the job stalled (downloaders and source builds stream output
/// continuously; five minutes of silence means the tamad hung).
pub const INSTALL_EVENT_STALL: Duration = Duration::from_secs(300);

/// The execution host resolved for a backend operation (name for logs +
/// the pool handle for RPC).
#[derive(Clone)]
pub struct BackendTamad {
    /// The provider that resolves this backend (name for error messages).
    pub provider_name: String,
    /// The tamad's display name (for error messages).
    pub tamad_name: String,
    pub handle: Arc<TamadHandle>,
}

/// Resolve the tamad that executes install/update/remove for a backend.
///
/// 1. A Local provider whose engine matches the backend type and that has
///    a tamad assigned.
/// 2. Fallback (single-node): the sole Local provider with a tamad.
///
/// Error strings are user-facing (surfaced as the job error / API error).
pub async fn resolve_backend_tamad(
    state: &Arc<ProxyState>,
    backend_type: &InstallationType,
) -> std::result::Result<BackendTamad, String> {
    let providers = tama_core::db::queries::list_providers(state.db_pool().as_ref())
        .await
        .map_err(|e| format!("failed to list providers: {e}"))?;
    let engine_str = backend_type.to_string();
    let local: Vec<_> = providers
        .into_iter()
        .filter(|p| p.provider_type.is_local() && p.tamad_id.is_some())
        .collect();

    if local.is_empty() {
        return Err(
            "no local provider with a tamad assigned — create one (POST /tama/v1/providers) and try again"
                .to_string(),
        );
    }
    let matching: Vec<_> = local
        .iter()
        .filter(|p| p.engine.to_string() == engine_str)
        .collect();
    let provider = match (matching.len(), local.len()) {
        (1, _) => matching[0].clone(),
        (0, 1) => local[0].clone(),
        (0, n) => {
            return Err(format!(
                "multiple local providers have tamads assigned ({n} total) and none matches engine '{engine_str}' — create a provider for this backend"
            ))
        }
        (n, _) => {
            return Err(format!(
                "multiple providers match engine '{engine_str}' ({n} found) — assign a tamad to exactly one of them"
            ))
        }
    };
    let tamad_id = provider.tamad_id.clone().expect("filtered to Some");
    let handle = state
        .tamad_pool()
        .handle_for_provider(Some(&tamad_id))
        .await
        .ok_or_else(|| {
            format!(
                "tamad of provider '{}' is not registered (tamad id {tamad_id}) — check the tamad registry and retry",
                provider.name
            )
        })?;
    Ok(BackendTamad {
        provider_name: provider.name,
        tamad_name: handle.connection.name.clone(),
        handle,
    })
}

/// Bridge a tamad job into the web `JobManager` until a terminal state.
///
/// - `running` event → `append_log` (the same log UX as local execution)
/// - `succeeded` → `Ok(result_json)`
/// - `failed` / stream error / EOF / stall → `Err(actionable message)`
pub async fn relay_tamad_job(
    jobs: &Arc<JobManager>,
    job: &Arc<Job>,
    handle: &Arc<TamadHandle>,
    tamad_job_id: &str,
    stall: Duration,
) -> std::result::Result<String, String> {
    let mut stream = handle
        .stream_job(tamad_job_id)
        .await
        .map_err(|e| format!("tamad job stream failed: {e}"))?;

    loop {
        let next = tokio::time::timeout(stall, stream.message()).await;
        let ev = match next {
            Err(_) => {
                return Err(format!(
                    "tamad job stalled: no progress for {}s",
                    stall.as_secs()
                ))
            }
            Ok(Err(e)) => return Err(format!("tamad job stream error: {e}")),
            Ok(Ok(None)) => {
                // Stream ended without a terminal event: the tamad went
                // away mid-job.
                return Err("tamad disconnected mid-job (no terminal job event)".to_string());
            }
            Ok(Ok(Some(ev))) => ev,
        };

        match ev.status.as_str() {
            "succeeded" => return Ok(ev.result_json),
            "failed" => return Err(format!("tamad job failed: {}", ev.error.trim())),
            _ => {
                let line = ev.message.trim();
                if !line.is_empty() && line != "started" {
                    jobs.append_log(job, line.to_string()).await;
                }
            }
        }
    }
}

/// Terminal job result as reported by the tamad (`installs::InstallResult`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstallResultDto {
    pub installed: bool,
    pub version: String,
    pub path: String,
}

/// Fail a web job with a log line + error (releases the active slot).
async fn fail_job(jobs: &Arc<JobManager>, job: &Arc<Job>, message: String) {
    jobs.append_log(job, format!("Error: {message}")).await;
    let _ = jobs.finish(job, JobStatus::Failed, Some(message)).await;
}

/// Everything the handler resolved for one install dispatch.
#[derive(Debug, Clone)]
pub struct InstallDispatch {
    pub backend_type: InstallationType,
    /// Backend name (the installation DB key).
    pub name: String,
    /// Version string for the tamad ("latest" or a tag; empty for
    /// tts_kokoro, which always installs its pinned tag).
    pub version: String,
    /// GPU variant folder (e.g. "cpu", "cuda").
    pub gpu_variant: String,
    /// Source-code git URL (empty → prebuilt download).
    pub git_url: String,
    /// Allow overwriting an existing version directory.
    pub force: bool,
    /// The source recorded on the installation row after success.
    pub source: InstallationSource,
}

/// Run one installation end-to-end as a web job: resolve the tamad,
/// dispatch `InstallProvider`, relay job events into the `JobManager` log,
/// and on a succeeded terminal persist the installation row (proxy =
/// single DB writer). Every failure path fails the job with an actionable
/// error message.
pub async fn execute_install(
    state: &Arc<ProxyState>,
    jobs: &Arc<JobManager>,
    job: &Arc<Job>,
    d: &InstallDispatch,
) {
    let backend_tamad = match resolve_backend_tamad(state, &d.backend_type).await {
        Ok(bt) => bt,
        Err(e) => {
            fail_job(jobs, job, e).await;
            return;
        }
    };

    let req = tama_core::tamad::InstallProviderRequest {
        name: d.name.clone(),
        engine: d.backend_type.to_string(),
        version: d.version.clone(),
        gpu_variant: d.gpu_variant.clone(),
        force: d.force,
        git_url: d.git_url.clone(),
    };
    tracing::info!(
        backend = %d.name,
        tamad = %backend_tamad.tamad_name,
        version = %d.version,
        "dispatching install to tamad"
    );

    let tamad_job_id = match backend_tamad.handle.install_provider(&req).await {
        Ok(id) => id,
        Err(e) => {
            fail_job(
                jobs,
                job,
                format!(
                    "install dispatch to tamad '{}' failed: {e}",
                    backend_tamad.tamad_name
                ),
            )
            .await;
            return;
        }
    };

    let result = relay_tamad_job(
        jobs,
        job,
        &backend_tamad.handle,
        &tamad_job_id,
        INSTALL_EVENT_STALL,
    )
    .await;
    match result {
        Ok(result_json) => {
            let res: InstallResultDto = match serde_json::from_str(&result_json) {
                Ok(r) => r,
                Err(e) => {
                    fail_job(
                        jobs,
                        job,
                        format!("tamad returned an invalid install result: {e}"),
                    )
                    .await;
                    return;
                }
            };
            let mgr = InstallationManager::new(state.db_pool());
            let installed_version = res.version.clone();
            let info = InstallationInfo {
                name: d.name.clone(),
                backend_type: d.backend_type.clone(),
                version: installed_version.clone(),
                path: std::path::PathBuf::from(res.path),
                installed_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|t| t.as_secs() as i64)
                    .unwrap_or(0),
                gpu_variant: d.gpu_variant.clone(),
                source: Some(d.source.clone()),
                docker_config: None,
            };
            match mgr.add_installation(&info).await {
                Ok(()) => {
                    tracing::info!(
                        backend = %d.name,
                        version = %installed_version,
                        "installation registered in DB"
                    );
                    let _ = jobs.finish(job, JobStatus::Succeeded, None).await;
                }
                Err(e) => {
                    fail_job(
                        jobs,
                        job,
                        format!("installed on tamad but DB registration failed: {e}"),
                    )
                    .await;
                }
            }
        }
        Err(e) => fail_job(jobs, job, e).await,
    }
}

/// Everything the handler resolved for one update dispatch.
#[derive(Debug, Clone)]
pub struct UpdateDispatch {
    pub backend_type: InstallationType,
    /// Backend name (the installation DB key).
    pub name: String,
    /// GPU variant folder of the active installation.
    pub gpu_variant: String,
    /// The already-resolved new version (the handler calls
    /// `check_latest_version` before dispatching).
    pub version: String,
    /// Source-code git URL (empty → prebuilt download).
    pub git_url: String,
    /// The source recorded on the installation row after success.
    pub source: InstallationSource,
}

/// Run one update end-to-end as a web job: resolve the tamad, dispatch
/// `UpdateProvider`, relay events, and on a succeeded terminal apply the
/// version change in the DB (deactivating the old row) + refresh the
/// update-check record.
pub async fn execute_update(
    state: &Arc<ProxyState>,
    jobs: &Arc<JobManager>,
    job: &Arc<Job>,
    d: &UpdateDispatch,
    checker: Arc<UpdateChecker>,
) {
    let backend_tamad = match resolve_backend_tamad(state, &d.backend_type).await {
        Ok(bt) => bt,
        Err(e) => {
            fail_job(jobs, job, e).await;
            return;
        }
    };

    let req = tama_core::tamad::UpdateProviderRequest {
        name: d.name.clone(),
        version: d.version.clone(),
        engine: d.backend_type.to_string(),
        gpu_variant: d.gpu_variant.clone(),
        git_url: d.git_url.clone(),
    };
    tracing::info!(
        backend = %d.name,
        tamad = %backend_tamad.tamad_name,
        version = %d.version,
        "dispatching update to tamad"
    );

    let tamad_job_id = match backend_tamad.handle.update_provider(&req).await {
        Ok(id) => id,
        Err(e) => {
            fail_job(
                jobs,
                job,
                format!(
                    "update dispatch to tamad '{}' failed: {e}",
                    backend_tamad.tamad_name
                ),
            )
            .await;
            return;
        }
    };

    let result = relay_tamad_job(
        jobs,
        job,
        &backend_tamad.handle,
        &tamad_job_id,
        INSTALL_EVENT_STALL,
    )
    .await;
    let Ok(result_json) = result else {
        fail_job(jobs, job, result.unwrap_err()).await;
        return;
    };

    let res: InstallResultDto = match serde_json::from_str(&result_json) {
        Ok(r) => r,
        Err(e) => {
            fail_job(
                jobs,
                job,
                format!("tamad returned an invalid update result: {e}"),
            )
            .await;
            return;
        }
    };

    let mgr = InstallationManager::new(state.db_pool());
    let updated_version = res.version.clone();
    match mgr
        .update_version(
            &d.name,
            &d.gpu_variant,
            updated_version.clone(),
            std::path::PathBuf::from(res.path),
            Some(d.source.clone()),
        )
        .await
    {
        Ok(()) => {
            tracing::info!(
                backend = %d.name,
                version = %updated_version,
                "update registered in DB"
            );
            let _ = jobs.finish(job, JobStatus::Succeeded, None).await;
            // Refresh the Updates Center record (fire-and-forget: it does
            // a network check and must not prolong the job).
            let pool = state.db_pool();
            let name = d.name.clone();
            let backend_type = d.backend_type.clone();
            let gpu_variant = d.gpu_variant.clone();
            tokio::spawn(async move {
                if let Err(e) = checker
                    .check_backend(pool.as_ref(), &name, &backend_type, &gpu_variant)
                    .await
                {
                    tracing::debug!(backend = %name, error = %e, "post-update check failed");
                }
            });
        }
        Err(e) => {
            fail_job(
                jobs,
                job,
                format!("updated on tamad but DB registration failed: {e}"),
            )
            .await;
        }
    }
}

/// Remove a backend (or one variant / one version) from a tamad host:
/// `RemoveProvider` RPC (kills the backend's processes + deletes the
/// versioned install directories).
pub async fn remove_on_tamad(
    state: &Arc<ProxyState>,
    backend_type: &InstallationType,
    name: &str,
    gpu_variant: Option<&str>,
    version: Option<&str>,
) -> std::result::Result<(), String> {
    let backend_tamad = resolve_backend_tamad(state, backend_type).await?;
    tracing::info!(
        backend = %name,
        tamad = %backend_tamad.tamad_name,
        variant = gpu_variant.unwrap_or("<all>"),
        version = version.unwrap_or("<all>"),
        "dispatching remove to tamad"
    );
    let req = tama_core::tamad::RemoveProviderRequest {
        name: name.to_string(),
        engine: backend_type.to_string(),
        gpu_variant: gpu_variant.unwrap_or_default().to_string(),
        version: version.unwrap_or_default().to_string(),
    };
    backend_tamad
        .handle
        .remove_provider(&req)
        .await
        .map_err(|e| format!("remove on tamad '{}' failed: {e}", backend_tamad.tamad_name))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use tama_core::tamad::pool::test_support::{
        grpc_conn, job_event, start_stub, terminal_success, StubTamad,
    };
    use tama_core::tamad::JobEvent;

    const TAMAD_ID: &str = "uuid-install-tamad";

    fn result_json(version: &str, path: &str) -> String {
        serde_json::json!({ "installed": true, "version": version, "path": path }).to_string()
    }

    /// StubTamad scripted with the given job events; returns the stub plus
    /// the `down` channel (cutting the mid-job streams when replaced to
    /// true).
    fn stub_with(
        events: Vec<JobEvent>,
        install_fail: bool,
        update_fail: bool,
    ) -> (StubTamad, Arc<tokio::sync::watch::Sender<bool>>) {
        let (down_tx, _) = tokio::sync::watch::channel(false);
        let down = Arc::new(down_tx);
        let stub = StubTamad {
            fail_first_n: 0,
            succeed_until: usize::MAX,
            down: Arc::clone(&down),
            calls: Arc::new(AtomicUsize::new(0)),
            successes: Arc::new(AtomicUsize::new(0)),
            pull_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            pull_job_id: "job-pull".to_string(),
            pull_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            install_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            install_job_id: "job-install".to_string(),
            install_dispatch_fail: Arc::new(tokio::sync::Mutex::new(install_fail)),
            update_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            update_job_id: "job-update".to_string(),
            update_dispatch_fail: Arc::new(tokio::sync::Mutex::new(update_fail)),
            remove_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            remove_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stream_job_events: Arc::new(tokio::sync::Mutex::new(events)),
            stream_job_calls: Arc::new(AtomicUsize::new(0)),
            stream_job_events_by_id: Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            bench_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            bench_job_id: "job-bench".to_string(),
            bench_dispatch_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_gpus: vec![],
            load_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            load_delays: std::collections::HashMap::new(),
            load_model_fail: Arc::new(tokio::sync::Mutex::new(false)),
            stats_processes: vec![],
            logs_requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            log_messages: vec![],
            stream_log_frames: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stream_log_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            stream_log_refuse: false,
        };
        (stub, down)
    }

    /// ProxyState + stub tamad in the pool + a provider row for llama_cpp.
    ///
    /// `with_provider = false` → no provider row (resolution-failure test).
    async fn setup(
        (stub, _down): (StubTamad, Arc<tokio::sync::watch::Sender<bool>>),
        with_provider: bool,
    ) -> (Arc<ProxyState>, crate::testing::postgres::SchemaGuard) {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let state = Arc::new(ProxyState::new(
            tama_core::config::Config::default(),
            None,
            pool,
        ));

        if with_provider {
            tama_core::db::queries::insert_provider(
                state.db_pool().as_ref(),
                "loc-llama",
                "local",
                "llama_cpp",
                Some(TAMAD_ID),
                None,
                None,
            )
            .await
            .expect("provider insert");
        }
        let addr = start_stub(stub.clone()).await;
        let conn = grpc_conn(TAMAD_ID, "stub", &format!("grpc://{addr}"));
        state
            .tamad_pool()
            .upsert_connection(&conn)
            .await
            .expect("pool upsert");
        (state, guard)
    }

    fn jobs() -> Arc<JobManager> {
        Arc::new(JobManager::new())
    }

    fn install_dispatch(version: &str) -> InstallDispatch {
        InstallDispatch {
            backend_type: InstallationType::LlamaCpp,
            name: "llama_cpp".to_string(),
            version: version.to_string(),
            gpu_variant: "cuda".to_string(),
            git_url: String::new(),
            force: false,
            source: InstallationSource::Prebuilt {
                version: version.to_string(),
            },
        }
    }

    async fn job_state(job: &Arc<Job>) -> (JobStatus, Option<String>) {
        let st = job.state.read().await;
        (st.status, st.error.clone())
    }

    async fn job_log(job: &Arc<Job>) -> Vec<String> {
        job.log_head
            .read()
            .await
            .iter()
            .cloned()
            .chain(job.log_tail.read().await.iter().cloned())
            .collect()
    }

    /// Success: the stub receives the dispatch, the job log carries the
    /// progress lines, the job succeeds, and the installation row is
    /// persisted from the result JSON (proxy = DB writer).
    #[tokio::test]
    async fn test_execute_install_success_writes_db_row_and_log() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("install/llama_cpp/cuda/b9123/llama-server");
        let events = vec![
            job_event("job-install", 0, "cloning repo", "running"),
            job_event("job-install", 0, "building llama-server", "running"),
            terminal_success(
                "job-install",
                &result_json("b9123", marker.to_str().unwrap()),
            ),
        ];
        let (stub, _down) = stub_with(events, false, false);
        let (state, _guard) = setup((stub.clone(), _down), true).await;
        let jobs = jobs();
        let job = jobs
            .submit(
                crate::web_types::JobKind::Install,
                Some(InstallationType::LlamaCpp),
            )
            .await
            .expect("submit");

        execute_install(&state, &jobs, &job, &install_dispatch("b9123")).await;

        // The stub recorded the dispatch verbatim.
        let reqs = stub.install_requests.lock().await;
        assert_eq!(reqs.len(), 1, "exactly one install dispatch");
        assert_eq!(reqs[0].engine, "llama_cpp");
        assert_eq!(reqs[0].version, "b9123");
        assert_eq!(reqs[0].gpu_variant, "cuda");
        assert!(reqs[0].git_url.is_empty(), "prebuilt: empty git_url");

        // Job succeeded with the progress lines in its log.
        let (status, error) = job_state(&job).await;
        assert_eq!(status, JobStatus::Succeeded, "error: {error:?}");
        let log = job_log(&job).await;
        assert!(
            log.iter().any(|l| l == "cloning repo"),
            "log must carry tamad progress lines: {log:?}"
        );
        assert!(log.iter().any(|l| l == "building llama-server"));

        // DB row persisted from the result JSON.
        let mgr = InstallationManager::new(state.db_pool());
        let active = mgr
            .get_active("llama_cpp", "cuda")
            .await
            .unwrap()
            .expect("installation row");
        assert_eq!(active.version, "b9123");
        assert_eq!(active.path, marker);
        assert_eq!(active.gpu_variant, "cuda");
    }

    /// Dispatch failure (tamad offline): the job fails with a dispatch
    /// error and no installation row is written.
    #[tokio::test]
    async fn test_execute_install_dispatch_failure_fails_job() {
        let (stub, _down) = stub_with(Vec::new(), true, false);
        let (state, _guard) = setup((stub, _down), true).await;
        let jobs = jobs();
        let job = jobs
            .submit(
                crate::web_types::JobKind::Install,
                Some(InstallationType::LlamaCpp),
            )
            .await
            .expect("submit");

        execute_install(&state, &jobs, &job, &install_dispatch("b9123")).await;

        let (status, error) = job_state(&job).await;
        assert_eq!(status, JobStatus::Failed);
        assert!(
            error
                .as_deref()
                .unwrap_or_default()
                .contains("install dispatch to tamad"),
            "got: {error:?}"
        );
        let mgr = InstallationManager::new(state.db_pool());
        assert!(
            mgr.get_active("llama_cpp", "cuda").await.unwrap().is_none(),
            "no row may be written on dispatch failure"
        );
    }

    /// No local provider with a tamad: the job fails with an actionable
    /// resolution error.
    #[tokio::test]
    async fn test_execute_install_no_provider_fails_job() {
        let (stub, _down) = stub_with(Vec::new(), false, false);
        let (state, _guard) = setup((stub, _down), false).await;
        let jobs = jobs();
        let job = jobs
            .submit(
                crate::web_types::JobKind::Install,
                Some(InstallationType::LlamaCpp),
            )
            .await
            .expect("submit");

        execute_install(&state, &jobs, &job, &install_dispatch("b9123")).await;

        let (status, error) = job_state(&job).await;
        assert_eq!(status, JobStatus::Failed);
        assert!(
            error
                .as_deref()
                .unwrap_or_default()
                .contains("no local provider with a tamad"),
            "got: {error:?}"
        );
    }

    /// Tamad reports a failed terminal: the job fails with the tamad's
    /// error and no installation row is written.
    #[tokio::test]
    async fn test_execute_install_tamad_failure_event_fails_job() {
        let events = vec![tama_core::tamad::pool::test_support::job_event_failed(
            "job-install",
            "synthetic tamad build error",
        )];
        let (stub, _down) = stub_with(events, false, false);
        let (state, _guard) = setup((stub, _down), true).await;
        let jobs = jobs();
        let job = jobs
            .submit(
                crate::web_types::JobKind::Install,
                Some(InstallationType::LlamaCpp),
            )
            .await
            .expect("submit");

        execute_install(&state, &jobs, &job, &install_dispatch("b9123")).await;

        let (status, error) = job_state(&job).await;
        assert_eq!(status, JobStatus::Failed);
        assert!(
            error
                .as_deref()
                .unwrap_or_default()
                .contains("synthetic tamad build error"),
            "got: {error:?}"
        );
        let mgr = InstallationManager::new(state.db_pool());
        assert!(mgr.get_active("llama_cpp", "cuda").await.unwrap().is_none());
    }

    /// Update: the new version row is applied (old row deactivated) and the
    /// job succeeds.
    #[tokio::test]
    async fn test_execute_update_success_writes_new_version() {
        let tmp = tempfile::tempdir().unwrap();
        let old_bin = tmp.path().join("install/llama_cpp/cuda/b8000/llama-server");
        let new_bin = tmp.path().join("install/llama_cpp/cuda/b9123/llama-server");

        let seed = stub_with(Vec::new(), false, false);
        let (state, _guard) = setup((seed.0, seed.1), true).await;
        // Seed the current installation.
        let mgr = InstallationManager::new(state.db_pool());
        mgr.add_installation(&InstallationInfo {
            name: "llama_cpp".into(),
            backend_type: InstallationType::LlamaCpp,
            version: "b8000".into(),
            path: old_bin.clone(),
            installed_at: 1_700_000_000,
            gpu_variant: "cuda".into(),
            source: Some(InstallationSource::Prebuilt {
                version: "b8000".into(),
            }),
            docker_config: None,
        })
        .await
        .expect("seed installation");

        let events = vec![
            job_event("job-update", 0, "downloading b9123", "running"),
            terminal_success(
                "job-update",
                &result_json("b9123", new_bin.to_str().unwrap()),
            ),
        ];
        // Restart the stub with the scripted events.
        let (stub, _down) = stub_with(events, false, false);
        let addr = tama_core::tamad::pool::test_support::start_stub(stub.clone()).await;
        let conn = grpc_conn(TAMAD_ID, "stub", &format!("grpc://{addr}"));
        state
            .tamad_pool()
            .upsert_connection(&conn)
            .await
            .expect("pool upsert");

        let jobs = jobs();
        let job = jobs
            .submit(
                crate::web_types::JobKind::Update,
                Some(InstallationType::LlamaCpp),
            )
            .await
            .expect("submit");

        execute_update(
            &state,
            &jobs,
            &job,
            &UpdateDispatch {
                backend_type: InstallationType::LlamaCpp,
                name: "llama_cpp".to_string(),
                gpu_variant: "cuda".to_string(),
                version: "b9123".to_string(),
                git_url: String::new(),
                source: InstallationSource::Prebuilt {
                    version: "b9123".into(),
                },
            },
            Arc::new(UpdateChecker::default()),
        )
        .await;

        let reqs = stub.update_requests.lock().await;
        assert_eq!(reqs.len(), 1, "exactly one update dispatch");
        assert_eq!(reqs[0].version, "b9123");

        let (status, error) = job_state(&job).await;
        assert_eq!(status, JobStatus::Succeeded, "error: {error:?}");

        let active = mgr
            .get_active("llama_cpp", "cuda")
            .await
            .unwrap()
            .expect("updated row");
        assert_eq!(active.version, "b9123", "new version must be active");
        assert_eq!(active.path, new_bin);
    }

    /// Relay stall: a job stream that never produces a terminal event fails
    /// the job with a stall error (short timeout in this test).
    #[tokio::test]
    async fn test_relay_tamad_job_stall_fails() {
        let events = vec![job_event("job-install", 0, "hanging", "running")];
        let (stub, _down) = stub_with(events, false, false);
        let (state, _guard) = setup((stub, _down), true).await;
        let jobs = jobs();
        let job = jobs
            .submit(
                crate::web_types::JobKind::Install,
                Some(InstallationType::LlamaCpp),
            )
            .await
            .expect("submit");

        // Open the stream through the dispatch so the stall is observable
        // end-to-end (dispatch → relay → stall).
        let handle = state.tamad_pool().get(TAMAD_ID).await.expect("handle");
        let err = relay_tamad_job(
            &jobs,
            &job,
            &handle,
            "job-install",
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
        assert!(err.contains("stalled"), "got: {err}");
    }

    /// Relay EOF: the stream ends before a terminal event (tamad died
    /// mid-job).
    #[tokio::test]
    async fn test_relay_tamad_job_eof_fails() {
        // One running event; the stream is held open and then cut via
        // `down` once the relay has subscribed.
        let (stub, down) = stub_with(
            vec![job_event("job-install", 0, "working", "running")],
            false,
            false,
        );
        let (state, _guard) = setup((stub.clone(), down.clone()), true).await;
        let jobs = jobs();
        let job = jobs
            .submit(
                crate::web_types::JobKind::Install,
                Some(InstallationType::LlamaCpp),
            )
            .await
            .expect("submit");
        let handle = state.tamad_pool().get(TAMAD_ID).await.expect("handle");

        let relay = tokio::spawn({
            let jobs = jobs.clone();
            let job = job.clone();
            let handle = handle.clone();
            async move {
                relay_tamad_job(&jobs, &job, &handle, "job-install", Duration::from_secs(30)).await
            }
        });

        // Cut the stream after the relay has subscribed.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        loop {
            if stub
                .stream_job_calls
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "relay never subscribed"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }
        down.send_replace(true);

        let err = relay.await.unwrap().unwrap_err();
        assert!(err.contains("disconnected mid-job"), "got: {err}");
    }
}
