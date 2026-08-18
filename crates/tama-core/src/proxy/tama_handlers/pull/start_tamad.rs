//! Tamad-hosted pull relay (plan-191 Task 6).
//!
//! When the proxy's `pull_backend` config names a registered tamad, queued
//! model pulls are dispatched to that tamad over gRPC (`PullModel`). The
//! download runs on the tamad's disk; this module relays the tamad's
//! `StreamJob` events into the proxy's existing PullJob / DB queue / SSE
//! progress tracking and captures the terminal event's `result_json` — the
//! host verified the file on its own disk and ships the hashes/size/
//! metadata — which the caller consumes in
//! [`super::verify::complete_pull_from_tamad_result`] to persist the
//! registry rows without touching proxy-local files.
//!
//! Fail-loud policy (ADR-0010): a dispatch or relay failure fails the pull
//! — the proxy never falls back to downloading locally.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::proxy::pull_jobs::PullJob;
use crate::proxy::tama_handlers::QuantPullSpec;
use crate::proxy::ProxyState;

/// How long the relay waits for the next tamad job event before declaring
/// the pull stalled (a healthy download reports progress every few seconds;
/// the hf-CLI path reports on stderr lines).
const EVENT_TIMEOUT_SECS: u64 = 120;

/// Outcome of a relayed tamad-hosted pull (plan-191 follow-up B extends the
/// terminal set with `cancelled`, mirroring the tamad job's terminal
/// status).
pub(super) enum TamadPullOutcome {
    /// The tamad reported success. Carries the terminal event's
    /// `result_json` (the host's verified file/size/hash/metadata payload)
    /// — the shared completion phase persists the registry rows from it.
    Succeeded(String),
    /// The pull was cancelled on the host (`CancelJob`; the user asked to
    /// cancel from the UI).
    Cancelled,
    /// Dispatch or relay failure with a human-readable message.
    Failed(String),
}

/// Attempt a tamad-hosted pull for a queued item (plan-191 Task 6).
///
/// Returns `None` when the proxy is not configured to route pulls to a
/// tamad (`pull_backend` unset) — the caller fails the pull (ADR-0010).
/// When `Some`, the relay ran to completion:
/// - return value — `TamadPullOutcome`:
///   - `Succeeded(result_json)` — the tamad reported success and the
///     `result_json` terminal payload (hashes/sizes/metadata) is ready for
///     the shared completion phase.
///   - `Cancelled` — the pull was cancelled on the host.
///   - `Failed(message)` — dispatch or relay failure; the caller marks the
///     pull failed with this message.
#[allow(clippy::too_many_arguments)]
pub(super) async fn try_start_tamad_pull(
    state: &Arc<ProxyState>,
    pull_jobs: &Arc<tokio::sync::RwLock<HashMap<String, PullJob>>>,
    job_id: &str,
    repo_id: &str,
    filename: &str,
    spec: &QuantPullSpec,
    dest_dir: &std::path::Path,
) -> Option<TamadPullOutcome> {
    let pull_backend = state.config.read().await.proxy.pull_backend.clone()?;

    let handle = match state.tamad_pool.get(&pull_backend).await {
        Some(h) => h,
        None => {
            // Explicitly configured but not registered: fail loud rather
            // than silently switching to local pulls (ADR-0010).
            return Some(TamadPullOutcome::Failed(format!(
                "pull_backend '{pull_backend}' is not a registered tamad"
            )));
        }
    };

    // The proxy-level HF token (env / token file) — same source the local
    // path uses. The token is sent over the authenticated gRPC channel and
    // never logged.
    let hf_token = crate::models::pull::get_hf_token().unwrap_or_default();

    let request = crate::tamad::PullModelRequest {
        repo_id: repo_id.to_string(),
        quants: vec![filename.to_string()],
        model_name: String::new(),
        backend: spec.quant.clone().unwrap_or_default(),
        hf_token,
        repo_pull: false,
        // Write to exactly the directory the proxy expects (the tamad's
        // own models_dir may be configured differently).
        dest_dir: dest_dir.to_string_lossy().to_string(),
    };

    tracing::info!(
        job_id,
        repo = repo_id,
        file = filename,
        tamad = %pull_backend,
        "Dispatching pull to tamad"
    );

    let tamad_job_id = match handle.pull_model(&request).await {
        Ok(id) => id,
        Err(e) => {
            return Some(TamadPullOutcome::Failed(format!(
                "tamad pull dispatch failed: {e:#}"
            )));
        }
    };

    // Remember the host job id so the cancel endpoint can dispatch
    // `CancelJob` to the right runner (plan-191 follow-up B).
    {
        let mut jobs = pull_jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
            job.tamad_job_id = Some(tamad_job_id.clone());
        }
    }

    Some(relay_job(state, pull_jobs, job_id, &handle, &tamad_job_id).await)
}

/// Stream the tamad's job events until a terminal state, mirroring them
/// into the PullJob (bytes) and the DB queue (SSE progress).
async fn relay_job(
    state: &Arc<ProxyState>,
    pull_jobs: &Arc<tokio::sync::RwLock<HashMap<String, PullJob>>>,
    job_id: &str,
    handle: &Arc<crate::tamad::pool::TamadHandle>,
    tamad_job_id: &str,
) -> TamadPullOutcome {
    let mut stream = match handle.stream_job(tamad_job_id).await {
        Ok(s) => s,
        Err(e) => return TamadPullOutcome::Failed(format!("tamad job stream failed: {e:#}")),
    };

    loop {
        let next =
            tokio::time::timeout(Duration::from_secs(EVENT_TIMEOUT_SECS), stream.message()).await;
        let ev = match next {
            Err(_elapsed) => {
                return TamadPullOutcome::Failed(format!(
                    "pull stalled: no tamad progress for {EVENT_TIMEOUT_SECS}s"
                ))
            }
            Ok(Err(e)) => {
                return TamadPullOutcome::Failed(format!("tamad job stream error: {e:?}"))
            }
            Ok(Ok(None)) => {
                // Stream ended without a terminal event: the tamad went
                // away mid-pull.
                return TamadPullOutcome::Failed(
                    "tamad disconnected mid-pull (no terminal job event)".to_string(),
                );
            }
            Ok(Ok(Some(e))) => e,
        };

        // Mirror the event into the in-memory PullJob (dashboard reads this).
        {
            let mut jobs = pull_jobs.write().await;
            if let Some(job) = jobs.get_mut(job_id) {
                if event_total_bytes(&ev) > 0 {
                    job.total_bytes = Some(event_total_bytes(&ev));
                }
                if ev.bytes_downloaded > 0 {
                    job.bytes_pulled = ev.bytes_downloaded as u64;
                }
            }
        }

        // Mirror into the DB queue item (drives the SSE progress stream).
        if let Some(svc) = state.pull_queue().as_ref() {
            let _ = svc
                .update_progress(job_id, ev.bytes_downloaded, event_total_bytes_i64(&ev))
                .await;
        }

        match ev.status.as_str() {
            "succeeded" => {
                // Carry the terminal result payload (host-verified
                // hashes/sizes/metadata) to the completion phase.
                return TamadPullOutcome::Succeeded(ev.result_json);
            }
            "cancelled" => return TamadPullOutcome::Cancelled,
            "failed" => {
                return TamadPullOutcome::Failed(format!("tamad pull failed: {}", ev.error.trim()))
            }
            _ => {}
        }
    }
}

fn event_total_bytes(ev: &crate::tamad::JobEvent) -> u64 {
    ev.total_bytes.max(0) as u64
}

fn event_total_bytes_i64(ev: &crate::tamad::JobEvent) -> Option<i64> {
    (ev.total_bytes > 0).then_some(ev.total_bytes)
}
