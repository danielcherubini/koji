//! App config database query functions.
//!
//! CRUD operations for the singleton config tables (`app_general`, `app_proxy`,
//! `app_supervisor`, `app_compaction`) and the multi-row `sampling_templates` table.
//! Singleton tables store exactly one row with `id = 1`.

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use serde_json;

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
    pub download_queue_poll_interval_secs: u64,
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
}

/// A row from the `app_supervisor` table.
#[derive(Debug, Clone, PartialEq)]
pub struct SupervisorRecord {
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
pub fn upsert_general(
    conn: &Connection,
    log_level: &LogLevel,
    models_dir: Option<&str>,
    logs_dir: Option<&str>,
    hf_token: Option<&str>,
    update_check_interval: u32,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO app_general (id, log_level, models_dir, logs_dir, hf_token, update_check_interval)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![log_level.as_str(), models_dir, logs_dir, hf_token, update_check_interval as i64],
    )
    .context("Failed to upsert app_general")?;
    Ok(())
}

/// Get the general app config row. Returns None if no row exists.
pub fn get_general(conn: &Connection) -> Result<Option<GeneralRecord>> {
    let mut stmt = conn.prepare(
        "SELECT log_level, models_dir, logs_dir, hf_token, update_check_interval
         FROM app_general WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(GeneralRecord {
            log_level: row.get::<_, String>(0)?,
            models_dir: row.get::<_, Option<String>>(1)?,
            logs_dir: row.get::<_, Option<String>>(2)?,
            hf_token: row.get::<_, Option<String>>(3)?,
            update_check_interval: row.get::<_, i64>(4)? as u32,
        })
    })?;
    match rows.next() {
        Some(Ok(record)) => Ok(Some(record)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Insert or replace the proxy config row (id=1).
/// `authenticator_skip_paths` and `oauth2_scopes` are stored as JSON strings.
#[allow(clippy::too_many_arguments)]
pub fn upsert_proxy(
    conn: &Connection,
    host: &str,
    port: u16,
    auto_unload: bool,
    idle_timeout_secs: u64,
    startup_timeout_secs: u64,
    circuit_breaker_threshold: u32,
    circuit_breaker_cooldown_seconds: u64,
    metrics_retention_secs: u64,
    download_queue_poll_interval_secs: u64,
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
) -> Result<()> {
    let skip_paths_json = serde_json::to_string(authenticator_skip_paths)
        .context("Failed to serialize authenticator_skip_paths to JSON")?;
    let scopes_json = serde_json::to_string(oauth2_scopes)
        .context("Failed to serialize oauth2_scopes to JSON")?;

    conn.execute(
        "INSERT OR REPLACE INTO app_proxy (id, host, port, auto_unload, idle_timeout_secs, startup_timeout_secs,
            circuit_breaker_threshold, circuit_breaker_cooldown_seconds, metrics_retention_secs,
            download_queue_poll_interval_secs, max_loaded_models, authenticator_url, authenticator_skip_paths,
            oauth2_enabled, oauth2_client_id, oauth2_client_secret, oauth2_authorize_url, oauth2_token_url,
            oauth2_userinfo_url, oauth2_logout_url, oauth2_redirect_uri, oauth2_scopes, oauth2_session_ttl_secs)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
        params![
            host,
            port as i64,
            auto_unload as i32,
            idle_timeout_secs,
            startup_timeout_secs,
            circuit_breaker_threshold as i64,
            circuit_breaker_cooldown_seconds,
            metrics_retention_secs,
            download_queue_poll_interval_secs,
            max_loaded_models as i64,
            authenticator_url,
            skip_paths_json,
            oauth2_enabled as i32,
            oauth2_client_id,
            oauth2_client_secret,
            oauth2_authorize_url,
            oauth2_token_url,
            oauth2_userinfo_url,
            oauth2_logout_url,
            oauth2_redirect_uri,
            scopes_json,
            oauth2_session_ttl_secs,
        ],
    )
    .context("Failed to upsert app_proxy")?;
    Ok(())
}

/// Get the proxy config row. Returns None if no row exists.
/// `authenticator_skip_paths` and `oauth2_scopes` are deserialized from JSON.
pub fn get_proxy(conn: &Connection) -> Result<Option<ProxyRecord>> {
    let mut stmt = conn.prepare(
        "SELECT host, port, auto_unload, idle_timeout_secs, startup_timeout_secs,
                circuit_breaker_threshold, circuit_breaker_cooldown_seconds,
                metrics_retention_secs, download_queue_poll_interval_secs,
                max_loaded_models, authenticator_url, authenticator_skip_paths,
                oauth2_enabled, oauth2_client_id, oauth2_client_secret,
                oauth2_authorize_url, oauth2_token_url, oauth2_userinfo_url,
                oauth2_logout_url, oauth2_redirect_uri, oauth2_scopes, oauth2_session_ttl_secs
         FROM app_proxy WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)? as u16,
            row.get::<_, i32>(2)? != 0,
            row.get::<_, i64>(3)? as u64,
            row.get::<_, i64>(4)? as u64,
            row.get::<_, i64>(5)? as u32,
            row.get::<_, i64>(6)? as u64,
            row.get::<_, i64>(7)? as u64,
            row.get::<_, i64>(8)? as u64,
            row.get::<_, i64>(9)? as u32,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, i32>(12)? != 0,
            row.get::<_, String>(13)?,
            row.get::<_, String>(14)?,
            row.get::<_, String>(15)?,
            row.get::<_, String>(16)?,
            row.get::<_, Option<String>>(17)?,
            row.get::<_, Option<String>>(18)?,
            row.get::<_, String>(19)?,
            row.get::<_, String>(20)?,
            row.get::<_, i64>(21)? as u64,
        ))
    })?;
    match rows.next() {
        Some(Ok(record)) => {
            let (
                host,
                port,
                auto_unload,
                idle_timeout_secs,
                startup_timeout_secs,
                circuit_breaker_threshold,
                circuit_breaker_cooldown_seconds,
                metrics_retention_secs,
                download_queue_poll_interval_secs,
                max_loaded_models,
                authenticator_url,
                skip_paths_str,
                oauth2_enabled,
                oauth2_client_id,
                oauth2_client_secret,
                oauth2_authorize_url,
                oauth2_token_url,
                oauth2_userinfo_url,
                oauth2_logout_url,
                oauth2_redirect_uri,
                scopes_str,
                oauth2_session_ttl_secs,
            ) = record;
            let authenticator_skip_paths: Vec<String> = serde_json::from_str(&skip_paths_str)
                .map_err(|e| anyhow!("Failed to deserialize authenticator_skip_paths: {e}"))?;
            let oauth2_scopes: Vec<String> = serde_json::from_str(&scopes_str)
                .map_err(|e| anyhow!("Failed to deserialize oauth2_scopes: {e}"))?;
            Ok(Some(ProxyRecord {
                host,
                port,
                auto_unload,
                idle_timeout_secs,
                startup_timeout_secs,
                circuit_breaker_threshold,
                circuit_breaker_cooldown_seconds,
                metrics_retention_secs,
                download_queue_poll_interval_secs,
                max_loaded_models,
                authenticator_url,
                authenticator_skip_paths,
                oauth2_enabled,
                oauth2_client_id,
                oauth2_client_secret,
                oauth2_authorize_url,
                oauth2_token_url,
                oauth2_userinfo_url,
                oauth2_logout_url,
                oauth2_redirect_uri,
                oauth2_scopes,
                oauth2_session_ttl_secs,
            }))
        }
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Insert or replace the supervisor config row (id=1).
pub fn upsert_supervisor(
    conn: &Connection,
    restart_policy: &RestartPolicy,
    max_restarts: u32,
    restart_delay_ms: u64,
    health_check_interval_ms: u64,
    health_check_timeout_ms: u64,
    health_check_retries: u32,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO app_supervisor (id, restart_policy, max_restarts, restart_delay_ms,
            health_check_interval_ms, health_check_timeout_ms, health_check_retries)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            restart_policy.as_str(),
            max_restarts as i64,
            restart_delay_ms,
            health_check_interval_ms,
            health_check_timeout_ms,
            health_check_retries as i64,
        ],
    )
    .context("Failed to upsert app_supervisor")?;
    Ok(())
}

/// Get the supervisor config row. Returns None if no row exists.
pub fn get_supervisor(conn: &Connection) -> Result<Option<SupervisorRecord>> {
    let mut stmt = conn.prepare(
        "SELECT restart_policy, max_restarts, restart_delay_ms, health_check_interval_ms,
                health_check_timeout_ms, health_check_retries
         FROM app_supervisor WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(SupervisorRecord {
            restart_policy: row.get::<_, String>(0)?,
            max_restarts: row.get::<_, i64>(1)? as u32,
            restart_delay_ms: row.get::<_, i64>(2)? as u64,
            health_check_interval_ms: row.get::<_, i64>(3)? as u64,
            health_check_timeout_ms: row.get::<_, i64>(4)? as u64,
            health_check_retries: row.get::<_, i64>(5)? as u32,
        })
    })?;
    match rows.next() {
        Some(Ok(record)) => Ok(Some(record)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Insert or replace the compaction config row (id=1).
pub fn upsert_compaction(
    conn: &Connection,
    enabled: bool,
    server_path: Option<&str>,
    device: &CompactionDevice,
    port: Option<u16>,
    request_timeout_ms: u64,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO app_compaction (id, enabled, server_path, device, port, request_timeout_ms)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![
            enabled as i32,
            server_path,
            device.as_str(),
            port.map(|p| p as i64),
            request_timeout_ms,
        ],
    )
    .context("Failed to upsert app_compaction")?;
    Ok(())
}

/// Get the compaction config row. Returns None if no row exists.
pub fn get_compaction(conn: &Connection) -> Result<Option<CompactionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT enabled, server_path, device, port, request_timeout_ms
         FROM app_compaction WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(CompactionRecord {
            enabled: row.get::<_, i32>(0)? != 0,
            server_path: row.get::<_, Option<String>>(1)?,
            device: row.get::<_, String>(2)?,
            port: row.get::<_, Option<i64>>(3)?.map(|p| p as u16),
            request_timeout_ms: row.get::<_, i64>(4)? as u64,
        })
    })?;
    match rows.next() {
        Some(Ok(record)) => Ok(Some(record)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

/// Insert or update a sampling template.
#[allow(clippy::too_many_arguments)]
pub fn upsert_sampling_template(
    conn: &Connection,
    name: &str,
    temperature: Option<f64>,
    top_k: Option<u32>,
    top_p: Option<f64>,
    min_p: Option<f64>,
    presence_penalty: Option<f64>,
    frequency_penalty: Option<f64>,
    repeat_penalty: Option<f64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sampling_templates (name, temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(name) DO UPDATE SET
             temperature = excluded.temperature,
             top_k = excluded.top_k,
             top_p = excluded.top_p,
             min_p = excluded.min_p,
             presence_penalty = excluded.presence_penalty,
             frequency_penalty = excluded.frequency_penalty,
             repeat_penalty = excluded.repeat_penalty",
        params![
            name,
            temperature,
            top_k.map(|v| v as i64),
            top_p,
            min_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
        ],
    )
    .context("Failed to upsert sampling template")?;
    Ok(())
}

/// Get all sampling templates.
pub fn get_all_sampling_templates(conn: &Connection) -> Result<Vec<SamplingTemplateRecord>> {
    let mut stmt = conn.prepare(
        "SELECT name, temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty
         FROM sampling_templates ORDER BY name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SamplingTemplateRecord {
            name: row.get::<_, String>(0)?,
            temperature: row.get::<_, Option<f64>>(1)?,
            top_k: row.get::<_, Option<i64>>(2)?.map(|v| v as u32),
            top_p: row.get::<_, Option<f64>>(3)?,
            min_p: row.get::<_, Option<f64>>(4)?,
            presence_penalty: row.get::<_, Option<f64>>(5)?,
            frequency_penalty: row.get::<_, Option<f64>>(6)?,
            repeat_penalty: row.get::<_, Option<f64>>(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Delete all sampling templates.
pub fn delete_all_sampling_templates(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM sampling_templates", [])?;
    Ok(())
}

/// Seed default rows into all singleton tables and the 4 built-in sampling templates.
///
/// Uses `INSERT OR IGNORE` so it is safe to call multiple times — existing rows
/// are preserved on subsequent calls.
pub fn seed_defaults(conn: &Connection) -> Result<()> {
    // General defaults
    conn.execute(
        "INSERT OR IGNORE INTO app_general (id, log_level, models_dir, logs_dir, hf_token, update_check_interval)
         SELECT 1, 'info', NULL, NULL, NULL, 12
         WHERE NOT EXISTS (SELECT 1 FROM app_general WHERE id = 1)",
        [],
    )
    .context("Failed to seed app_general defaults")?;

    // Proxy defaults
    conn.execute(
        "INSERT OR IGNORE INTO app_proxy (id, host, port, auto_unload, idle_timeout_secs, startup_timeout_secs,
            circuit_breaker_threshold, circuit_breaker_cooldown_seconds, metrics_retention_secs,
            download_queue_poll_interval_secs, max_loaded_models, authenticator_url, authenticator_skip_paths,
            oauth2_enabled, oauth2_client_id, oauth2_client_secret, oauth2_authorize_url, oauth2_token_url,
            oauth2_userinfo_url, oauth2_logout_url, oauth2_redirect_uri, oauth2_scopes, oauth2_session_ttl_secs)
         SELECT 1, '0.0.0.0', 11434, 0, 300, 120, 3, 60, 86400, 2, 1, NULL, '[\"/health\",\"/metrics\"]',
            0, '', '', '', '', NULL, NULL, '', '[\"openid\",\"profile\",\"email\"]', 86400
         WHERE NOT EXISTS (SELECT 1 FROM app_proxy WHERE id = 1)",
        [],
    )
    .context("Failed to seed app_proxy defaults")?;

    // Supervisor defaults
    conn.execute(
        "INSERT OR IGNORE INTO app_supervisor (id, restart_policy, max_restarts, restart_delay_ms,
            health_check_interval_ms, health_check_timeout_ms, health_check_retries)
         SELECT 1, 'always', 10, 3000, 5000, 30000, 3
         WHERE NOT EXISTS (SELECT 1 FROM app_supervisor WHERE id = 1)",
        [],
    )
    .context("Failed to seed app_supervisor defaults")?;

    // Compaction defaults
    conn.execute(
        "INSERT OR IGNORE INTO app_compaction (id, enabled, server_path, device, port, request_timeout_ms)
         SELECT 1, 0, NULL, 'cpu', NULL, 30000
         WHERE NOT EXISTS (SELECT 1 FROM app_compaction WHERE id = 1)",
        [],
    )
    .context("Failed to seed app_compaction defaults")?;

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
        conn.execute(
            "INSERT OR IGNORE INTO sampling_templates (name, temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                *name,
                *temperature,
                *top_k,
                *top_p,
                *min_p,
                *presence_penalty,
                *frequency_penalty,
                *repeat_penalty,
            ],
        )
        .with_context(|| format!("Failed to seed sampling template '{name}'"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{CompactionDevice, LogLevel, RestartPolicy};
    use crate::db::open_in_memory;

    /// Helper: open an in-memory DB with migrations applied.
    fn test_conn() -> Connection {
        open_in_memory().unwrap().conn
    }

    // ── seed_defaults ──────────────────────────────────────────────────

    #[test]
    fn test_seed_defaults_creates_all_rows() {
        let conn = test_conn();

        // No rows before seeding
        assert!(get_general(&conn).unwrap().is_none());
        assert!(get_proxy(&conn).unwrap().is_none());
        assert!(get_supervisor(&conn).unwrap().is_none());
        assert!(get_compaction(&conn).unwrap().is_none());
        assert!(get_all_sampling_templates(&conn).unwrap().is_empty());

        seed_defaults(&conn).unwrap();

        // All singleton tables should have a row now
        let general = get_general(&conn).unwrap().unwrap();
        assert_eq!(general.log_level, "info");
        assert_eq!(general.update_check_interval, 12);

        let proxy = get_proxy(&conn).unwrap().unwrap();
        assert_eq!(proxy.host, "0.0.0.0");
        assert_eq!(proxy.port, 11434);

        let supervisor = get_supervisor(&conn).unwrap().unwrap();
        assert_eq!(supervisor.restart_policy, "always");
        assert_eq!(supervisor.max_restarts, 10);

        let compaction = get_compaction(&conn).unwrap().unwrap();
        assert!(!compaction.enabled);
        assert_eq!(compaction.device, "cpu");

        // 4 sampling templates
        let templates = get_all_sampling_templates(&conn).unwrap();
        assert_eq!(templates.len(), 4);
        let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"coding"));
        assert!(names.contains(&"chat"));
        assert!(names.contains(&"analysis"));
        assert!(names.contains(&"creative"));

        // Verify seed is idempotent — calling again should not duplicate
        seed_defaults(&conn).unwrap();
        assert_eq!(get_all_sampling_templates(&conn).unwrap().len(), 4);
    }

    // ── general ────────────────────────────────────────────────────────

    #[test]
    fn test_general_roundtrip() {
        let conn = test_conn();

        // Before upsert, get returns None
        assert!(get_general(&conn).unwrap().is_none());

        // Upsert
        upsert_general(
            &conn,
            &LogLevel::Debug,
            Some("/data/models"),
            Some("/var/log/tama"),
            Some("hf_abc123"),
            30,
        )
        .unwrap();

        // Get and verify
        let general = get_general(&conn).unwrap().unwrap();
        assert_eq!(general.log_level, "debug");
        assert_eq!(general.models_dir, Some("/data/models".to_string()));
        assert_eq!(general.logs_dir, Some("/var/log/tama".to_string()));
        assert_eq!(general.hf_token, Some("hf_abc123".to_string()));
        assert_eq!(general.update_check_interval, 30);

        // Upsert again (update)
        upsert_general(&conn, &LogLevel::Warn, None, None, None, 60).unwrap();

        let general = get_general(&conn).unwrap().unwrap();
        assert_eq!(general.log_level, "warn");
        assert_eq!(general.models_dir, None);
        assert_eq!(general.logs_dir, None);
        assert_eq!(general.hf_token, None);
        assert_eq!(general.update_check_interval, 60);
    }

    // ── proxy ──────────────────────────────────────────────────────────

    #[test]
    fn test_proxy_roundtrip() {
        let conn = test_conn();

        assert!(get_proxy(&conn).unwrap().is_none());

        let skip_paths = vec![
            "/health".to_string(),
            "/metrics".to_string(),
            "/custom".to_string(),
        ];
        upsert_proxy(
            &conn,
            "127.0.0.1",
            8080,
            true,
            600,
            180,
            5,
            120,
            43200,
            5,
            2,
            Some("http://auth:8080"),
            &skip_paths,
            false, // oauth2_enabled
            "",
            "",
            "",
            "",
            None,
            None,
            "",
            &[],
            0,
        )
        .unwrap();

        let proxy = get_proxy(&conn).unwrap().unwrap();
        assert_eq!(proxy.host, "127.0.0.1");
        assert_eq!(proxy.port, 8080);
        assert!(proxy.auto_unload);
        assert_eq!(proxy.idle_timeout_secs, 600);
        assert_eq!(proxy.startup_timeout_secs, 180);
        assert_eq!(proxy.circuit_breaker_threshold, 5);
        assert_eq!(proxy.circuit_breaker_cooldown_seconds, 120);
        assert_eq!(proxy.metrics_retention_secs, 43200);
        assert_eq!(proxy.download_queue_poll_interval_secs, 5);
        assert_eq!(proxy.max_loaded_models, 2);
        assert_eq!(
            proxy.authenticator_url,
            Some("http://auth:8080".to_string())
        );
        assert_eq!(proxy.authenticator_skip_paths, skip_paths);

        // Update with empty skip paths
        upsert_proxy(
            &conn,
            "0.0.0.0",
            11434,
            false,
            300,
            120,
            3,
            60,
            86400,
            2,
            1,
            None,
            &[],
            false, // oauth2_enabled
            "",
            "",
            "",
            "",
            None,
            None,
            "",
            &[],
            0,
        )
        .unwrap();

        let proxy = get_proxy(&conn).unwrap().unwrap();
        assert_eq!(proxy.authenticator_skip_paths, Vec::<String>::new());
        assert_eq!(proxy.authenticator_url, None);
    }

    // ── supervisor ─────────────────────────────────────────────────────

    #[test]
    fn test_supervisor_roundtrip() {
        let conn = test_conn();

        assert!(get_supervisor(&conn).unwrap().is_none());

        upsert_supervisor(&conn, &RestartPolicy::OnFailure, 5, 5000, 3000, 10000, 2).unwrap();

        let supervisor = get_supervisor(&conn).unwrap().unwrap();
        assert_eq!(supervisor.restart_policy, "on-failure");
        assert_eq!(supervisor.max_restarts, 5);
        assert_eq!(supervisor.restart_delay_ms, 5000);
        assert_eq!(supervisor.health_check_interval_ms, 3000);
        assert_eq!(supervisor.health_check_timeout_ms, 10000);
        assert_eq!(supervisor.health_check_retries, 2);

        // Update
        upsert_supervisor(&conn, &RestartPolicy::Always, 20, 1000, 10000, 60000, 5).unwrap();

        let supervisor = get_supervisor(&conn).unwrap().unwrap();
        assert_eq!(supervisor.restart_policy, "always");
        assert_eq!(supervisor.max_restarts, 20);
        assert_eq!(supervisor.restart_delay_ms, 1000);
    }

    // ── compaction ─────────────────────────────────────────────────────

    #[test]
    fn test_compaction_roundtrip() {
        let conn = test_conn();

        assert!(get_compaction(&conn).unwrap().is_none());

        upsert_compaction(
            &conn,
            true,
            Some("/usr/local/bin/llmlingua"),
            &CompactionDevice::Cuda,
            Some(8888),
            60000,
        )
        .unwrap();

        let compaction = get_compaction(&conn).unwrap().unwrap();
        assert!(compaction.enabled);
        assert_eq!(
            compaction.server_path,
            Some("/usr/local/bin/llmlingua".to_string())
        );
        assert_eq!(compaction.device, "cuda");
        assert_eq!(compaction.port, Some(8888));
        assert_eq!(compaction.request_timeout_ms, 60000);

        // Update with defaults
        upsert_compaction(&conn, false, None, &CompactionDevice::Cpu, None, 30000).unwrap();

        let compaction = get_compaction(&conn).unwrap().unwrap();
        assert!(!compaction.enabled);
        assert_eq!(compaction.server_path, None);
        assert_eq!(compaction.device, "cpu");
        assert_eq!(compaction.port, None);
        assert_eq!(compaction.request_timeout_ms, 30000);
    }

    // ── sampling templates ─────────────────────────────────────────────

    #[test]
    fn test_sampling_template_crud() {
        let conn = test_conn();

        // Initially empty
        assert!(get_all_sampling_templates(&conn).unwrap().is_empty());

        // Upsert a new template
        upsert_sampling_template(
            &conn,
            "coding",
            Some(0.3),
            Some(50),
            Some(0.9),
            Some(0.05),
            Some(0.1),
            None,
            None,
        )
        .unwrap();

        // Upsert another
        upsert_sampling_template(
            &conn,
            "chat",
            Some(0.7),
            Some(40),
            Some(0.95),
            Some(0.05),
            Some(0.0),
            None,
            None,
        )
        .unwrap();

        let templates = get_all_sampling_templates(&conn).unwrap();
        assert_eq!(templates.len(), 2);

        // Verify values
        let coding = templates.iter().find(|t| t.name == "coding").unwrap();
        assert_eq!(coding.temperature, Some(0.3));
        assert_eq!(coding.top_k, Some(50));
        assert_eq!(coding.top_p, Some(0.9));
        assert_eq!(coding.min_p, Some(0.05));
        assert_eq!(coding.presence_penalty, Some(0.1));

        // Upsert updates existing template (coding)
        upsert_sampling_template(
            &conn,
            "coding",
            Some(0.5),
            Some(100),
            Some(0.95),
            Some(0.1),
            Some(0.2),
            Some(0.1),
            Some(1.5),
        )
        .unwrap();

        let templates = get_all_sampling_templates(&conn).unwrap();
        assert_eq!(templates.len(), 2); // Still 2, not 3

        let coding = templates.iter().find(|t| t.name == "coding").unwrap();
        assert_eq!(coding.temperature, Some(0.5));
        assert_eq!(coding.top_k, Some(100));
        assert_eq!(coding.repeat_penalty, Some(1.5));

        // Delete all
        delete_all_sampling_templates(&conn).unwrap();
        assert!(get_all_sampling_templates(&conn).unwrap().is_empty());
    }

    // ── proxy OAuth2 ───────────────────────────────────────────────────

    #[test]
    fn test_oauth2_proxy_roundtrip() {
        let conn = test_conn();

        assert!(get_proxy(&conn).unwrap().is_none());

        let scopes = vec![
            "openid".to_string(),
            "profile".to_string(),
            "email".to_string(),
        ];
        upsert_proxy(
            &conn,
            "0.0.0.0",
            11434,
            false,
            300,
            120,
            3,
            60,
            86400,
            2,
            1,
            None,
            &[],
            true, // oauth2_enabled
            "my-client-id",
            "my-client-secret",
            "https://auth.example.com/authorize",
            "https://auth.example.com/token",
            Some("https://auth.example.com/userinfo"),
            Some("https://auth.example.com/logout"),
            "http://localhost:11434/callback",
            &scopes,
            3600, // session_ttl_secs
        )
        .unwrap();

        let proxy = get_proxy(&conn).unwrap().unwrap();
        assert!(proxy.oauth2_enabled);
        assert_eq!(proxy.oauth2_client_id, "my-client-id");
        assert_eq!(proxy.oauth2_client_secret, "my-client-secret");
        assert_eq!(
            proxy.oauth2_authorize_url,
            "https://auth.example.com/authorize"
        );
        assert_eq!(proxy.oauth2_token_url, "https://auth.example.com/token");
        assert_eq!(
            proxy.oauth2_userinfo_url,
            Some("https://auth.example.com/userinfo".to_string())
        );
        assert_eq!(
            proxy.oauth2_logout_url,
            Some("https://auth.example.com/logout".to_string())
        );
        assert_eq!(proxy.oauth2_redirect_uri, "http://localhost:11434/callback");
        assert_eq!(proxy.oauth2_scopes, scopes);
        assert_eq!(proxy.oauth2_session_ttl_secs, 3600);

        // Update with disabled OAuth2 and default scopes
        upsert_proxy(
            &conn,
            "0.0.0.0",
            11434,
            false,
            300,
            120,
            3,
            60,
            86400,
            2,
            1,
            None,
            &[],
            false, // oauth2_enabled
            "",
            "",
            "",
            "",
            None,
            None,
            "",
            &["openid".to_string(), "profile".to_string()],
            86400,
        )
        .unwrap();

        let proxy = get_proxy(&conn).unwrap().unwrap();
        assert!(!proxy.oauth2_enabled);
        assert_eq!(proxy.oauth2_scopes, vec!["openid", "profile"]);
        assert_eq!(proxy.oauth2_session_ttl_secs, 86400);
    }
}
