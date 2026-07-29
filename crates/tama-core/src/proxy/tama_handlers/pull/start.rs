use std::sync::Arc;

use crate::models::repo_path;
use crate::proxy::pull_jobs::PullJobStatus;
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
            let _ = svc.update_status(
                &job_id_clone,
                "failed",
                0,
                None,
                Some("Invalid filename"),
                None,
            );
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
            let _ = svc.update_status(
                &job_id_clone,
                "failed",
                0,
                None,
                Some("Invalid repo_id"),
                None,
            );
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
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Failed to get models dir: {}", e)),
                    None,
                );
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
            let _ = svc.update_status(
                &job_id_clone,
                "failed",
                0,
                None,
                Some(&format!("Failed to create dest dir: {}", e)),
                None,
            );
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
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Failed to create destination subdirectory: {}", e)),
                    None,
                );
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
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!(
                        "Another pull of '{}' is already in progress",
                        filename_clone
                    )),
                    None,
                );
            }
            return;
        }
    }

    // Resolve URL and auth headers (shared by HEAD + pull)
    let resolve_url = crate::models::pull::hf_resolve_url(&repo_id_clone, &filename_clone);
    let headers = crate::models::pull::hf_auth_headers();

    // HEAD request to get total_bytes upfront
    let client = reqwest::Client::new();
    if let Ok(resp) = client
        .head(&resolve_url)
        .headers(headers.clone())
        .send()
        .await
    {
        let total = crate::models::pull::parse_content_length(resp.headers());
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.total_bytes = total;
        }
        drop(jobs);
    }

    // Spawn a task that polls file size every 500ms to update bytes_pulled
    // and pushes progress updates to the DB queue for SSE streaming.
    let poll_jobs = Arc::clone(&pull_jobs_arc);
    let poll_job_id = job_id_clone.clone();
    let poll_dest = dest_path.clone();
    let poll_pull_queue = state_clone.pull_queue().clone();
    let poll_handle = tokio::spawn(async move {
        let mut last_progress_pct: u32 = 0;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            // If the job is no longer running, stop polling
            {
                let jobs = poll_jobs.read().await;
                if let Some(job) = jobs.get(&poll_job_id) {
                    if !matches!(job.status, PullJobStatus::Running) {
                        break;
                    }
                } else {
                    break;
                }
            }
            // Read file size from disk
            if let Ok(meta) = tokio::fs::metadata(&poll_dest).await {
                let bytes_pulled = meta.len();
                let mut jobs = poll_jobs.write().await;
                if let Some(job) = jobs.get_mut(&poll_job_id) {
                    job.bytes_pulled = bytes_pulled;
                    // Push progress to DB queue for SSE streaming (throttled to 1% steps)
                    if let Some(total) = job.total_bytes {
                        if total > 0 {
                            let pct = (bytes_pulled as f64 / total as f64 * 100.0) as u32;
                            if pct > last_progress_pct {
                                last_progress_pct = pct;
                                drop(jobs);
                                if let Some(ref svc) = poll_pull_queue {
                                    let _ = svc.update_progress(
                                        &poll_job_id,
                                        bytes_pulled as i64,
                                        Some(total as i64),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    // Create progress callback that updates job status and emits SSE events
    let progress_jobs = Arc::clone(&pull_jobs_arc);
    let progress_job_id = job_id_clone.clone();
    let progress_queue = state_clone.pull_queue().clone();
    let progress_callback: crate::models::pull::ProgressCallback =
        Arc::new(move |pulled: u64, total: u64| {
            let job_id = progress_job_id.clone();
            // Use try_write to avoid blocking the pull task
            if let Ok(mut jobs) = progress_jobs.try_write() {
                if let Some(job) = jobs.get_mut(&job_id) {
                    job.bytes_pulled = pulled;
                    if total > 0 && job.total_bytes.is_none() {
                        job.total_bytes = Some(total);
                    }
                }
            }
            // Emit SSE progress event directly
            if let Some(ref svc) = progress_queue {
                let _ = svc.update_progress(&job_id, pulled as i64, Some(total as i64));
            }
        });

    tracing::info!(
        job_id = %job_id_clone,
        repo = %repo_id_clone,
        file = %filename_clone,
        "Beginning file pull via parallel puller"
    );

    // Build pull URL (endpoint + headers already resolved above for HEAD)
    let pull_url = resolve_url;

    // Build client with HTTP/2 keep-alive
    let pull_client = match reqwest::Client::builder()
        .http2_keep_alive_timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(format!("Failed to build HTTP client: {}", e));
            }
            drop(jobs);
            poll_handle.abort();
            in_flight_clone.lock().await.remove(&dest_path);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Failed to build HTTP client: {}", e)),
                    None,
                );
            }
            return;
        }
    };

    // Pull directly to dest_path (no cache intermediary)
    let total_size = match crate::models::pull::pull_chunked_with_progress(
        &pull_client,
        &pull_url,
        &dest_path,
        8, // max connections
        Some(progress_callback),
        Some(&headers),
    )
    .await
    {
        Ok(size) => size,
        Err(e) => {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(format!("Pull failed: {}", e));
            }
            drop(jobs);
            poll_handle.abort();
            in_flight_clone.lock().await.remove(&dest_path);
            if let Some(ref svc) = state_clone.pull_queue() {
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Pull failed: {}", e)),
                    None,
                );
            }
            return;
        }
    };

    let pull_duration = pull_start.elapsed();
    tracing::info!(
        job_id = %job_id_clone,
        bytes = total_size,
        duration = ?pull_duration,
        "Pull phase complete, entering verify phase"
    );

    // Stop the file size polling task.
    poll_handle.abort();

    // Record final pulled byte count.
    {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.bytes_pulled = total_size;
            job.total_bytes = Some(total_size);
        }
    }

    // Verify the file at its destination. On failure the file is deleted
    // so no corrupt data lingers.
    let outcome = super::verify::run_verification(
        Arc::clone(&pull_jobs_arc),
        state_clone.db_dir.clone(),
        state_clone.pull_queue().clone(),
        job_id_clone.clone(),
        repo_id_clone.clone(),
        filename_clone.clone(),
        spec_clone.quant.clone(),
        dest_path.clone(),
        total_size,
    )
    .await;

    // Calculate duration for DB event
    let duration_ms = Some(pull_start.elapsed().as_millis() as u64);

    // Parse GGUF metadata (soft failure — don't fail the pull)
    // Skip mmproj files — they're vision projectors, not LLM models.
    // Skip MTP files too — they're draft models for speculative decoding,
    // not the main LLM, and their architecture metadata is not what we
    // want to record on the parent model config.
    let skip_gguf_parse = matches!(
        crate::config::QuantKind::from_filename(&filename_clone),
        crate::config::QuantKind::Mmproj | crate::config::QuantKind::Mtp
    );
    let gguf_metadata = if outcome.passed && !skip_gguf_parse {
        match crate::models::gguf::parse_gguf_metadata(&dest_path) {
            Ok(meta) => {
                tracing::info!(
                    job_id = %job_id_clone,
                    architecture = ?meta.architecture,
                    context_length = ?meta.context_length,
                    "GGUF metadata parsed"
                );
                Some(meta)
            }
            Err(e) => {
                tracing::warn!(
                    job_id = %job_id_clone,
                    error = %e,
                    "GGUF metadata parsing failed — using defaults"
                );
                None
            }
        }
    } else {
        None
    };

    // Store GGUF metadata in PullJob for SSE streaming
    {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.gguf_metadata = gguf_metadata.clone();
            // Also set the serialized field for SSE events (frontend reads this)
            job.gguf_context_length = gguf_metadata.as_ref().and_then(|m| m.context_length);
        }
    }

    // Only register the model in config/card once the file is at its
    // destination and known-good. setup_model_after_pull creates the
    // matching model_configs row, which the model_files row below FKs to.
    if outcome.passed {
        let model_id = super::verify::setup_model_after_pull(
            Arc::clone(&state_clone),
            &repo_id_clone,
            &spec_clone,
            &dest_dir,
            gguf_metadata.clone(),
            outcome.is_primary_shard,
        )
        .await;

        // Persist the hash + verification state to model_files now that
        // the parent model_configs row exists. Use the id returned by
        // setup_model_after_pull so there's no case-sensitive lookup in
        // between that could miss.
        match (state_clone.model_mgr(), model_id) {
            (Some(mgr), Some(mid)) => {
                if let Err(e) = mgr.upsert_file(
                    mid,
                    &repo_id_clone,
                    &filename_clone,
                    spec_clone.quant.as_deref(),
                    outcome.expected_sha.as_deref(),
                    Some(total_size as i64),
                ) {
                    tracing::error!(
                        job_id = %job_id_clone,
                        model_id = mid,
                        file = %filename_clone,
                        error = %e,
                        "upsert_model_file failed"
                    );
                } else {
                    tracing::info!(
                        job_id = %job_id_clone,
                        model_id = mid,
                        file = %filename_clone,
                        "model_files row written"
                    );
                }
                // Tag the model_files row with the file kind so downstream
                // consumers can distinguish MTP draft models from regular
                // GGUF quants. `upsert_file` does not currently accept a
                // `kind` parameter, so we issue a follow-up UPDATE. Mirrors
                // the `QuantKind::from_filename` logic used to drive the
                // card's `kind` field.
                let db_kind = match crate::config::QuantKind::from_filename(&filename_clone) {
                    crate::config::QuantKind::Model => "model",
                    crate::config::QuantKind::Mmproj => "mmproj",
                    crate::config::QuantKind::Mtp => "mtp",
                };
                if db_kind != "model" {
                    if let Err(e) = mgr.conn().execute(
                        "UPDATE model_files SET kind = ?1
                          WHERE model_id = ?2 AND filename = ?3",
                        rusqlite::params![db_kind, mid, filename_clone],
                    ) {
                        tracing::warn!(
                            job_id = %job_id_clone,
                            model_id = mid,
                            file = %filename_clone,
                            kind = db_kind,
                            error = %e,
                            "model_files kind UPDATE failed"
                        );
                    }
                }
                if let Err(e) = mgr.update_verification(
                    mid,
                    &filename_clone,
                    outcome.ok,
                    outcome.err.as_deref(),
                ) {
                    tracing::warn!(
                        job_id = %job_id_clone,
                        model_id = mid,
                        file = %filename_clone,
                        error = %e,
                        "update_verification failed"
                    );
                }
            }
            (None, _) => {
                tracing::warn!(
                    job_id = %job_id_clone,
                    "db_dir not configured — model_files row skipped"
                );
            }
            (Some(_), None) => {
                tracing::info!(
                    job_id = %job_id_clone,
                    repo = %repo_id_clone,
                    "non-primary shard — model_files row skipped (expected for non-primary shards)"
                );
            }
        }
    }

    // Release the in-flight lock after setup and verification complete
    // to prevent concurrent retries from starting mid-processing.
    in_flight_clone.lock().await.remove(&dest_path);

    // Update DB queue item with final status
    let final_status = if outcome.passed {
        "completed"
    } else {
        "failed"
    };
    let error_msg = if !outcome.passed {
        outcome.err.as_deref()
    } else {
        None
    };

    if let Some(ref svc) = state_clone.pull_queue() {
        let _ = svc.update_status(
            &job_id_clone,
            final_status,
            total_size as i64,
            Some(total_size as i64),
            error_msg,
            duration_ms,
        );
    }

    // Update in-memory PullJob with duration
    {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.duration_ms = duration_ms;
        }
    }
}
