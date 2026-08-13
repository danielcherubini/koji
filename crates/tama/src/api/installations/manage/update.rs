use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::UpdateQuery;
use crate::api::error::{error_body, error_response};
use crate::api::helpers::open_backend_manager;
use crate::api::installations::types::{InstallResponse, JobAdapter};
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/update
pub async fn update_installation(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<UpdateQuery>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if let Err(resp) = crate::api::installations::reject_traversal(&name, "backend name") {
        return resp;
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

    let config_dir = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    let config_dir_clone = config_dir.clone();

    let mgr = match open_backend_manager(&state).await {
        Ok(mgr) => mgr,
        Err(e) => return e,
    };

    // Determine gpu_variant: use explicit value or auto-infer from manager
    let lookup_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            // Auto-infer: find unique variant for this backend
            let versions = match mgr.list_versions(&name, None) {
                Ok(Some(v)) => v,
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
                        format!("Failed to query backend: {}", e),
                        None,
                    )
                }
            };
            let mut variants: Vec<String> =
                versions.iter().map(|v| v.gpu_variant.clone()).collect();
            variants.sort();
            variants.dedup();
            match variants.len() {
                1 => variants.into_iter().next().unwrap(),
                _ => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        format!(
                            "Backend '{}' has multiple variants. Please specify gpu_variant. Available: {}",
                            name,
                            variants.join(", ")
                        ),
                        Some("ValidationError"),
                    )
                }
            }
        }
    };

    let backend_info = match mgr.get_active(&name, &lookup_variant) {
        Ok(Some(info)) => info,
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
    };

    let backend_type = backend_info.backend_type.clone();

    // Docker backends cannot be updated via the binary update flow
    if backend_info.docker_config.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "update not supported for docker backends",
            Some("ValidationError"),
        );
    }

    // Check latest version
    let latest_version =
        match tama_core::installations::check_latest_version(&backend_type, None, None).await {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to check latest version: {}", e),
                    None,
                )
            }
        };

    // Submit job
    let job = match jobs
        .submit(
            crate::web_types::JobKind::Update,
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

    // Use versioned path structure for the update target
    let target_dir = match tama_core::installations::backends_dir() {
        Ok(d) => tama_core::installations::get_backend_install_path(
            &d,
            &backend_type,
            &backend_info.gpu_variant,
            &latest_version,
        ),
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get backends dir: {}", e),
                None,
            )
        }
    };

    // Build update options — always use latest_version, not the old version from the registry.
    let source = match backend_info.source.clone() {
        Some(src) => match src {
            tama_core::installations::InstallationSource::Prebuilt { .. } => {
                tama_core::installations::InstallationSource::Prebuilt {
                    version: latest_version.clone(),
                }
            }
            tama_core::installations::InstallationSource::SourceCode { git_url, .. } => {
                tama_core::installations::InstallationSource::SourceCode {
                    version: latest_version.clone(),
                    git_url,
                    commit: None,
                }
            }
        },
        None => {
            // Fallback: use source code if no source recorded
            tama_core::installations::InstallationSource::SourceCode {
                version: latest_version.clone(),
                git_url: match &backend_type {
                    tama_core::installations::InstallationType::LlamaCpp => {
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                    tama_core::installations::InstallationType::IkLlama => {
                        "https://github.com/ikawrakow/ik_llama.cpp.git"
                    }
                    other => {
                        tracing::warn!(
                            "No source URL configured for backend type {:?}, using llama.cpp fallback",
                            other
                        );
                        "https://github.com/ggml-org/llama.cpp.git"
                    }
                }
                .to_string(),
                commit: None,
            }
        }
    };

    let options = tama_core::installations::InstallOptions {
        backend_type: backend_type.clone(),
        source,
        target_dir,
        gpu_variant: backend_info.gpu_variant.clone(),
        allow_overwrite: true,
    };

    // Clone variables needed for the post-update check
    let checker = web_state.update_checker.clone();
    let backend_type_clone = backend_type.clone();

    // Spawn the update task
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    let name_clone = name.clone();
    let latest_version_clone = latest_version.clone();
    let gpu_variant_clone = backend_info.gpu_variant.clone();
    tokio::spawn(async move {
        let adapter = Arc::new(JobAdapter {
            jobs: jobs_clone.clone(),
            job: job_clone.clone(),
        });

        let client = reqwest::Client::builder()
            .user_agent("tama-backend-manager")
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        let result = match tama_core::installations::update_installation_with_progress(
            mgr,
            &client,
            &name_clone,
            &gpu_variant_clone,
            options,
            latest_version_clone,
            Some(adapter),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        };

        match result {
            Ok(_) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::web_types::JobStatus::Succeeded, None)
                    .await;
                // Refresh the update check record so the Updates Center reflects the new version
                let _ = checker
                    .check_backend(
                        &config_dir_clone,
                        &name_clone,
                        &backend_type_clone,
                        &gpu_variant_clone,
                    )
                    .await;
            }
            Err(e) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::web_types::JobStatus::Failed, Some(e))
                    .await;
            }
        }
    });

    Json(InstallResponse {
        job_id: job.id.to_string(),
        kind: "update".to_string(),
        backend_type: format!("{}", backend_type),
        notices: vec![],
    })
    .into_response()
}
