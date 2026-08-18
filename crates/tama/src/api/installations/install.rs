use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use super::types::*;
use crate::api::error::{error_body, error_response};
use crate::api::helpers::open_backend_manager;
use crate::api::installations::tamad_job;
use crate::web_types::WebState;
use tama_core::installations::{InstallationSource, InstallationType};
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/install
///
/// Installations are executed on the *tamad* of the provider that resolves
/// the backend (plan-191 Task 7 / ADR-0010). The proxy validates the
/// request, submits a JobManager job (same UX as before), dispatches
/// `InstallProvider` to the tamad, and persists the installation row when
/// the tamad job succeeds (proxy = single DB writer).
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
        "llama_cpp" => InstallationType::LlamaCpp,
        "ik_llama" => InstallationType::IkLlama,
        "tts_kokoro" => InstallationType::TtsKokoro,
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

    // Backend name (the installation registry key).
    let backend_name = match &backend_type {
        InstallationType::LlamaCpp => "llama_cpp",
        InstallationType::IkLlama => "ik_llama",
        InstallationType::TtsKokoro => "tts_kokoro",
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                format!("Unsupported backend type: {}", backend_type),
                Some("ValidationError"),
            )
        }
    }
    .to_string();

    // TTS backends use a dedicated source install (Kokoro-FastAPI at a
    // pinned tag). Dispatched to the tamad like any other install; the
    // version is always the pinned tag (resolved on the host).
    if matches!(backend_type, InstallationType::TtsKokoro) {
        let job = match submit_install_job(&jobs, &backend_type).await {
            Ok(j) => j,
            Err(resp) => return resp,
        };

        let dispatch = tamad_job::InstallDispatch {
            backend_type,
            name: backend_name,
            version: String::new(), // host installs the pinned KOKORO_FASTAPI_TAG
            gpu_variant: "cpu".to_string(),
            git_url: String::new(),
            force: req.force,
            source: InstallationSource::SourceCode {
                version: tama_core::installations::tts_kokoro::paths::KOKORO_FASTAPI_TAG
                    .to_string(),
                git_url: tama_core::installations::tts_kokoro::paths::KOKORO_FASTAPI_URL
                    .to_string(),
                commit: None,
            },
        };
        spawn_tamad_job(
            &state,
            &jobs,
            &job,
            "Dispatching TTS install to backend host…".to_string(),
            dispatch,
        );

        return Json(InstallResponse {
            job_id: job.id.to_string(),
            kind: "install".to_string(),
            backend_type: req.backend_type,
            notices: vec!["TTS backend installs from source at a pinned tag".to_string()],
        })
        .into_response();
    }

    // Compute effective build_from_source
    let is_linux = std::env::consts::OS == "linux";
    let is_cuda = matches!(req.gpu_variant, tama_core::gpu::GpuVariant::Cuda { .. });
    let is_ik_llama = matches!(backend_type, InstallationType::IkLlama);

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

    // Quick-fail on build prerequisites for source builds. This probe runs
    // on the PROXY host, so the 400 reflects the *reporting* host's
    // toolchain — accurate for single-host deployments (tamad on the same
    // box); on multi-host topologies the build runs on the provider's
    // tamad, which is authoritative and re-probes its own host at build
    // time (tamad `install_from_source` — a missing tool there fails the
    // job with an actionable error instead).
    // (plan-191 Task 9: the local CUDA-version probe was removed — builds
    // execute on the tamad host.)
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
            .get_or_compute(tama_core::gpu::detect_build_prerequisites)
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
    let job = match submit_install_job(&jobs, &backend_type).await {
        Ok(j) => j,
        Err(resp) => return resp,
    };

    // Resolve the install source (for the tamad dispatch + the DB row that
    // is written when the tamad job succeeds).
    let version = req.version.unwrap_or_else(|| "latest".to_string());
    // Source-code git URL (empty → the tamad downloads a prebuilt binary).
    let git_url = if effective_build_from_source {
        match &backend_type {
            InstallationType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
            InstallationType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
            other => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    format!("Unsupported backend type: {}", other),
                    Some("ValidationError"),
                )
            }
        }
        .to_string()
    } else {
        String::new()
    };
    let source = if git_url.is_empty() {
        InstallationSource::Prebuilt {
            version: version.clone(),
        }
    } else {
        InstallationSource::SourceCode {
            version: version.clone(),
            git_url: git_url.clone(),
            commit: None,
        }
    };

    let dispatch = tamad_job::InstallDispatch {
        backend_type,
        name: backend_name,
        version,
        gpu_variant: req.gpu_variant.to_string(),
        git_url,
        force: req.force,
        source,
    };
    spawn_tamad_job(
        &state,
        &jobs,
        &job,
        "Dispatching install to backend host…".to_string(),
        dispatch,
    );

    Json(InstallResponse {
        job_id: job.id.to_string(),
        kind: "install".to_string(),
        backend_type: req.backend_type,
        notices,
    })
    .into_response()
}

/// Submit an install job, mapping JobErrors to HTTP responses.
async fn submit_install_job(
    jobs: &crate::web_types::JobManager,
    backend_type: &InstallationType,
) -> Result<Arc<crate::web_types::Job>, axum::response::Response> {
    match jobs
        .submit(
            crate::web_types::JobKind::Install,
            Some(backend_type.clone()),
        )
        .await
    {
        Ok(j) => Ok(j),
        Err(crate::web_types::JobError::AlreadyRunning(existing_id)) => {
            let mut body = error_body(
                "another backend job is already running",
                Some("ConflictError"),
            );
            body["job_id"] = serde_json::json!(existing_id);
            Err((StatusCode::CONFLICT, Json(body)).into_response())
        }
        Err(_) => Err(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create job",
            None,
        )),
    }
}

/// Spawn a tamad-executed backend job bridged to the JobManager.
fn spawn_tamad_job(
    state: &Arc<ProxyState>,
    jobs: &Arc<crate::web_types::JobManager>,
    job: &Arc<crate::web_types::Job>,
    dispatch_line: String,
    dispatch: tamad_job::InstallDispatch,
) {
    let state = state.clone();
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    tokio::spawn(async move {
        jobs_clone.append_log(&job_clone, dispatch_line).await;
        tamad_job::execute_install(&state, &jobs_clone, &job_clone, &dispatch).await;
    });
}

/// Query params for DELETE /tama/v1/backends/:name
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// DELETE /tama/v1/backends/:name
///
/// Removes a backend (or one of its variants) from the system:
/// 1. `RemoveProvider` on the backend's tamad — kills any running backend
///    processes and deletes the versioned install directories on the host.
/// 2. Cleans the proxy DB rows (proxy = single DB writer).
///
/// Tamad failure fails the request with 500; nothing is deleted from the
/// DB in that case (fail loud).
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

    // Step 1: remove on the backend host (kill processes + delete dirs).
    if let Err(e) = tamad_job::remove_on_tamad(
        &state,
        &backends_to_remove[0].backend_type,
        &name,
        gpu_variant.as_deref(),
        None,
    )
    .await
    {
        tracing::warn!(backend = %name, error = %e, "tamad removal failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to remove backend on host: {}", e),
            None,
        );
    }

    // Step 2: remove DB rows (Some = remove specific variant, None = all).
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
    let pool = state.db_pool();
    let _ = tama_core::db::queries::delete_update_checks_for_backend(&pool, &name).await;

    Json(DeleteResponse { removed: true }).into_response()
}
