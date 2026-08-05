use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::error::error_response;
use crate::api::helpers::shared_repository;
use crate::web_types::WebState;
use tama_core::proxy::tama_handlers::OkResponse;
use tama_core::proxy::ProxyState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckDto {
    pub item_type: String,            // "backend" or "model"
    pub item_id: String,              // backend: "name:variant" (e.g. "llama_cpp:cpu") or model ID
    pub variant: Option<String>,      // GPU variant for backends (e.g. "cpu", "vulkan", "cuda")
    pub repo_id: Option<String>,      // HF repo_id for models (e.g. "unsloth/Qwen3.6-35B-A3B-GGUF")
    pub display_name: Option<String>, // user-friendly model name from config
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String,
    pub error_message: Option<String>,
    pub details_json: Option<serde_json::Value>,
    pub checked_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatesListResponse {
    pub backends: Vec<UpdateCheckDto>,
    pub models: Vec<UpdateCheckDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResponse {
    pub triggered: bool,
    pub message: String,
}

/// Internal helper for parsing per-quant detail objects from `details_json`.
/// The frontend parses `details_json` directly; this struct exists so the
/// API layer can extract quant-level data when needed (e.g. for logging).
#[derive(Debug, Clone, Deserialize)]
pub struct QuantDetailJson {
    pub quant_name: Option<String>,
    pub filename: String,
    pub current_hash: Option<String>,
    pub latest_hash: Option<String>,
    pub update_available: bool,
    pub status: String,
}

/// GET /tama/v1/updates - Returns cached results from DB
pub async fn get_updates(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl axum::response::IntoResponse {
    let config_dir = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    let checker = &web_state.update_checker;
    match checker.get_results(&config_dir).await {
        Ok(records) => {
            // Pooled pre-pass: collect all model IDs and resolve display names
            // in a single spawn_blocking call instead of opening the repo per-record.
            let model_ids: Vec<i64> = records
                .iter()
                .filter(|r| r.item_type == "model")
                .filter_map(|r| r.item_id.parse::<i64>().ok())
                .collect();
            let display_names: std::collections::HashMap<i64, String> =
                match tokio::task::spawn_blocking(move || {
                    let repo = shared_repository(&web_state).ok()?;
                    let repo = repo.lock().unwrap();
                    let mut map = std::collections::HashMap::new();
                    for id in model_ids {
                        if let Ok(Some(m)) = repo.get_model_config(id) {
                            if let Some(name) = m.display_name {
                                map.insert(id, name);
                            }
                        }
                    }
                    Some(map)
                })
                .await
                {
                    Ok(Some(m)) => m,
                    _ => std::collections::HashMap::new(),
                };

            let mut backends = Vec::new();
            let mut models = Vec::new();
            for r in records {
                let details: Option<serde_json::Value> = r
                    .details_json
                    .as_ref()
                    .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok());

                // Extract repo_id from details JSON if present (for models)
                let repo_id = details
                    .as_ref()
                    .and_then(|d| d.get("repo_id"))
                    .and_then(|v| v.as_str())
                    .map(String::from);

                // Parse per-quant details from details_json (internal use only;
                // frontend parses details_json directly).
                let _quants: Vec<QuantDetailJson> = details
                    .as_ref()
                    .and_then(|d| d.get("quants"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|q| serde_json::from_value(q.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();
                // For models, look up display_name from the pooled pre-pass.
                let display_name = if r.item_type == "model" {
                    r.item_id
                        .parse::<i64>()
                        .ok()
                        .and_then(|id| display_names.get(&id).cloned())
                } else {
                    None
                };
                // Parse variant from item_id for backends (format: "name:variant")
                let (parsed_item_id, variant) = if r.item_type == "backend" {
                    if let Some(colon_idx) = r.item_id.rfind(':') {
                        let name = &r.item_id[..colon_idx];
                        let var = &r.item_id[colon_idx + 1..];
                        (name.to_string(), Some(var.to_string()))
                    } else {
                        // Legacy format — no variant separator
                        (r.item_id.clone(), None)
                    }
                } else {
                    (r.item_id.clone(), None)
                };

                let dto = UpdateCheckDto {
                    item_type: r.item_type,
                    item_id: parsed_item_id,
                    variant,
                    repo_id,
                    display_name,
                    current_version: r.current_version,
                    latest_version: r.latest_version,
                    update_available: r.update_available,
                    status: r.status,
                    error_message: r.error_message,
                    details_json: details,
                    checked_at: r.checked_at,
                };
                if dto.item_type == "backend" {
                    backends.push(dto);
                } else {
                    models.push(dto);
                }
            }
            Json(UpdatesListResponse { backends, models }).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// POST /tama/v1/updates/check - Trigger full re-check
pub async fn trigger_check(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl axum::response::IntoResponse {
    let config_dir = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    let checker = web_state.update_checker.clone();
    // Run in background, return immediately
    tokio::spawn(async move {
        if let Err(e) = checker.run_check(&config_dir).await {
            tracing::error!("Background update check failed: {}", e);
        }
    });

    Json(CheckResponse {
        triggered: true,
        message: "Update check started".to_string(),
    })
    .into_response()
}

/// Query params for POST /tama/v1/updates/check/:item_type/:item_id
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CheckSingleQuery {
    #[serde(default)]
    pub gpu_variant: Option<String>,
}

/// POST /tama/v1/updates/check/:item_type/:item_id — trigger an update check
/// for one backend variant or model.
///
/// For backends, use `?gpu_variant=xxx` to check a specific variant.
/// If not provided, checks the active variant (legacy behavior).
pub async fn check_item_for_update(
    Extension(web_state): Extension<WebState>,
    State(state): State<Arc<ProxyState>>,
    Path((item_type, item_id)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<CheckSingleQuery>,
) -> impl axum::response::IntoResponse {
    let config_dir = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    let checker = &web_state.update_checker;
    let result = match item_type.as_str() {
        "backend" => {
            let config_dir_clone = config_dir.clone();
            let item_id_clone = item_id.clone();
            let requested_variant = query.gpu_variant.clone();
            let bt_result = tokio::task::spawn_blocking(
                move || -> anyhow::Result<Option<(tama_core::backends::BackendType, String, bool)>> {
                    let mgr = tama_core::backends::BackendManager::open(&config_dir_clone)?;
                    let versions = mgr.list_versions(&item_id_clone, None)?;

                    // If a specific variant is requested, find that variant
                    // Otherwise, fall back to the active variant (legacy behavior)
                    let versions = match versions {
                        Some(v) => v,
                        None => return Ok(None),
                    };

                    let record = if let Some(ref variant) = requested_variant {
                        versions.iter().find(|v| v.gpu_variant == *variant)
                    } else {
                        // No is_active field on BackendInfo; use first as fallback
                        versions.first()
                    };

                    Ok(record.map(|r| {
                        let is_docker = r.docker_config.is_some();
                        (
                            match r.backend_type {
                                tama_core::backends::BackendType::LlamaCpp => {
                                    tama_core::backends::BackendType::LlamaCpp
                                }
                                tama_core::backends::BackendType::IkLlama => {
                                    tama_core::backends::BackendType::IkLlama
                                }
                                _ => tama_core::backends::BackendType::Custom,
                            },
                            r.gpu_variant.clone(),
                            is_docker,
                        )
                    }))
                },
            )
            .await;

            match bt_result {
                Ok(Ok(Some((bt, gpu_variant, is_docker)))) => {
                    if is_docker {
                        // Docker backends have no release feed to check
                        return Json(OkResponse::OK).into_response();
                    }
                    checker
                        .check_backend(&config_dir, &item_id, &bt, &gpu_variant)
                        .await
                        .map(|_| ())
                }
                Ok(Ok(None)) => Err(anyhow::anyhow!("Backend not found")),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(anyhow::anyhow!("Join error: {}", e)),
            }
        }
        "model" => {
            let item_id_clone = item_id.clone();
            let repo_handle = match shared_repository(&web_state) {
                Ok(h) => h,
                Err(_) => {
                    return error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Database not configured",
                        Some("ServiceUnavailableError"),
                    )
                }
            };
            let rid_result = tokio::task::spawn_blocking({
                let repo_handle = repo_handle.clone();
                move || -> anyhow::Result<(Option<i64>, Option<String>)> {
                    let repo = repo_handle.lock().unwrap();
                    // Convert config_key to repo_id to look up model_id
                    let repo_id = tama_core::models::config_key_to_repo_id(&item_id_clone);
                    let record = repo.get_model_config_by_repo_id(&repo_id)?;
                    Ok(record
                        .map(|r| (Some(r.id), Some(r.repo_id.clone())))
                        .unwrap_or((None, None)))
                }
            })
            .await;

            match rid_result {
                Ok(Ok((Some(model_id), Some(repo_id)))) => checker
                    .check_model(&config_dir, model_id, Some(&repo_id))
                    .await
                    .map(|_| ()),
                Ok(Ok((None, _))) | Ok(Ok((_, None))) => {
                    Err(anyhow::anyhow!("Model not found in DB"))
                }
                Ok(Err(e)) => Err(e),
                Err(e) => Err(anyhow::anyhow!("Join error: {}", e)),
            }
        }
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Invalid item_type",
                Some("ValidationError"),
            )
        }
    };

    match result {
        Ok(_) => Json(OkResponse::OK).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}
