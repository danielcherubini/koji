//! Tama server binary
//!
//! Bare `tama` starts the proxy server with web UI. The global app config is
//! loaded from Postgres; `config.toml` is a bootstrap file holding only the
//! `[database]` section (plan-190). `tama migrate` is the one-time v2→v3
//! cutover tool (plan-190 Task 10) — it dispatches before anything else and
//! never reads `config.toml`.

use anyhow::{Context, Result};
use clap::Parser;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tama_core::config::Config;
use tama_core::proxy::{ProxyServer, ProxyState};

mod admin;

#[cfg(feature = "ssr")]
mod log_runtime;

/// Bounded capacity of the structured-log capture channel (plan-195
/// task 3): the hot path is a non-blocking `try_send`; when full the
/// layer drops the newest event and bumps the shared drop counter.
const LOG_STORE_CHANNEL_CAPACITY: usize = 1024;

/// Set up HF_TOKEN environment variable from config if present.
fn setup_hf_token(config: &Config) {
    if let Some(token) = &config.general.hf_token {
        if !token.is_empty() {
            std::env::set_var("HF_TOKEN", token);
            tracing::info!("HF_TOKEN configured from config file");
        }
    }
}

fn main() -> Result<()> {
    // Dispatch on the subcommand BEFORE anything else: `migrate` takes its
    // own args and must never read config.toml or connect to Postgres the
    // way the server does.
    let cli = tama_web::cli::Cli::parse();
    match cli.command {
        Some(tama_web::cli::Command::Migrate(args)) => {
            let opts = tama_web::migrate::MigrateOpts::from_cli(args);
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("building tokio runtime for migrate")?;
            rt.block_on(tama_web::migrate::run(opts)).map(|_| ())
        }
        // plan-193 T6: `tama admin` is a CLI, not an SSR thing — the
        // dispatch is BEFORE any `ssr` feature gate (same shape as
        // `Migrate` above). Exit codes: 0 ok / 2 not-found / 13
        // budget-exhausted (the CLI literal matching the wire word
        // `budget_exhausted`) / 1 otherwise.
        Some(tama_web::cli::Command::Admin(args)) => {
            let (filter_handle, _file_writer) = init_default_tracing();
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("building tokio runtime for admin")?;
            match rt.block_on(admin::run(args, filter_handle)) {
                Ok(()) => Ok(()),
                Err(admin_error) => {
                    eprintln!("tama: {admin_error}");
                    std::process::exit(admin_error.exit_code);
                }
            }
        }
        None => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("building tokio runtime")?;
            rt.block_on(run_server())
        }
    }
}

async fn run_server() -> Result<()> {
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

    // Install the ONE global tracing subscriber (console + JSON file layers)
    // BEFORE the pool is created so connection-retry and migration logs are
    // visible. It starts at `info` with a dynamic `EnvFilter` behind a
    // `reload::Handle`; the JSON file layer writes through a
    // `SwappableFileWriter` that discards until the Postgres-backed config
    // provides `logs_dir`. After `Config::load_from_pool`, `init_tracing`
    // applies the DB-derived log_level and installs the real file writer —
    // no second global `.init()` (which would panic).
    //
    // The structured-log capture channel is created at the SAME instant
    // (the [`tama_core::logstore::LogStoreLayer`] needs the sender baked
    // into the global subscriber; a layer can't be grafted on later).
    // Records accumulate in the bounded channel (full → drop-newest) until
    // the writer task starts after `logs_dir` is known from config.
    let (logstore_tx, logstore_rx) =
        tokio::sync::mpsc::channel::<tama_core::logstore::LogRecord>(LOG_STORE_CHANNEL_CAPACITY);
    let logstore_dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
    // One clone for the tamad `StreamLogs` ingest wiring (plan-195
    // task 7, used by `TamadPool::set_log_tx` in the SSR block below);
    // the layer installed next takes the original.
    let tamad_ingest_tx = logstore_tx.clone();
    let (filter_handle, file_writer) =
        install_default_tracing(Some((logstore_tx, logstore_dropped.clone())));

    // The capture channel is only drained in the SSR build (where the web
    // bootstrap owns the writer's lifetime). The CSR-only build does not
    // run the server, so its channel legitimately fills and drops.
    #[cfg(not(feature = "ssr"))]
    let _ = logstore_rx;
    #[cfg(not(feature = "ssr"))]
    let _ = tamad_ingest_tx;

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

    // Apply the DB-derived log_level to the live subscriber (dynamic filter
    // via the reload handle) and install the real JSON file writer behind
    // the swappable writer. The guard must stay in scope for the program's
    // lifetime to keep the background writer thread alive — if dropped,
    // file logging silently stops. On normal exit, WorkerGuard::Drop
    // flushes remaining entries; on SIGKILL/panic-abort the last few
    // buffered lines may be lost (inherent trade-off of non-blocking
    // writes; console layer is synchronous).
    let _log_guard = init_tracing(&config, &filter_handle, &file_writer)?;

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

    // Load registered tamads into the pool and start a per-tamad stats
    // stream task for each (plan-191 Task 4). Failures are logged, never
    // fatal: stream tasks reconnect on their own, and the management API
    // re-upserts connections on register/update.
    if let Err(e) = proxy_state.tamad_pool().load_all().await {
        tracing::error!("Failed to load tamad pool at startup: {}", e);
    }

    #[cfg(feature = "ssr")]
    {
        // ── Structured log store runtime (plan-195 task 3) ──────────
        // The capture channel started at subscriber install is drained
        // by the writer task from now on. The returned
        // `crate::log_runtime::LogRuntime` is the WorkerGuard for
        // structured logging: it stays in scope until app exit
        // (dropping its JoinHandle / cancel token early silently stops
        // persisting — see the type's docs and `tama_core::logstore::
        // writer`), then cancel + await the final status after the
        // server is down. The writer never touches SSE; a
        // small bridge task publishes degraded transitions on the
        // `/tama/v1/logs/events` broadcast only (routed in task 4).
        let logs_path = log_store_path(&config)?;
        // Retention bounds from the (boot-loaded) General config —
        // see `start_log_runtime`'s docs for why they are boot-time
        // (changing them takes effect on the next boot).
        let log_retention = Some(tama_core::logstore::PruneBounds {
            max_age_secs: i64::from(config.general.log_retention_days) * 86400,
            max_rows: i64::try_from(config.general.log_retention_rows).unwrap_or(i64::MAX),
            max_bytes: i64::try_from(config.general.log_retention_max_mb)
                .map(|mb| mb * 1024 * 1024)
                .unwrap_or(i64::MAX),
        });
        let log_runtime = crate::log_runtime::start_log_runtime(
            &logs_path,
            logstore_rx,
            logstore_dropped,
            log_retention,
        )
        .await?;
        // Wire the per-tamad `StreamLogs` ingest (plan-195 task 7): the
        // pool's ingest tasks share this SAME capture channel — tamad
        // rows land in the store indistinguishable from the proxy's own
        // rows. Defensively a no-op when the pool has no (grpc) tamads
        // yet: the ingest tasks poll for the channel until set, and
        // `load_all` upserts handled the streaming side already.
        proxy_state.tamad_pool().set_log_tx(tamad_ingest_tx);
        // The web state's `/tama/v1/logs/status` receiver — cloned BEFORE the
        // read endpoint below is moved out of the runtime.
        let log_status_rx = log_runtime.status_rx();

        // Status → SSE bridge: a small task owns a `status_rx` clone
        // and, on degraded transitions only, pushes self-describing
        // JSON onto the per-endpoint broadcast held in
        // `WebState.log_events_tx` (mirrors the `update_tx` pattern —
        // the SSE handler creates the sender when a client connects).
        let log_events_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        {
            let mut bridge_rx = log_runtime.status_rx();
            let bridge_tx = log_events_tx.clone();
            tokio::spawn(async move {
                let mut was_degraded = false;
                while bridge_rx.changed().await.is_ok() {
                    let status = *bridge_rx.borrow_and_update();
                    let frame = if status.degraded && !was_degraded {
                        Some(serde_json::json!({
                            "event": "log_store_degraded",
                            "since": status.degraded_since,
                            "channel_len": status.channel_len,
                            "ring_len": status.ring_len,
                        }))
                    } else if !status.degraded && was_degraded {
                        Some(serde_json::json!({
                            "event": "log_store_restored",
                            "had_entries": status.channel_len + status.ring_len,
                            "ring_flushed": status.ring_len == 0,
                        }))
                    } else {
                        None
                    };
                    if let Some(frame) = frame {
                        publish_log_event(&bridge_tx, frame).await;
                    }
                    was_degraded = status.degraded;
                }
            });
        }

        // Read API (plan-195 task 4): the read-endpoint move into the web
        // state here (second connection to `tama-logs.db`; the read
        // endpoints never touch the writer connection, WAL: 1 writer +
        // N readers). The legacy on-demand tail provider (tamad engine
        // log + local `*.log`) goes through TTL cache, so concurrent UI
        // polls repeat the fetch.
        let log_read = std::sync::Arc::new(std::sync::Mutex::new(log_runtime.reader));
        let log_tail: std::sync::Arc<dyn tama_core::proxy::tama_handlers::LogTailProvider> =
            std::sync::Arc::new(tama_core::proxy::tama_handlers::CachingTailProvider::new(
                std::sync::Arc::new(tama_core::proxy::tama_handlers::TamadTailProvider::new(
                    proxy_state.tamad_pool(),
                    Some(
                        config
                            .logs_dir()
                            .unwrap_or_else(|_| db_dir.clone().unwrap_or_default().join("logs")),
                    ),
                )),
            ));

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
                log_filter: Some(filter_handle.clone()),
                log_status: Some(Arc::new(log_status_rx)),
                log_events_tx: log_events_tx.clone(),
                log_read: Some(log_read),
                log_tail: Some(log_tail),
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

        // Clone app state for shutdown cleanup (unloads TTS backends)
        let cleanup_state = Arc::clone(&app_state);
        let on_shutdown = async move {
            // Unload TTS backends
            let tts_backends: Vec<String> = cleanup_state.state.tts_backend_names().await;
            for name in tts_backends {
                if let Err(e) = cleanup_state.state.unload_model(&name).await {
                    tracing::warn!("Failed to unload TTS backend '{}': {}", name, e);
                }
            }
        };

        // Take the writer JoinHandle out of the runtime so it can be
        // awaited below; the cancel token + reader live on in
        // `log_runtime` until app exit (WorkerGuard rule).
        let writer_handle = log_runtime.writer_handle;

        // Use the listener module which handles OS signals + graceful shutdown
        tama_core::proxy::server::listener::run(app, addr, Some(on_shutdown), None).await?;

        // ── Log writer WorkerGuard (the `LogRuntime`, plan-195 task 3):
        // the guard's token + reader stayed in scope until the last
        // event could be logged; now cancel + await the final status
        // (holding them longer would lose nothing). ──
        log_runtime.writer_token.cancel();
        match tokio::time::timeout(std::time::Duration::from_secs(5), writer_handle).await {
            Ok(Ok(final_status)) => {
                if final_status.degraded {
                    tracing::warn!(
                        retries_seen = final_status.retries_seen,
                        "log store writer exited DEGRADED — check disk/db health"
                    );
                } else {
                    tracing::debug!(
                        dropped_count = final_status.dropped_count,
                        "log store writer drained cleanly"
                    );
                }
            }
            Ok(Err(e)) => tracing::warn!("log store writer task failed: {e}"),
            Err(_) => tracing::warn!("log store writer did not join within 5s"),
        }
        Ok(())
    }

    #[cfg(not(feature = "ssr"))]
    {
        // CSR-only build: nothing to run (web UI is handled by browser)
        Ok(())
    }
}

/// Path of the SQLite structured log store: `tama-logs.db` under the
/// resolved `logs_dir` (the same `<logs_dir or base_dir/logs>`
/// resolution the JSON log file uses).
fn log_store_path(config: &Config) -> Result<std::path::PathBuf> {
    config
        .logs_dir()
        .with_context(|| "Failed to resolve logs directory from config")
        .map(|dir| dir.join("tama-logs.db"))
}

/// Publish one self-describing JSON frame onto the `/tama/v1/logs/events`
/// per-endpoint broadcast. `None` (no SSE client connected) and send
/// failures (the listener went away) are normal and silently dropped —
/// events are never re-published.
async fn publish_log_event(
    tx: &Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>>,
    frame: serde_json::Value,
) {
    let guard = tx.lock().await;
    if let Some(sender) = guard.as_ref() {
        let _ = sender.send(frame.to_string());
    }
}

/// JSON file writer that discards writes until the real non-blocking writer
/// is installed.
///
/// The global subscriber must be installed at process start — BEFORE the
/// Postgres pool exists — but the log file path comes from the DB-backed
/// config (`logs_dir`), so the file can't be opened yet. The JSON layer is
/// therefore wired to this writer from the start: it drops entries until
/// `install` is called after `Config::load_from_pool`, then forwards to the
/// real file. This keeps exactly ONE global subscriber init per process
/// (a second `SubscriberInitExt::init()` would panic).
#[derive(Clone)]
struct SwappableFileWriter {
    inner: Arc<Mutex<Option<tracing_appender::non_blocking::NonBlocking>>>,
}

impl SwappableFileWriter {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Install the real non-blocking writer. The returned `WorkerGuard`
    /// must be kept alive for the process lifetime.
    fn install(&self, writer: tracing_appender::non_blocking::NonBlocking) {
        *self.inner.lock().unwrap() = Some(writer);
    }
}

impl std::io::Write for SwappableFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(writer) = &mut *self.inner.lock().unwrap() {
            writer.write(buf)
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(writer) = &mut *self.inner.lock().unwrap() {
            writer.flush()
        } else {
            Ok(())
        }
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for SwappableFileWriter {
    type Writer = SwappableFileWriter;

    fn make_writer(&self) -> Self::Writer {
        self.clone()
    }
}

/// Install the SINGLE global tracing subscriber for the process.
///
/// Two layers, both gated by a dynamic `EnvFilter` (initially `info`) held
/// behind a `reload::Handle`:
/// - Console: pretty-formatted output to stdout
/// - File: JSON lines through a `SwappableFileWriter` (discarding until the
///   real writer is installed by `init_tracing` after the DB config load)
///
/// When `log_capture` is `Some((tx, dropped_counter))` (the proxy server
/// path — plan-195 task 3), a `LogStoreLayer` is added so events also flow
/// into the bounded channel the writer task drains. It MUST be baked in at
/// install — a layer can't be grafted onto a global subscriber afterwards.
///
/// Returns the filter handle (to apply the DB-derived log_level) and the
/// swappable file writer (to install the real log file). Must be called
/// exactly once — a second `init()` would panic.
fn install_default_tracing(
    log_capture: Option<(
        tokio::sync::mpsc::Sender<tama_core::logstore::LogRecord>,
        Arc<std::sync::atomic::AtomicU64>,
    )>,
) -> (
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>,
    SwappableFileWriter,
) {
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, reload, util::SubscriberInitExt, Registry,
    };

    let env_filter =
        tama_core::logstore::filter::build_log_filter(&tama_core::config::LogLevel::Info, "")
            .expect("building the startup log filter (empty directives always parse)");
    let (filter, filter_handle) = reload::Layer::new(env_filter);
    let file_writer = SwappableFileWriter::new();

    match log_capture {
        Some((store_tx, dropped)) => {
            Registry::default()
                .with(filter)
                .with(tama_core::logstore::build_layer(
                    store_tx,
                    tama_core::logstore::Source::proxy(),
                    dropped,
                ))
                .with(
                    fmt::layer()
                        .with_target(false)
                        .with_file(false)
                        .with_line_number(false),
                )
                .with(fmt::layer().json().with_writer(file_writer.clone()))
                .init();
        }
        None => {
            Registry::default()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_target(false)
                        .with_file(false)
                        .with_line_number(false),
                )
                .with(fmt::layer().json().with_writer(file_writer.clone()))
                .init();
        }
    }

    (filter_handle, file_writer)
}

/// Console-only install for the `tama admin` CLI (no structured-log
/// capture: the CLI is short-lived and its output is the console).
fn init_default_tracing() -> (
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>,
    SwappableFileWriter,
) {
    install_default_tracing(None)
}

/// Apply the DB-derived tracing configuration to the live subscriber:
/// - Update the dynamic `EnvFilter` to `config.general.log_level`
/// - Install the real non-blocking JSON file writer (hourly-rolling
///   `tama.log.*` via `RollingFileAppender`, plan-195 task 4) behind the
///   swappable writer
///
/// No second global `.init()` — the subscriber installed by
/// `init_default_tracing` serves the whole process (plan-190 Task 3: the
/// config comes from Postgres, so this runs after pool + migrations).
///
/// Returns the WorkerGuard that must be kept alive for the program's lifetime.
fn init_tracing(
    config: &Config,
    filter_handle: &tracing_subscriber::reload::Handle<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::Registry,
    >,
    file_writer: &SwappableFileWriter,
) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    // Apply the DB-derived log level + durable directives to the live
    // subscriber (`build_log_filter` merges `RUST_LOG` internally).
    let log_directives = config.general.log_directives.clone().unwrap_or_default();
    let env_filter =
        tama_core::logstore::filter::build_log_filter(&config.general.log_level, &log_directives)
            .with_context(|| {
            format!("building the log filter from the DB config (directives: {log_directives:?})")
        })?;
    filter_handle
        .modify(|f| *f = env_filter)
        .context("updating dynamic log-level filter after DB config load")?;

    // Ensure logs directory exists
    let logs_dir = config
        .logs_dir()
        .with_context(|| "Failed to resolve logs directory from config")?;
    std::fs::create_dir_all(&logs_dir)
        .with_context(|| format!("Failed to create logs directory: {}", logs_dir.display()))?;

    // Hourly rolling file writer for the JSON output (plan-195 task 4) —
    // replaces the manual size-based one-shot rotation at boot: the
    // rolling appender rotates at every hour boundary in-process and
    // prunes to the newest 24 files. `filename_prefix("tama.log")` +
    // HOURLY yields `tama.log.<YYYY-MM-DD-HH>`-style files in
    // `<logs_dir>`; the swappable install keeps the before-config window
    // dropping as before.
    let appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::HOURLY)
        .max_log_files(24)
        .filename_prefix("tama.log")
        .build(&logs_dir)
        .with_context(|| {
            format!(
                "Failed to create the rolling log writer in {}",
                logs_dir.display()
            )
        })?;

    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    file_writer.install(non_blocking);

    Ok(guard)
}

#[cfg(test)]
mod tracing_tests {
    use super::SwappableFileWriter;
    use std::io::Write;
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
    use tama_core::logstore::filter::build_log_filter;

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
            let filter = build_log_filter(&level, "").expect("valid filter from config level");
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

    /// The DB-configured `log_level` is applied to the live subscriber via
    /// the `reload::Handle` AFTER startup — exactly one global subscriber
    /// exists; events before the update are gated by the initial level,
    /// events after by the DB-derived level.
    #[test]
    fn test_reload_handle_applies_db_log_level_after_startup() {
        use tracing_subscriber::{reload, Registry};

        let collected: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let writer = Collector(collected.clone());
        let (filter, handle) =
            reload::Layer::new(build_log_filter(&LogLevel::Info, "").expect("valid filter"));
        let subscriber = Registry::default().with(filter).with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone()),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!("pre-update-debug");
            handle
                .modify(|f| *f = build_log_filter(&LogLevel::Debug, "").expect("valid filter"))
                .expect("reload handle must work against a live subscriber");
            tracing::debug!("post-update-debug");
            tracing::info!("post-update-info");
        });
        let text = String::from_utf8_lossy(&collected.lock().unwrap()).to_string();
        assert!(
            !text.contains("pre-update-debug"),
            "initial info filter must gate DEBUG before the DB level is applied"
        );
        assert!(
            text.contains("post-update-debug"),
            "DB log_level=debug must open DEBUG events after startup"
        );
        assert!(text.contains("post-update-info"));
    }

    /// The swappable file writer discards writes until the real non-blocking
    /// writer is installed (after DB config load), then forwards — so the
    /// JSON layer is installed once at process start and only the writer is
    /// swapped in, never a second global subscriber.
    #[test]
    fn test_swappable_file_writer_discards_then_forwards() {
        let collected: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let mut sw = SwappableFileWriter::new();
        sw.write_all(b"pre-install").unwrap();
        let (non_blocking, guard) = tracing_appender::non_blocking(Collector(collected.clone()));
        sw.install(non_blocking);
        sw.write_all(b"post-install").unwrap();
        drop(guard); // flush the background writer thread
        let text = String::from_utf8_lossy(&collected.lock().unwrap()).to_string();
        assert!(
            !text.contains("pre-install"),
            "writes before install must be discarded"
        );
        assert!(
            text.contains("post-install"),
            "writes after install must be forwarded"
        );
    }
}
