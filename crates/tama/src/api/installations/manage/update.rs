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
use crate::api::installations::tamad_job;
use crate::api::installations::types::InstallResponse;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/update
///
/// Updates an installed backend to the latest released version. The update
/// is executed on the backend's tamad (plan-191 Task 7 / ADR-0010); the
/// proxy relays job events into the JobManager (unchanged UX) and applies
/// the DB version change when the tamad job succeeds.
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

    let mgr = match open_backend_manager(&state).await {
        Ok(mgr) => mgr,
        Err(e) => return e,
    };

    // Determine gpu_variant: use explicit value or auto-infer from manager
    let lookup_variant = match query.gpu_variant {
        Some(v) => v,
        None => {
            // Auto-infer: find unique variant for this backend
            let versions = match mgr.list_versions(&name, None).await {
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

    let backend_info = match mgr.get_active(&name, &lookup_variant).await {
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

    // Resolve the update source: preserve the installation's recorded
    // source type (prebuilt ↔ source code) for the new version.
    let git_url = match backend_info.source.clone() {
        Some(tama_core::installations::InstallationSource::SourceCode { git_url, .. }) => git_url,
        Some(_) | None => {
            // Prebuilt (or none recorded): download the latest prebuilt.
            String::new()
        }
    };
    let new_version_str = latest_version.clone();
    let new_source = if git_url.is_empty() {
        tama_core::installations::InstallationSource::Prebuilt {
            version: new_version_str.clone(),
        }
    } else {
        tama_core::installations::InstallationSource::SourceCode {
            version: new_version_str.clone(),
            git_url: git_url.clone(),
            commit: None,
        }
    };

    // Spawn the update as a tamad-hosted job bridged to the JobManager:
    // dispatch UpdateProvider to the backend's tamad, relay job events
    // into the job log, and apply the DB version change when it succeeds.
    let dispatch = tamad_job::UpdateDispatch {
        backend_type: backend_type.clone(),
        name: name.clone(),
        gpu_variant: backend_info.gpu_variant.clone(),
        version: new_version_str,
        git_url,
        source: new_source,
    };
    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    tokio::spawn({
        let state = state.clone();
        let checker = web_state.update_checker.clone();
        async move {
            jobs_clone
                .append_log(
                    &job_clone,
                    "Dispatching update to backend host…".to_string(),
                )
                .await;
            tamad_job::execute_update(&state, &jobs_clone, &job_clone, &dispatch, checker).await;
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
