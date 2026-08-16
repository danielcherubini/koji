use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::types::*;
use crate::api::error::{error_body, error_response, error_response_simple};
use crate::api::helpers::open_backend_manager;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/install
pub async fn install_installation(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<InstallRequest>,
) -> impl IntoResponse {
    // Validate backend_type: non-empty and <= 64 chars
    if req.backend_type.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "backend_type cannot be empty",
            Some("ValidationError"),
        );
    }
    if req.backend_type.len() > 64 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "backend_type must be at most 64 characters",
            Some("ValidationError"),
        );
    }

    // Validate version: if provided, must be non-empty, <= 128 chars, and a single path segment
    if let Some(ref version) = req.version {
        if version.is_empty() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "version cannot be empty",
                Some("ValidationError"),
            );
        }
        if version.len() > 128 {
            return error_response(
                StatusCode::BAD_REQUEST,
                "version must be at most 128 characters",
                Some("ValidationError"),
            );
        }
        // Reject path traversal and multi-segment paths
        if let Err(resp) = crate::api::installations::reject_traversal(version, "version") {
            return resp;
        }
    }

    let jobs = match web_state.jobs.as_ref() {
        Some(j) => j.clone(),
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job manager not configured",
                None,
            )
        }
    };

    // Parse backend type
    let backend_type = match req.backend_type.as_str() {
        "llama_cpp" => tama_core::installations::InstallationType::LlamaCpp,
        "ik_llama" => tama_core::installations::InstallationType::IkLlama,
        "tts_kokoro" => tama_core::installations::InstallationType::TtsKokoro,
        "docker" => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "docker backends use POST /tama/v1/backends, not /install",
                Some("ValidationError"),
            )
        }
        "custom" => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Custom backends cannot be installed via API",
                Some("ValidationError"),
            )
        }
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Unknown backend type: {}", req.backend_type),
                Some("ValidationError"),
            )
        }
    };

    // TTS backends use a dedicated installer (downloads model files from HuggingFace).
    // They skip the normal prebuilt/source install flow entirely.
    let is_tts = matches!(
        backend_type,
        tama_core::installations::InstallationType::TtsKokoro
    );

    if is_tts {
        // For TTS backends: submit job first, then spawn the install task.
        let job = match jobs
            .submit(
                crate::web_types::JobKind::Install,
                Some(backend_type.clone()),
            )
            .await
        {
            Ok(j) => j,
            Err(crate::web_types::JobError::AlreadyRunning(existing_id)) => {
                let mut body = error_body(
                    "another backend job is already running",
                    Some("ConflictError"),
                );
                body["job_id"] = serde_json::json!(existing_id);
                return (StatusCode::CONFLICT, Json(body)).into_response();
            }
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create job",
                    None,
                )
            }
        };

        let backend_name = match &backend_type {
            tama_core::installations::InstallationType::TtsKokoro => "tts_kokoro",
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Unsupported backend type for TTS install",
                    None,
                )
            }
        };

        // Capture the Postgres pool for the background task
        let pool = match state.db_pool() {
            Some(p) => p,
            None => {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Database not configured",
                    None,
                )
            }
        };

        let jobs_clone = jobs.clone();
        let job_clone = job.clone();
        let bt = backend_type.clone();
        tokio::spawn(async move {
            let adapter = JobAdapter {
                jobs: jobs_clone.clone(),
                job: job_clone.clone(),
            };

            // Build the manager from the shared Postgres pool and run the TTS installer.
            let mgr = tama_core::installations::InstallationManager::new(pool);

            let progress = Box::new(adapter);
            match bt {
                tama_core::installations::InstallationType::TtsKokoro => {
                    match tama_core::installations::install_tts_kokoro(mgr, progress).await {
                        Ok(()) => {}
                        Err(e) => {
                            jobs_clone
                                .append_log(&job_clone, format!("Error: {}", e))
                                .await;
                            let _ = jobs_clone
                                .finish(
                                    &job_clone,
                                    crate::web_types::JobStatus::Failed,
                                    Some(e.to_string()),
                                )
                                .await;
                            return;
                        }
                    }
                }
                _ => {
                    jobs_clone
                        .append_log(
                            &job_clone,
                            "Error: Unsupported backend type for TTS install".to_string(),
                        )
                        .await;
                    let _ = jobs_clone
                        .finish(
                            &job_clone,
                            crate::web_types::JobStatus::Failed,
                            Some("Unsupported backend type for TTS install".to_string()),
                        )
                        .await;
                    return;
                }
            }

            let _ = jobs_clone
                .finish(&job_clone, crate::web_types::JobStatus::Succeeded, None)
                .await;
        });

        return Json(InstallResponse {
            job_id: job.id.to_string(),
            kind: "install".to_string(),
            backend_type: req.backend_type,
            notices: vec![format!(
                "Downloading {} model files from HuggingFace...",
                backend_name
            )],
        })
        .into_response();
    }

    // Compute effective build_from_source
    let is_linux = std::env::consts::OS == "linux";
    let is_cuda = matches!(req.gpu_variant, tama_core::gpu::GpuVariant::Cuda { .. });
    let is_ik_llama = matches!(
        backend_type,
        tama_core::installations::InstallationType::IkLlama
    );

    let mut notices: Vec<String> = Vec::new();
    let effective_build_from_source = if is_ik_llama {
        notices.push("ik_llama always builds from source".to_string());
        true
    } else if is_linux && is_cuda {
        notices.push("no prebuilt CUDA binary for Linux; building from source".to_string());
        true
    } else {
        req.build_from_source
    };

    // Check prerequisites if source build
    if effective_build_from_source {
        let cache = match web_state.capabilities.as_ref() {
            Some(c) => c.clone(),
            None => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "capabilities cache not configured",
                    None,
                )
            }
        };

        let caps = match cache
            .get_or_compute(
                tama_core::gpu::detect_build_prerequisites,
                tama_core::gpu::detect_cuda_version,
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Capability detection failed: {}", e),
                    None,
                )
            }
        };

        if !caps.git_available {
            return error_response(
                StatusCode::BAD_REQUEST,
                "missing build prerequisite: git",
                Some("ValidationError"),
            );
        }
        if !caps.cmake_available {
            return error_response(
                StatusCode::BAD_REQUEST,
                "missing build prerequisite: cmake",
                Some("ValidationError"),
            );
        }
        if !caps.compiler_available {
            return error_response(
                StatusCode::BAD_REQUEST,
                "missing build prerequisite: compiler",
                Some("ValidationError"),
            );
        }
    }

    // Submit job
    let job = match jobs
        .submit(
            crate::web_types::JobKind::Install,
            Some(backend_type.clone()),
        )
        .await
    {
        Ok(j) => j,
        Err(crate::web_types::JobError::AlreadyRunning(existing_id)) => {
            let mut body = error_body(
                "another backend job is already running",
                Some("ConflictError"),
            );
            body["job_id"] = serde_json::json!(existing_id);
            return (StatusCode::CONFLICT, Json(body)).into_response();
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create job",
                None,
            )
        }
    };

    // Build install options
    let version = req.version.unwrap_or_else(|| "latest".to_string());
    let git_url = match backend_type {
        tama_core::installations::InstallationType::LlamaCpp => {
            "https://github.com/ggml-org/llama.cpp.git"
        }
        tama_core::installations::InstallationType::IkLlama => {
            "https://github.com/ikawrakow/ik_llama.cpp.git"
        }
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Unsupported backend type: {}", backend_type),
                Some("ValidationError"),
            )
        }
    };

    let source = if effective_build_from_source {
        tama_core::installations::InstallationSource::SourceCode {
            version: version.clone(),
            git_url: git_url.to_string(),
            commit: None,
        }
    } else {
        tama_core::installations::InstallationSource::Prebuilt {
            version: version.clone(),
        }
    };

    // Compute the versioned target directory
    let gpu_variant = req.gpu_variant.to_string();

    let target_dir = match tama_core::installations::backends_dir() {
        Ok(d) => {
            if !matches!(
                backend_type,
                tama_core::installations::InstallationType::LlamaCpp
                    | tama_core::installations::InstallationType::IkLlama
            ) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Unsupported backend type: {}", backend_type),
                    Some("ValidationError"),
                );
            }
            tama_core::installations::get_backend_install_path(
                &d,
                &backend_type,
                &gpu_variant,
                &version,
            )
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get backends dir: {}", e),
                None,
            )
        }
    };

    // Capture values needed for DB registration before source is moved
    let reg_backend_type = backend_type.clone();
    let reg_version = version.clone();
    let reg_gpu_variant = gpu_variant.clone();
    let reg_source = source.clone();
    let reg_backend_name = match backend_type {
        tama_core::installations::InstallationType::LlamaCpp => "llama_cpp",
        tama_core::installations::InstallationType::IkLlama => "ik_llama",
        _ => "custom",
    }
    .to_string();
    // config_dir obtained inside the spawn closure via Config::config_dir()

    let options = tama_core::installations::InstallOptions {
        backend_type: backend_type.clone(),
        source,
        target_dir,
        gpu_variant,
        allow_overwrite: req.force,
    };

    // Spawn the install task
    let db_pool = state.db_pool();
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    tokio::spawn(async move {
        let adapter = Arc::new(JobAdapter {
            jobs: jobs_clone.clone(),
            job: job_clone.clone(),
        });

        let result = match tama_core::installations::installer::install_installation_with_progress(
            options,
            Some(adapter),
            None, // No registry client available in background job
        )
        .await
        {
            Ok(binary_path) => Ok(binary_path),
            Err(e) => Err(e.to_string()),
        };

        match result {
            Ok(binary_path) => {
                // Register the installation in the DB so `resolve_backend_path` can find it.
                if let Some(pool) = db_pool {
                    let installed_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let mgr = tama_core::installations::InstallationManager::new(pool);
                    let reg_result = mgr
                        .add_installation(&tama_core::installations::InstallationInfo {
                            name: reg_backend_name,
                            backend_type: reg_backend_type,
                            version: reg_version,
                            path: binary_path,
                            installed_at,
                            gpu_variant: reg_gpu_variant,
                            source: Some(reg_source),
                            docker_config: None,
                        })
                        .await;
                    if let Err(e) = reg_result {
                        tracing::warn!("Failed to register backend in DB: {}", e);
                    }
                }
                let _ = jobs_clone
                    .finish(&job_clone, crate::web_types::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                // Emit the error as a log line so it appears in the build log panel.
                jobs_clone
                    .append_log(&job_clone, format!("Error: {}", e))
                    .await;
                let _ = jobs_clone
                    .finish(&job_clone, crate::web_types::JobStatus::Failed, Some(e))
                    .await;
            }
        }
    });

    Json(InstallResponse {
        job_id: job.id.to_string(),
        kind: "install".to_string(),
        backend_type: req.backend_type,
        notices,
    })
    .into_response()
}

/// Query params for DELETE /tama/v1/backends/:name
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// DELETE /tama/v1/backends/:name
pub async fn remove_installation(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<RemoveQuery>,
) -> impl IntoResponse {
    let jobs = match web_state.jobs.as_ref() {
        Some(j) => j.clone(),
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job manager not configured",
                None,
            )
        }
    };

    // Open manager and get backend
    if let Err(resp) = crate::api::installations::reject_traversal(&name, "backend name") {
        return resp;
    }

    let gpu_variant = query.gpu_variant;

    let mgr = match open_backend_manager(&state).await {
        Ok(mgr) => mgr,
        Err(e) => return e,
    };

    // If gpu_variant is provided, only remove that variant (all its versions);
    // otherwise remove all variants.
    let backends_to_remove: Vec<tama_core::installations::InstallationInfo> =
        if let Some(variant) = &gpu_variant {
            // Specific variant requested — get ALL versions of that variant
            match mgr.list_versions(&name, Some(variant.as_str())).await {
                Ok(Some(versions)) if !versions.is_empty() => versions,
                Ok(Some(_)) | Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(error_body(
                            format!("Backend '{}' not found", name),
                            Some("NotFoundError"),
                        )),
                    )
                        .into_response()
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(error_body(format!("Failed to get backend: {}", e), None)),
                    )
                        .into_response()
                }
            }
        } else {
            // No variant specified — iterate ALL variants
            match mgr.list_versions(&name, None).await {
                Ok(Some(versions)) => versions,
                Ok(None) => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(error_body(
                            format!("Backend '{}' not found", name),
                            Some("NotFoundError"),
                        )),
                    )
                        .into_response()
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(error_body(format!("Failed to get backend: {}", e), None)),
                    )
                        .into_response()
                }
            }
        };

    // Check if a job is running for this backend
    if let Some(active_job) = jobs.active().await {
        let active_type = active_job
            .backend_type
            .as_ref()
            .map(|b| b.to_string())
            .unwrap_or_default();
        if active_type == backends_to_remove[0].backend_type.to_string() {
            return error_response(
                StatusCode::CONFLICT,
                "a job is currently running for this backend",
                Some("ConflictError"),
            );
        }
    }

    // Block 2: remove files on the blocking pool, then delete the DB rows.
    let backends_for_block2 = backends_to_remove.clone();
    #[allow(clippy::result_large_err)]
    match tokio::task::spawn_blocking(move || -> Result<(), axum::response::Response> {
        // Remove files for each variant
        for info in &backends_for_block2 {
            if let Err(e) = tama_core::installations::safe_remove_installation(info) {
                let err_msg = e.to_string();
                if err_msg.contains("outside the managed backends directory") {
                    return Err(error_response(
                        StatusCode::CONFLICT,
                        "path is outside the managed backends directory; remove manually",
                        Some("ConflictError"),
                    ));
                }
                return Err(error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to remove files: {}", e),
                    None,
                ));
            }
        }

        Ok(())
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(resp)) => return resp,
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn error: {}", e),
            )
        }
    }

    // Remove from DB (Some = remove specific variant, None = remove all variants)
    if let Err(e) = mgr.delete_all_versions(&name, gpu_variant.as_deref()).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove: {}", e),
            None,
        );
    }

    // Clean up update_check records — use LIKE pattern to match all variants
    // (e.g., "llama_cpp:cpu", "llama_cpp:cuda") plus legacy format.
    // (Postgres, plan-190 Task 4; best-effort.)
    if let Some(pool) = state.db_pool() {
        let _ = tama_core::db::queries::delete_update_checks_for_backend(&pool, &name).await;
    }

    Json(DeleteResponse { removed: true }).into_response()
}
