use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::check::CheckSingleQuery;
use crate::api::error::{error_body, error_response};
use crate::api::helpers::shared_repository;
use crate::web_types::WebState;
use tama_core::backends::{
    check_latest_version, get_backend_install_path, BackendManager, BackendSource, BackendType,
    InstallOptions,
};
use tama_core::proxy::ProxyState;

/// Request body for POST /tama/v1/updates/apply/model/:id.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelUpdateRequest {
    pub quants: Vec<String>, // Quant keys like "Q4_K_M", "Q8_0"
}

/// Response body for POST /tama/v1/updates/apply/model/:id.
#[derive(Debug, Clone, Serialize)]
pub struct ModelUpdateResponse {
    pub job_ids: Vec<String>,
    pub total: usize,
}

/// POST /tama/v1/updates/apply/backend/:name - Trigger backend update
///
/// Use `?gpu_variant=xxx` to update a specific variant.
/// If not provided, updates the active variant (legacy behavior).
pub async fn apply_backend_update(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path(name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<CheckSingleQuery>,
) -> impl axum::response::IntoResponse {
    let config_dir = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    // Load backend info from DB — discover gpu_variant dynamically
    let requested_variant = query.gpu_variant.clone();
    let bt_result = tokio::task::spawn_blocking({
        let config_dir = config_dir.clone();
        let name = name.clone();
        let requested_variant = requested_variant.clone();
        move || -> anyhow::Result<(Option<BackendType>, Option<String>)> {
            let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
            let versions = mgr.list_versions(&name, None)?;

            // If a specific variant is requested, find that variant
            // Otherwise, fall back to the active variant (legacy behavior)
            let versions = match versions {
                Some(v) => v,
                None => return Ok((None, None)),
            };

            let record = if let Some(ref variant) = requested_variant {
                versions.iter().find(|v| v.gpu_variant == *variant)
            } else {
                // No is_active field on BackendInfo; use first as fallback
                versions.first()
            };

            Ok(record
                .map(|r| {
                    let bt = match r.backend_type {
                        BackendType::LlamaCpp => BackendType::LlamaCpp,
                        BackendType::IkLlama => BackendType::IkLlama,
                        _ => BackendType::Custom,
                    };
                    (Some(bt), Some(r.gpu_variant.clone()))
                })
                .unwrap_or((None, None)))
        }
    })
    .await;

    let (backend_type, current_version) = match bt_result {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    let (Some(backend_type), Some(_version)) = (backend_type, current_version) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "Backend not found",
            Some("NotFoundError"),
        );
    };

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

    let latest_version = match check_latest_version(&backend_type, None, None).await {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                format!("Failed to check latest version: {}", e),
                None,
            )
        }
    };

    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    let name_clone = name.clone();
    let config_dir_clone = config_dir.clone();
    tokio::spawn(async move {
        let config_dir = config_dir_clone;
        let name_for_prep = name_clone.clone();
        let prep = tokio::task::spawn_blocking(
            move || -> Result<(BackendManager, Option<Vec<_>>), String> {
                let mgr = match BackendManager::open(&config_dir) {
                    Ok(m) => m,
                    Err(e) => return Err(format!("Failed to open backend manager: {}", e)),
                };
                match mgr.list_versions(&name_for_prep, None) {
                    Ok(v) => Ok((mgr, v)),
                    Err(e) => Err(format!(
                        "Failed to list versions for backend '{}': {}",
                        name_for_prep, e
                    )),
                }
            },
        )
        .await;
        let (mgr, all_versions) = match prep {
            Ok(Ok((m, Some(v)))) => (m, v),
            Ok(Ok((_, None))) => {
                tracing::error!("Backend '{}' not found during update", name_clone);
                return;
            }
            Ok(Err(msg)) => {
                tracing::error!("{}", msg);
                return;
            }
            Err(e) => {
                tracing::error!("spawn error: {}", e);
                return;
            }
        };
        let backend_info = match all_versions.first() {
            Some(info) => info.clone(),
            None => {
                tracing::error!("Backend '{}' has no versions during update", name_clone);
                return;
            }
        };
        // Use versioned path structure for the update target
        let target_dir = match tama_core::backends::backends_dir() {
            Ok(d) => get_backend_install_path(
                &d,
                &backend_type,
                &backend_info.gpu_variant,
                &latest_version,
            ),
            Err(e) => {
                tracing::error!("Failed to resolve backends_dir for update: {}", e);
                return;
            }
        };
        let options = InstallOptions {
            backend_type: backend_type.clone(),
            source: backend_info
                .source
                .clone()
                .unwrap_or_else(|| BackendSource::SourceCode {
                    version: "main".to_string(),
                    git_url: "https://github.com/ggml-org/llama.cpp.git".to_string(),
                    commit: None,
                }),
            target_dir,
            gpu_variant: backend_info.gpu_variant.clone(),
            allow_overwrite: true,
        };

        let client = reqwest::Client::builder()
            .user_agent("tama-backend-manager")
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        match tama_core::backends::update_backend_with_progress(
            mgr,
            &client,
            &name_clone,
            &backend_info.gpu_variant,
            options,
            latest_version,
            None,
        )
        .await
        {
            Ok(_) => {
                let _ = jobs_clone
                    .finish(&job_clone, crate::web_types::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                let _ = jobs_clone
                    .finish(
                        &job_clone,
                        crate::web_types::JobStatus::Failed,
                        Some(e.to_string()),
                    )
                    .await;
            }
        }
    });

    Json(serde_json::json!({ "job_id": job.id.to_string(), "kind": "update" })).into_response()
}

/// POST /tama/v1/updates/apply/model/:id - Enqueue selected quants through the pull queue.
///
/// Accepts `{ "quants": ["Q4_K_M", "Q8_0"] }` and returns immediately with job IDs.
pub async fn apply_model_update(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<i64>,
    Json(req): Json<ModelUpdateRequest>,
) -> impl axum::response::IntoResponse {
    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };

    // 1. Resolve model: get repo_id and model files for requested quant keys
    let req_quants = req.quants.clone();
    let res_result = tokio::task::spawn_blocking({
        let repo_handle = repo_handle.clone();
        move || -> anyhow::Result<(String, Vec<(String, String)>)> {
            let repo = repo_handle.lock().unwrap();
            let model_record = repo
                .get_model_config(id)?
                .ok_or_else(|| anyhow::anyhow!("Model not found"))?;
            let repo_id = model_record.repo_id;

            // Get model files for this model
            let model_files = repo.get_model_files(id)?;

            // Filter to only the requested quant keys (where quant column matches).
            // Skip files with NULL/None quant — they won't match any requested key.
            let files_to_update: Vec<(String, String)> = model_files
                .into_iter()
                .filter(|f| f.quant.as_ref().is_some_and(|q| req_quants.contains(q)))
                .map(|f| (f.quant.clone().unwrap_or_default(), f.filename))
                .collect();

            Ok((repo_id, files_to_update))
        }
    })
    .await;

    let (repo_id, files_to_update) = match res_result {
        Ok(Ok(val)) => val,
        Ok(Err(e)) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Join error: {}", e),
                None,
            )
        }
    };

    // 2. Validate: ensure all requested quants exist for this model
    let valid_keys: std::collections::HashSet<&str> =
        files_to_update.iter().map(|(k, _)| k.as_str()).collect();
    let invalid_quants: Vec<String> = req
        .quants
        .iter()
        .filter(|q| !valid_keys.contains(q.as_str()))
        .cloned()
        .collect();

    if !invalid_quants.is_empty() {
        let mut body = error_body("Invalid quant keys", Some("ValidationError"));
        body["invalid_quants"] = serde_json::json!(invalid_quants);
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response();
    }

    // 3. Deduplicate within this request (avoid double-enqueue if same filename appears twice)
    let mut seen_filenames = std::collections::HashSet::new();
    let unique_files: Vec<(String, String)> = files_to_update
        .into_iter()
        .filter(|(_, fn_)| seen_filenames.insert(fn_.clone()))
        .collect();

    // 4. Pre-check for duplicate enqueues and enqueue each quant — all in one spawn_blocking.
    let svc = match state.pull_queue().as_ref() {
        Some(s) => s.clone(),
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Download queue not configured",
                Some("ServiceUnavailableError"),
            )
        }
    };

    let enqueue_result = tokio::task::spawn_blocking(
        move || -> Result<Vec<String>, (StatusCode, serde_json::Value)> {
            let repo = shared_repository(&web_state).map_err(|resp| {
                (
                    resp.status(),
                    serde_json::json!({ "error": "Database not configured" }),
                )
            })?;
            let repo = repo.lock().unwrap();

            // Phase 1: Preflight — check all items for duplicates before creating any jobs.
            for (quant_key, filename) in &unique_files {
                match repo.get_active_pull_by_filename(&repo_id, filename) {
                    Ok(Some(existing)) => {
                        let mut body = error_body(
                            format!(
                                "Download already in progress for quant '{}' ({})",
                                quant_key, filename
                            ),
                            Some("ConflictError"),
                        );
                        body["existing_job_id"] = serde_json::json!(existing.job_id);
                        return Err((StatusCode::CONFLICT, body));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            serde_json::json!({
                                "error": format!("Queue check failed for '{}': {}", filename, e)
                            }),
                        ));
                    }
                }
            }

            // Phase 2: All preflight checks passed — generate job IDs and enqueue.
            let mut job_ids = Vec::new();
            for (quant_key, filename) in &unique_files {
                let job_id = uuid::Uuid::new_v4().to_string();

                if let Err(e) = svc.enqueue(
                    &job_id,
                    &repo_id,
                    filename,
                    Some(quant_key.as_str()),
                    "model",
                    Some(quant_key.as_str()),
                    None,
                ) {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        serde_json::json!({ "error": e.to_string() }),
                    ));
                }

                job_ids.push(job_id);
            }

            Ok(job_ids)
        },
    )
    .await;

    let job_ids = match enqueue_result {
        Ok(Ok(ids)) => ids,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("spawn error: {}", e) })),
            )
                .into_response()
        }
    };

    let total = job_ids.len();
    Json(ModelUpdateResponse { job_ids, total }).into_response()
}
