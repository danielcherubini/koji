/// Start the Tama web control plane UI server.
#[cfg(feature = "web-ui")]
pub async fn cmd_web(
    port: u16,
    _proxy_url: String,
    _logs_dir: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;

    // Load config from the default SQLite database
    let config = tama_core::config::Config::load().unwrap_or_default();

    let state = Arc::new(tama_core::proxy::ProxyState::new(config, None));

    // Set web-specific fields
    let mut state_inner = (*state).clone();
    state_inner.web_jobs = Some(Arc::new(tama_core::web_types::JobManager::new()));
    state_inner.web_capabilities = Some(Arc::new(tama_core::web_types::CapabilitiesCache::new()));
    state_inner.web_binary_version = env!("CARGO_PKG_VERSION").to_string();
    let state = Arc::new(state_inner);

    let jobs_for_shutdown = state.web_jobs.clone();
    let app = tama_web::router::build_web_routes().with_state(state);
    tracing::info!("Tama web UI listening on http://{}", addr);

    // Use axum-server for timeout-based graceful shutdown.
    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();

    tokio::spawn(async move {
        // Wait for Ctrl+C in standalone mode
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Web UI initiating graceful shutdown (timeout: 5s)...");
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));

        // Cleanup: kill all child processes for active jobs
        if let Some(jobs) = jobs_for_shutdown {
            if let Some(active_job) = jobs.active().await {
                tracing::info!("Killing children of active job {}...", active_job.id);
                jobs.kill_children(&active_job).await;
            }
        }
    });

    // axum_server::from_tcp takes a std::net::TcpListener
    let std_listener = std::net::TcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;

    let result = axum_server::from_tcp(std_listener)
        .handle(handle)
        .serve(app.into_make_service())
        .await;

    result?;
    Ok(())
}
