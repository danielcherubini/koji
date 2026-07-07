use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use super::utils::resolve_model_id;
use crate::proxy::process::{force_kill_process_group, is_process_group_alive, kill_process_group};
use crate::proxy::tama_handlers::ModelResponse;
use crate::proxy::{BackendState, ProxyState};
use tracing::{info, warn};

/// Handle listing all configured models (Tama management API).
pub async fn handle_tama_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models = state.build_status_response().await;
    let models_obj = models.get("models").and_then(|v| v.as_object());

    let result: Vec<serde_json::Value> = models_obj
        .into_iter()
        .flat_map(|models_obj| {
            models_obj.iter().filter_map(|(_key, model)| {
                model
                    .as_object()
                    .and_then(|model| serde_json::to_value(model).ok())
            })
        })
        .collect();

    Json(serde_json::json!({
        "models": result
    }))
}

/// Handle getting a single model's state (Tama management API).
pub async fn handle_tama_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    // Check if already loaded (by server name or model name)
    let model_state = state.get_model_state(&model_id).await;

    if let Some(ms) = model_state {
        let owned_by = ms.backend();
        let created = match ms.load_time() {
            Some(load_time) => load_time
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::ZERO)
                .as_secs(),
            None => 0,
        };
        return Json(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": created,
            "owned_by": owned_by,
            "ready": ms.is_ready()
        }))
        .into_response();
    }

    // Check if it's a configured (but not loaded) model
    let model_configs = state.model_configs.read().await;
    let config = state.config.read().await;
    let servers = config.resolve_backends_for_model(&model_configs, &model_id);
    if let Some((config_name, server_cfg, _)) = servers.first() {
        if server_cfg.enabled {
            return Json(serde_json::json!({
                "id": config_name,
                "object": "model",
                "created": 0,
                "owned_by": server_cfg.backend,
                "ready": false
            }))
            .into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "message": "Model not found",
                "type": "NotFoundError"
            }
        })),
    )
        .into_response()
}

/// Handle loading a model (Tama management API).
pub async fn handle_tama_load_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    let model_card = state.get_model_card(&model_id).await;
    let target_gpu = state
        .resolve_model_gpu_device(&model_id, model_card.as_ref())
        .await;
    let _ = state.evict_lru_if_needed(target_gpu).await;
    match state.load_model(&model_id, model_card.as_ref(), &()).await {
        Ok(backend_name) => {
            let model_state = state.get_model_state(&backend_name).await;
            let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
            Json(ModelResponse {
                id: model_id,
                loaded,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to load model: {}", e),
                    "type": "LoadModelError"
                }
            })),
        )
            .into_response(),
    }
}

/// Handle cancelling a loading model (Tama management API).
///
/// Kills a loading backend process and returns the model to idle. The handler
/// uses a read-write lock double-check pattern to avoid races with the
/// load_model path.
///
/// There is a narrow race window where load_model's health check succeeds after
/// cancel removes the entry, and load_model then calls mgr.insert_active()
/// unconditionally. A future fix would add a re-check in load_model before
/// insert_active under the write lock.
pub async fn handle_tama_cancel_load(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;

    // ── Step b: read lock — initial check ──────────────────────────────
    let (backend_name, pid) = {
        let models = state.models.read().await;
        let entry = match models.get(&model_id) {
            Some(e) => e,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is not currently loading",
                            "type": "ModelNotLoadingError"
                        }
                    })),
                )
                    .into_response();
            }
        };

        match entry {
            BackendState::Starting { backend_pid, .. } => (model_id.clone(), *backend_pid),
            BackendState::Ready { .. } => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is already loaded",
                            "type": "ModelAlreadyLoadedError"
                        }
                    })),
                )
                    .into_response();
            }
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is not currently loading",
                            "type": "ModelNotLoadingError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    }; // read lock dropped

    // ── Step c+d: write lock — re-validate and remove ──────────────────
    {
        let mut models = state.models.write().await;
        match models.get(&backend_name) {
            Some(BackendState::Starting { .. }) => {
                // TODO: race with load_model's mgr.insert_active() — if health check
                // succeeds between here and the kill below, load_model may insert a
                // stale active_models DB row. A future fix would add a re-check in
                // load_model before insert_active under the write lock.
                models.remove(&backend_name);
            }
            Some(BackendState::Ready { .. }) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is already loaded",
                            "type": "ModelAlreadyLoadedError"
                        }
                    })),
                )
                    .into_response();
            }
            _ => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model is not currently loading",
                            "type": "ModelNotLoadingError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    } // write lock dropped

    // ── Step f: kill process group ─────────────────────────────────────
    if pid > 0 {
        // First attempt: SIGTERM
        if let Err(e) = kill_process_group(pid).await {
            warn!("Cancel kill failed for '{}': {}", backend_name, e);
        }

        // Poll up to 2s for the group to die (every 250ms)
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !is_process_group_alive(pid) {
                break;
            }
        }

        // Escalate: SIGKILL if still alive
        if is_process_group_alive(pid) {
            if let Err(e) = force_kill_process_group(pid).await {
                warn!("Cancel force kill failed for '{}': {}", backend_name, e);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // ── Step g: clean up DB ────────────────────────────────────────────
    if let Some(mgr) = state.model_mgr() {
        if let Err(e) = mgr.remove_active(&backend_name) {
            warn!(
                "Failed to remove active entry for '{}': {}",
                backend_name, e
            );
        }
    }

    // ── Step h: log ────────────────────────────────────────────────────
    info!("Model '{}' cancel completed", backend_name);

    // ── Step i: return response ────────────────────────────────────────
    Json(ModelResponse {
        id: model_id,
        loaded: false,
    })
    .into_response()
}

/// Handle unloading a model (Tama management API).
pub async fn handle_tama_unload_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    // Get the server name for this model
    let backend_name = state.get_available_backend_for_model(&model_id).await;

    match backend_name {
        Some(backend_name) => {
            // Unload the model
            match state.unload_model(&backend_name).await {
                Ok(_) => {
                    let model_state = state.get_model_state(&model_id).await;
                    let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
                    Json(ModelResponse {
                        id: model_id,
                        loaded,
                    })
                    .into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Failed to unload model: {}", e),
                            "type": "UnloadModelError"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured or not loaded",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}
