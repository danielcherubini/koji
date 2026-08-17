use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use super::utils::resolve_config_key;
use crate::gpu::ModelState;
use crate::installations::docker::{remove_container, stop_container};
use crate::process::{force_kill_process_group, is_process_group_alive, kill_process_group};
use crate::proxy::tama_handlers::{ListModelsResponse, ListedModelResponse, ModelResponse};
use crate::proxy::{BackendState, ProxyState};
use tracing::{info, warn};

// TODO(plan-172): unrouted after plan-169 — delete
/// Handle listing all configured models (Tama management API).
pub async fn handle_tama_list_models(state: State<Arc<ProxyState>>) -> Json<ListModelsResponse> {
    use std::sync::atomic::Ordering::Relaxed;

    let config = state.config.read().await;
    let model_configs = state.registry.model_configs.read().await;
    let models = state.registry.models.read().await;
    let auto_unload = config.proxy.auto_unload;
    let idle_timeout_secs = config.proxy.idle_timeout_secs;

    let mut result: Vec<ListedModelResponse> = Vec::with_capacity(model_configs.len());

    for (model_name, model_config) in model_configs.iter() {
        let backend_path = config
            .backends
            .get(&model_config.backend)
            .and_then(|b| b.path.clone());

        let model_state = models.get(model_name);

        let (
            model_state,
            backend_pid,
            load_time_secs,
            last_accessed_secs_ago,
            idle_timeout_remaining_secs,
            consecutive_failures,
        ) = match model_state {
            Some(BackendState::Ready {
                backend_pid,
                load_time,
                last_accessed,
                consecutive_failures,
                ..
            }) => {
                let now = Instant::now();
                let secs_ago = now.duration_since(*last_accessed).as_secs();
                let elapsed = now.duration_since(*last_accessed);
                let remaining = if auto_unload {
                    let timeout = Duration::from_secs(idle_timeout_secs);
                    if elapsed < timeout {
                        Some((timeout - elapsed).as_secs())
                    } else {
                        Some(0)
                    }
                } else {
                    None
                };
                let load_secs = load_time
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                (
                    ModelState::Ready,
                    Some(*backend_pid),
                    Some(load_secs),
                    Some(secs_ago),
                    remaining,
                    Some(consecutive_failures.load(Relaxed)),
                )
            }
            Some(BackendState::Starting {
                consecutive_failures,
                ..
            }) => (
                ModelState::Starting,
                None,
                None,
                None,
                None,
                Some(consecutive_failures.load(Relaxed)),
            ),
            Some(BackendState::Unloading { .. }) => {
                (ModelState::Unloading, None, None, None, None, None)
            }
            Some(BackendState::Failed { .. }) => (ModelState::Failed, None, None, None, None, None),
            _ => (ModelState::Idle, None, None, None, None, None),
        };

        result.push(ListedModelResponse {
            id: model_config.db_id,
            display_name: model_config.display_name.clone(),
            backend: model_config.backend.clone(),
            backend_path,
            model: model_config.model.clone(),
            quant: model_config.quant.clone(),
            context_length: model_config.context_length,
            enabled: model_config.enabled,
            api_name: model_config.api_name.clone(),
            state: model_state,
            backend_pid,
            load_time_secs,
            last_accessed_secs_ago,
            idle_timeout_remaining_secs,
            consecutive_failures,
        });
    }

    Json(ListModelsResponse { models: result })
}

// TODO(plan-172): unrouted after plan-169 — delete
/// Handle getting a single model's state (Tama management API).
pub async fn handle_tama_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_config_key(&state, &model_id).await;
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
    let model_configs = state.registry.model_configs.read().await;
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
    let model_id = resolve_config_key(&state, &model_id).await;
    let model_toml = state.get_model_toml(&model_id).await;
    let target_gpu = state
        .resolve_model_gpu_device(&model_id, model_toml.as_ref())
        .await;
    let _ = state.evict_lru_if_needed(target_gpu).await;
    match state.load_model(&model_id, model_toml.as_ref(), &()).await {
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
    let model_id = resolve_config_key(&state, &model_id).await;

    // ── Step b: read lock — initial check ──────────────────────────────
    let (backend_name, pid, is_docker) = {
        let models = state.registry.models.read().await;
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
            BackendState::Starting {
                backend_pid,
                is_docker,
                ..
            } => (model_id.clone(), *backend_pid, *is_docker),
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
        let mut models = state.registry.models.write().await;
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

    // ── Step f: kill process or container ──────────────────────────────
    if pid > 0 {
        if is_docker {
            // Docker path: stop + remove container
            let container_name = format!("tama-{}", backend_name);
            let _ = stop_container(&container_name).await;
            let _ = remove_container(&container_name).await;
        } else {
            // Native path: kill process group
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
    }

    // ── Step g: clean up DB (Postgres, plan-190 Task 5) ────────────────
    let pool = state.db_pool();
    if let Err(e) = crate::db::queries::remove_active_model(&pool, &backend_name).await {
        warn!(
            "Failed to remove active entry for '{}': {}",
            backend_name, e
        );
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
    let model_id = resolve_config_key(&state, &model_id).await;
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
