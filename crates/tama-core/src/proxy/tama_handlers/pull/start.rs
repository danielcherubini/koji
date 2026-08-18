use std::sync::Arc;

use crate::models::repo_path;
use crate::proxy::tama_handlers::types::{is_safe_relative_path, QuantPullSpec};
use crate::proxy::ProxyState;

/// Start a pull from the queue.
///
/// This is the ONLY code path that starts a pull from the queue processor.
/// Takes a `job_id`, `state`, and `QuantPullSpec`, performs the actual pull,
/// and updates both the DB queue item and in-memory PullJob on completion/failure.
pub async fn start_pull_from_queue(
    state: Arc<ProxyState>,
    job_id: String,
    repo_id: String,
    filename: String,
    spec: QuantPullSpec,
) {
    let pull_jobs_arc = Arc::clone(&state.pull.pull_jobs);
    let in_flight_clone = Arc::clone(&state.pull.in_flight_pulls);
    let state_clone = Arc::clone(&state);
    let job_id_clone = job_id.clone();
    let repo_id_clone = repo_id.clone();
    let filename_clone = filename.clone();
    let spec_clone = spec.clone();

    // Record start time for duration calculation
    let pull_start = std::time::Instant::now();

    tracing::info!(
        job_id = %job_id_clone,
        repo = %repo_id_clone,
        file = %filename_clone,
        "Starting pull job"
    );

    // Validate filename and repo_id to prevent path traversal.
    if !is_safe_relative_path(&filename_clone) {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
            job.error = Some("Invalid filename".to_string());
        }
        drop(jobs);
        if let Some(ref svc) = state_clone.pull_queue() {
            let _ = svc
                .update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some("Invalid filename"),
                    None,
                )
                .await;
        }
        return;
    }
    if !crate::models::is_valid_repo_id(&repo_id_clone) {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
            job.error = Some("Invalid repo_id".to_string());
        }
        drop(jobs);
        if let Some(ref svc) = state_clone.pull_queue() {
            let _ = svc
                .update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some("Invalid repo_id"),
                    None,
                )
                .await;
        }
        return;
    }

    // Update status to Running
    {
        let mut jobs = pull_jobs_arc.write().await;
        let map_ptr = &*jobs as *const _;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Running;
            tracing::info!(
                job_id = %job_id_clone,
                map_addr = ?map_ptr,
                "Job transitioned to Running"
            );
        } else {
            tracing::warn!(job_id = %job_id_clone, "Job not found when setting Running");
            return;
        }
    }

    let models_dir = match state_clone.config.read().await.models_dir() {
        Ok(d) => d,
        Err(e) => {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(format!("Failed to get models dir: {}", e));
            }
            drop(jobs);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc
                    .update_status(
                        &job_id_clone,
                        "failed",
                        0,
                        None,
                        Some(&format!("Failed to get models dir: {}", e)),
                        None,
                    )
                    .await;
            }
            return;
        }
    };
    // Use the two-level org/repo directory structure (e.g. "unsloth/Qwen3.5-35B-A3B-GGUF")
    // to match the convention expected by ModelRegistry (models_dir/org/repo).
    let dest_dir = repo_path(&models_dir, &repo_id_clone);
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
            job.error = Some(format!("Failed to create dest dir: {}", e));
        }
        drop(jobs);
        if let Some(ref svc) = state_clone.pull_queue() {
            let _ = svc
                .update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Failed to create dest dir: {}", e)),
                    None,
                )
                .await;
        }
        return;
    }

    let dest_path = dest_dir.join(&filename_clone);

    // Create the parent directory of dest_path (e.g. for sharded files like
    // "UD-Q4_K_XL/shard.gguf" the subdirectory doesn't exist yet).
    if let Some(parent) = dest_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(format!("Failed to create destination subdirectory: {}", e));
            }
            drop(jobs);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc
                    .update_status(
                        &job_id_clone,
                        "failed",
                        0,
                        None,
                        Some(&format!("Failed to create destination subdirectory: {}", e)),
                        None,
                    )
                    .await;
            }
            return;
        }
    }

    // In-flight dedup guard: reject if another task is already pulling this path.
    // This prevents two concurrent tasks from writing to the same temp part files,
    // which would silently corrupt the assembled output.
    {
        let mut inflight = in_flight_clone.lock().await;
        if !inflight.insert(dest_path.clone()) {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(format!(
                    "Another pull of '{}' is already in progress",
                    filename_clone
                ));
            }
            drop(jobs);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc
                    .update_status(
                        &job_id_clone,
                        "failed",
                        0,
                        None,
                        Some(&format!(
                            "Another pull of '{}' is already in progress",
                            filename_clone
                        )),
                        None,
                    )
                    .await;
            }
            return;
        }
    }

    // ── Tamad-hosted pull (plan-191 Task 6 / follow-up B; ADR-0010) ──
    // The download ALWAYS runs on the tamad named by `proxy.pull_backend`;
    // the proxy relays StreamJob events into this PullJob + the DB queue
    // (SSE) and never downloads locally. Fail loud when no host is
    // configured — silent local fallback was removed with the ADR.
    match super::start_tamad::try_start_tamad_pull(
        &state_clone,
        &pull_jobs_arc,
        &job_id_clone,
        &repo_id_clone,
        &filename_clone,
        &spec_clone,
        &dest_dir,
    )
    .await
    {
        None => {
            let msg = "no pull host configured: set proxy.pull_backend (the proxy itself never downloads — ADR-0010)";
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(msg.to_string());
            }
            drop(jobs);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc
                    .update_status(&job_id_clone, "failed", 0, None, Some(msg), None)
                    .await;
            }
        }
        Some(super::start_tamad::TamadPullOutcome::Succeeded(result_json)) => {
            // Shared completion phase for the tamad-routed pull: model card
            // + `model_files`/verification rows — fed by the host's terminal
            // `result_json` (hashes/sizes/metadata verified on the host),
            // never by re-reading or re-hashing proxy-local files (ADR-0010).
            super::verify::complete_pull_from_tamad_result(
                &pull_jobs_arc,
                &state_clone,
                &in_flight_clone,
                &job_id_clone,
                &repo_id_clone,
                &filename_clone,
                &spec_clone,
                &dest_dir,
                &dest_path,
                pull_start,
                &result_json,
            )
            .await;
        }
        Some(super::start_tamad::TamadPullOutcome::Cancelled) => {
            tracing::info!(job_id = %job_id_clone, "pull cancelled on host");
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Cancelled;
            }
            drop(jobs);
            in_flight_clone.lock().await.remove(&dest_path);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc
                    .update_status(&job_id_clone, "cancelled", 0, None, None, None)
                    .await;
            }
        }
        Some(super::start_tamad::TamadPullOutcome::Failed(error)) => {
            tracing::error!(job_id = %job_id_clone, repo = %repo_id_clone, error = %error, "Tamad-hosted pull failed");
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(error.clone());
            }
            drop(jobs);
            in_flight_clone.lock().await.remove(&dest_path);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc
                    .update_status(&job_id_clone, "failed", 0, None, Some(&error), None)
                    .await;
            }
        }
    }
}
