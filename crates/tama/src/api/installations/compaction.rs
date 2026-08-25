//! Compaction backend management endpoints.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::field_update::FieldUpdate;
use tama_core::config::CompactionDevice;
use tama_core::proxy::ProxyState;

#[derive(Debug, Deserialize)]
pub struct CompactionToggleRequest {
    pub enabled: bool,
    pub device: Option<CompactionDevice>,
    #[serde(default)]
    pub port: FieldUpdate<u16>,
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
        let (config_to_save, was_enabled) = state
            .with_config_mut(|config| {
                if let Some(device) = &req.device {
                    config.compaction.device = device.clone();
                }
                match &req.port {
                    FieldUpdate::Set(v) => config.compaction.port = Some(*v),
                    FieldUpdate::Clear => config.compaction.port = None,
                    FieldUpdate::Unchanged => {}
                }
                if let Some(timeout) = &req.request_timeout_ms {
                    config.compaction.request_timeout_ms = *timeout;
                }
                let was_enabled = config.compaction.enabled;
                config.compaction.enabled = req.enabled;
                ((*config).clone(), was_enabled)
            })
            .await;
        // Persist to Postgres (plan-190 Task 3) — best effort, don't fail the toggle.
        let pool = state.db_pool();
        if let Err(e) = config_to_save.save(&pool).await {
            tracing::warn!(error = %e, "Failed to persist compaction config to database");
        }

        // If enabling and not already running, try to start
        if req.enabled && !was_enabled {
            // Try to load compaction backend (best effort — don't fail the toggle)
            if let Err(e) =
                tama_core::proxy::lifecycle::spec::load_compaction_on_tamad(&state).await
            {
                tracing::warn!("Failed to start compaction backend: {}", e);
            }
        }
    }

    // Check current running status
    let running = state.process_status("compaction").await.is_some();

    (
        StatusCode::OK,
        Json(CompactionToggleResponse {
            enabled: req.enabled,
            running,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_request_deserializes_device() {
        let req = serde_json::from_str::<CompactionToggleRequest>(
            r#"{"enabled":true,"device":"cuda:1","port":null,"request_timeout_ms":null}"#,
        );
        assert!(req.is_ok(), "Expected Ok, got: {:?}", req.err());
        let req = req.unwrap();
        assert!(
            matches!(req.device, Some(CompactionDevice::CudaDevice(1))),
            "Expected CudaDevice(1), got {:?}",
            req.device
        );
    }

    #[test]
    fn test_compaction_request_rejects_invalid_device() {
        let req = serde_json::from_str::<CompactionToggleRequest>(
            r#"{"enabled":true,"device":"tpu","port":null,"request_timeout_ms":null}"#,
        );
        assert!(
            req.is_err(),
            "Expected Err for invalid device 'tpu', got: {:?}",
            req.ok()
        );
    }

    #[test]
    fn test_compaction_request_device_optional() {
        let req = serde_json::from_str::<CompactionToggleRequest>(
            r#"{"enabled":true,"port":null,"request_timeout_ms":null}"#,
        );
        assert!(req.is_ok(), "Expected Ok, got: {:?}", req.err());
        let req = req.unwrap();
        assert!(req.device.is_none(), "Expected device to be None");
    }
}
