//! Model management handlers.
//!
//! Handles `/v1/models` (list) and `/v1/models/{id}` (get) endpoints.

use crate::proxy::ProxyState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

/// Find a matching model entry from backend responses.
/// - If entries has exactly one model → return it
/// - If multiple → try to match by config's `model` field against backend's `id` (file path)
/// - If no match → return first entry (best guess)
fn find_model_in_entries(
    entries: &[serde_json::Value],
    config_model: Option<&str>,
) -> Option<serde_json::Value> {
    if entries.is_empty() {
        return None;
    }
    if entries.len() == 1 {
        return Some(entries[0].clone());
    }
    // Multiple entries: try to match by config's model field (file path)
    if let Some(model_path) = config_model {
        for entry in entries {
            if let Some(id) = entry.get("id").and_then(|v| v.as_str()) {
                if id == model_path {
                    return Some(entry.clone());
                }
            }
        }
    }
    // No match found — return first entry as best guess
    Some(entries[0].clone())
}

#[axum::debug_handler]
pub async fn handle_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    // Phase 1: Look up model by model_id in config.
    // Match by config_name, api_name, or model field.
    let (config_name, server_cfg) = {
        let model_configs = state.model_configs.read().await;
        let mut found: Option<(&String, &crate::config::ModelConfig)> = None;

        for (name, cfg) in model_configs.iter() {
            if !cfg.enabled {
                continue;
            }
            if name == &model_id
                || cfg.api_name.as_deref() == Some(&*model_id)
                || cfg.model.as_deref() == Some(model_id.as_str())
            {
                found = Some((name, cfg));
                break;
            }
        }

        match found {
            Some((name, cfg)) => (name.clone(), cfg.clone()),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": "Model not found",
                            "type": "NotFoundError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    };

    // Phase 2: Check if the config's backend is loaded and Ready.
    if let Some(crate::proxy::ModelState::Ready { backend_url, .. }) =
        state.models.read().await.get(&config_name)
    {
        // Query backend's /v1/models and find matching entry
        let entries = fetch_models_from_backend(&state, backend_url).await;
        if let Some(mut entry) = find_model_in_entries(&entries, server_cfg.model.as_deref()) {
            entry["ready"] = serde_json::value::to_value(true).unwrap();
            return Json(entry).into_response();
        }
    }

    // Phase 3: Fallback — construct from config (no meta, ready: false).
    let model_id_val = server_cfg.api_name.as_deref().unwrap_or(&config_name);
    Json(serde_json::json!({
        "id": model_id_val,
        "object": "model",
        "created": 0,
        "owned_by": server_cfg.backend,
        "ready": false
    }))
    .into_response()
}

#[axum::debug_handler]
pub async fn handle_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    // Phase 1: Snapshot data under locks, then drop them before I/O.
    let (backend_info, has_available_llm, all_configs) = {
        let models = state.models.read().await;
        let configs = state.model_configs.read().await;

        // Collect (config_name, backend_url, is_ready) for all models
        let backend_info: Vec<_> = models
            .iter()
            .map(|(name, ms)| {
                if let crate::proxy::ModelState::Ready { backend_url, .. } = ms {
                    (name.clone(), Some(backend_url.clone()), true)
                } else {
                    (name.clone(), None, false)
                }
            })
            .collect();

        // Clone config map for use outside lock
        let configs = configs.clone();

        // Check if any non-TTS model is Ready or Starting (for wildcard ready flag)
        let has_available_llm = models.iter().any(|(_, s)| {
            !s.is_tts_backend()
                && (s.is_ready() || matches!(s, crate::proxy::ModelState::Starting { .. }))
        });

        (backend_info, has_available_llm, configs)
    };
    // All locks dropped here

    // Phase 2: Query all Ready backends concurrently.
    let futures: Vec<_> = backend_info
        .iter()
        .filter_map(|(_, url, _)| url.as_ref().map(|u| fetch_models_from_backend(&state, u)))
        .collect();
    let results: Vec<Vec<serde_json::Value>> = futures::future::join_all(futures).await;

    // Phase 3: Merge results and inject `ready`.
    let mut data: Vec<serde_json::Value> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // Track which config_names were served by backends (for fallback logic)
    let mut served_config_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // We need to correlate backend results with config_names.
    // The order of `results` matches the order of Ready backends in `backend_info`.
    let mut ready_iter = backend_info.iter().filter(|(_, _, ready)| *ready);
    for entries in results {
        if let Some((config_name, _, _)) = ready_iter.next() {
            served_config_names.insert(config_name.clone());
            for mut entry in entries {
                let id = entry
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if seen_ids.contains(&id) {
                    warn!("Duplicate model id {} from backends", id);
                    continue;
                }
                seen_ids.insert(id);

                // Inject ready
                entry["ready"] = serde_json::value::to_value(true).unwrap();
                data.push(entry);
            }
        }
    }

    // Phase 4: Add unloaded models (in config but not loaded on any backend).
    for (config_name, server_cfg) in all_configs.iter() {
        if !server_cfg.enabled {
            continue;
        }
        let model_id = server_cfg.api_name.as_deref().unwrap_or(config_name);
        if seen_ids.contains(model_id) {
            continue; // already added from backend
        }
        data.push(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": 0,
            "owned_by": server_cfg.backend,
            "ready": false
        }));
    }

    // Phase 5: Prepend wildcard entry.
    data.insert(
        0,
        serde_json::json!({
            "id": crate::proxy::WILDCARD_MODEL_NAME,
            "object": "model",
            "created": 0,
            "owned_by": "tama-proxy",
            "ready": has_available_llm
        }),
    );

    Json(serde_json::json!({
        "object": "list",
        "data": data
    }))
}

/// Parse a /v1/models response body and extract the `data` array.
/// Returns empty Vec if the response is invalid or missing `data`.
pub fn parse_models_response(body: &[u8]) -> Vec<serde_json::Value> {
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    parsed
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| arr.to_vec())
        .unwrap_or_default()
}

/// Query a single backend's /v1/models endpoint and return the `data` array.
/// Returns an empty Vec on any error (backend down, bad response, timeout).
pub async fn fetch_models_from_backend(
    state: &ProxyState,
    backend_url: &str,
) -> Vec<serde_json::Value> {
    let url = format!("{}/v1/models", backend_url);
    match state
        .client
        .get(&url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => match response.bytes().await {
            Ok(body) => parse_models_response(&body),
            Err(e) => {
                warn!("Failed to read response body from {}: {}", backend_url, e);
                Vec::new()
            }
        },
        Err(e) => {
            warn!("Failed to fetch /v1/models from {}: {}", backend_url, e);
            Vec::new()
        }
    }
}
