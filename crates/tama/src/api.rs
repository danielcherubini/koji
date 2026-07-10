use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use crate::api::error::{error_body, error_response, error_response_simple};
use crate::types::config::StructuredConfigBody;
use tama_core::proxy::ProxyState;

pub mod aliases;
pub mod backends;
pub mod backup;
pub mod benchmarks;
pub mod downloads;
pub mod error;
pub mod helpers;
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
    let dir = match state.config().read().await.logs_dir() {
        Ok(d) => d,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
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
    error_response_simple(
        StatusCode::GONE,
        "TOML config is no longer used. Use GET /tama/v1/config/structured instead.",
    )
}

#[derive(serde::Deserialize)]
pub struct ConfigBody {
    pub content: String,
}

/// Update the proxy's live in-memory config after a successful disk save.
async fn sync_proxy_config(state: &Arc<ProxyState>, new_config: tama_core::config::Config) {
    let mut config = state.config().write().await;
    *config = new_config;
}

/// Trigger the proxy to reload its model registry from the database.
async fn trigger_proxy_reload(
    state: &Arc<ProxyState>,
) -> Result<(), (StatusCode, serde_json::Value)> {
    state.reload_model_configs().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_body(format!("Failed to reload model configs: {}", e), None),
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
pub async fn save_config(
    State(_state): State<Arc<ProxyState>>,
    _body: Json<ConfigBody>,
) -> impl IntoResponse {
    error_response_simple(
        StatusCode::GONE,
        "TOML config is no longer used. Use POST /tama/v1/config/structured instead.",
    )
}

// ── Structured Config API (JSON-based for WASM) ─────────────────────────────────

/// GET /api/config/structured — returns full Config as JSON.
pub async fn get_structured_config(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let (cfg, _) = match load_config_from_state(&state).await {
        Ok(result) => result,
        Err((status, err)) => return (status, Json(err)).into_response(),
    };

    // Convert to mirror types for JSON serialization
    let structured: crate::types::config::Config = cfg.into();

    Json(structured).into_response()
}

/// POST /api/config/structured — accept JSON Config, persist to SQLite DB.
pub async fn save_structured_config(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<StructuredConfigBody>,
) -> impl IntoResponse {
    // Resolve the correct DB path from state
    let config_dir = match state
        .db_dir()
        .clone()
        .or_else(|| tama_core::config::Config::config_dir().ok())
    {
        Some(d) => d,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                "config directory not configured",
                Some("NotFoundError"),
            )
        }
    };
    let db_path = config_dir.join("tama.db");

    // Convert mirror types back to tama_core::Config
    let new_config: tama_core::config::Config = body.into();

    // Persist to SQLite DB (spawn_blocking for synchronous DB write)
    let db_path_for_save = db_path.clone();
    let new_config_for_save = new_config.clone();
    match tokio::task::spawn_blocking(move || new_config_for_save.to_db(&db_path_for_save)).await {
        Ok(Ok(_)) => {
            // Sync proxy config for hot-reload
            sync_proxy_config(&state, new_config).await;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

// ── Shared helpers (used by both model and non-model endpoints) ──────────────

/// Load config from the config directory derived from Arc<ProxyState>.
/// Returns (config, config_dir) on success.
/// Prefer db_dir (set at startup to Config::config_dir()) to ensure we
/// always open the correct database. Fall back to the system default
/// when db_dir is None (e.g. in tests that create ProxyState without a db_dir).
async fn load_config_from_state(
    proxy_state: &Arc<ProxyState>,
) -> Result<(tama_core::config::Config, std::path::PathBuf), (StatusCode, serde_json::Value)> {
    let config_dir = proxy_state
        .db_dir()
        .clone()
        .or_else(|| tama_core::config::Config::config_dir().ok())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                error_body("config directory not configured", Some("NotFoundError")),
            )
        })?;
    let db_path = config_dir.join("tama.db");
    let cfg = tokio::task::spawn_blocking(move || tama_core::config::Config::from_db(&db_path))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_body(e.to_string(), None),
            )
        })?
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_body(e.to_string(), None),
            )
        })?;
    Ok((cfg, config_dir))
}

// ── PATCH /tama/v1/config/structured ────────────────────────────────────────

/// Deep-merge a `ConfigPatchBody` into a mirror `Config`.
///
/// For each section: if `patch.section.is_some()`, merge field-by-field using
/// `.or()` / `unwrap_or()`; if `None`, keep the existing section entirely.
///
/// For `sampling_templates`: upsert — iterate the patch map, for each key:
/// if key exists in existing, merge field-by-field (each `SamplingParams`
/// field is already `Option<T>` so `.or()` works); if key doesn't exist,
/// insert new entry. Keys absent from patch are preserved.
///
/// For nested `oauth2` within `proxy`: same deep-merge pattern — if
/// `patch.oauth2.is_some()`, merge OAuth2 fields field-by-field; if `None`,
/// keep existing oauth2.
pub fn merge_config_patch(
    existing: crate::types::config::Config,
    patch: crate::types::config::ConfigPatchBody,
) -> crate::types::config::Config {
    let general = match patch.general {
        Some(p) => merge_general(existing.general, p),
        None => existing.general,
    };
    let supervisor = match patch.supervisor {
        Some(p) => merge_supervisor(existing.supervisor, p),
        None => existing.supervisor,
    };
    let sampling_templates =
        merge_sampling_templates(existing.sampling_templates, patch.sampling_templates);
    let proxy = match patch.proxy {
        Some(p) => merge_proxy(existing.proxy, p),
        None => existing.proxy,
    };
    let compaction = match patch.compaction {
        Some(p) => merge_compaction(existing.compaction, p),
        None => existing.compaction,
    };

    crate::types::config::Config {
        general,
        backends: existing.backends,
        supervisor,
        sampling_templates,
        proxy,
        compaction,
    }
}

fn merge_general(
    existing: crate::types::config::General,
    patch: crate::types::config::GeneralPatch,
) -> crate::types::config::General {
    crate::types::config::General {
        log_level: patch.log_level.unwrap_or(existing.log_level),
        models_dir: patch.models_dir.or(existing.models_dir),
        logs_dir: patch.logs_dir.or(existing.logs_dir),
        hf_token: patch.hf_token.or(existing.hf_token),
        update_check_interval: patch
            .update_check_interval
            .unwrap_or(existing.update_check_interval),
    }
}

fn merge_supervisor(
    existing: crate::types::config::Supervisor,
    patch: crate::types::config::SupervisorPatch,
) -> crate::types::config::Supervisor {
    crate::types::config::Supervisor {
        restart_policy: patch.restart_policy.unwrap_or(existing.restart_policy),
        max_restarts: patch.max_restarts.unwrap_or(existing.max_restarts),
        restart_delay_ms: patch.restart_delay_ms.unwrap_or(existing.restart_delay_ms),
        health_check_interval_ms: patch
            .health_check_interval_ms
            .unwrap_or(existing.health_check_interval_ms),
        health_check_timeout_ms: patch
            .health_check_timeout_ms
            .unwrap_or(existing.health_check_timeout_ms),
        health_check_retries: patch
            .health_check_retries
            .unwrap_or(existing.health_check_retries),
    }
}

fn merge_proxy(
    existing: crate::types::config::ProxyConfig,
    patch: crate::types::config::ProxyConfigPatch,
) -> crate::types::config::ProxyConfig {
    let oauth2 = match (existing.oauth2, patch.oauth2) {
        (e, Some(p)) => merge_oauth2(e, p),
        (e, None) => e,
    };
    crate::types::config::ProxyConfig {
        host: patch.host.unwrap_or(existing.host),
        port: patch.port.unwrap_or(existing.port),
        auto_unload: patch.auto_unload.unwrap_or(existing.auto_unload),
        idle_timeout_secs: patch
            .idle_timeout_secs
            .unwrap_or(existing.idle_timeout_secs),
        startup_timeout_secs: patch
            .startup_timeout_secs
            .unwrap_or(existing.startup_timeout_secs),
        circuit_breaker_threshold: patch
            .circuit_breaker_threshold
            .unwrap_or(existing.circuit_breaker_threshold),
        circuit_breaker_cooldown_seconds: patch
            .circuit_breaker_cooldown_seconds
            .unwrap_or(existing.circuit_breaker_cooldown_seconds),
        metrics_retention_secs: patch
            .metrics_retention_secs
            .unwrap_or(existing.metrics_retention_secs),
        download_queue_poll_interval_secs: patch
            .download_queue_poll_interval_secs
            .unwrap_or(existing.download_queue_poll_interval_secs),
        max_loaded_models: patch
            .max_loaded_models
            .unwrap_or(existing.max_loaded_models),
        authenticator_url: patch.authenticator_url.or(existing.authenticator_url),
        authenticator_skip_paths: patch
            .authenticator_skip_paths
            .unwrap_or(existing.authenticator_skip_paths),
        oauth2,
        api_keys_enabled: patch.api_keys_enabled.unwrap_or(existing.api_keys_enabled),
    }
}

fn merge_oauth2(
    existing: crate::types::config::OAuth2Config,
    patch: crate::types::config::OAuth2ConfigPatch,
) -> crate::types::config::OAuth2Config {
    crate::types::config::OAuth2Config {
        enabled: patch.enabled.unwrap_or(existing.enabled),
        client_id: patch.client_id.unwrap_or(existing.client_id),
        client_secret: patch.client_secret.unwrap_or(existing.client_secret),
        authorize_url: patch.authorize_url.unwrap_or(existing.authorize_url),
        token_url: patch.token_url.unwrap_or(existing.token_url),
        userinfo_url: patch.userinfo_url.or(existing.userinfo_url),
        logout_url: patch.logout_url.or(existing.logout_url),
        redirect_uri: patch.redirect_uri.unwrap_or(existing.redirect_uri),
        scopes: patch.scopes.unwrap_or(existing.scopes),
        session_ttl_secs: patch.session_ttl_secs.unwrap_or(existing.session_ttl_secs),
    }
}

fn merge_compaction(
    existing: crate::types::config::CompactionConfig,
    patch: crate::types::config::CompactionConfigPatch,
) -> crate::types::config::CompactionConfig {
    crate::types::config::CompactionConfig {
        enabled: patch.enabled.unwrap_or(existing.enabled),
        server_path: patch.server_path.or(existing.server_path),
        device: patch.device.unwrap_or(existing.device),
        port: patch.port.or(existing.port),
        request_timeout_ms: patch
            .request_timeout_ms
            .unwrap_or(existing.request_timeout_ms),
    }
}

fn merge_sampling_templates(
    mut existing: std::collections::BTreeMap<String, crate::types::config::SamplingParams>,
    patch: Option<std::collections::BTreeMap<String, crate::types::config::SamplingParams>>,
) -> std::collections::BTreeMap<String, crate::types::config::SamplingParams> {
    if let Some(patch_map) = patch {
        for (key, patch_params) in patch_map {
            existing
                .entry(key)
                .and_modify(|e| {
                    *e = crate::types::config::SamplingParams {
                        temperature: patch_params.temperature.or(e.temperature),
                        top_k: patch_params.top_k.or(e.top_k),
                        top_p: patch_params.top_p.or(e.top_p),
                        min_p: patch_params.min_p.or(e.min_p),
                        presence_penalty: patch_params.presence_penalty.or(e.presence_penalty),
                        frequency_penalty: patch_params.frequency_penalty.or(e.frequency_penalty),
                        repeat_penalty: patch_params.repeat_penalty.or(e.repeat_penalty),
                    };
                })
                .or_insert(patch_params);
        }
    }
    existing
}

/// PATCH /tama/v1/config/structured — surgical partial update.
///
/// Loads existing config from DB, deep-merges the patch, persists back to DB,
/// and syncs the proxy's in-memory config for hot-reload.
pub async fn patch_structured_config(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<crate::types::config::ConfigPatchBody>,
) -> impl IntoResponse {
    // Load existing config from DB
    let (existing_core, config_dir) = match load_config_from_state(&state).await {
        Ok(result) => result,
        Err((status, err)) => return (status, Json(err)).into_response(),
    };

    // Convert core Config to mirror types
    let existing_mirror: crate::types::config::Config = existing_core.into();

    // Deep-merge the patch
    let merged_mirror = merge_config_patch(existing_mirror, body);

    // Convert merged mirror Config back to core
    let merged_core: tama_core::config::Config = merged_mirror.into();

    // Persist to SQLite DB (spawn_blocking for synchronous DB write)
    let db_path = config_dir.join("tama.db");
    let merged_core_for_save = merged_core.clone();
    match tokio::task::spawn_blocking(move || merged_core_for_save.to_db(&db_path)).await {
        Ok(Ok(_)) => {
            // Sync proxy config for hot-reload
            sync_proxy_config(&state, merged_core).await;
            Json(serde_json::json!({ "ok": true })).into_response()
        }
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{
        CompactionConfig, Config, General, OAuth2Config, ProxyConfig, SamplingParams, Supervisor,
    };
    use tama_core::config::{
        CompactionDevice as CoreCompactionDevice, LogLevel as CoreLogLevel,
        RestartPolicy as CoreRestartPolicy,
    };

    fn sample_config() -> Config {
        Config {
            general: General {
                log_level: CoreLogLevel::Info,
                models_dir: Some("/models".to_string()),
                logs_dir: Some("/logs".to_string()),
                hf_token: None,
                update_check_interval: 12,
            },
            backends: std::collections::BTreeMap::new(),
            supervisor: Supervisor {
                restart_policy: CoreRestartPolicy::Always,
                max_restarts: 10,
                restart_delay_ms: 3000,
                health_check_interval_ms: 5000,
                health_check_timeout_ms: 30000,
                health_check_retries: 3,
            },
            sampling_templates: std::collections::BTreeMap::new(),
            proxy: ProxyConfig {
                host: "0.0.0.0".to_string(),
                port: 11434,
                auto_unload: false,
                idle_timeout_secs: 300,
                startup_timeout_secs: 120,
                circuit_breaker_threshold: 3,
                circuit_breaker_cooldown_seconds: 60,
                metrics_retention_secs: 86400,
                download_queue_poll_interval_secs: 2,
                max_loaded_models: 1,
                authenticator_url: None,
                authenticator_skip_paths: Vec::new(),
                oauth2: OAuth2Config::default(),
                api_keys_enabled: false,
            },
            compaction: CompactionConfig {
                enabled: false,
                server_path: None,
                device: CoreCompactionDevice::Cpu,
                port: None,
                request_timeout_ms: 30000,
            },
        }
    }

    /// PATCH with all-None body preserves entire config (no-op).
    #[test]
    fn test_merge_config_patch_all_none_preserves_all() {
        let existing = sample_config();
        let patch = crate::types::config::ConfigPatchBody {
            general: None,
            supervisor: None,
            sampling_templates: None,
            proxy: None,
            compaction: None,
        };

        let merged = merge_config_patch(existing.clone(), patch);

        assert_eq!(merged.general.log_level, existing.general.log_level);
        assert_eq!(merged.general.models_dir, existing.general.models_dir);
        assert_eq!(merged.general.logs_dir, existing.general.logs_dir);
        assert_eq!(
            merged.general.update_check_interval,
            existing.general.update_check_interval
        );
        assert_eq!(
            merged.supervisor.restart_policy,
            existing.supervisor.restart_policy
        );
        assert_eq!(
            merged.supervisor.max_restarts,
            existing.supervisor.max_restarts
        );
        assert_eq!(merged.proxy.host, existing.proxy.host);
        assert_eq!(merged.proxy.port, existing.proxy.port);
        // Compare oauth2 fields individually (OAuth2Config doesn't derive PartialEq)
        assert_eq!(merged.proxy.oauth2.enabled, existing.proxy.oauth2.enabled);
        assert_eq!(
            merged.proxy.oauth2.client_id,
            existing.proxy.oauth2.client_id
        );
        assert_eq!(merged.compaction.enabled, existing.compaction.enabled);
    }

    /// PATCH proxy.port only changes port, preserves all other proxy fields
    /// including oauth2.*.
    #[test]
    fn test_merge_config_patch_proxy_port_only() {
        let mut existing = sample_config();
        existing.proxy.oauth2.client_id = "existing-client".to_string();
        existing.proxy.oauth2.enabled = true;

        let patch = crate::types::config::ConfigPatchBody {
            general: None,
            supervisor: None,
            sampling_templates: None,
            proxy: Some(crate::types::config::ProxyConfigPatch {
                port: Some(8080),
                ..Default::default()
            }),
            compaction: None,
        };

        let merged = merge_config_patch(existing, patch);

        assert_eq!(merged.proxy.port, 8080);
        assert_eq!(merged.proxy.oauth2.client_id, "existing-client");
        assert!(merged.proxy.oauth2.enabled);
        assert_eq!(merged.proxy.host, "0.0.0.0");
    }

    /// PATCH oauth2.client_id deep-sets only that field.
    #[test]
    fn test_merge_config_patch_oauth2_client_id_deep_set() {
        let existing = sample_config();

        let patch = crate::types::config::ConfigPatchBody {
            general: None,
            supervisor: None,
            sampling_templates: None,
            proxy: Some(crate::types::config::ProxyConfigPatch {
                oauth2: Some(crate::types::config::OAuth2ConfigPatch {
                    client_id: Some("new-client-id".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            compaction: None,
        };

        let merged = merge_config_patch(existing, patch);

        assert_eq!(merged.proxy.oauth2.client_id, "new-client-id");
        // Other oauth2 fields should be defaults (empty strings from Default)
        // because we only set client_id, and other Option<String> fields are None
        assert_eq!(merged.proxy.oauth2.client_secret, "");
        assert_eq!(merged.proxy.host, "0.0.0.0");
    }

    /// PATCH sampling_templates with new key inserts it.
    /// PATCH sampling_templates with existing key merges fields.
    #[test]
    fn test_merge_config_patch_sampling_templates_upsert() {
        let mut existing = sample_config();
        existing.sampling_templates.insert(
            "existing_key".to_string(),
            SamplingParams {
                temperature: Some(0.5),
                top_k: Some(20),
                top_p: Some(0.9),
                min_p: None,
                presence_penalty: None,
                frequency_penalty: None,
                repeat_penalty: None,
            },
        );

        let patch = crate::types::config::ConfigPatchBody {
            general: None,
            supervisor: None,
            sampling_templates: Some({
                let mut map = std::collections::BTreeMap::new();
                // Upsert existing key — merge fields
                map.insert(
                    "existing_key".to_string(),
                    SamplingParams {
                        temperature: Some(0.7), // override
                        top_k: None,            // preserve existing 20
                        top_p: None,            // preserve existing 0.9
                        min_p: Some(0.05),      // new field
                        presence_penalty: None,
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                );
                // Insert new key
                map.insert(
                    "new_key".to_string(),
                    SamplingParams {
                        temperature: Some(1.0),
                        top_k: Some(50),
                        top_p: Some(0.95),
                        min_p: None,
                        presence_penalty: None,
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                );
                map
            }),
            proxy: None,
            compaction: None,
        };

        let merged = merge_config_patch(existing, patch);

        // Existing key merged
        let existing_merged = merged.sampling_templates.get("existing_key").unwrap();
        assert_eq!(existing_merged.temperature, Some(0.7)); // overridden
        assert_eq!(existing_merged.top_k, Some(20)); // preserved
        assert_eq!(existing_merged.top_p, Some(0.9)); // preserved
        assert_eq!(existing_merged.min_p, Some(0.05)); // new

        // New key inserted
        let new_entry = merged.sampling_templates.get("new_key").unwrap();
        assert_eq!(new_entry.temperature, Some(1.0));
        assert_eq!(new_entry.top_k, Some(50));
    }
}
