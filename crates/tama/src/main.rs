//! Tama server binary
//!
//! Starts the proxy server with web UI. The global app config is loaded
//! from Postgres; `config.toml` is a bootstrap file holding only the
//! `[database]` section (plan-190). No CLI arguments are accepted.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tama_core::config::Config;
use tama_core::proxy::{ProxyServer, ProxyState};

/// Set up HF_TOKEN environment variable from config if present.
fn setup_hf_token(config: &Config) {
    if let Some(token) = &config.general.hf_token {
        if !token.is_empty() {
            std::env::set_var("HF_TOKEN", token);
            tracing::info!("HF_TOKEN configured from config file");
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load the v3 bootstrap config FIRST: config.toml is a bootstrap file
    // containing ONLY a [database] section (plan-190). v3 has no SQLite-only
    // mode — a missing [database] section is a hard error.
    let config_dir = Config::config_dir().context("Failed to determine config directory")?;
    let db_bootstrap =
        tama_core::config::database::load_bootstrap(&config_dir)?.ok_or_else(|| {
            anyhow::anyhow!(
                "v3 requires a [database] section in config.toml (host/port/name/user/password). \
                 The app config now lives in Postgres — run `tama migrate` to copy your v2 data."
            )
        })?;

    // Minimal default tracing (console, info) BEFORE the pool is created so
    // connection-retry and migration logs are visible. The full subscriber
    // (DB-derived log_level + JSON file layer) is swapped in below, once the
    // Postgres-backed config is loaded.
    init_default_tracing();

    // main.rs is the SINGLE owner of the pool: load bootstrap → resolve
    // password → create pool → retry until reachable → run migrations →
    // seed config defaults → load config → share the Arc<PgPool> with
    // ProxyState + WebState.
    // Fail loud on a missing password env var (names the exact var).
    db_bootstrap
        .resolved_password()
        .with_context(|| "failed to resolve Postgres password")?;
    let pool = tama_core::db::pool::create_pool(&db_bootstrap)
        .await
        .context("creating Postgres pool")?;
    // Retry forever with backoff — the daemon stays alive while
    // Postgres comes up, logging each attempt.
    tama_core::db::pool::connect_with_retry(&pool, std::time::Duration::from_secs(1))
        .await
        .context("connecting to Postgres")?;
    // A migration failure (not a connection failure) exits non-zero.
    tama_core::db::postgres::run_migrations(&pool)
        .await
        .context("applying Postgres migrations")?;
    // Idempotent seed: a fresh Postgres holds defaults until `tama migrate`
    // copies the operator's real config.
    tama_core::db::queries::seed_defaults(&pool)
        .await
        .context("seeding default app config")?;

    // Load the global app config from Postgres (log_level, logs_dir, ...).
    let config = Config::load_from_pool(&pool)
        .await
        .context("loading app config from Postgres")?;

    // Swap in the full tracing subscriber built from the DB-derived
    // log_level + logs_dir. The guard must stay in scope for the program's
    // lifetime to keep the background writer thread alive — if dropped,
    // file logging silently stops. On normal exit, WorkerGuard::Drop
    // flushes remaining entries; on SIGKILL/panic-abort the last few
    // buffered lines may be lost (inherent trade-off of non-blocking
    // writes; console layer is synchronous).
    let _log_guard = init_tracing(&config)?;

    // Set up HF_TOKEN from config before any hf_hub usage
    setup_hf_token(&config);

    let db_pool: Arc<sqlx::PgPool> = Arc::new(pool);

    // Parse host and port from config
    let host = config.proxy.host.clone();
    let port = config.proxy.port;
    let auto_unload = config.proxy.auto_unload;
    let idle_timeout = config.proxy.idle_timeout_secs;

    let (host_addr, warning) = match host.parse::<std::net::IpAddr>() {
        Ok(addr) => (addr, false),
        Err(_) => (
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            true,
        ),
    };
    let addr = SocketAddr::new(host_addr, port);

    if warning {
        tracing::warn!("Invalid host '{}' - using 127.0.0.1", host);
    }

    tracing::info!("Starting tama on {}", addr);
    tracing::info!(
        "Auto-unload: {} (idle timeout: {}s)",
        auto_unload,
        idle_timeout
    );

    let db_dir = Some(config_dir.clone());

    // One-shot migrations run from the Postgres pool (plan-190 Task 8):
    // the backend registry TOML import and the legacy flat-layout backend
    // file migration (idempotent; marker-file guarded).
    if let Some(ref dir) = db_dir {
        if let Err(e) =
            tama_core::db::backfill::migrate_backend_registry_toml(db_pool.as_ref(), dir).await
        {
            tracing::error!("Backend registry TOML migration failed: {}", e);
        }
        if let Err(e) = tama_core::installations::migration::migrate_legacy_backends(
            db_pool.as_ref(),
            &config_dir.join("backends"),
        )
        .await
        {
            tracing::error!("Legacy backend migration failed: {}", e);
        }
    }

    // Create shared proxy state
    let proxy_state = Arc::new(ProxyState::new(
        config.clone(),
        db_dir.clone(),
        db_pool.clone(),
    ));

    #[cfg(feature = "ssr")]
    {
        // Create WebState separately from ProxyState.
        // WebState is owned by the tama crate, not tama-core.
        let web_state = {
            let (tx, _) = tokio::sync::broadcast::channel::<tama_core::updates::UpdateEvent>(256);
            let mut checker = tama_core::updates::UpdateChecker::new();
            checker.set_update_events_tx(tx);
            Arc::new(tama_web::web_types::WebState {
                jobs: Some(Arc::new(tama_web::web_types::JobManager::new())),
                capabilities: Some(Arc::new(tama_web::web_types::CapabilitiesCache::new())),
                update_checker: Arc::new(checker),
                binary_version: env!("CARGO_PKG_VERSION").to_string(),
                update_tx: Arc::new(tokio::sync::Mutex::new(None)),
                upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
                db_pool: db_pool.clone(),
            })
        };

        // Combined app state (used for shutdown cleanup)
        let app_state = Arc::new(tama_web::app_state::AppState::new(
            proxy_state.clone(),
            web_state.clone(),
        ));

        // Build the unified router: proxy routes + web UI routes on a single server.
        // Proxy routes use State<Arc<ProxyState>> as before.
        // Web routes use Extension<WebState> to access web-specific state.
        let web_routes = tama_web::router::build_web_routes(web_state.clone());
        let server = ProxyServer::new(proxy_state.clone()).await;
        let app = server.into_unified_router(web_routes).await;

        // Clone app state for shutdown cleanup (unloads TTS backends + kills job children)
        let cleanup_state = Arc::clone(&app_state);
        let on_shutdown = async move {
            // Kill children of any active backend job
            if let Some(jobs) = &cleanup_state.web_state.jobs {
                if let Some(active_job) = jobs.active().await {
                    tracing::info!("Killing children of active job {}...", active_job.id);
                    jobs.kill_children(&active_job).await;
                }
            }
            // Unload TTS backends
            let tts_backends: Vec<String> = cleanup_state.state.tts_backend_names().await;
            for name in tts_backends {
                if let Err(e) = cleanup_state.state.unload_tts_backend(&name).await {
                    tracing::warn!("Failed to unload TTS backend '{}': {}", name, e);
                }
            }
        };

        // Use the listener module which handles OS signals + graceful shutdown
        tama_core::proxy::server::listener::run(app, addr, Some(on_shutdown), None).await
    }

    #[cfg(not(feature = "ssr"))]
    {
        // CSR-only build: nothing to run (web UI is handled by browser)
        Ok(())
    }
}

/// Build the `EnvFilter` for a log level (the level from the DB-backed
/// config is authoritative).
///
/// RUST_LOG target-specific directives (e.g. "tama_core::backends=debug")
/// are added on top. Bare level directives (e.g. "warn") are ignored so
/// they can't override the configured log_level and silence the file logger.
fn build_log_filter(log_level: &tama_core::config::LogLevel) -> tracing_subscriber::EnvFilter {
    let level: tracing::Level = (*log_level).into();
    let mut env_filter = tracing_subscriber::EnvFilter::new(format!("{}", level));
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        for directive in rust_log.split(',') {
            let directive = directive.trim();
            // Only add directives with a target (contain '='). Bare levels
            // like "warn" or "info" would set the default and override the
            // config level — we want the config to be authoritative.
            if directive.is_empty() || !directive.contains('=') {
                continue;
            }
            if let Ok(parsed) = directive.parse::<tracing_subscriber::filter::Directive>() {
                env_filter = env_filter.add_directive(parsed);
            }
        }
    }
    env_filter
}

/// Minimal default tracing subscriber: pretty console output at `info`.
///
/// Installed at process start — BEFORE the Postgres pool is created — so
/// connection-retry and migration logs are visible. Once the DB-backed
/// config is loaded, `init_tracing` replaces this subscriber with the full
/// one (DB-derived log_level + JSON file layer). The swap is safe: it
/// happens at startup, before any spans outlive it.
fn init_default_tracing() {
    use tracing_subscriber::{
        fmt,
        layer::{Layer, SubscriberExt},
        util::SubscriberInitExt,
    };

    let env_filter = build_log_filter(&tama_core::config::LogLevel::Info);
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_file(false)
                .with_line_number(false)
                .with_filter(env_filter),
        )
        .init();
}

/// Initialize the full tracing setup with two layers:
/// - Console: pretty-formatted output to stdout
/// - File: JSON lines written non-blockingly to tama.log with size-based rotation
///
/// Uses the DB-derived `log_level` + `logs_dir` (plan-190 Task 3: the
/// config comes from Postgres, so this runs after pool + migrations).
///
/// Returns the WorkerGuard that must be kept alive for the program's lifetime.
fn init_tracing(config: &Config) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, layer::Layer, layer::SubscriberExt, util::SubscriberInitExt};

    let env_filter = build_log_filter(&config.general.log_level);

    // Ensure logs directory exists
    let logs_dir = config
        .logs_dir()
        .with_context(|| "Failed to resolve logs directory from config")?;
    std::fs::create_dir_all(&logs_dir)
        .with_context(|| format!("Failed to create logs directory: {}", logs_dir.display()))?;

    // Size-based rotation check on startup (reuses constants from logging module)
    let log_path = logs_dir.join("tama.log");
    if log_path.exists() {
        if let Ok(meta) = std::fs::metadata(&log_path) {
            if meta.len() > tama_core::logging::MAX_LOG_SIZE {
                tama_core::logging::rotate_logs(&logs_dir, "tama")?;
            }
        }
    }

    // Open non-blocking file writer for JSON output
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    // Build two-layer subscriber
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_file(false)
                .with_line_number(false)
                .with_filter(env_filter.clone()),
        )
        .with(
            fmt::layer()
                .json()
                .with_writer(non_blocking)
                .with_filter(env_filter),
        )
        .init();

    Ok(guard)
}

#[cfg(test)]
mod tracing_tests {
    use super::build_log_filter;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::{Layer, SubscriberExt};

    /// Simple line collector implementing `io::Write` for test capture.
    #[derive(Clone)]
    struct Collector(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Collector {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.lock().unwrap().flush()
        }
    }

    use tama_core::config::LogLevel;

    /// A DB-configured `log_level` takes effect after startup: events are
    /// gated by the filter built from the level, through a real subscriber.
    #[test]
    fn test_build_log_filter_honors_log_level() {
        let cases: [(LogLevel, bool, bool); 4] = [
            (LogLevel::Debug, true, true),
            (LogLevel::Info, false, true),
            (LogLevel::Warn, false, false),
            (LogLevel::Error, false, false),
        ];
        for (level, expects_debug, expects_info) in cases {
            let filter = build_log_filter(&level);
            let collected: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
            let writer = Collector(collected.clone());
            let subscriber = tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || writer.clone())
                    .with_filter(filter),
            );
            tracing::subscriber::with_default(subscriber, || {
                tracing::debug!("marker-debug");
                tracing::info!("marker-info");
            });
            let lines = collected.lock().unwrap();
            let text = String::from_utf8_lossy(&lines).to_string();
            assert_eq!(
                text.contains("marker-debug"),
                expects_debug,
                "level {level:?} must {expects_debug:?} for DEBUG events"
            );
            assert_eq!(
                text.contains("marker-info"),
                expects_info,
                "level {level:?} must {expects_info:?} for INFO events"
            );
        }
    }
}
