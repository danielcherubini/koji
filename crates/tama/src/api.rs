use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::api::error::{error_body, error_response, error_response_simple};
use crate::types::config::StructuredConfigBody;
use crate::web_types::WebState;
use tama_core::logstore::{apply_reload, build_log_filter, LogFilterError};
use tama_core::proxy::tama_handlers::OkResponse;
use tama_core::proxy::ProxyState;

pub mod aliases;
pub mod backup;
pub mod benchmarks;
pub mod error;
pub mod field_update;
pub mod helpers;
pub mod hf;
pub mod installations;
pub mod middleware;
pub mod models;
pub mod openapi;
pub mod providers;
pub mod pulls;
pub mod repo_pulls;
pub mod self_update;
pub mod sse;
pub mod tamads;
pub mod updates;

// Re-export for backward compatibility
pub use models::*;

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
/// (Bundles write-lock + Langfuse refresh — see `ProxyState::replace_config`.)
async fn sync_proxy_config(state: &Arc<ProxyState>, new_config: tama_core::config::Config) {
    state.replace_config(new_config).await;
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

/// POST /api/config/structured — accept JSON Config, persist to Postgres DB.
pub async fn save_structured_config(
    State(state): State<Arc<ProxyState>>,
    Json(body): Json<StructuredConfigBody>,
) -> impl IntoResponse {
    let pool = state.db_pool();

    // Convert mirror types back to tama_core::Config
    let new_config: tama_core::config::Config = body.into();

    // Persist to Postgres DB (plan-190 Task 3: async pool-based save)
    match new_config.clone().save(&pool).await {
        Ok(()) => {
            // Sync proxy config for hot-reload
            sync_proxy_config(&state, new_config).await;
            Json(OkResponse::OK).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

// ── Shared helpers (used by both model and non-model endpoints) ──────────────

/// Load config from the Postgres pool (plan-190 Task 3), falling back to
/// the in-memory snapshot when no pool is attached (test-only; production
/// `main.rs` always creates the pool). Returns (config, config_dir).
/// Prefer db_dir (set at startup to Config::config_dir()) to ensure we
/// always resolve the correct config directory. Fall back to the system
/// default when db_dir is None (e.g. in tests that create ProxyState
/// without a db_dir).
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
    let pool = proxy_state.db_pool();
    let cfg = tama_core::config::Config::load_from_pool(&pool)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_body(format!("Failed to load config: {}", e).as_str(), None),
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
    let lifecycle = match patch.lifecycle {
        Some(p) => merge_lifecycle(existing.lifecycle, p),
        None => existing.lifecycle,
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
    let langfuse = match patch.langfuse {
        Some(p) => merge_langfuse(existing.langfuse, p),
        None => existing.langfuse,
    };

    crate::types::config::Config {
        general,
        backends: existing.backends,
        lifecycle,
        sampling_templates,
        proxy,
        compaction,
        langfuse,
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
        log_directives: patch.log_directives.or(existing.log_directives),
        log_retention_days: patch
            .log_retention_days
            .unwrap_or(existing.log_retention_days),
        log_retention_rows: patch
            .log_retention_rows
            .unwrap_or(existing.log_retention_rows),
        log_retention_max_mb: patch
            .log_retention_max_mb
            .unwrap_or(existing.log_retention_max_mb),
    }
}

fn merge_lifecycle(
    existing: crate::types::config::Lifecycle,
    patch: crate::types::config::LifecyclePatch,
) -> crate::types::config::Lifecycle {
    crate::types::config::Lifecycle {
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
        pull_queue_poll_interval_secs: patch
            .pull_queue_poll_interval_secs
            .unwrap_or(existing.pull_queue_poll_interval_secs),
        max_loaded_models: patch
            .max_loaded_models
            .unwrap_or(existing.max_loaded_models),
        authenticator_url: patch.authenticator_url.or(existing.authenticator_url),
        authenticator_skip_paths: patch
            .authenticator_skip_paths
            .unwrap_or(existing.authenticator_skip_paths),
        oauth2,
        api_keys_enabled: patch.api_keys_enabled.unwrap_or(existing.api_keys_enabled),
        pull_backend: match &patch.pull_backend {
            Some(v) => v.clone(),
            None => existing.pull_backend,
        },
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

fn merge_langfuse(
    existing: crate::types::config::LangfuseConfig,
    patch: crate::types::config::LangfuseConfigPatch,
) -> crate::types::config::LangfuseConfig {
    crate::types::config::LangfuseConfig {
        enabled: patch.enabled.unwrap_or(existing.enabled),
        public_key: patch.public_key.unwrap_or(existing.public_key),
        secret_key: patch.secret_key.unwrap_or(existing.secret_key),
        host: patch.host.unwrap_or(existing.host),
        environment: patch.environment.unwrap_or(existing.environment),
        capture_input: patch.capture_input.unwrap_or(existing.capture_input),
        capture_output: patch.capture_output.unwrap_or(existing.capture_output),
        capture_streaming: patch
            .capture_streaming
            .unwrap_or(existing.capture_streaming),
        telemetry_max_bytes: patch
            .telemetry_max_bytes
            .unwrap_or(existing.telemetry_max_bytes),
        electricity_price_per_kwh: patch
            .electricity_price_per_kwh
            .unwrap_or(existing.electricity_price_per_kwh),
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

/// Validate the merged general log config at the API boundary, BEFORE
/// anything is persisted.
///
/// A bad directive persisted would brick the filter at the next boot, so
/// this runs the exact same builder the proxy startup and `tama admin`
/// use (`tama_core::logstore::build_log_filter`): validate-and-apply can
/// never disagree with boot behavior. `build_log_filter` reads the
/// `RUST_LOG` env var internally and the durable `log_directives` win
/// over it for the same target (documented in `docs/api/config.md`).
pub fn validate_log_config(general: &tama_core::config::General) -> Result<(), LogFilterError> {
    let directives = general.log_directives.as_deref().unwrap_or_default();
    build_log_filter(&general.log_level, directives).map(|_| ())
}

/// Persist a merged config and — when `log_level` or `log_directives`
/// changed — live-apply the new filter to the running subscriber (no
/// restart). Caller must have run [`validate_log_config`] first.
///
/// Returns the number of active directives in the reloaded filter (1 base
/// level directive + every env/config directive); `0` when the log config
/// did not change or no live filter is wired in this state (tests — the
/// persisted values then take effect at next boot). A post-validation
/// [`LogFilterError`] is mapped to 500: validation ran the same builder
/// moments earlier, so a failure now is a logic bug.
async fn persist_and_apply_log_config(
    state: &Arc<ProxyState>,
    web_state: &WebState,
    existing: &tama_core::config::Config,
    merged: &tama_core::config::Config,
) -> Result<usize, (StatusCode, String)> {
    // (2) Persist to Postgres DB (plan-190 Task 3: async pool-based save).
    merged
        .save(&state.db_pool())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let log_changed = merged.general.log_level != existing.general.log_level
        || merged.general.log_directives != existing.general.log_directives;
    if !log_changed {
        return Ok(0);
    }

    // (3) Live-apply the new filter through the reload handle.
    match &web_state.log_filter {
        Some(handle) => apply_reload(
            handle,
            &merged.general.log_level,
            merged.general.log_directives.as_deref().unwrap_or_default(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        None => {
            // Not wired (tests): persist only — the new level takes effect
            // at next boot. Never an error.
            tracing::debug!("log filter not wired in this state; persisted level applies at boot");
            Ok(0)
        }
    }
}

/// PATCH /tama/v1/config/structured — surgical partial update.
///
/// Loads existing config from DB, deep-merges the patch, validates the
/// merged log config at the boundary (400 on a bad directive, nothing
/// persisted), persists back to DB, live-applies the filter when the log
/// level or directives changed, and syncs the proxy's in-memory config
/// for hot-reload.
pub async fn patch_structured_config(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Json(body): Json<crate::types::config::ConfigPatchBody>,
) -> impl IntoResponse {
    // Load existing config from DB
    let (existing_core, _config_dir) = match load_config_from_state(&state).await {
        Ok(result) => result,
        Err((status, err)) => return (status, Json(err)).into_response(),
    };

    // Convert core Config to mirror types
    let existing_mirror: crate::types::config::Config = existing_core.clone().into();

    // Deep-merge the patch
    let merged_mirror = merge_config_patch(existing_mirror, body);

    // Convert merged mirror Config back to core
    let merged_core: tama_core::config::Config = merged_mirror.into();

    // (1) Validate the log config BEFORE persisting: an invalid directive
    // must 400 with the DB row untouched.
    if let Err(e) = validate_log_config(&merged_core.general) {
        return error_response_simple(StatusCode::BAD_REQUEST, e.to_string());
    }

    // (2) + (3) Persist, then live-apply the filter on a log change.
    match persist_and_apply_log_config(&state, &web_state, &existing_core, &merged_core).await {
        Ok(active_directives) => {
            if active_directives > 0 {
                tracing::info!(
                    active_directives,
                    log_level = merged_core.general.log_level.as_str(),
                    "log filter reloaded live"
                );
            }
            // Sync proxy config for hot-reload
            sync_proxy_config(&state, merged_core).await;
            Json(OkResponse::OK).into_response()
        }
        Err((status, message)) => error_response_simple(status, message),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{
        CompactionConfig, Config, General, LangfuseConfig, Lifecycle, OAuth2Config, ProxyConfig,
        SamplingParams,
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
                log_directives: None,
                log_retention_days: 7,
                log_retention_rows: 50_000,
                log_retention_max_mb: 256,
            },
            backends: std::collections::BTreeMap::new(),
            lifecycle: Lifecycle {
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
                pull_queue_poll_interval_secs: 2,
                max_loaded_models: 1,
                authenticator_url: None,
                authenticator_skip_paths: Vec::new(),
                oauth2: OAuth2Config::default(),
                api_keys_enabled: false,
                pull_backend: None,
            },
            compaction: CompactionConfig {
                enabled: false,
                server_path: None,
                device: CoreCompactionDevice::Cpu,
                port: None,
                request_timeout_ms: 30000,
            },
            langfuse: LangfuseConfig::default(),
        }
    }

    /// PATCH with all-None body preserves entire config (no-op).
    #[test]
    fn test_merge_config_patch_all_none_preserves_all() {
        let existing = sample_config();
        let patch = crate::types::config::ConfigPatchBody {
            general: None,
            lifecycle: None,
            sampling_templates: None,
            proxy: None,
            compaction: None,
            langfuse: None,
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
            merged.lifecycle.restart_policy,
            existing.lifecycle.restart_policy
        );
        assert_eq!(
            merged.lifecycle.max_restarts,
            existing.lifecycle.max_restarts
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
            lifecycle: None,
            sampling_templates: None,
            proxy: Some(crate::types::config::ProxyConfigPatch {
                port: Some(8080),
                ..Default::default()
            }),
            compaction: None,
            langfuse: None,
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
            lifecycle: None,
            sampling_templates: None,
            proxy: Some(crate::types::config::ProxyConfigPatch {
                oauth2: Some(crate::types::config::OAuth2ConfigPatch {
                    client_id: Some("new-client-id".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            compaction: None,
            langfuse: None,
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
            lifecycle: None,
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
            langfuse: None,
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

    // The save-response drift-guard test lives in tests/config_structured_test.rs
    // (needs a Postgres pool; plan-190 Task 3).

    // ── Log config: validate + live-apply (plan-195 task 3) ──────────

    use crate::types::config::GeneralPatch;
    use crate::web_types::WebState;
    use tama_core::config::LogLevel;
    use tracing_subscriber::layer::SubscriberExt;

    /// Building the web state for the PATCH handler tests: real Postgres
    /// pool from the schema guard, log runtime unwired (except where a
    /// live filter handle is the point of the test).
    fn web_state_for_test(pool: &std::sync::Arc<sqlx::PgPool>) -> WebState {
        WebState {
            jobs: None,
            capabilities: None,
            update_checker: std::sync::Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            db_pool: pool.clone(),
            log_filter: None,
            log_status: None,
            log_events_tx: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            log_read: None,
            log_tail: None,
        }
    }

    /// Validation rejects a directive-looking string that fails to parse
    /// — and names the offending literal — while empty and bare-word
    /// strings (not directives — same target-only rule as boot) are ok.
    #[test]
    fn test_validate_log_config() {
        let mut general = tama_core::config::General::default();
        assert!(validate_log_config(&general).is_ok(), "no directives");

        general.log_directives = Some("".to_string());
        assert!(validate_log_config(&general).is_ok(), "empty string");

        general.log_directives = Some("not a directive at all".to_string());
        assert!(
            validate_log_config(&general).is_ok(),
            "no '=' means 'not a directive' (skipped, per the boot rule)"
        );

        general.log_directives = Some("tama_core=debug,tama=warn".to_string());
        assert!(validate_log_config(&general).is_ok(), "valid directives");

        general.log_directives = Some("probe_target=not-a-level:-".to_string());
        let err = validate_log_config(&general).expect_err("invalid directive must be Err");
        assert!(
            err.to_string().contains("probe_target=not-a-level:-"),
            "error must carry the offending literal, got: {err}"
        );
    }

    /// PATCH with an invalid `log_directives` is a 400 at the boundary and
    /// the app_general row is unchanged (asserted via a second read).
    #[tokio::test]
    async fn test_patch_structured_config_invalid_directives_400_and_no_persist() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = std::sync::Arc::new(guard.pool.clone());
        let config_dir = tempfile::tempdir().expect("config dir");
        let config = tama_core::config::Config::load_from_pool(&pool)
            .await
            .expect("load seeded config");
        let state = std::sync::Arc::new(tama_core::proxy::ProxyState::new(
            config,
            Some(config_dir.path().to_path_buf()),
            pool.clone(),
        ));

        let before = tama_core::db::queries::get_general(&pool)
            .await
            .expect("pre-patch row");

        let body = crate::types::config::ConfigPatchBody {
            general: Some(GeneralPatch {
                log_level: Some(LogLevel::Debug),
                log_directives: Some("probe_target=not-a-level:-".to_string()),
                ..Default::default()
            }),
            lifecycle: None,
            sampling_templates: None,
            proxy: None,
            compaction: None,
            langfuse: None,
        };

        let web_state = web_state_for_test(&pool);
        let response = patch_structured_config(
            axum::extract::State(state.clone()),
            axum::extract::Extension(web_state.clone()),
            Json(body),
        )
        .await
        .into_response();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "invalid directive must 400 and persist nothing"
        );

        let after = tama_core::db::queries::get_general(&pool)
            .await
            .expect("post-patch row");
        assert_eq!(
            before, after,
            "the app_general row must be untouched on a 400"
        );

        guard.finish().await;
    }

    /// PATCH with valid log config persists the new values AND the live
    /// filter is reloaded through the `reload::Handle` — the apply returns
    /// the directive count (1 base-level + the config directive), no
    /// restart.
    #[tokio::test]
    async fn test_patch_structured_config_valid_directives_persist_and_apply_live() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = std::sync::Arc::new(guard.pool.clone());
        let config_dir = tempfile::tempdir().expect("config dir");
        let config = tama_core::config::Config::load_from_pool(&pool)
            .await
            .expect("load seeded config");
        let state = std::sync::Arc::new(tama_core::proxy::ProxyState::new(
            config,
            Some(config_dir.path().to_path_buf()),
            pool.clone(),
        ));

        // A LIVE filter the way main.rs wires it: reload::Layer + the
        // subscriber holding that layer (must stay in scope for the test).
        let (filter, handle) = tracing_subscriber::reload::Layer::new(
            tama_core::logstore::build_log_filter(&LogLevel::Info, "").expect("initial filter"),
        );
        let _subscriber = tracing_subscriber::registry().with(filter);
        let mut web_state = web_state_for_test(&pool);
        web_state.log_filter = Some(handle);

        // Handler: persist + live-apply.
        let body = crate::types::config::ConfigPatchBody {
            general: Some(GeneralPatch {
                log_level: Some(LogLevel::Debug),
                log_directives: Some("probe_target=trace".to_string()),
                ..Default::default()
            }),
            lifecycle: None,
            sampling_templates: None,
            proxy: None,
            compaction: None,
            langfuse: None,
        };
        let response = patch_structured_config(
            axum::extract::State(state.clone()),
            axum::extract::Extension(web_state.clone()),
            Json(body),
        )
        .await
        .into_response();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "valid log config persists"
        );

        // The row carries the new values ...
        let general = tama_core::db::queries::get_general(&pool)
            .await
            .expect("post-patch row")
            .expect("row exists");
        assert_eq!(general.log_level, "debug");
        assert_eq!(
            general.log_directives,
            Some("probe_target=trace".to_string())
        );

        // ... and the apply path itself returns Ok(directive count):
        // 1 base-level directive + 1 config directive. (The persisted
        // config is already debug + directives, so `before` is the
        // simulated pre-patch state — a genuine level/directive change.)
        let after_cfg = tama_core::config::Config::load_from_pool(&pool)
            .await
            .expect("persisted config");
        let mut before_cfg = after_cfg.clone();
        before_cfg.general.log_level = LogLevel::Info;
        before_cfg.general.log_directives = None;
        let count = persist_and_apply_log_config(&state, &web_state, &before_cfg, &after_cfg)
            .await
            .expect("post-validation apply must succeed");
        assert_eq!(count, 2, "1 base-level + 1 config directive");

        guard.finish().await;
    }
}
