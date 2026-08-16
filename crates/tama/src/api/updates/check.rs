use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::error::error_response;
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
    // `config_dir` is no longer needed here: update-check results come from
    // Postgres (plan-190 Task 4). The resolution is kept so handlers without
    // a configured config dir keep failing loudly.
    let _config_dir = match crate::api::helpers::resolve_config_dir(&state) {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    let pool = state.db_pool();

    let checker = &web_state.update_checker;
    match checker.get_results(&pool).await {
        Ok(records) => {
            // Pre-pass: resolve display names per model from Postgres.
            // (plan-190 Task 5)
            let model_ids: Vec<i64> = records
                .iter()
                .filter(|r| r.item_type == "model")
                .filter_map(|r| r.item_id.parse::<i64>().ok())
                .collect();
            let mut display_names: std::collections::HashMap<i64, String> =
                std::collections::HashMap::new();
            for id in &model_ids {
                if let Ok(Some(m)) = tama_core::db::queries::get_model_config(&pool, *id).await {
                    if let Some(name) = m.display_name {
                        display_names.insert(*id, name);
                    }
                }
            }

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
    let pool = state.db_pool();

    let checker = web_state.update_checker.clone();
    // Run in background, return immediately
    tokio::spawn(async move {
        if let Err(e) = checker.run_check(&pool).await {
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
    // Validate item_type first so a malformed request fails with 400 even
    // when the pool is unavailable.
    if !matches!(item_type.as_str(), "backend" | "model") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Invalid item_type",
            Some("ValidationError"),
        );
    }

    let pool = state.db_pool();

    let checker = &web_state.update_checker;
    let result = match item_type.as_str() {
        "backend" => {
            let requested_variant = query.gpu_variant.clone();
            let bt_result = {
                let mgr = tama_core::installations::InstallationManager::new(pool.clone());
                let versions = mgr.list_versions(&item_id, None).await;

                // If a specific variant is requested, find that variant
                // Otherwise, fall back to the active variant (legacy behavior)
                let versions = match versions {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Backend not found",
                            None,
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

                let record = if let Some(ref variant) = requested_variant {
                    versions.iter().find(|v| v.gpu_variant == *variant)
                } else {
                    // No is_active field on InstallationInfo; use first as fallback
                    versions.first()
                };

                record.map(|r| {
                    let is_docker = r.docker_config.is_some();
                    (
                        match r.backend_type {
                            tama_core::installations::InstallationType::LlamaCpp => {
                                tama_core::installations::InstallationType::LlamaCpp
                            }
                            tama_core::installations::InstallationType::IkLlama => {
                                tama_core::installations::InstallationType::IkLlama
                            }
                            _ => tama_core::installations::InstallationType::Custom,
                        },
                        r.gpu_variant.clone(),
                        is_docker,
                    )
                })
            };

            match bt_result {
                Some((bt, gpu_variant, is_docker)) => {
                    if is_docker {
                        // Docker backends have no release feed to check
                        return Json(OkResponse::OK).into_response();
                    }
                    checker
                        .check_backend(&pool, &item_id, &bt, &gpu_variant)
                        .await
                        .map(|_| ())
                }
                None => Err(anyhow::anyhow!("Backend not found")),
            }
        }
        "model" => {
            // Convert config_key to repo_id to look up model_id (Postgres, plan-190 Task 5).
            let repo_id = tama_core::models::config_key_to_repo_id(&item_id);
            let record = match tama_core::db::queries::get_model_config_by_repo_id(&pool, &repo_id)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
                }
            };
            match record {
                Some(r) => checker
                    .check_model(&pool, r.id, Some(&r.repo_id))
                    .await
                    .map(|_| ()),
                None => Err(anyhow::anyhow!("Model not found in DB")),
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
