//! Status, health, and metrics handlers.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use tracing;

use crate::proxy::handlers::metrics::{
    format_backend_metrics, format_system_metrics, format_tama_metrics,
};
use crate::proxy::ProxyState;

/// Typed response for the `/status` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_utilization_pct: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vram: Option<VramStatus>,
    pub auto_unload: bool,
    pub idle_timeout_secs: u64,
    pub metrics: ProxyMetrics,
    pub models: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VramStatus {
    pub used_mib: u64,
    pub total_mib: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProxyMetrics {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub models_loaded: u64,
    pub models_unloaded: u64,
}

/// Returns the current proxy status.
///
/// Builds a JSON status response from the proxy state including backend
/// information and runtime status.
#[axum::debug_handler]
pub async fn handle_status(state: State<Arc<ProxyState>>) -> Json<StatusResponse> {
    let response = state.build_status_response().await;
    // Convert from serde_json::Value to typed StatusResponse
    let result: StatusResponse = serde_json::from_value(response).unwrap_or_else(|e| {
        tracing::error!("Failed to deserialize status response: {}", e);
        StatusResponse {
            cpu_usage_pct: 0.0,
            ram_used_mib: 0,
            ram_total_mib: 0,
            gpu_utilization_pct: None,
            vram: None,
            auto_unload: false,
            idle_timeout_secs: 0,
            metrics: ProxyMetrics {
                total_requests: 0,
                successful_requests: 0,
                failed_requests: 0,
                models_loaded: 0,
                models_unloaded: 0,
            },
            models: BTreeMap::new(),
        }
    });
    Json(result)
}

/// Reloads model configurations from disk.
///
/// Triggers a hot reload of all model configurations. Returns JSON
/// `{ "ok": true }` on success or a 500 error with details on failure.
#[axum::debug_handler]
pub async fn handle_reload_configs(state: State<Arc<ProxyState>>) -> impl IntoResponse {
    match state.reload_model_configs().await {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// Health check endpoint.
///
/// Returns a static JSON response indicating service health.
/// Useful for container orchestration and monitoring systems.
#[axum::debug_handler]
pub async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "tama-proxy"
    }))
}

/// Returns merged proxy and backend metrics in Prometheus exposition format.
///
/// Fetches `/metrics` from all Ready (non-TTS) backends concurrently,
/// injects `{backend="<name>"}` labels, and appends Tama's own proxy
/// metrics prefixed with `tama:`. Returns `text/plain; version=0.0.4`.
#[axum::debug_handler]
pub async fn handle_metrics(state: State<Arc<ProxyState>>) -> Response {
    // Collect Ready non-TTS backends and drop the lock immediately
    let backends: Vec<(String, String)> = {
        let models = state.models.read().await;
        models
            .iter()
            .filter_map(|(name, ms)| {
                if ms.is_ready() && !ms.is_tts_backend() {
                    ms.backend_url().map(|url| (name.clone(), url.to_string()))
                } else {
                    None
                }
            })
            .collect()
    };
    let active_count = backends.len();

    // Fetch metrics from each backend concurrently
    let mut backend_metrics = Vec::new();
    let client = state.client.clone();
    let mut set = tokio::task::JoinSet::new();

    for (backend_name, backend_url) in &backends {
        let client = client.clone();
        let url = format!("{}/metrics", backend_url);
        let name = backend_name.clone();
        set.spawn(async move {
            match client
                .get(&url)
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) => match resp.text().await {
                    Ok(body) => {
                        let lines: Vec<&str> = body.lines().collect();
                        Some(format_backend_metrics(&lines, &name))
                    }
                    Err(e) => {
                        tracing::warn!(
                            backend = %name,
                            error = %e,
                            "Failed to read backend metrics body"
                        );
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        backend = %name,
                        error = %e,
                        "Failed to fetch backend metrics"
                    );
                    None
                }
            }
        });
    }

    while let Some(result) = set.join_next().await {
        match result {
            Ok(Some(metrics)) => backend_metrics.push(metrics),
            Ok(None) => {} // Backend failed, already logged
            Err(e) => {
                tracing::warn!(error = %e, "Backend metrics task panicked");
            }
        }
    }

    // Build final output: backend metrics + system metrics + Tama proxy metrics
    let mut output = String::new();
    for block in backend_metrics {
        output.push_str(&block);
        if !block.ends_with('\n') {
            output.push('\n');
        }
    }
    let sys = state.system_metrics.read().await;
    output.push_str(&format_system_metrics(&sys));
    output.push_str(&format_tama_metrics(&state.metrics, active_count));

    Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(axum::body::Body::from(output))
        .unwrap()
}
