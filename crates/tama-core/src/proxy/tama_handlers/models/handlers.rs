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
///
/// plan-191 Task 5: the load goes through the model's provider tamad
/// (`LoadModel` RPC); the proxy records desired state and mirrors the
/// result in its BackendState cache.
pub async fn handle_tama_load_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_config_key(&state, &model_id).await;

    match crate::proxy::lifecycle::spec::load_model_on_tamad(&state, &model_id).await {
        Ok(_) => Json(ModelResponse {
            id: model_id,
            loaded: true,
        })
        .into_response(),
        Err(e) => {
            tracing::warn!("Failed to load model {}: {}", model_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to load model: {}", e),
                        "type": "LoadModelError"
                    }
                })),
            )
                .into_response()
        }
    }
}

/// Handle cancelling a loading model (Tama management API).
///
/// plan-191 Task 5: operates on **desired state**, not on a local process.
/// Cancelling clears the desired row and issues `UnloadModel` to the
/// provider's tamad. If the load RPC is still in flight, the reconciler's
/// next tick unloads the model once it appears in the tamad's snapshot
/// (loads are short; a cancel is therefore best-effort for in-flight
/// loads).
pub async fn handle_tama_cancel_load(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_config_key(&state, &model_id).await;

    // Is the model desired, mirrored, or running on its tamad?
    let pool = state.db_pool();
    let desired = crate::db::queries::get_desired(&pool, &model_id)
        .await
        .ok()
        .flatten();
    let mirrored = state
        .get_available_backend_for_model(&model_id)
        .await
        .is_some();
    let on_tamad =
        match crate::proxy::lifecycle::spec::resolve_provider_for_model(&state, &model_id)
            .await
            .ok()
        {
            Some(p) => match p.tamad_id.as_ref() {
                Some(tamad_id) => state.tamad_pool().get(tamad_id).await.is_some(),
                None => false,
            },
            None => false,
        };

    if desired.is_none() && !mirrored && !on_tamad {
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

    // Clear desired state (best-effort: the row may not exist).
    if let Err(e) = crate::db::queries::clear_desired(&pool, &model_id).await {
        warn!("cancel: clear_desired for '{}' failed: {}", model_id, e);
    }

    // Best-effort unload on the tamad (may be not-loaded yet: the
    // reconciler unloads it on its next tick).
    if let Err(e) = crate::proxy::lifecycle::spec::unload_model_on_tamad(&state, &model_id).await {
        warn!("cancel: unload RPC for '{}' failed: {}", model_id, e);
    }

    // Remove the local mirror entry, if any.
    state.remove_mirror_by_model(&model_id).await;

    info!("Model '{}' cancel completed", model_id);

    Json(ModelResponse {
        id: model_id,
        loaded: false,
    })
    .into_response()
}

/// Handle unloading a model (Tama management API).
///
/// plan-191 Task 5: clearing the model's **desired** state is the primary
/// action — once it is no longer desired, the reconciler unloads it on the
/// provider's tamad (this is the safety net if the RPC below can't reach
/// the tamad). The `UnloadModel` RPC is issued best-effort for immediate
/// convergence; a failure to reach the tamad (offline, no provider) is
/// logged, not an error, because the reconciler will converge the tamad's
/// process table to the desired set on its next tick.
/// Unloading a model that is not loaded on the tamad is a no-op
/// (idempotent).
pub async fn handle_tama_unload_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_config_key(&state, &model_id).await;

    // Configured at all?
    {
        let model_configs = state.registry.model_configs.read().await;
        if !model_configs.contains_key(&model_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "message": "Model not configured or not loaded",
                        "type": "NotFoundError"
                    }
                })),
            )
                .into_response();
        }
    }

    let pool = state.db_pool();

    // Primary action: clear desired state (best-effort: the row may not
    // exist, and the reconciler uses it to decide convergence).
    if let Err(e) = crate::db::queries::clear_desired(&pool, &model_id).await {
        warn!("unload: clear_desired for '{}' failed: {}", model_id, e);
    }

    // Best-effort immediate physical unload on the tamad. The reconciler
    // retries on its next tick if this can't reach the tamad.
    if let Err(e) = crate::proxy::lifecycle::spec::unload_model_on_tamad(&state, &model_id).await {
        warn!("unload: RPC for '{}' failed: {}", model_id, e);
    }

    // Drop the local mirror.
    state.remove_mirror_by_model(&model_id).await;

    Json(ModelResponse {
        id: model_id,
        loaded: false,
    })
    .into_response()
}
