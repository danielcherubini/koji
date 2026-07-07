//! Tama server binary
//!
//! Starts the proxy server with web UI. All configuration is loaded from the
//! config file — no CLI arguments are accepted.

use anyhow::Result;
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
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load configuration
    let config = Config::load()?;

    // Set up HF_TOKEN from config before any hf_hub usage
    setup_hf_token(&config);

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

    // Database setup and migrations
    let db_dir = Config::config_dir().ok();
    if let Some(ref dir) = db_dir {
        match tama_core::db::open(dir) {
            Ok(db_result) => {
                if db_result.needs_backfill {
                    tracing::info!("Running initial backfill...");
                    if let Err(e) =
                        tama_core::db::backfill::run_initial_backfill(&db_result.conn, &config)
                            .await
                    {
                        tracing::error!("Initial backfill failed: {}", e);
                    }
                }

                // Always run the backend registry TOML migration (runs once, then renames the file)
                if let Err(e) =
                    tama_core::db::backfill::migrate_backend_registry_toml(&db_result.conn, dir)
                {
                    tracing::error!("Backend registry TOML migration failed: {}", e);
                }

                // Run unified TOML → DB migration
                let db_path = dir.join("tama.db");
                if let Err(e) = tama_core::db::backfill::migrate_toml_to_db(dir, &db_path) {
                    tracing::error!("TOML → DB migration failed: {}", e);
                }
            }
            Err(e) => tracing::error!("Failed to open DB for backfill check: {}", e),
        }
    }

    // Create shared proxy state
    let proxy_state = Arc::new(ProxyState::new(config.clone(), db_dir));

    #[cfg(feature = "ssr")]
    {
        // Ensure logs directory exists
        if let Ok(logs_dir) = config.logs_dir() {
            let _ = std::fs::create_dir_all(&logs_dir);
        }

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
            let models = cleanup_state.state.models().read().await;
            let tts_backends: Vec<String> = models
                .iter()
                .filter(|(_, ms)| ms.is_tts_backend())
                .map(|(name, _)| name.clone())
                .collect();
            drop(models);
            for name in tts_backends {
                if let Err(e) = cleanup_state.state.unload_tts_backend(&name).await {
                    tracing::warn!("Failed to unload TTS backend '{}': {}", name, e);
                }
            }
        };

        // Use the listener module which handles OS signals + graceful shutdown
        tama_core::proxy::server::listener::run(app, addr, Some(on_shutdown), None).await
    }
}
