//! Structured log-store runtime (plan-195 task 3) — the SSR-boot wiring
//! for the SQLite structured log store (`tama-logs.db`) the SSR boot
//! drains into.
//!
//! The capture channel + the log-store layer are installed on the global
//! tracing subscriber at process start (before the Postgres config
//! provides `logs_dir`); records accumulate in the bounded channel
//! (full -> drop-newest) until [`start_log_runtime`] opens the store and
//! spawns the writer task that drains it. Batching, journal-style
//! degradation, and drop-marker details live in
//! `tama_core::logstore::writer`.
//!
//! ## Ownership (WorkerGuard rule)
//!
//! The returned [`LogRuntime`] is the WorkerGuard for structured logging
//! — the same rule as the JSON-file `WorkerGuard` in
//! `crates/tama/src/main.rs`: the writer `JoinHandle` / cancel token
//! STAY in scope until app exit (dropping them early silently stops
//! persisting — the remaining channel records are never persisted),
//! then cancel + await the final status after the server is down (see
//! the `tama_core::logstore::writer` module's shutdown notes).

use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use anyhow::{Context, Result};
use tama_core::logstore::{
    spawn_log_writer_with_config, LogRecord, LogStore, LogStoreStatus, PruneBounds, WriterConfig,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Everything the server holds to own the structured log runtime until
/// app exit (see the module docs for the WorkerGuard rule).
pub struct LogRuntime {
    /// Separate log-read connection (`tama-logs.db`). The writer owns
    /// the store's single write connection; WAL allows one writer + N
    /// readers — task 4's read endpoints use this handle.
    pub reader: LogStore,
    /// Cancel token for the writer — cancel only at shutdown.
    pub writer_token: CancellationToken,
    /// The writer `JoinHandle`; awaits the final [`LogStoreStatus`] after
    /// cancel.
    pub writer_handle: JoinHandle<LogStoreStatus>,
    status_tx: watch::Sender<LogStoreStatus>,
}

impl LogRuntime {
    /// Fresh receiver on the writer's status broadcast (SP/MP snapshot
    /// pattern — AGENTS.md). WebState holds one for `/tama/v1/logs`
    /// status reads; the SSE bridge holds its own clone.
    pub fn status_rx(&self) -> watch::Receiver<LogStoreStatus> {
        self.status_tx.subscribe()
    }
}

/// Boot the structured log runtime at `logs_path` (i.e. `tama-logs.db`
/// under the resolved `logs_dir`) and spawn the writer task draining
/// `rx` (with the `dropped` counter) into it. This is EXACTLY the boot
/// wiring `run_server` inlined pre-extraction (plan-195 task 3):
/// open the writer store connection, open the separate read connection,
/// report the preexisting rows, create the status watch channel + the
/// writer's cancel token, and `spawn_log_writer` (with the retention
/// configuration below).
///
/// ## Retention
///
/// `retention` is the store's retention (`PruneBounds`), applied by
/// the writer task itself on its single write connection (SQLite WAL
/// has exactly one writer; a separate pruning connection would fight
/// it for the WAL exclusive lock). It runs at most once per
/// `WriterConfig::prune_interval` (1 h) and the deleted count is
/// surfaced as `LogStoreStatus::last_prune_deleted` on the status
/// broadcast (and, through it, on `GET /tama/v1/logs/status`).
///
/// **Boot-loaded:** the bounds are read from config once at boot —
/// changing `general.log_retention_days` / `log_retention_rows` /
/// `log_retention_max_mb` takes effect on the NEXT boot (live reload
/// of retention is future work; config-save does not re-configure the
/// already-running writer). `None` = feature off (legacy/tests).
pub async fn start_log_runtime(
    logs_path: &Path,
    rx: mpsc::Receiver<LogRecord>,
    dropped: Arc<AtomicU64>,
    retention: Option<PruneBounds>,
) -> Result<LogRuntime> {
    // The writer owns the store's SINGLE write connection.
    let writer_store = LogStore::open(logs_path)
        .with_context(|| format!("opening the log store at {}", logs_path.display()))?;
    // A second, separate connection for the read endpoints (task 4)
    // — WAL allows one writer + N readers; same PRAGMAs via open.
    let reader = LogStore::open(logs_path).with_context(|| "opening the log-read connection")?;
    let preexisting_rows = reader.last_id().map(|id| id.unwrap_or(0)).unwrap_or(0);
    tracing::info!(
        path = %logs_path.display(),
        preexisting_rows,
        "structured log store ready"
    );

    let (status_tx, _) = watch::channel(LogStoreStatus::ok());
    let writer_token = CancellationToken::new();
    // Retention rides the writer's own config (writer tests keep the
    // interval injectable; the 1 h production cadence is the
    // `WriterConfig::default()` and nothing here overrides it).
    let writer_config = WriterConfig {
        retention,
        ..Default::default()
    };
    let writer_handle = spawn_log_writer_with_config(
        writer_config,
        writer_store,
        rx,
        dropped,
        status_tx.clone(),
        writer_token.clone(),
    );

    Ok(LogRuntime {
        reader,
        writer_token,
        writer_handle,
        status_tx,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;
    use std::time::Instant;

    use super::start_log_runtime;
    use tama_core::logstore::{build_layer, LogRecord, LogStore, Source};
    use tokio::sync::mpsc;
    use tokio::time::{sleep, timeout, Duration};
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    /// Boot acceptance (plan-195 task 3) against a fixture logs dir:
    /// booting creates `<logs_dir>/tama-logs.db`, a startup-level
    /// `info!` lands in the table (layer -> channel -> writer -> SQLite,
    /// verified through a FRESH read connection, as a fresh consumer
    /// such as task 4's endpoints would see it), and canceling the
    /// writer token joins it cleanly with no degradation.
    ///
    /// Determinism: the routing guard is scoped with `with_default`
    /// to a closure containing ONLY the emit (the thread-local default
    /// is pushed, the event dispatched, and the guard popped with no
    /// yield in between — no reliance on a longer-lived guard or on
    /// which thread two separate statements happen to run on), and the
    /// poll loop exits only when OUR smoke row is visible (not on any
    /// row count — `start_log_runtime`'s own "structured log store
    /// ready" row reaches the same channel and the writer flushes a
    /// lone record immediately, so it can land first).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_boot_persists_startup_row_and_drains_cleanly() {
        let logs_dir = tempfile::tempdir().expect("temp logs dir");
        let logs_path = logs_dir.path().join("tama-logs.db");

        // The capture layer goes into a test-local subscriber (a bare
        // `Layered<LogStoreLayer, Registry>`) — deliberately NOT the full
        // `install_default_tracing` / `init_tracing` path: what is under
        // test is the layer -> writer persistence wiring. The receiver
        // goes to the writer this test's boot starts. The layer is
        // CLONED (cheap: channel sender + atomic) for the emit
        // subscriber because the subscriber itself is NOT `Clone`
        // (Registry isn't, and only Box<L> gets a Layer impl in
        // tracing-subscriber 0.3).
        let (tx, rx) = mpsc::channel::<LogRecord>(1024);
        let dropped = Arc::new(AtomicU64::new(0));
        let layer = build_layer(tx, Source::proxy(), dropped.clone());
        let emit_subscriber = Registry::default().with(layer.clone());

        let runtime = start_log_runtime(&logs_path, rx, dropped, None)
            .await
            .expect("boot the log runtime");

        // Startup row at the default level (info), as boot logging emits —
        // guarded for the duration of the emit ONLY. The emit
        // subscriber is dropped afterwards, leaving `layer` as the
        // channel sender's last owner.
        tracing::subscriber::with_default(emit_subscriber, || {
            tracing::info!(target: "log_runtime_test", "boot smoke row");
        });

        // Poll a freshly opened connection (real-time, 50 ms steps, 2 s
        // deadline, early exit on success) until the smoke row is
        // visible. The exit condition is the smoke ROW, not an id/count:
        // `start_log_runtime` has already emitted "structured log store
        // ready" through this same layer, and the writer flushes a
        // lone record immediately — the boot row can be persisted
        // milliseconds before our record is flushed.
        let started = Instant::now();
        let deadline = Duration::from_secs(2);
        let mut saw_smoke_row = false;
        while !saw_smoke_row {
            let probe = LogStore::open(&logs_path).expect("fresh probe open");
            let (entries, _) = probe.query(&Default::default()).expect("probe query");
            saw_smoke_row = entries
                .iter()
                .any(|e| e.msg.get("message") == Some(&serde_json::json!("boot smoke row")));
            if !saw_smoke_row {
                assert!(
                    started.elapsed() < deadline,
                    "writer must persist the startup row within 2 s"
                );
                sleep(Duration::from_millis(50)).await;
            }
        }

        // The runtime's own read connection (used by task 4's
        // endpoints) sees it too, and it is the event we emitted
        // (message + target). No exact row count is asserted — the
        // boot row of `start_log_runtime` may sit alongside it.
        let (entries, _) = runtime
            .reader
            .query(&Default::default())
            .expect("query the smoke row");
        assert!(
            entries.iter().any(|e| {
                e.msg.get("message") == Some(&serde_json::json!("boot smoke row"))
                    && e.msg.get("target") == Some(&serde_json::json!("log_runtime_test"))
            }),
            "the captured startup row must be the tracing info! event"
        );
        assert!(
            std::fs::metadata(&logs_path).is_ok(),
            "tama-logs.db must exist under the resolved logs_dir"
        );

        // Drop the layer BEFORE cancel: with the emit subscriber gone,
        // it is the channel sender's last owner, so dropping it closes
        // the channel — the writer's post-cancel drain sees the channel
        // CLOSED and finishes immediately (real shutdown has the same
        // shape — the process and its subscriber go away). Otherwise the
        // post-cancel drain waits out its 2 s bound.
        drop(layer);
        runtime.writer_token.cancel();
        let final_status = timeout(Duration::from_secs(5), runtime.writer_handle)
            .await
            .expect("writer must join within 5 s of cancel")
            .expect("writer task must not fail");
        assert!(
            !final_status.degraded,
            "boot + drain must end healthy, not degraded"
        );
        assert!(
            final_status.last_prune_deleted.is_none(),
            "no retention bounds configured → no prune runs at boot"
        );
    }
}
