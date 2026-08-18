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
use crate::api::installations::tamad_job;
use crate::web_types::WebState;
use tama_core::installations::{check_latest_version, InstallationType};
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
    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool.clone());
    // Load backend info from DB — discover gpu_variant dynamically
    let requested_variant = query.gpu_variant.clone();
    let versions = match mgr.list_versions(&name, None).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "Backend not found",
                Some("NotFoundError"),
            )
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    // If a specific variant is requested, find that variant
    // Otherwise, fall back to the active variant (legacy behavior)
    let record = if let Some(ref variant) = requested_variant {
        versions.iter().find(|v| v.gpu_variant == *variant)
    } else {
        // No is_active field on InstallationInfo; use first as fallback
        versions.first()
    };

    let (backend_type, current_version) = record
        .map(|r| {
            let bt = match r.backend_type {
                InstallationType::LlamaCpp => InstallationType::LlamaCpp,
                InstallationType::IkLlama => InstallationType::IkLlama,
                _ => InstallationType::Custom,
            };
            (Some(bt), Some(r.gpu_variant.clone()))
        })
        .unwrap_or((None, None));

    let (Some(backend_type), Some(_version)) = (backend_type, current_version) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "Backend not found",
            Some("NotFoundError"),
        );
    };

    // The guard above implies `record` is Some — re-bind for dispatch.
    let Some(backend_info) = record else {
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

    // Resolve the update source: preserve the installation's recorded
    // source type (prebuilt ↔ source code) for the new version. The update
    // itself executes on the backend's tamad (plan-191 Task 10 / ADR-0010);
    // the proxy relays job events and applies the DB version change on
    // success — same flow as POST /tama/v1/backends/:name/update.
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
    // 1. Resolve model: get repo_id and model files for requested quant keys
    //    (Postgres, plan-190 Task 5).
    let pool = web_state.db_pool.as_ref();

    let req_quants = req.quants.clone();
    let model_record = match tama_core::db::queries::get_model_config(pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "Model not found",
                Some("NotFoundError"),
            )
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };
    let repo_id = model_record.repo_id;
    let model_files = match tama_core::db::queries::get_model_files(pool, id).await {
        Ok(f) => f,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };
    let files_to_update: Vec<(String, String)> = model_files
        .into_iter()
        .filter(|f| f.quant.as_ref().is_some_and(|q| req_quants.contains(q)))
        .map(|f| (f.quant.clone().unwrap_or_default(), f.filename))
        .collect();

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

    // 4. Pre-check for duplicate enqueues and enqueue each quant.
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

    // Phase 1: Preflight — check all items for duplicates before creating any jobs.
    for (quant_key, filename) in &unique_files {
        let existing = match tama_core::db::queries::get_active_item_by_repo_filename(
            pool, &repo_id, filename,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("Queue check failed for '{}': {}", filename, e)
                    })),
                )
                    .into_response()
            }
        };
        if let Some(existing) = existing {
            let mut body = error_body(
                format!(
                    "Download already in progress for quant '{}' ({})",
                    quant_key, filename
                ),
                Some("ConflictError"),
            );
            body["existing_job_id"] = serde_json::json!(existing.job_id);
            return (StatusCode::CONFLICT, Json(body)).into_response();
        }
    }

    // Phase 2: All preflight checks passed — generate job IDs and enqueue.
    let mut job_ids = Vec::new();
    for (quant_key, filename) in &unique_files {
        let job_id = uuid::Uuid::new_v4().to_string();

        if let Err(e) = svc
            .enqueue(
                &job_id,
                &repo_id,
                filename,
                Some(quant_key.as_str()),
                "model",
                Some(quant_key.as_str()),
                None,
            )
            .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }

        job_ids.push(job_id);
    }

    let total = job_ids.len();
    Json(ModelUpdateResponse { job_ids, total }).into_response()
}
