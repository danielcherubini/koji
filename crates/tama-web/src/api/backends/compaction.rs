//! Compaction backend management endpoints.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use tama_core::proxy::ProxyState;

#[derive(Debug, Deserialize)]
pub struct CompactionToggleRequest {
    pub enabled: bool,
    pub device: Option<String>,
    pub port: Option<Option<u16>>,
    pub request_timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CompactionToggleResponse {
    pub enabled: bool,
    pub running: bool,
}

/// POST /tama/v1/backends/compaction
/// Toggle compaction config and trigger start/stop.
pub async fn update_compaction(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<CompactionToggleRequest>,
) -> impl IntoResponse {
    // Update config
    {
        let mut config = state.config.write().await;
        if let Some(device) = &req.device {
            config.compaction.device = device.clone();
        }
        if let Some(port) = &req.port {
            config.compaction.port = *port;
        }
        if let Some(timeout) = &req.request_timeout_ms {
            config.compaction.request_timeout_ms = *timeout;
        }
        let was_enabled = config.compaction.enabled;
        config.compaction.enabled = req.enabled;

        // Persist config to disk — follow existing pattern from save_structured_config
        if let Some(ref config_path) = config.loaded_from {
            let config_dir = config_path.parent().unwrap_or(config_path);
            let toml_path = config_dir.join("config.toml");
            if let Ok(toml_str) = toml::to_string_pretty(&*config) {
                let _ = tokio::fs::write(&toml_path, toml_str).await;
            }
        }

        // If enabling and not already running, try to start
        if req.enabled && !was_enabled {
            drop(config);
            // Try to load compaction backend (best effort — don't fail the toggle)
            if let Err(e) = state.load_compaction_backend().await {
                tracing::warn!("Failed to start compaction backend: {}", e);
            }
        }
        // If disabling and was running, we could stop it but there's no unload_compaction_backend()
        // The compaction backend will be cleaned up on shutdown. For now, just update config.
    }

    // Check current running status
    let running = {
        let models = state.models.read().await;
        models
            .get("compaction")
            .map(|s| s.is_ready())
            .unwrap_or(false)
    };

    (
        StatusCode::OK,
        Json(CompactionToggleResponse {
            enabled: req.enabled,
            running,
        }),
    )
        .into_response()
}
