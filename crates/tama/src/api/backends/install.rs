use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::types::*;
use crate::api::error::error_response;
use crate::api::helpers::open_backend_manager;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/install
pub async fn install_backend(
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
        if version.contains('/') || version.contains('\\') || version.contains("..") {
            return error_response(
                StatusCode::BAD_REQUEST,
                "version must be a single path segment (no slashes or '..')",
                Some("ValidationError"),
            );
        }
    }

    // Validate gpu_type version fields: if present, must be non-empty and <= 32 chars
    match &req.gpu_type {
        GpuTypeDto::Cuda { version } | GpuTypeDto::Rocm { version } => {
            if version.is_empty() {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "gpu type version cannot be empty",
                    Some("ValidationError"),
                );
            }
            if version.len() > 32 {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "gpu type version must be at most 32 characters",
                    Some("ValidationError"),
                );
            }
        }
        _ => {}
    }

    let jobs = match &state.web_jobs {
        Some(j) => j,
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
        "llama_cpp" => tama_core::backends::BackendType::LlamaCpp,
        "ik_llama" => tama_core::backends::BackendType::IkLlama,
        "tts_kokoro" => tama_core::backends::BackendType::TtsKokoro,
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
    let is_tts = matches!(backend_type, tama_core::backends::BackendType::TtsKokoro);

    if is_tts {
        // For TTS backends: submit job first, then spawn the install task.
        let job = match jobs
            .submit(
                tama_core::web_types::JobKind::Install,
                Some(backend_type.clone()),
            )
            .await
        {
            Ok(j) => j,
            Err(tama_core::web_types::JobError::AlreadyRunning(existing_id)) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "another backend job is already running",
                        "job_id": existing_id
                    })),
                )
                    .into_response();
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
            tama_core::backends::BackendType::TtsKokoro => "tts_kokoro",
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Unsupported backend type for TTS install",
                    None,
                )
            }
        };

        // Capture config_dir for the background task
        let config_dir = match tama_core::config::Config::config_dir() {
            Ok(d) => d,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("config_dir: {}", e),
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

            // Open manager and run TTS installer
            let config_dir_clone = config_dir.clone();
            let reg_result = tokio::task::spawn_blocking(move || {
                tama_core::backends::BackendManager::open(&config_dir_clone)
            })
            .await
            .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
            .and_then(|r| r);

            let mgr = match reg_result {
                Ok(r) => r,
                Err(e) => {
                    // Log the error and finish the job as failed
                    jobs_clone
                        .append_log(&job_clone, format!("Error: {}", e))
                        .await;
                    let _ = jobs_clone
                        .finish(
                            &job_clone,
                            tama_core::web_types::JobStatus::Failed,
                            Some("Failed to open manager".to_string()),
                        )
                        .await;
                    return;
                }
            };

            let progress = Box::new(adapter);
            match bt {
                tama_core::backends::BackendType::TtsKokoro => {
                    match tama_core::backends::install_tts_kokoro(mgr, progress).await {
                        Ok(()) => {}
                        Err(e) => {
                            jobs_clone
                                .append_log(&job_clone, format!("Error: {}", e))
                                .await;
                            let _ = jobs_clone
                                .finish(
                                    &job_clone,
                                    tama_core::web_types::JobStatus::Failed,
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
                            tama_core::web_types::JobStatus::Failed,
                            Some("Unsupported backend type for TTS install".to_string()),
                        )
                        .await;
                    return;
                }
            }

            let _ = jobs_clone
                .finish(&job_clone, tama_core::web_types::JobStatus::Succeeded, None)
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

    // Convert GPU type
    let gpu_type = match &req.gpu_type {
        GpuTypeDto::Cuda { version } => Some(tama_core::gpu::GpuType::Cuda {
            version: version.clone(),
        }),
        GpuTypeDto::Vulkan => Some(tama_core::gpu::GpuType::Vulkan),
        GpuTypeDto::Metal => Some(tama_core::gpu::GpuType::Metal),
        GpuTypeDto::Rocm { version } => Some(tama_core::gpu::GpuType::RocM {
            version: version.clone(),
        }),
        GpuTypeDto::CpuOnly => Some(tama_core::gpu::GpuType::CpuOnly),
        GpuTypeDto::Custom => Some(tama_core::gpu::GpuType::Custom),
    };

    // Compute effective build_from_source
    let is_linux = std::env::consts::OS == "linux";
    let is_cuda = matches!(&req.gpu_type, GpuTypeDto::Cuda { .. });
    let is_ik_llama = matches!(backend_type, tama_core::backends::BackendType::IkLlama);

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
        let cache = match &state.web_capabilities {
            Some(c) => c,
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
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("Capability detection failed: {}", e) })),
                )
                    .into_response();
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
            tama_core::web_types::JobKind::Install,
            Some(backend_type.clone()),
        )
        .await
    {
        Ok(j) => j,
        Err(tama_core::web_types::JobError::AlreadyRunning(existing_id)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "another backend job is already running",
                    "job_id": existing_id
                })),
            )
                .into_response();
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
        tama_core::backends::BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
        tama_core::backends::BackendType::IkLlama => {
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
        tama_core::backends::BackendSource::SourceCode {
            version: version.clone(),
            git_url: git_url.to_string(),
            commit: None,
        }
    } else {
        tama_core::backends::BackendSource::Prebuilt {
            version: version.clone(),
        }
    };

    // Compute the versioned target directory
    let gpu_variant = gpu_type
        .as_ref()
        .map(|g| g.variant_folder().to_string())
        .unwrap_or_else(|| "cpu".to_string());

    let target_dir = match tama_core::backends::backends_dir() {
        Ok(d) => {
            if !matches!(
                backend_type,
                tama_core::backends::BackendType::LlamaCpp
                    | tama_core::backends::BackendType::IkLlama
            ) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Unsupported backend type: {}", backend_type),
                    Some("ValidationError"),
                );
            }
            tama_core::backends::get_backend_install_path(&d, &backend_type, &gpu_variant, &version)
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get backends dir: {}", e),
                None,
            )
        }
    };

    // Capture values needed for DB registration before gpu_type/source are moved
    let reg_backend_type = backend_type.clone();
    let reg_version = version.clone();
    let reg_gpu_type = gpu_type.clone();
    let reg_gpu_variant = gpu_variant.clone();
    let reg_source = source.clone();
    let reg_backend_name = match backend_type {
        tama_core::backends::BackendType::LlamaCpp => "llama_cpp",
        tama_core::backends::BackendType::IkLlama => "ik_llama",
        _ => "custom",
    }
    .to_string();
    // config_dir obtained inside the spawn closure via Config::config_dir()

    let options = tama_core::backends::InstallOptions {
        backend_type: backend_type.clone(),
        source,
        target_dir,
        gpu_type,
        gpu_variant,
        allow_overwrite: req.force,
    };

    // Spawn the install task
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    tokio::spawn(async move {
        let adapter = Arc::new(JobAdapter {
            jobs: jobs_clone.clone(),
            job: job_clone.clone(),
        });

        let result = match tama_core::backends::installer::install_backend_with_progress(
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
                if let Ok(config_dir) = tama_core::config::Config::config_dir() {
                    let installed_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let reg_result = tokio::task::spawn_blocking(move || {
                        let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
                        mgr.add_installation(&tama_core::backends::BackendInfo {
                            name: reg_backend_name,
                            backend_type: reg_backend_type,
                            version: reg_version,
                            path: binary_path,
                            installed_at,
                            gpu_type: reg_gpu_type,
                            gpu_variant: reg_gpu_variant,
                            source: Some(reg_source),
                        })
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
                    .and_then(|r| r);
                    if let Err(e) = reg_result {
                        tracing::warn!("Failed to register backend in DB: {}", e);
                    }
                }
                let _ = jobs_clone
                    .finish(&job_clone, tama_core::web_types::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                // Emit the error as a log line so it appears in the build log panel.
                jobs_clone
                    .append_log(&job_clone, format!("Error: {}", e))
                    .await;
                let _ = jobs_clone
                    .finish(&job_clone, tama_core::web_types::JobStatus::Failed, Some(e))
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
pub async fn remove_backend(
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<RemoveQuery>,
) -> impl IntoResponse {
    let jobs = match &state.web_jobs {
        Some(j) => j,
        None => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "job manager not configured",
                None,
            )
        }
    };

    let config_dir = state.db_dir.clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    // Open manager and get backend
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Invalid backend name: path separators or traversal sequences not allowed"
            })),
        )
            .into_response();
    }

    let gpu_variant = query.gpu_variant;

    let mgr = match open_backend_manager(&state).await {
        Ok(mgr) => mgr,
        Err(e) => return e,
    };

    // If gpu_variant is provided, only remove that variant (all its versions);
    // otherwise remove all variants.
    let backends_to_remove: Vec<tama_core::backends::BackendInfo> =
        if let Some(variant) = &gpu_variant {
            // Specific variant requested — get ALL versions of that variant
            match mgr.list_versions(&name, Some(variant.as_str())) {
                Ok(Some(versions)) if !versions.is_empty() => versions,
                Ok(Some(_)) | Ok(None) => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        format!("Backend '{}' not found", name),
                        Some("NotFoundError"),
                    )
                }
                Err(e) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to get backend: {}", e),
                        None,
                    )
                }
            }
        } else {
            // No variant specified — iterate ALL variants
            match mgr.list_versions(&name, None) {
                Ok(Some(versions)) => versions,
                Ok(None) => {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        format!("Backend '{}' not found", name),
                        Some("NotFoundError"),
                    )
                }
                Err(e) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to get backend: {}", e),
                        None,
                    )
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
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "a job is currently running for this backend"
                })),
            )
                .into_response();
        }
    }

    // Remove files for each variant
    for info in &backends_to_remove {
        if let Err(e) = tama_core::backends::safe_remove_installation(info) {
            let err_msg = e.to_string();
            if err_msg.contains("outside the managed backends directory") {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "path is outside the managed backends directory; remove manually"
                    })),
                )
                    .into_response();
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to remove files: {}", e),
                None,
            );
        }
    }

    // Remove from DB (Some = remove specific variant, None = remove all variants)
    let variant_to_remove = gpu_variant.as_deref();
    if let Err(e) = mgr.delete_all_versions(&name, variant_to_remove) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove: {}", e),
            None,
        );
    }

    // Clean up update_check records — use LIKE pattern to match all variants
    // (e.g., "llama_cpp:cpu", "llama_cpp:cuda") plus legacy format.
    if let Ok(open) = tama_core::db::open(&config_dir) {
        let escaped_name = name
            .replace('\\', "\\\\")
            .replace('_', "\\_")
            .replace('%', "\\%");
        let pattern = format!("{}:%", escaped_name);
        let _ = tama_core::db::queries::delete_update_checks_by_pattern(
            &open.conn, "backend", &pattern,
        );
        // Also delete legacy format (no variant separator)
        let _ = tama_core::db::queries::delete_update_check(&open.conn, "backend", &name);
    }

    Json(DeleteResponse { removed: true }).into_response()
}
