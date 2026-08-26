//! App config database query functions (Postgres, plan-190 Task 3).
//!
//! CRUD operations for the singleton config tables (`app_general`, `app_proxy`,
//! `app_lifecycle`, `app_compaction`, `app_langfuse`) and the multi-row
//! `sampling_templates` table. All functions are async and take a
//! `&PgPool` — the caller (or `main.rs` at startup) owns the pool.
//! Singleton tables store exactly one row with `id = 1`.

use anyhow::{anyhow, Context, Result};
use sqlx::{PgPool, Row};

use crate::config::types::{CompactionDevice, LogLevel, RestartPolicy};

// ── Typed record structs ─────────────────────────────────────────────

/// A row from the `app_general` table.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneralRecord {
    pub log_level: String,
    pub models_dir: Option<String>,
    pub logs_dir: Option<String>,
    pub hf_token: Option<String>,
    pub update_check_interval: u32,
}

/// A row from the `app_proxy` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ProxyRecord {
    pub host: String,
    pub port: u16,
    pub auto_unload: bool,
    pub idle_timeout_secs: u64,
    pub startup_timeout_secs: u64,
    pub circuit_breaker_threshold: u32,
    pub circuit_breaker_cooldown_seconds: u64,
    pub metrics_retention_secs: u64,
    pub pull_queue_poll_interval_secs: u64,
    pub max_loaded_models: u32,
    pub authenticator_url: Option<String>,
    pub authenticator_skip_paths: Vec<String>,
    // OAuth2 fields (added in migration v35)
    pub oauth2_enabled: bool,
    pub oauth2_client_id: String,
    pub oauth2_client_secret: String,
    pub oauth2_authorize_url: String,
    pub oauth2_token_url: String,
    pub oauth2_userinfo_url: Option<String>,
    pub oauth2_logout_url: Option<String>,
    pub oauth2_redirect_uri: String,
    pub oauth2_scopes: Vec<String>,
    pub oauth2_session_ttl_secs: u64,
    pub api_keys_enabled: bool,
    /// Registered tamad connection id that executes queued model pulls
    /// (plan-191 Task 6). `NULL` → no pull host set; pulls fail with the
    /// explicit "no pull host configured" error (ADR-0010).
    pub pull_backend: Option<String>,
}

/// A row from the `app_lifecycle` table.
#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleRecord {
    pub restart_policy: String,
    pub max_restarts: u32,
    pub restart_delay_ms: u64,
    pub health_check_interval_ms: u64,
    pub health_check_timeout_ms: u64,
    pub health_check_retries: u32,
}

/// A row from the `app_compaction` table.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionRecord {
    pub enabled: bool,
    pub server_path: Option<String>,
    pub device: String,
    pub port: Option<u16>,
    pub request_timeout_ms: u64,
}

/// A row from the `app_langfuse` table.
#[derive(Debug, Clone, PartialEq)]
pub struct LangfuseRecord {
    pub enabled: bool,
    pub public_key: String,
    pub secret_key: String,
    pub host: String,
    pub environment: String,
    pub capture_input: bool,
    pub capture_output: bool,
    pub capture_streaming: bool,
    pub telemetry_max_bytes: usize,
    pub electricity_price_per_kwh: f64,
}

/// A row from the `sampling_templates` table.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplingTemplateRecord {
    pub name: String,
    pub temperature: Option<f64>,
    pub top_k: Option<u32>,
    pub top_p: Option<f64>,
    pub min_p: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub repeat_penalty: Option<f64>,
}

/// Insert or replace the general app config row (id=1).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_general(
    pool: &PgPool,
    log_level: &LogLevel,
    models_dir: Option<&str>,
    logs_dir: Option<&str>,
    hf_token: Option<&str>,
    update_check_interval: u32,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO app_general (id, log_level, models_dir, logs_dir, hf_token, update_check_interval)
         VALUES (1, $1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET
             log_level = EXCLUDED.log_level,
             models_dir = EXCLUDED.models_dir,
             logs_dir = EXCLUDED.logs_dir,
             hf_token = EXCLUDED.hf_token,
             update_check_interval = EXCLUDED.update_check_interval",
    )
    .bind(log_level.as_str())
    .bind(models_dir)
    .bind(logs_dir)
    .bind(hf_token)
    .bind(i64::from(update_check_interval))
    .execute(pool)
    .await
    .context("Failed to upsert app_general")?;
    Ok(())
}

/// Get the general app config row. Returns None if no row exists.
pub async fn get_general(pool: &PgPool) -> Result<Option<GeneralRecord>> {
    let row = sqlx::query(
        "SELECT log_level, models_dir, logs_dir, hf_token, update_check_interval
         FROM app_general WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("Failed to read app_general")?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(GeneralRecord {
        log_level: row.get::<String, _>("log_level"),
        models_dir: row.get::<Option<String>, _>("models_dir"),
        logs_dir: row.get::<Option<String>, _>("logs_dir"),
        hf_token: row.get::<Option<String>, _>("hf_token"),
        update_check_interval: row
            .get::<i64, _>("update_check_interval")
            .try_into()
            .context("app_general.update_check_interval out of range")?,
    }))
}

/// Insert or replace the proxy config row (id=1).
/// `authenticator_skip_paths` and `oauth2_scopes` are stored as JSON strings.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_proxy(
    pool: &PgPool,
    host: &str,
    port: u16,
    auto_unload: bool,
    idle_timeout_secs: u64,
    startup_timeout_secs: u64,
    circuit_breaker_threshold: u32,
    circuit_breaker_cooldown_seconds: u64,
    metrics_retention_secs: u64,
    pull_queue_poll_interval_secs: u64,
    max_loaded_models: u32,
    authenticator_url: Option<&str>,
    authenticator_skip_paths: &[String],
    oauth2_enabled: bool,
    oauth2_client_id: &str,
    oauth2_client_secret: &str,
    oauth2_authorize_url: &str,
    oauth2_token_url: &str,
    oauth2_userinfo_url: Option<&str>,
    oauth2_logout_url: Option<&str>,
    oauth2_redirect_uri: &str,
    oauth2_scopes: &[String],
    oauth2_session_ttl_secs: u64,
    api_keys_enabled: bool,
    pull_backend: Option<&str>,
) -> Result<()> {
    let skip_paths_json = serde_json::to_string(authenticator_skip_paths)
        .context("Failed to serialize authenticator_skip_paths to JSON")?;
    let scopes_json = serde_json::to_string(oauth2_scopes)
        .context("Failed to serialize oauth2_scopes to JSON")?;

    sqlx::query(
        "INSERT INTO app_proxy (id, host, port, auto_unload, idle_timeout_secs, startup_timeout_secs,
            circuit_breaker_threshold, circuit_breaker_cooldown_seconds, metrics_retention_secs,
            pull_queue_poll_interval_secs, max_loaded_models, authenticator_url, authenticator_skip_paths,
            oauth2_enabled, oauth2_client_id, oauth2_client_secret, oauth2_authorize_url, oauth2_token_url,
            oauth2_userinfo_url, oauth2_logout_url, oauth2_redirect_uri, oauth2_scopes, oauth2_session_ttl_secs,
            api_keys_enabled, pull_backend)
         VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24)
         ON CONFLICT (id) DO UPDATE SET
             host = EXCLUDED.host,
             port = EXCLUDED.port,
             auto_unload = EXCLUDED.auto_unload,
             idle_timeout_secs = EXCLUDED.idle_timeout_secs,
             startup_timeout_secs = EXCLUDED.startup_timeout_secs,
             circuit_breaker_threshold = EXCLUDED.circuit_breaker_threshold,
             circuit_breaker_cooldown_seconds = EXCLUDED.circuit_breaker_cooldown_seconds,
             metrics_retention_secs = EXCLUDED.metrics_retention_secs,
             pull_queue_poll_interval_secs = EXCLUDED.pull_queue_poll_interval_secs,
             max_loaded_models = EXCLUDED.max_loaded_models,
             authenticator_url = EXCLUDED.authenticator_url,
             authenticator_skip_paths = EXCLUDED.authenticator_skip_paths,
             oauth2_enabled = EXCLUDED.oauth2_enabled,
             oauth2_client_id = EXCLUDED.oauth2_client_id,
             oauth2_client_secret = EXCLUDED.oauth2_client_secret,
             oauth2_authorize_url = EXCLUDED.oauth2_authorize_url,
             oauth2_token_url = EXCLUDED.oauth2_token_url,
             oauth2_userinfo_url = EXCLUDED.oauth2_userinfo_url,
             oauth2_logout_url = EXCLUDED.oauth2_logout_url,
             oauth2_redirect_uri = EXCLUDED.oauth2_redirect_uri,
             oauth2_scopes = EXCLUDED.oauth2_scopes,
             oauth2_session_ttl_secs = EXCLUDED.oauth2_session_ttl_secs,
             api_keys_enabled = EXCLUDED.api_keys_enabled,
             pull_backend = EXCLUDED.pull_backend",
    )
    .bind(host)
    .bind(i64::from(port))
    .bind(auto_unload)
    .bind(idle_timeout_secs as i64)
    .bind(startup_timeout_secs as i64)
    .bind(i64::from(circuit_breaker_threshold))
    .bind(circuit_breaker_cooldown_seconds as i64)
    .bind(metrics_retention_secs as i64)
    .bind(pull_queue_poll_interval_secs as i64)
    .bind(i64::from(max_loaded_models))
    .bind(authenticator_url)
    .bind(skip_paths_json)
    .bind(oauth2_enabled)
    .bind(oauth2_client_id)
    .bind(oauth2_client_secret)
    .bind(oauth2_authorize_url)
    .bind(oauth2_token_url)
    .bind(oauth2_userinfo_url)
    .bind(oauth2_logout_url)
    .bind(oauth2_redirect_uri)
    .bind(scopes_json)
    .bind(oauth2_session_ttl_secs as i64)
    .bind(api_keys_enabled)
    .bind(pull_backend)
    .execute(pool)
    .await
    .map_err(|e| {
        // `app_proxy` has exactly one FK (`pull_backend →
        // tamad_registry(id)`). Map that violation to an actionable
        // message instead of surfacing raw Postgres FK text.
        if e.as_database_error()
            .is_some_and(|db| matches!(db.code().as_deref(), Some("23503")))
        {
            return anyhow!(
                "pull_backend '{}' is not a registered tamad — register it first (POST /tama/v1/tamads), then set pull_backend to that id",
                pull_backend.unwrap_or("???")
            );
        }
        anyhow!("Failed to upsert app_proxy: {e}")
    })?;
    Ok(())
}

/// Get the proxy config row. Returns None if no row exists.
/// `authenticator_skip_paths` and `oauth2_scopes` are deserialized from JSON.
pub async fn get_proxy(pool: &PgPool) -> Result<Option<ProxyRecord>> {
    let row = sqlx::query(
        "SELECT host, port, auto_unload, idle_timeout_secs, startup_timeout_secs,
                circuit_breaker_threshold, circuit_breaker_cooldown_seconds,
                metrics_retention_secs, pull_queue_poll_interval_secs,
                max_loaded_models, authenticator_url, authenticator_skip_paths,
                oauth2_enabled, oauth2_client_id, oauth2_client_secret,
                oauth2_authorize_url, oauth2_token_url, oauth2_userinfo_url,
                oauth2_logout_url, oauth2_redirect_uri, oauth2_scopes, oauth2_session_ttl_secs,
                api_keys_enabled, pull_backend
         FROM app_proxy WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("Failed to read app_proxy")?;
    let Some(row) = row else {
        return Ok(None);
    };
    let skip_paths_str: String = row.get("authenticator_skip_paths");
    let authenticator_skip_paths: Vec<String> = serde_json::from_str(&skip_paths_str)
        .map_err(|e| anyhow!("Failed to deserialize authenticator_skip_paths: {e}"))?;
    let scopes_str: String = row.get("oauth2_scopes");
    let oauth2_scopes: Vec<String> = serde_json::from_str(&scopes_str)
        .map_err(|e| anyhow!("Failed to deserialize oauth2_scopes: {e}"))?;
    Ok(Some(ProxyRecord {
        host: row.get("host"),
        port: row
            .get::<i64, _>("port")
            .try_into()
            .context("app_proxy.port out of range")?,
        auto_unload: row.get("auto_unload"),
        idle_timeout_secs: u64::try_from(row.get::<i64, _>("idle_timeout_secs"))
            .context("app_proxy.idle_timeout_secs out of range")?,
        startup_timeout_secs: u64::try_from(row.get::<i64, _>("startup_timeout_secs"))
            .context("app_proxy.startup_timeout_secs out of range")?,
        circuit_breaker_threshold: u32::try_from(row.get::<i64, _>("circuit_breaker_threshold"))
            .context("app_proxy.circuit_breaker_threshold out of range")?,
        circuit_breaker_cooldown_seconds: u64::try_from(
            row.get::<i64, _>("circuit_breaker_cooldown_seconds"),
        )
        .context("app_proxy.circuit_breaker_cooldown_seconds out of range")?,
        metrics_retention_secs: u64::try_from(row.get::<i64, _>("metrics_retention_secs"))
            .context("app_proxy.metrics_retention_secs out of range")?,
        pull_queue_poll_interval_secs: u64::try_from(
            row.get::<i64, _>("pull_queue_poll_interval_secs"),
        )
        .context("app_proxy.pull_queue_poll_interval_secs out of range")?,
        max_loaded_models: u32::try_from(row.get::<i64, _>("max_loaded_models"))
            .context("app_proxy.max_loaded_models out of range")?,
        authenticator_url: row.get("authenticator_url"),
        authenticator_skip_paths,
        oauth2_enabled: row.get("oauth2_enabled"),
        oauth2_client_id: row.get("oauth2_client_id"),
        oauth2_client_secret: row.get("oauth2_client_secret"),
        oauth2_authorize_url: row.get("oauth2_authorize_url"),
        oauth2_token_url: row.get("oauth2_token_url"),
        oauth2_userinfo_url: row.get("oauth2_userinfo_url"),
        oauth2_logout_url: row.get("oauth2_logout_url"),
        oauth2_redirect_uri: row.get("oauth2_redirect_uri"),
        oauth2_scopes,
        oauth2_session_ttl_secs: u64::try_from(row.get::<i64, _>("oauth2_session_ttl_secs"))
            .context("app_proxy.oauth2_session_ttl_secs out of range")?,
        api_keys_enabled: row.get("api_keys_enabled"),
        pull_backend: row.get("pull_backend"),
    }))
}

/// Clear `pull_backend` if it points at the given (about-to-be-deleted)
/// tamad. Called by the tamad delete path before the registry row is
/// removed, since the FK to `tamad_registry(id)` would otherwise block
/// the deletion (plan-191 review fix).
pub async fn clear_pull_backend_for_tamad(pool: &PgPool, tamad_id: &str) -> Result<()> {
    sqlx::query("UPDATE app_proxy SET pull_backend = NULL WHERE pull_backend = $1")
        .bind(tamad_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Insert or replace the lifecycle config row (id=1).
pub async fn upsert_lifecycle(
    pool: &PgPool,
    restart_policy: &RestartPolicy,
    max_restarts: u32,
    restart_delay_ms: u64,
    health_check_interval_ms: u64,
    health_check_timeout_ms: u64,
    health_check_retries: u32,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO app_lifecycle (id, restart_policy, max_restarts, restart_delay_ms,
            health_check_interval_ms, health_check_timeout_ms, health_check_retries)
         VALUES (1, $1, $2, $3, $4, $5, $6)
         ON CONFLICT (id) DO UPDATE SET
             restart_policy = EXCLUDED.restart_policy,
             max_restarts = EXCLUDED.max_restarts,
             restart_delay_ms = EXCLUDED.restart_delay_ms,
             health_check_interval_ms = EXCLUDED.health_check_interval_ms,
             health_check_timeout_ms = EXCLUDED.health_check_timeout_ms,
             health_check_retries = EXCLUDED.health_check_retries",
    )
    .bind(restart_policy.as_str())
    .bind(i64::from(max_restarts))
    .bind(restart_delay_ms as i64)
    .bind(health_check_interval_ms as i64)
    .bind(health_check_timeout_ms as i64)
    .bind(i64::from(health_check_retries))
    .execute(pool)
    .await
    .context("Failed to upsert app_lifecycle")?;
    Ok(())
}

/// Get the lifecycle config row. Returns None if no row exists.
pub async fn get_lifecycle(pool: &PgPool) -> Result<Option<LifecycleRecord>> {
    let row = sqlx::query(
        "SELECT restart_policy, max_restarts, restart_delay_ms, health_check_interval_ms,
                health_check_timeout_ms, health_check_retries
         FROM app_lifecycle WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("Failed to read app_lifecycle")?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(LifecycleRecord {
        restart_policy: row.get("restart_policy"),
        max_restarts: u32::try_from(row.get::<i64, _>("max_restarts"))
            .context("app_lifecycle.max_restarts out of range")?,
        restart_delay_ms: u64::try_from(row.get::<i64, _>("restart_delay_ms"))
            .context("app_lifecycle.restart_delay_ms out of range")?,
        health_check_interval_ms: u64::try_from(row.get::<i64, _>("health_check_interval_ms"))
            .context("app_lifecycle.health_check_interval_ms out of range")?,
        health_check_timeout_ms: u64::try_from(row.get::<i64, _>("health_check_timeout_ms"))
            .context("app_lifecycle.health_check_timeout_ms out of range")?,
        health_check_retries: u32::try_from(row.get::<i64, _>("health_check_retries"))
            .context("app_lifecycle.health_check_retries out of range")?,
    }))
}

/// Insert or replace the compaction config row (id=1).
pub async fn upsert_compaction(
    pool: &PgPool,
    enabled: bool,
    server_path: Option<&str>,
    device: &CompactionDevice,
    port: Option<u16>,
    request_timeout_ms: u64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO app_compaction (id, enabled, server_path, device, port, request_timeout_ms)
         VALUES (1, $1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET
             enabled = EXCLUDED.enabled,
             server_path = EXCLUDED.server_path,
             device = EXCLUDED.device,
             port = EXCLUDED.port,
             request_timeout_ms = EXCLUDED.request_timeout_ms",
    )
    .bind(enabled)
    .bind(server_path)
    .bind(device.as_str())
    .bind(port.map(i64::from))
    .bind(request_timeout_ms as i64)
    .execute(pool)
    .await
    .context("Failed to upsert app_compaction")?;
    Ok(())
}

/// Get the compaction config row. Returns None if no row exists.
pub async fn get_compaction(pool: &PgPool) -> Result<Option<CompactionRecord>> {
    let row = sqlx::query(
        "SELECT enabled, server_path, device, port, request_timeout_ms
         FROM app_compaction WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("Failed to read app_compaction")?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(CompactionRecord {
        enabled: row.get("enabled"),
        server_path: row.get("server_path"),
        device: row.get("device"),
        port: row.get::<Option<i64>, _>("port").map(|p| p as u16),
        request_timeout_ms: u64::try_from(row.get::<i64, _>("request_timeout_ms"))
            .context("app_compaction.request_timeout_ms out of range")?,
    }))
}

/// Insert or replace the langfuse config row (id=1).
pub async fn upsert_langfuse(pool: &PgPool, record: &LangfuseRecord) -> Result<()> {
    sqlx::query(
        "INSERT INTO app_langfuse (id, enabled, public_key, secret_key, host, environment,
            capture_input, capture_output, capture_streaming, telemetry_max_bytes, electricity_price_per_kwh)
         VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (id) DO UPDATE SET
             enabled = EXCLUDED.enabled,
             public_key = EXCLUDED.public_key,
             secret_key = EXCLUDED.secret_key,
             host = EXCLUDED.host,
             environment = EXCLUDED.environment,
             capture_input = EXCLUDED.capture_input,
             capture_output = EXCLUDED.capture_output,
             capture_streaming = EXCLUDED.capture_streaming,
             telemetry_max_bytes = EXCLUDED.telemetry_max_bytes,
             electricity_price_per_kwh = EXCLUDED.electricity_price_per_kwh",
    )
    .bind(record.enabled)
    .bind(&record.public_key)
    .bind(&record.secret_key)
    .bind(&record.host)
    .bind(&record.environment)
    .bind(record.capture_input)
    .bind(record.capture_output)
    .bind(record.capture_streaming)
    .bind(i64::try_from(record.telemetry_max_bytes).unwrap_or(i64::MAX))
    .bind(record.electricity_price_per_kwh)
    .execute(pool)
    .await
    .context("Failed to upsert app_langfuse")?;
    Ok(())
}

/// Get the langfuse config row. Returns None if no row exists.
pub async fn get_langfuse(pool: &PgPool) -> Result<Option<LangfuseRecord>> {
    let row = sqlx::query(
        "SELECT enabled, public_key, secret_key, host, environment,
                capture_input, capture_output, capture_streaming,
                telemetry_max_bytes, electricity_price_per_kwh
         FROM app_langfuse WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .context("Failed to read app_langfuse")?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(LangfuseRecord {
        enabled: row.get("enabled"),
        public_key: row.get("public_key"),
        secret_key: row.get("secret_key"),
        host: row.get("host"),
        environment: row.get("environment"),
        capture_input: row.get("capture_input"),
        capture_output: row.get("capture_output"),
        capture_streaming: row.get("capture_streaming"),
        telemetry_max_bytes: usize::try_from(row.get::<i64, _>("telemetry_max_bytes"))
            .context("app_langfuse.telemetry_max_bytes out of range")?,
        electricity_price_per_kwh: row.get("electricity_price_per_kwh"),
    }))
}

/// Insert or update a sampling template.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_sampling_template(
    pool: &PgPool,
    name: &str,
    temperature: Option<f64>,
    top_k: Option<u32>,
    top_p: Option<f64>,
    min_p: Option<f64>,
    presence_penalty: Option<f64>,
    frequency_penalty: Option<f64>,
    repeat_penalty: Option<f64>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO sampling_templates (name, temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT(name) DO UPDATE SET
             temperature = EXCLUDED.temperature,
             top_k = EXCLUDED.top_k,
             top_p = EXCLUDED.top_p,
             min_p = EXCLUDED.min_p,
             presence_penalty = EXCLUDED.presence_penalty,
             frequency_penalty = EXCLUDED.frequency_penalty,
             repeat_penalty = EXCLUDED.repeat_penalty",
    )
    .bind(name)
    .bind(temperature)
    .bind(top_k.map(i64::from))
    .bind(top_p)
    .bind(min_p)
    .bind(presence_penalty)
    .bind(frequency_penalty)
    .bind(repeat_penalty)
    .execute(pool)
    .await
    .context("Failed to upsert sampling template")?;
    Ok(())
}

/// Get all sampling templates.
pub async fn get_all_sampling_templates(pool: &PgPool) -> Result<Vec<SamplingTemplateRecord>> {
    let rows = sqlx::query(
        "SELECT name, temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty
         FROM sampling_templates ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .context("Failed to read sampling_templates")?;
    let templates = rows
        .into_iter()
        .map(|row| SamplingTemplateRecord {
            name: row.get("name"),
            temperature: row.get("temperature"),
            top_k: row.get::<Option<i64>, _>("top_k").map(|v| v as u32),
            top_p: row.get("top_p"),
            min_p: row.get("min_p"),
            presence_penalty: row.get("presence_penalty"),
            frequency_penalty: row.get("frequency_penalty"),
            repeat_penalty: row.get("repeat_penalty"),
        })
        .collect();
    Ok(templates)
}

/// Delete all sampling templates.
pub async fn delete_all_sampling_templates(pool: &PgPool) -> Result<()> {
    sqlx::query("DELETE FROM sampling_templates")
        .execute(pool)
        .await
        .context("Failed to delete sampling templates")?;
    Ok(())
}

/// List the configured backends as `(name, gpu_variant)` pairs.
///
/// Used by `Config::load_from_pool` to assemble the in-memory
/// `backends` map (path/version are DB-managed and intentionally omitted).
pub async fn list_config_backends(pool: &PgPool) -> Result<Vec<(String, String)>> {
    let rows =
        sqlx::query("SELECT name, gpu_variant FROM provider_configs ORDER BY name, gpu_variant")
            .fetch_all(pool)
            .await
            .context("Failed to read provider_configs")?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("name"),
                row.get::<String, _>("gpu_variant"),
            )
        })
        .collect())
}

/// Seed default rows into all singleton tables and the 4 built-in sampling templates.
///
/// Uses `ON CONFLICT ... DO NOTHING` so it is safe to call multiple times —
/// existing rows are preserved on subsequent calls.
pub async fn seed_defaults(pool: &PgPool) -> Result<()> {
    // General defaults
    sqlx::query(
        "INSERT INTO app_general (id, log_level, models_dir, logs_dir, hf_token, update_check_interval)
         VALUES (1, 'info', NULL, NULL, NULL, 12)
         ON CONFLICT (id) DO NOTHING",
    )
    .execute(pool)
    .await
    .context("Failed to seed app_general defaults")?;

    // Proxy defaults
    sqlx::query(
        "INSERT INTO app_proxy (id, host, port, auto_unload, idle_timeout_secs, startup_timeout_secs,
            circuit_breaker_threshold, circuit_breaker_cooldown_seconds, metrics_retention_secs,
            pull_queue_poll_interval_secs, max_loaded_models, authenticator_url, authenticator_skip_paths,
            oauth2_enabled, oauth2_client_id, oauth2_client_secret, oauth2_authorize_url, oauth2_token_url,
            oauth2_userinfo_url, oauth2_logout_url, oauth2_redirect_uri, oauth2_scopes, oauth2_session_ttl_secs,
            api_keys_enabled)
         VALUES (1, '0.0.0.0', 11434, FALSE, 300, 120, 3, 60, 86400, 2, 1, NULL, '[\"/health\",\"/metrics\"]',
            FALSE, '', '', '', '', NULL, NULL, '', '[\"openid\",\"profile\",\"email\"]', 86400, FALSE)
         ON CONFLICT (id) DO NOTHING",
    )
    .execute(pool)
    .await
    .context("Failed to seed app_proxy defaults")?;

    // Lifecycle defaults
    sqlx::query(
        "INSERT INTO app_lifecycle (id, restart_policy, max_restarts, restart_delay_ms,
            health_check_interval_ms, health_check_timeout_ms, health_check_retries)
         VALUES (1, 'always', 10, 3000, 5000, 30000, 3)
         ON CONFLICT (id) DO NOTHING",
    )
    .execute(pool)
    .await
    .context("Failed to seed app_lifecycle defaults")?;

    // Compaction defaults
    sqlx::query(
        "INSERT INTO app_compaction (id, enabled, server_path, device, port, request_timeout_ms)
         VALUES (1, FALSE, NULL, 'cpu', NULL, 30000)
         ON CONFLICT (id) DO NOTHING",
    )
    .execute(pool)
    .await
    .context("Failed to seed app_compaction defaults")?;

    // Langfuse defaults (schema defaults fill every other column)
    sqlx::query("INSERT INTO app_langfuse (id) VALUES (1) ON CONFLICT (id) DO NOTHING")
        .execute(pool)
        .await
        .context("Failed to seed app_langfuse defaults")?;

    // Built-in sampling templates — must match Config::default() in loader.rs
    let templates = [
        (
            "coding",
            Some(0.3f64),
            Some(50i64),
            Some(0.9f64),
            Some(0.05f64),
            Some(0.1f64),
            None::<f64>,
            None::<f64>,
        ),
        (
            "chat",
            Some(0.7f64),
            Some(40i64),
            Some(0.95f64),
            Some(0.05f64),
            Some(0.0f64),
            None::<f64>,
            None::<f64>,
        ),
        (
            "analysis",
            Some(0.3f64),
            Some(20i64),
            Some(0.9f64),
            Some(0.05f64),
            Some(0.0f64),
            None::<f64>,
            None::<f64>,
        ),
        (
            "creative",
            Some(0.9f64),
            Some(50i64),
            Some(0.95f64),
            Some(0.02f64),
            Some(0.0f64),
            None::<f64>,
            None::<f64>,
        ),
    ];

    for (
        name,
        temperature,
        top_k,
        top_p,
        min_p,
        presence_penalty,
        frequency_penalty,
        repeat_penalty,
    ) in &templates
    {
        sqlx::query(
            "INSERT INTO sampling_templates (name, temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(name)
        .bind(temperature)
        .bind(top_k)
        .bind(top_p)
        .bind(min_p)
        .bind(presence_penalty)
        .bind(frequency_penalty)
        .bind(repeat_penalty)
        .execute(pool)
        .await
        .with_context(|| format!("Failed to seed sampling template '{name}'"))?;
    }

    Ok(())
}
