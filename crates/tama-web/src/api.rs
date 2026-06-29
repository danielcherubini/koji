use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use tama_core::proxy::ProxyState;

pub mod aliases;
pub mod backends;
pub mod backup;
pub mod benchmarks;
pub mod downloads;
pub mod hf;
pub mod logs;
pub mod middleware;
pub mod models;
pub mod openapi;
pub mod self_update;
pub mod updates;

// Re-export for backward compatibility
pub use models::*;

/// Query parameters for GET /api/logs
#[derive(serde::Deserialize)]
pub struct LogsQuery {
    /// Number of lines to return (default: 200)
    #[serde(default = "default_lines")]
    pub lines: usize,
}
fn default_lines() -> usize {
    200
}

pub async fn get_logs(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let dir = match state.config.read().await.logs_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    let log_path = dir.join("tama.log");
    // Use spawn_blocking for synchronous file I/O to avoid blocking the Tokio runtime.
    let log_path_clone = log_path.clone();
    let n = query.lines;
    let lines = tokio::task::spawn_blocking(move || {
        tama_core::logging::tail_lines(&log_path_clone, n).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    Json(serde_json::json!({ "lines": lines })).into_response()
}

pub async fn get_config(State(_state): State<Arc<ProxyState>>) -> impl IntoResponse {
    (
        StatusCode::GONE,
        Json(serde_json::json!({"error": "TOML config is no longer used. Use GET /tama/v1/config/structured instead."})),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct ConfigBody {
    pub content: String,
}

/// Update the proxy's live in-memory config after a successful disk save.
async fn sync_proxy_config(state: &ProxyState, new_config: tama_core::config::Config) {
    let mut config = state.config.write().await;
    *config = new_config;
}

/// Trigger the proxy to reload its model registry from the database.
async fn trigger_proxy_reload(state: &ProxyState) -> Result<(), (StatusCode, serde_json::Value)> {
    state.reload_model_configs().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": format!("Failed to reload model configs: {}", e)}),
        )
    })?;
    // Aliases are nice-to-have; log a warning but don't fail the whole operation.
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!(error = %e, "Failed to reload aliases");
    }
    Ok(())
}

/// Body for structured config save.
///
/// Note: `models` is intentionally excluded — model configs are stored in the
/// SQLite database and managed through the `/tama/v1/models/:id` CRUD endpoints.
/// Only global config sections (general, backends, supervisor, proxy, etc.) are
/// persisted to the SQLite database through this endpoint.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StructuredConfigBody {
    pub general: crate::types::config::General,
    #[serde(default)]
    pub backends: std::collections::BTreeMap<String, crate::types::config::BackendConfig>,
    #[serde(default)]
    pub supervisor: crate::types::config::Supervisor,
    #[serde(default)]
    pub sampling_templates:
        std::collections::BTreeMap<String, crate::types::config::SamplingParams>,
    #[serde(default)]
    pub proxy: crate::types::config::ProxyConfig,
    #[serde(default)]
    pub compaction: crate::types::config::CompactionConfig,
}

pub async fn save_config(
    State(_state): State<Arc<ProxyState>>,
    _body: Json<ConfigBody>,
) -> impl IntoResponse {
    (
        StatusCode::GONE,
        Json(serde_json::json!({"error": "TOML config is no longer used. Use POST /tama/v1/config/structured instead."})),
    )
        .into_response()
}

// ── Structured Config API (JSON-based for WASM) ─────────────────────────────────

/// GET /api/config/structured — returns full Config as JSON.
pub async fn get_structured_config(State(_state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Load config from SQLite DB
    let cfg = match tokio::task::spawn_blocking(tama_core::config::Config::load).await {
        Ok(Ok(cfg)) => cfg,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Convert to mirror types for JSON serialization
    let structured: crate::types::config::Config = cfg.into();

    Json(structured).into_response()
}

/// POST /api/config/structured — accept JSON Config, persist as TOML.
pub async fn save_structured_config(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<StructuredConfigBody>,
) -> impl IntoResponse {
    // Convert mirror types back to tama_core::Config
    let new_config: tama_core::config::Config = body.into();

    // Persist to SQLite DB (spawn_blocking for synchronous DB write)
    let new_config_for_save = new_config.clone();
    match tokio::task::spawn_blocking(move || new_config_for_save.save()).await {
        Ok(Ok(_)) => {
            // Sync proxy config for hot-reload
            sync_proxy_config(&state, new_config).await;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

// ── Shared helpers (used by both model and non-model endpoints) ──────────────

/// Load config from the config directory derived from ProxyState.
/// Returns (config, config_dir) on success.
/// Prefer db_dir (set at startup to Config::config_dir()) to ensure we
/// always open the correct database. Fall back to the system default
/// when db_dir is None (e.g. in tests that create ProxyState without a db_dir).
async fn load_config_from_state(
    state: &ProxyState,
) -> Result<(tama_core::config::Config, std::path::PathBuf), (StatusCode, serde_json::Value)> {
    let config_dir = state
        .db_dir
        .clone()
        .or_else(|| tama_core::config::Config::config_dir().ok())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                serde_json::json!({"error": "config directory not configured"}),
            )
        })?;
    let db_path = config_dir.join("tama.db");
    let cfg = tokio::task::spawn_blocking(move || {
        tama_core::config::Config::from_db(&db_path)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}),
        )
    })?;
    Ok((cfg, config_dir))
}
