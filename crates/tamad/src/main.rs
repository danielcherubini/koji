use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

mod bench;
mod compaction_server;
mod download;
mod gpu;
mod host_installs;
mod installs;
mod jobs;
mod lifecycle;
mod process;
mod process_table;
mod pulls;
mod register;
mod server;
mod state;
mod stats;

use process_table::ProcessTable;
use register::Registrar;
use state::TamadState;

#[derive(Debug, Clone)]
struct CliArgs {
    addr: String,
    protocol: String,
    name: Option<String>,
    public_url: Option<String>,
    models_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    /// `--no-replay-desired`: the boot sweep (the replay of `desired`
    /// models from the host-disk store, plan-193 T2) is OFF. The
    /// default (flag absent) = the sweep is ON. The plan's one
    /// operational switch (an operator deploy-safety valve — there is no
    /// proxy-side toggle, so "no feature flag" holds).
    no_replay_desired: bool,
}

impl CliArgs {
    fn from_args() -> Result<Self> {
        let mut addr = "0.0.0.0:50051".to_string();
        let mut protocol = "grpc".to_string();
        let mut name: Option<String> = None;
        let mut public_url: Option<String> = None;
        let mut models_dir: Option<PathBuf> = None;
        let mut data_dir: Option<PathBuf> = None;
        let mut no_replay_desired = false;

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--addr" => {
                    addr = args.next().unwrap_or_else(|| "0.0.0.0:50051".to_string());
                }
                "--protocol" => {
                    protocol = args.next().unwrap_or_else(|| "grpc".to_string());
                }
                "--name" => {
                    name = args.next();
                }
                "--public-url" => {
                    public_url = args.next();
                }
                "--models-dir" => {
                    models_dir = args.next().map(PathBuf::from);
                }
                "--data-dir" => {
                    data_dir = args.next().map(PathBuf::from);
                }
                "--no-replay-desired" => {
                    no_replay_desired = true;
                }
                _ => {
                    eprintln!("Unknown argument: {}", arg);
                    std::process::exit(1);
                }
            }
        }

        Ok(Self {
            addr,
            protocol,
            name,
            public_url,
            models_dir,
            data_dir,
            no_replay_desired,
        })
    }
}

#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = CliArgs::from_args()?;
    let state = Arc::new(TamadState::from_cli(&args)?);
    info!(
        addr = %args.addr,
        protocol = %args.protocol,
        name = %state.name,
        public_url = %state.public_url,
        "Starting tamad daemon"
    );

    if let (Some(proxy_url), Some(proxy_token)) = (&state.proxy_url, &state.proxy_token) {
        let registrar = Registrar::new(
            proxy_url.clone(),
            proxy_token.clone(),
            state.name.clone(),
            state.public_url.clone(),
            state.protocol.clone(),
            state.token().to_string(),
        );
        tokio::spawn(registrar.run_loop());
    }

    let process_table = Arc::new(ProcessTable::default());
    let lifecycle = Arc::new(crate::lifecycle::TamadLifecycle::new(
        Arc::clone(&process_table),
        Arc::clone(&state.store),
        Arc::clone(&state),
    ));

    // Reap docker containers left on THIS HOST by a previous crashed
    // instance (the proxy used to do this on its own startup; now the host
    // owner — the tamad — does it). No-ops when Docker is unavailable.
    let startup = crate::host_installs::docker::startup_reconcile();
    tokio::spawn(async move {
        if let Err(e) = startup.await {
            warn!(error = %e, "docker startup reconciliation failed (continuing)");
        }
    });

    // Respawn supervisor (plan-193 T2): The dead-PID restraint only
    // enqueue-respawn jobs; this long-lived task dispatches each job
    // via `lifecycle.load`. It is started before the gRPC listen, so
    // that even models that crash in-flight at boot time already have
    // a decision path in motion.
    let _respawn_supervisor =
        crate::lifecycle::TamadLifecycle::start_respawn_supervisor(&lifecycle);

    // Reconciliation sweep for orphaned `starting` rows (plan-194):
    // defense in depth — adopts a healthy orphan as verified-ready and
    // tears down one past its health deadline, every 5 seconds.
    let _reconciler = crate::lifecycle::TamadLifecycle::start_starting_reconciler(&lifecycle);

    {
        // Boot sweep (plan-193 T2 entry): If desired, re-fire all
        // stored lines as running processes (started up — slow models
        // must not block the listen). `--no-replay-desired` turns the
        // sweep off; that path emits exactly one line of log.
        let lifecycle = Arc::clone(&lifecycle);
        let replay = !args.no_replay_desired;
        tokio::spawn(async move {
            lifecycle.replay_desired(replay).await;
        });
    }

    let mut server_task = tokio::spawn({
        let addr = args.addr.clone();
        let protocol = args.protocol.clone();
        let state = Arc::clone(&state);
        let process_table = Arc::clone(&process_table);
        let lifecycle = Arc::clone(&lifecycle);
        async move { server::start(&addr, &protocol, state, process_table, lifecycle).await }
    });

    // Graceful shutdown (plan-191 follow-up A): on SIGTERM/SIGINT the daemon
    // kills every backend process group before exiting — backends must never
    // be left orphaned on this host.
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let terminate = async {
        #[cfg(unix)]
        {
            let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
            term.recv().await
        }
        #[cfg(not(unix))]
        {
            std::future::pending().await
        }
    };

    tokio::select! {
        r = &mut server_task => {
            if let Err(e) = r {
                tracing::error!(error = %e, "server task failed; shutting down");
            }
        }
        _ = &mut ctrl_c => info!("Ctrl-C received; shutting down"),
        _ = terminate => info!("SIGTERM received; shutting down"),
    }

    info!("Stopping all backend process groups before exit");
    if let Err(e) = lifecycle.kill_all().await {
        warn!(error = %e, "kill_all reported errors; exiting anyway");
    }
    server_task.abort();
    Ok(())
}
