use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use super::utils::resolve_config_key;
use crate::gpu::ModelState;
use crate::proxy::tama_handlers::{ListModelsResponse, ListedModelResponse, ModelResponse};
use crate::proxy::ProxyState;
use tracing::{info, warn};

// TODO(plan-172): unrouted after plan-169 — delete
/// Handle listing all configured models (Tama management API).
pub async fn handle_tama_list_models(state: State<Arc<ProxyState>>) -> Json<ListModelsResponse> {
    let config = state.config.read().await;
    let model_configs = state.registry.model_configs.read().await;

    // Lifecycle state now comes from the row-backed per-model snapshots
    // (plan-193 Task 4 flip: `collect_model_state_snapshots` reads the live
    // ProcessInfo rows, not the mirror).
    let mut snap_by_id = std::collections::HashMap::new();
    for snap in state.collect_model_state_snapshots().await {
        snap_by_id.insert(snap.id.clone(), snap);
    }

    let mut result: Vec<ListedModelResponse> = Vec::with_capacity(model_configs.len());

    for (model_name, model_config) in model_configs.iter() {
        let backend_path = config
            .backends
            .get(&model_config.backend)
            .and_then(|b| b.path.clone());

        // row-backed per-model lifecycle state; the mirror-only detail
        // (pid, load time, idle count) has no wire source and reads None.
        let lifecycle_state = snap_by_id
            .get(model_name)
            .map(|s| s.state.clone())
            .unwrap_or(ModelState::Idle);

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
            state: lifecycle_state,
            backend_pid: None,
            load_time_secs: None,
            last_accessed_secs_ago: None,
            idle_timeout_remaining_secs: None,
            consecutive_failures: None,
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
    // Whether the model is currently live on the wire (plan-193 T4 flip:
    // rows, not the mirror). `created`/`owned_by` have no wire source and
    // read 0 / empty; `ready` is the lifecycle fact that matters.
    let live = crate::proxy::live_rows(state.tamad_pool().as_ref()).await;
    // Merge the row with the configured backend so `owned_by` still resolves
    // (the wire row has no provider field); `created` stays 0 (no wire source).
    let owned = state
        .registry
        .model_configs
        .read()
        .await
        .get(&model_id)
        .map(|mc| mc.backend.clone())
        .unwrap_or(String::new());
    if let Some(r) = live.row(&model_id) {
        return Json(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": 0,
            "owned_by": owned,
            "ready": r.status == "ready"
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
/// (`LoadModel` RPC); *desired* state lives in the tamad's host-side store
/// (no proxy-side DB copy anymore; plan-193 T7).
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
/// plan-191 Task 5 / plan-193 T7: operates on the model's ***desired***
/// wire state (the tamad row's `desired` flag), not on a local process.
/// Cancelling issues `UnloadModel` to the provider's tamad. If the load
/// RPC is still in flight, the tamad's host-side store handles the
/// post-load unload (loads are short, so a cancel is best-effort).
pub async fn handle_tama_cancel_load(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_config_key(&state, &model_id).await;

    // Is the model desired (its live wire row reports `desired`),
    // mirrored, or running on its tamad? plan-193 T7: the desired fact
    // comes off the wire row — the last Postgres read of model desire
    // is gone.
    let desired = crate::proxy::live_rows(state.tamad_pool().as_ref())
        .await
        .row(&model_id)
        .filter(|r| r.desired);
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

    // Best-effort unload on the tamad (may be not-loaded yet). Lifecycle
    // truth is the live tamad rows — the tamad's host-side store clears
    // the model's desired flag onUnload().
    if let Err(e) = crate::proxy::lifecycle::spec::unload_model_on_tamad(&state, &model_id).await {
        warn!("cancel: unload RPC for '{}' failed: {}", model_id, e);
    }

    // No mirror to remove: rows now track lifecycle truth directly.

    info!("Model '{}' cancel completed", model_id);

    Json(ModelResponse {
        id: model_id,
        loaded: false,
    })
    .into_response()
}

/// Handle unloading a model (Tama management API).
///
/// plan-191 Task 5 / plan-193 T7: issuing `UnloadModel` to the provider's
/// tamad makes the host-side store clear the model's **desired** flag and
/// drop the process (this is the safety net if the RPC below can't reach
/// the tamad). A failure to reach the tamad (offline, no provider) is
/// logged, not an error: the tamad's host-side store keeps the desired
/// truth. Unloading a model that is not loaded on the tamad is a no-op
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

    // Best-effort immediate physical unload on the tamad. Lifecycle
    // truth is the live tamad rows — the tamad's host-side store clears
    // the model's desired flag on the UnloadModel RPC.
    if let Err(e) = crate::proxy::lifecycle::spec::unload_model_on_tamad(&state, &model_id).await {
        warn!("unload: RPC for '{}' failed: {}", model_id, e);
    }

    // No mirror to drop: rows now track lifecycle truth directly.

    Json(ModelResponse {
        id: model_id,
        loaded: false,
    })
    .into_response()
}
