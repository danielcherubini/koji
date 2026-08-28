//! The writer task: drains the bounded record channel into the
//! [`LogStore`] in batches, with journaled-style degradation
//! (plan-195 task 2).
//!
//! ## Ownership and blocking posture
//!
//! The layer (hot path) does ONLY a `try_send` (drop-newest policy, no
//! backpressure). The writer task owns the store's sole connection, all
//! batching policy, the in-memory degradation ring, the dropped-event
//! marker, and the status broadcast. Every `insert_batch` runs inside
//! `tokio::task::spawn_blocking` — rusqlite is synchronous, and a slow
//! disk must not stall runtime worker threads. (`prune`/`delete_all`
//! have the same posture in task 4's endpoints.)
//!
//! ## Degradation (journaled-style)
//!
//! On a failed `insert_batch`, the writer enters DEGRADED: the failed
//! batch is kept as the pending retry set (retried every
//! `retry_interval`), and every arrival while degraded is admitted to a
//! bounded in-memory ring (FIFO, capped by `ring_max_entries` and
//! `ring_max_bytes`). Admission: `level >= WARN` OR `msg.dropped ==
//! true` (drop markers regardless of level); info-and-below arrivals
//! are discarded. On the first successful retry, the ring drains
//! oldest-first through the normal inserts, bound by `drain_timeout`
//! (on elapsed bound, the rest is discarded with a WARN, and the
//! `ring_discarded` counter bumps). State transitions broadcast on
//! [`LogStoreStatus`] via `watch::Sender` — the SP/MP snapshot pattern
//! (AGENTS.md); channel_len/ring_len ride the 1 s heartbeat at most.
//!
//! ## Drop markers
//!
//! When the shared drop counter (bumped by the layer on channel-full
//! drops) stays `> 0` for `drop_marker_window` (measured in
//! `tokio::time` — that is, real wall clock in production, testable
//! under `time::pause`), the writer itself ENQUEUES a synthetic
//! `LogRecord` —
//! `{"message": "log store: dropped N events since <ts>", "dropped":
//! true, "dropped_count": N, "dropped_since_ts": "<rfc3339>"}`, source
//! `log-store`, level WARN — and resets the counter. It flows through
//! the normal insert path: downstream, an unremarkable row.
//!
//! ## Retention
//!
//! When `WriterConfig::retention` is `Some(bounds)`, the writer also
//! owns the store's retention prune: every tick it checks whether the
//! virtual clock reached `last_prune + prune_interval` (default 1 h;
//! `None` bounds = feature off, e.g. tests), and when due it runs
//! `LogStore::prune` on the SAME single write connection, in the same
//! `spawn_blocking` posture as the inserts (rusqlite is sync; the due
//! timestamp still advances on failure — no retry storm, the next due
//! tick picks it up again). The deleted count lands on the status
//! broadcast as `LogStoreStatus::last_prune_deleted`. The design is
//! bound to the writer task by SQLite's single-writer WAL contract:
//! a second connection issuing `DELETE` would fight this one on WAL
//! exclusivity.
//!
//! ## Shutdown — read this before wiring (task 3)
//!
//! `CancellationToken`-driven: on cancel, stop receiving, drain the
//! remaining channel records, and persist everything pending under a
//! 2 s bound. **WorkerGuard rule (same as `WorkerGuard` in
//! `crates/tama/src/main.rs`): dropping the returned `JoinHandle` /
//! cancel token BEFORE the app exits silently stops logging — the
//! remaining channel records are never persisted.** Hold the guard
//! until the last event could be logged, then cancel + await the final
//! status.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::logstore::db::LogStore;
use crate::logstore::types::{LogRecord, LogstoreLevel, PruneBounds, Source};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

/// Source label of the synthetic drop marker.
const MARKER_SOURCE: &str = "log-store";

/// Publisher state snapshot for consumers of the word task (task 3
/// proceeds `watch::channel(LogStoreStatus::ok())`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogStoreStatus {
    /// Whether the writer is in degradation mode.
    pub degraded: bool,
    /// Since when (unix millis) the writer has been degrading
    /// (`None` outside degradation — absence is a state marker, not a
    /// missing entry).
    pub degraded_since: Option<i64>,
    /// Current channel backlog (events not yet in a batch).
    pub channel_len: usize,
    /// Current ring size (events ringed during degradation).
    pub ring_len: usize,
    /// Total dropped (channel-full) event count so far — includes
    /// not-moment-marked ones; the counter resets on marker emission.
    pub dropped_count: u64,
    /// Number of observed backoff retries (repeats while degrading).
    pub retries_seen: u64,
    /// Rows deleted by the last retention prune (`None` = no prune
    /// has run yet — e.g. `retention` is off). Zero means the last
    /// prune was a clean no-op (store already within bounds).
    pub last_prune_deleted: Option<i64>,
}

impl LogStoreStatus {
    /// Healthy initial state (all zeros, not degraded) — the initial
    /// value for `watch::channel` (task 3 depends on this).
    pub fn ok() -> Self {
        Self {
            degraded: false,
            degraded_since: None,
            channel_len: 0,
            ring_len: 0,
            dropped_count: 0,
            retries_seen: 0,
            last_prune_deleted: None,
        }
    }
}

/// Tunables of the writer (defaults are the proven production values;
/// the tests inject short timers via `WriterConfig { ..Default::default() }`).
#[derive(Debug, Clone)]
pub struct WriterConfig {
    /// Per-batch insert cap (rows).
    pub batch_max_rows: usize,
    /// Collect time for the first/waiting record, before abusing
    /// `try_recv`.
    pub batch_wait: Duration,
    /// Per-batch estimated JSON byte cap (collection halts on
    /// overflow; the current row is always shipped with the batch).
    pub byte_guard: usize,
    /// Ring capacity (entries).
    pub ring_max_entries: usize,
    /// Ring capacity (estimated JSON bytes).
    pub ring_max_bytes: usize,
    /// Interval between degraded-mode retry attempts.
    pub retry_interval: Duration,
    /// Ring drain cap after recovery.
    pub drain_timeout: Duration,
    /// Window for dropped> 0 before the writer enqueues the drop
    /// marker.
    pub drop_marker_window: Duration,
    /// Retention bounds for the periodic prune (`None` = feature off,
    /// e.g. legacy/tests). Boots onto the writer's single connection.
    pub retention: Option<PruneBounds>,
    /// Minimum time between retention prunes (the due check rides the
    /// existing tick — no extra timer). Tests inject tiny values.
    pub prune_interval: Duration,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            batch_max_rows: 200,
            batch_wait: Duration::from_millis(250),
            byte_guard: 256 * 1024,
            ring_max_entries: 1024,
            ring_max_bytes: 4 * 1024 * 1024,
            retry_interval: Duration::from_secs(1),
            drain_timeout: Duration::from_secs(5),
            drop_marker_window: Duration::from_secs(5),
            retention: None,
            prune_interval: Duration::from_secs(3600),
        }
    }
}

/// Spawns the writer task with [`WriterConfig::default`]. See the
/// module docs for ownership, degradation, and the shutdown caveat.
pub fn spawn_log_writer(
    store: LogStore,
    rx: mpsc::Receiver<LogRecord>,
    dropped: Arc<AtomicU64>,
    status_tx: watch::Sender<LogStoreStatus>,
    token: CancellationToken,
) -> tokio::task::JoinHandle<LogStoreStatus> {
    spawn_log_writer_with_config(
        WriterConfig::default(),
        store,
        rx,
        dropped,
        status_tx,
        token,
    )
}

/// Same as [`spawn_log_writer`], with an injected config (tests inject
/// short timers).
pub fn spawn_log_writer_with_config(
    cfg: WriterConfig,
    store: LogStore,
    rx: mpsc::Receiver<LogRecord>,
    dropped: Arc<AtomicU64>,
    status_tx: watch::Sender<LogStoreStatus>,
    token: CancellationToken,
) -> tokio::task::JoinHandle<LogStoreStatus> {
    let store = Arc::new(store);
    tokio::spawn(run_writer(cfg, store, rx, dropped, status_tx, token))
}

/// Unix milliseconds of the wall clock, epoch-fallback to 0 (the layer
/// uses the same convention for `ts`).
fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One insert batch through the blocking pool (rusqlite is sync — slow
/// disks must not stall runtime worker threads). Empty batches no-op.
//
// SAFETY of sharing `LogStore` across the blocking pool: every write
// goes through this one function and, therefore, at most one blocking
// pool thread touches the store connection at a time; readers open
// their own connections (see `db.rs`). The inner `RefCell`s are the
// only sync-sensitive parts.
async fn insert_batch_blocked(store: &Arc<LogStore>, batch: &[LogRecord]) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    let store = Arc::clone(store);
    let batch = batch.to_vec();
    tokio::task::spawn_blocking(move || {
        store
            .insert_batch(&batch)
            .map(|_ids| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("insert task failed to join: {e}"))?
}

/// One retention prune through the blocking pool (same posture as
/// [`insert_batch_blocked`] — rusqlite is sync and runs only on this
/// one connection, so pruning here needs no extra write lock).
async fn prune_blocked(store: &Arc<LogStore>, bounds: &PruneBounds) -> Result<i64, String> {
    let store = Arc::clone(store);
    let bounds = *bounds;
    tokio::task::spawn_blocking(move || store.prune(&bounds))
        .await
        .map_err(|e| format!("prune task failed to join: {e}"))
        .and_then(|r| r.map_err(|e| e.to_string()))
}

// SAFETY: rusqlite's connection is `!Sync` (RefCell statement-cache).
// The log store enforces a single-writer rig: every insert flows
// through `insert_batch_blocked` (the writer task is the only caller,
// and the blocking pool never runs two of those closures at once for
// the same store), and readers open separate connections (see
// `db.rs`). No second thread ever issues an SQL call on this
// connection, so the documented rig is honored: sharing via
// `Arc<LogStore>` + spawn_blocking cannot soundably bring race into
// the connection.
unsafe impl Sync for LogStore {}

/// Ring admission (degraded mode): warn-and-up, or the writer's own
/// drop markers (`dropped == true` at any level), are kept; info and
/// below are discarded. Caps: entries (FIFO-evict oldest) and estimated
/// bytes.
fn admit_to_ring(
    record: LogRecord,
    ring: &mut VecDeque<LogRecord>,
    ring_max_entries: usize,
    ring_max_bytes: usize,
) {
    let admitted = record.level.as_u8() >= LogstoreLevel::WARN.as_u8()
        || record.msg.get("dropped") == Some(&json!(true));
    if !admitted {
        return;
    }
    ring.push_back(record);
    while ring.len() > ring_max_entries || estimated_bytes(ring) > ring_max_bytes {
        ring.pop_front();
    }
}

/// Sum of estimated JSON bytes over the queue.
fn estimated_bytes(ring: &VecDeque<LogRecord>) -> usize {
    ring.iter()
        .map(|r| r.msg.to_string().len() + r.source.as_str().len())
        .sum()
}

/// The synthetic drop marker the writer enqueues after
/// `drop_marker_window` passes while the shared drop counter stayed > 0.
fn drop_marker_record(count: u64, since: std::time::SystemTime) -> LogRecord {
    let since_dt: DateTime<Utc> = DateTime::from(since);
    let since_ts = since_dt.to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut msg = json!({
        "dropped": true,
        "dropped_count": count,
        "dropped_since_ts": since_ts,
    });
    if let Some(obj) = msg.as_object_mut() {
        obj.insert(
            "message".to_owned(),
            json!(format!(
                "log store: dropped {count} events since {since_ts}"
            )),
        );
    }
    LogRecord {
        ts: now_unix_ms(),
        level: LogstoreLevel::WARN,
        source: Source::parse(MARKER_SOURCE).unwrap_or_else(Source::proxy),
        msg,
    }
}

/// Runs the writer loop to completion, returning the final status
/// snapshot (the `JoinHandle`'s value — see the shutdown note in the
/// module docs about holding the handle until the app exits).
async fn run_writer(
    cfg: WriterConfig,
    store: Arc<LogStore>,
    mut rx: mpsc::Receiver<LogRecord>,
    dropped: Arc<AtomicU64>,
    status_tx: watch::Sender<LogStoreStatus>,
    token: CancellationToken,
) -> LogStoreStatus {
    let mut status = LogStoreStatus::ok();
    // In-flight retry set while degraded.
    let mut pending: Vec<LogRecord> = Vec::new();
    let mut degraded = false;
    let mut ring: VecDeque<LogRecord> = VecDeque::new();
    let mut retry_at: Option<tokio::time::Instant> = None;
    // When (tick time + wall clock) the current drop period began.
    // Rebuilt whenever the counter returns to zero, so each marker
    // covers its own `drop_marker_window` period.
    let mut drop_since: Option<(tokio::time::Instant, std::time::SystemTime)> = None;
    // When the next retention prune is due (`None` = never run → due
    // at the first tick). Checked on every tick — it rides the
    // existing cadence, no extra timer.
    let mut last_prune_at: Option<tokio::time::Instant> = None;
    let mut tick = tokio::time::interval(cfg.batch_wait);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let est = |r: &LogRecord| r.msg.to_string().len() + r.source.as_str().len();

    // Moment of the last status broadcast (forced sends from the
    // degraded branch note it here; the loop end applies the 1 s
    // force-on-change rule).
    let mut last_status_sent: Option<tokio::time::Instant> = None;
    // The spill record: a record that opened the *next* batch
    // because the byte guard overflowed. It is owned here (mpsc
    // receivers cannot put records back), so it takes priority
    // over new arrivals — being chronologically older than
    // everything still in the channel, preserving strict FIFO.
    let mut starter: Option<LogRecord> = None;

    loop {
        // Snapshot for the status heartbeat at the end of the loop:
        // a degraded-state flip forces an immediate broadcast.
        let was_degraded = status.degraded;
        // ── Degradation: drive the retry, admit ring arrivals ─────────
        if degraded {
            // A spill record waiting for the next batch is ringed
            // first — it is older than every record still in the
            // channel, and the retry may be far away.
            if let Some(r) = starter.take() {
                admit_to_ring(r, &mut ring, cfg.ring_max_entries, cfg.ring_max_bytes);
                status.channel_len = rx.len();
                status.ring_len = ring.len();
                status.dropped_count = dropped.load(Ordering::SeqCst);
                last_status_sent = Some(tokio::time::Instant::now());
                let _ = status_tx.send_if_modified(|cur| {
                    if *cur != status {
                        *cur = status;
                        true
                    } else {
                        false
                    }
                });
                continue;
            }
            let now = tokio::time::Instant::now();
            let retry_due = retry_at.is_none_or(|t| now >= t);
            if retry_due {
                let batch = std::mem::take(&mut pending);
                if insert_batch_blocked(&store, &batch).await.is_err() {
                    // Another backoff retry observed.
                    status.retries_seen += 1;
                    pending = batch;
                } else {
                    // Recovery: the ring drains oldest-first through the
                    // normal inserts, bounded by `drain_timeout`.
                    status.retries_seen += 1;
                    degraded = false;
                    let deadline = tokio::time::Instant::now() + cfg.drain_timeout;
                    while !ring.is_empty() {
                        let n = cfg.batch_max_rows.min(ring.len());
                        let chunk: Vec<LogRecord> = ring.drain(..n).collect();
                        if insert_batch_blocked(&store, &chunk).await.is_err() {
                            // Store broke mid-drain: chunk goes back as
                            // the retry set, re-degrade.
                            degraded = true;
                            pending = chunk;
                            break;
                        }
                        if tokio::time::Instant::now() >= deadline {
                            let discarded = ring.len();
                            ring.clear();
                            if discarded > 0 {
                                tracing::warn!(
                                    "log store: discarded {discarded} ringed records after the drain bound"
                                );
                            }
                            break;
                        }
                    }
                }
                retry_at = if degraded {
                    Some(tokio::time::Instant::now() + cfg.retry_interval)
                } else {
                    None
                };
                if !degraded {
                    status.degraded = false;
                    status.degraded_since = None;
                }
            } else {
                // Sleep until the retry is due; stay responsive to
                // arrivals (ringed), cancellation, and heartbeat.
                let until = retry_at.expect("retry scheduled");
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        return finish_shutdown(&store, &mut rx, &mut status, pending).await;
                    }
                    _ = tokio::time::sleep_until(until) => {}
                    rec = rx.recv() => {
                        match rec {
                            Some(r) => admit_to_ring(r, &mut ring, cfg.ring_max_entries, cfg.ring_max_bytes),
                            None => {
                                return finish_shutdown(&store, &mut rx, &mut status, pending).await;
                            }
                        }
                    }
                    _ = tick.tick() => {}
                }
            }
            status.channel_len = rx.len();
            status.ring_len = ring.len();
            status.dropped_count = dropped.load(Ordering::SeqCst);
            last_status_sent = Some(tokio::time::Instant::now());
            let _ = status_tx.send_if_modified(|cur| {
                if *cur != status {
                    *cur = status;
                    true
                } else {
                    false
                }
            });
            continue;
        }

        // ── Normal mode: collect & flush, watch drops ─────────────────
        let head: Option<LogRecord> = if let Some(s) = starter.take() {
            Some(s)
        } else {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    return finish_shutdown(&store, &mut rx, &mut status, Vec::new()).await;
                }
                rec = rx.recv() => {
                    // Channel closed: every sender dropped → graceful
                    // termination (drain remains, final status), not
                    // another empty spin of the loop.
                    match rec {
                        Some(r) => Some(r),
                        None => {
                            return finish_shutdown(&store, &mut rx, &mut status, Vec::new()).await;
                        }
                    }
                }
                _ = tick.tick() => {
                    // Drop-window observation (also the idle tick). The
                    // period rebuilds whenever the counter hits zero, so
                    // each marker covers its own `drop_marker_window`.
                    let count = dropped.load(Ordering::SeqCst);
                    let due: Option<(u64, std::time::SystemTime)> = match drop_since {
                        _ if count == 0 => {
                            drop_since = None;
                            None
                        }
                        None => {
                            drop_since = Some((
                                tokio::time::Instant::now(),
                                std::time::SystemTime::now(),
                            ));
                            None
                        }
                        Some((start, wall)) => tokio::time::Instant::now()
                            .saturating_duration_since(start)
                            .ge(&cfg.drop_marker_window)
                            .then_some((count, wall)),
                    };
                    if let Some((count, since)) = due {
                        let record = drop_marker_record(count, since);
                        drop_since = None;
                        dropped.store(0, Ordering::SeqCst);
                        let marker = record.clone();
                        if insert_batch_blocked(&store, &[marker]).await.is_err() {
                            // Marker flush failed: into the retry path.
                            degraded = true;
                            status.degraded = true;
                            status.degraded_since = Some(now_unix_ms());
                            pending = vec![record];
                            retry_at = Some(tokio::time::Instant::now() + cfg.retry_interval);
                        }
                    }
                    // Retention: the due check rides this same tick.
                    if let Some(bounds) = cfg.retention {
                        let now = tokio::time::Instant::now();
                        let due = last_prune_at.is_none_or(|t| now >= t + cfg.prune_interval);
                        if due {
                            // Advance the due timestamp FIRST: the next
                            // attempt happens at the next due tick
                            // regardless of this outcome (no tight
                            // failure loop).
                            last_prune_at = Some(now);
                            match prune_blocked(&store, &bounds).await {
                                Ok(n) => {
                                    tracing::debug!(
                                        deleted = n,
                                        "log store retention prune"
                                    );
                                    status.last_prune_deleted = Some(n);
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "log store retention prune failed"
                                    );
                                }
                            }
                        }
                    }
                    None
                }
            }
        };
        if let Some(first) = head {
            // Collect the batch within the row and byte budgets. A
            // record that cannot fit under the byte guard becomes the
            // `starter` for the next batch (it cannot be put back into
            // the channel).
            let mut batch = Vec::new();
            let mut overflow: Option<LogRecord> = None;
            batch.push(first);
            let mut bytes = est(&batch[0]);
            'collect: while let Ok(next) = rx.try_recv() {
                let e = est(&next);
                if !batch.is_empty() && bytes + e > cfg.byte_guard {
                    overflow = Some(next);
                    break 'collect;
                }
                bytes += e;
                batch.push(next);
                if batch.len() >= cfg.batch_max_rows {
                    break 'collect;
                }
            }
            starter = overflow;
            if insert_batch_blocked(&store, &batch).await.is_err() {
                degraded = true;
                status.degraded = true;
                status.degraded_since = Some(now_unix_ms());
                pending = batch;
                retry_at = Some(tokio::time::Instant::now() + cfg.retry_interval);
            }
        }
        // Status heartbeat: degraded-state flips and drop-counter
        // changes force an immediate broadcast; channel_len/ring_len
        // updates otherwise ride the 1 s cadence ("at most every 1 s").
        let new_dropped = dropped.load(Ordering::SeqCst);
        let event = status.degraded != was_degraded || new_dropped != status.dropped_count;
        let cadence = last_status_sent.is_none_or(|t| {
            tokio::time::Instant::now().saturating_duration_since(t) >= Duration::from_secs(1)
        });
        if event || cadence {
            status.channel_len = rx.len();
            status.ring_len = ring.len();
            status.dropped_count = new_dropped;
            last_status_sent = Some(tokio::time::Instant::now());
            let _ = status_tx.send_if_modified(|cur| {
                if *cur != status {
                    *cur = status;
                    true
                } else {
                    false
                }
            });
        }
    }
}

/// Shutdown (cancellation or channel closed): best-effort final flush —
/// the pending retry set (once), then the remaining channel records —
/// and the final status as seen at shutdown time.
async fn finish_shutdown(
    store: &Arc<LogStore>,
    rx: &mut mpsc::Receiver<LogRecord>,
    status: &mut LogStoreStatus,
    pending: Vec<LogRecord>,
) -> LogStoreStatus {
    let remaining = tokio::time::timeout(Duration::from_secs(2), async {
        let mut out = Vec::new();
        while let Some(r) = rx.recv().await {
            out.push(r);
        }
        out
    })
    .await
    .unwrap_or_default();

    if !pending.is_empty() {
        // One best-effort persist; the state flags are left exactly as
        // they were at cancel time (clearing degradation is the job of
        // a successful retry in the running loop, not of shutdown).
        let _ = insert_batch_blocked(store, &pending).await;
    }
    if !remaining.is_empty() {
        let _ = insert_batch_blocked(store, &remaining).await;
    }
    status.channel_len = 0;
    status.ring_len = 0;
    *status
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logstore::types::{LogEntry, LogQuery, PruneBounds, QueryOrder};
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use tokio::time::sleep;

    /// Test fixture: a file-based store (a second connection reads it
    /// while the writer owns the first — WAL allows it) plus a handle.
    struct Fixture {
        #[allow(dead_code)] // held so the store file outlives the test
        dir: tempfile::TempDir,
        path: PathBuf,
        dropped: Arc<AtomicU64>,
        status_rx: watch::Receiver<LogStoreStatus>,
        token: CancellationToken,
        handle: tokio::task::JoinHandle<LogStoreStatus>,
    }

    impl Fixture {
        /// Opens a fresh view of the store (WAL allows a second connection).
        fn peek(&self) -> LogStore {
            LogStore::open(&self.path).expect("check connection")
        }

        /// All rows, oldest-first (pages through the 200-row default
        /// query limit).
        fn rows_asc(&self) -> Vec<LogEntry> {
            let store = self.peek();
            let mut entries = Vec::new();
            let mut cursor = None;
            loop {
                let (page, next) = store
                    .query(&LogQuery {
                        order: QueryOrder::Asc,
                        cursor,
                        ..Default::default()
                    })
                    .expect("query");
                entries.extend(page);
                match next {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
            entries
        }
    }

    /// Spawns a writer on a fresh store. Optionally fault-injects the
    /// first `insert_batch`. The test keeps the channel's send side
    /// itself (dropping it later closes the channel to the writer).
    fn start(cfg: WriterConfig, fault_first_insert: bool) -> (Fixture, mpsc::Sender<LogRecord>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("logstore.db");
        let store = LogStore::open(&path).expect("open store");
        if fault_first_insert {
            store.fail_next_insert_for_tests(true);
        }
        let (tx, rx) = mpsc::channel(1024);
        let dropped = Arc::new(AtomicU64::new(0));
        let (status_tx, status_rx) = watch::channel(LogStoreStatus::ok());
        let token = CancellationToken::new();
        let handle =
            spawn_log_writer_with_config(cfg, store, rx, dropped.clone(), status_tx, token.clone());
        (
            Fixture {
                dir,
                path,
                dropped,
                status_rx,
                token,
                handle,
            },
            tx,
        )
    }

    fn rec(ts: i64, level: LogstoreLevel, message: &str) -> LogRecord {
        LogRecord {
            ts,
            level,
            source: Source::proxy(),
            msg: json!({ "message": message }),
        }
    }

    /// Waits (under `time::pause`, where fake sleeps can outpace the
    /// real blocking pool that performs the inserts) for the status to
    /// satisfy `is`, mixing real sleeps (let the pool finish) with fake
    /// time advances (let the writer timers fire).
    async fn await_status(
        fx: &Fixture,
        mut is: impl FnMut(&LogStoreStatus) -> bool,
        fake_ms: u64,
        iters: u32,
    ) {
        for _ in 0..iters {
            if is(&fx.status_rx.borrow()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
            sleep(Duration::from_millis(fake_ms)).await;
        }
    }

    /// Cancels (or, with `cancel=false`, just awaits) the writer and
    /// returns its final status; the fixture is consumed. The test is
    /// responsible for dropping (or just ending the scope of) its own
    /// send side beforehand to close the channel.
    async fn finish(fx: Fixture, cancel: bool) -> LogStoreStatus {
        if cancel {
            fx.token.cancel();
        }
        fx.handle.await.expect("writer task completes")
    }

    /// `LogStoreStatus::ok()` is the healthy initial snapshot (task 3
    /// builds the channel DTO on top).
    #[test]
    fn test_logstore_status_ok_is_zero() {
        let s = LogStoreStatus::ok();
        assert!(!s.degraded);
        assert!(s.degraded_since.is_none());
        assert_eq!(s.channel_len, 0);
        assert_eq!(s.ring_len, 0);
        assert_eq!(s.dropped_count, 0);
        assert_eq!(s.retries_seen, 0);
        assert!(s.last_prune_deleted.is_none(), "no prune has run yet");
    }

    /// Retention: the writer is configured with a `max_rows` bound and the
    /// prune is due from writer start — it prunes interleaved with the
    /// flushes, and the deleted count lands on the status channel. A
    /// follow-up feed within the prune interval is NOT pruned again
    /// (the store grows past the bound; the status keeps the first
    /// prune's count) (fake time).
    #[tokio::test(start_paused = true)]
    async fn test_writer_prunes_to_retention_bounds_once_per_interval() {
        let cfg = WriterConfig {
            retention: Some(PruneBounds {
                // Age bound effectively off (but below the i64::MAX
                // the store's `* 1000` age-cutoff math accepts).
                max_age_secs: i64::MAX / 1000,
                max_rows: 10,
                max_bytes: i64::MAX,
            }),
            // 10 s (fake) — comfortably past everything the test
            // advances while feeding, comfortably under the fake
            // deadline if the due-timestamp failed to advance.
            prune_interval: Duration::from_secs(10),
            ..Default::default()
        };
        let (fx, tx) = start(cfg, false);

        // Phase 1: 50 records in a 10-row bound → the first (immediately
        // due) prune fires once the flushes pass it.
        for i in 1..=50 {
            tx.try_send(rec(i, LogstoreLevel::INFO, "m")).expect("send");
        }
        await_status(&fx, |s| s.last_prune_deleted.is_some(), 50, 500).await;
        let first = fx
            .status_rx
            .borrow()
            .last_prune_deleted
            .expect("phase 1: prune reported on the status");
        assert!(first > 0, "the 50-row feed deleted rows beyond the bound");
        // A few recordings may still be in flight between the prune and
        // the assert: the store is at the bound plus a handful.
        let rows = fx.rows_asc();
        assert!(
            rows.len() <= 15,
            "store pruned to ~10 rows (observed {})",
            rows.len()
        );

        // Phase 2: feed 40 more within the prune interval (10 s fake).
        // The flushes land, but NO second prune runs — the store grows
        // past the bound, and the status keeps the first prune's count.
        for i in 51..=90 {
            tx.try_send(rec(i, LogstoreLevel::INFO, "m")).expect("send");
        }
        for _ in 0..200 {
            if fx.rows_asc().len() > 15 {
                break; // grew past the bound: flushes landed
            }
            std::thread::sleep(Duration::from_millis(2));
            sleep(Duration::from_millis(20)).await;
        }
        let rows = fx.rows_asc();
        assert!(
            rows.len() > 15,
            "40 more records flushed within the interval grew the store"
        );
        let st = fx.status_rx.borrow();
        assert_eq!(
            st.last_prune_deleted,
            Some(first),
            "no second prune within the prune interval"
        );
        drop(st);
        drop(tx);
        let final_status = finish(fx, false).await;
        assert!(!final_status.degraded);
    }

    /// No retention configured (`retention: None`, the default):
    /// zero prune runs — the store keeps every row and
    /// `last_prune_deleted` stays `None`, however far the clock
    /// advances (fake time).
    #[tokio::test(start_paused = true)]
    async fn test_writer_no_prune_without_retention() {
        let cfg = WriterConfig::default(); // retention: None
        let (fx, tx) = start(cfg, false);
        for i in 1..=50 {
            tx.try_send(rec(i, LogstoreLevel::INFO, "m")).expect("send");
        }
        // Advance well past multiple default (1 h) prune intervals.
        for _ in 0..300 {
            if fx.rows_asc().len() == 50 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
            sleep(Duration::from_millis(1_000)).await;
        }
        assert_eq!(fx.rows_asc().len(), 50, "nothing deleted without bounds");
        drop(tx);
        let final_status = finish(fx, false).await;
        assert!(final_status.last_prune_deleted.is_none());
    }

    /// Batch timing: 3 records + a tick shorter than the default 250 ms
    /// collect time → one flush of all three (fake time).
    #[tokio::test(start_paused = true)]
    async fn test_writer_flushes_first_record_batch_after_wait() {
        let (fx, tx) = start(WriterConfig::default(), false);
        for ts in [10i64, 20, 30] {
            tx.try_send(rec(ts, LogstoreLevel::INFO, "m"))
                .expect("send");
        }
        // Fake time can outrun the real pool thread doing the insert, so
        // blend real + fake advance until the rows land.
        for _ in 0..500 {
            if fx.rows_asc().len() == 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
            sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(
            fx.rows_asc().iter().map(|e| e.ts).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
        assert!(!fx.status_rx.borrow().degraded);
        drop(tx);
        let final_status = finish(fx, false).await;
        assert!(!final_status.degraded);
    }

    /// 400-record feed drains in 200-row batches — all rows land in
    /// order (fake time).
    #[tokio::test(start_paused = true)]
    async fn test_writer_batches_cap_200() {
        let (fx, tx) = start(WriterConfig::default(), false);
        for i in 1..=400 {
            tx.try_send(rec(i, LogstoreLevel::INFO, "m")).expect("send");
        }
        // Same blend as the flush test: two 200-row batches flow through
        // the real pool while fake time advances.
        for _ in 0..500 {
            if fx.rows_asc().len() == 400 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
            sleep(Duration::from_millis(10)).await;
        }

        let rows = fx.rows_asc();
        assert_eq!(rows.len(), 400, "all 400 rows land (2 x 200-row batches)");
        assert_eq!(rows.first().map(|e| e.ts), Some(1));
        assert_eq!(rows.last().map(|e| e.ts), Some(400));
        drop(tx);
        let final_status = finish(fx, false).await;
        assert!(!final_status.degraded);
        assert_eq!(final_status.retries_seen, 0);
    }

    /// Degradation toggle: a fault-injected first insert flips
    /// `degraded` on (within the retry interval, fake time, 100 ms
    /// retry); on recovery the pending batch enters through the normal
    /// path in its original order (ids ascending) and `degraded`
    /// clears.
    #[tokio::test(start_paused = true)]
    async fn test_degraded_flip_and_pending_drain_order_on_recovery() {
        let cfg = WriterConfig {
            retry_interval: Duration::from_millis(100),
            ..Default::default()
        };
        let (fx, tx) = start(cfg, true);
        for ts in [1i64, 2, 3] {
            tx.try_send(rec(ts, LogstoreLevel::WARN, "w"))
                .expect("send");
        }
        // The paused fake clock can auto-advance the whole failed-insert →
        // backoff retry → recovery cycle in one jump, so assert the
        // *outcome* pair (degradation was observed AND recovered), not a
        // mid-flight snapshot.
        await_status(&fx, |s| s.retries_seen >= 1, 10, 500).await;
        await_status(&fx, |s| s.retries_seen >= 1 && !s.degraded, 10, 500).await;

        let rows = fx.rows_asc();
        assert_eq!(
            rows.iter().map(|e| e.ts).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "recovery replay keeps the original order (ids ascending)"
        );
        let st = fx.status_rx.borrow();
        assert!(!st.degraded, "recovery clears degraded");
        assert!(st.degraded_since.is_none());
        assert_eq!(st.ring_len, 0);
        assert!(st.retries_seen >= 1, "the failed retry is observed");
        drop(st);
        drop(tx);
        let final_status = finish(fx, false).await;
        assert!(!final_status.degraded);
    }

    /// Ring admission during degradation: warn-level (and every
    /// `dropped` marker) is kept; info-level is dropped; the ring caps
    /// FIFO-evict the oldest. While the store is still broken,
    /// nothing new is stored beyond the pending set.
    #[tokio::test(start_paused = true)]
    async fn test_ring_admission_and_fifo_cap_during_degradation() {
        let cfg = WriterConfig {
            retry_interval: Duration::from_secs(5), // stays degraded for the observation
            ring_max_entries: 2,
            ..Default::default()
        };
        let (fx, tx) = start(cfg, true);
        // One record → the (fault-injected) first flush fails.
        tx.try_send(rec(1, LogstoreLevel::WARN, "first"))
            .expect("send");
        await_status(&fx, |s| s.degraded, 50, 200).await;

        // Arrivals *while* degraded: warn admitted, info dropped, marker
        // (dropped=true, info-level) admitted; cap 2 → FIFO-evicts warn.
        tx.try_send(rec(2, LogstoreLevel::WARN, "second"))
            .expect("send");
        tx.try_send(rec(3, LogstoreLevel::INFO, "info — will be dropped"))
            .expect("send");
        let mut marker = rec(4, LogstoreLevel::INFO, "marker");
        marker.msg = json!({ "message": "satellite", "dropped": true });
        tx.try_send(marker).expect("send");
        tx.try_send(rec(5, LogstoreLevel::WARN, "fourth"))
            .expect("send");
        sleep(Duration::from_millis(400)).await;

        let st = fx.status_rx.borrow();
        assert!(st.degraded);
        assert_eq!(
            st.ring_len, 2,
            "warn + marker admitted, info dropped, cap 2 evicted the warn"
        );
        assert!(
            fx.rows_asc().is_empty(),
            "the pending retry cycle keeps the store quiet"
        );
        drop(st);
        drop(tx);

        // Cancel mid-degradation: nothing else should fail, and the
        // final status reports the degradation.
        let final_status = finish(fx, true).await;
        assert!(
            final_status.degraded,
            "still degraded at cancel (store reset once)"
        );
    }

    /// 2 drop periods within a 5 s window → ONE marker (throttled);
    /// ≥ 5 s apart → TWO markers. The marker row has source `log-store`,
    /// level WARN, and the documented msg shape.
    #[tokio::test(start_paused = true)]
    async fn test_marker_throttling_close_periods() {
        let (fx, tx) = start(WriterConfig::default(), false);
        fx.dropped.fetch_add(2, Ordering::SeqCst);
        sleep(Duration::from_millis(4_000)).await; // < 5 s window
        fx.dropped.fetch_add(3, Ordering::SeqCst);
        sleep(Duration::from_millis(6_000)).await; // window expires (first observation)

        let rows = fx.rows_asc();
        assert_eq!(
            rows.len(),
            1,
            "drop periods inside one 5 s window produce one marker"
        );
        let e = &rows[0];
        assert_eq!(e.level, LogstoreLevel::WARN);
        assert_eq!(e.source.as_str(), "log-store");
        assert_eq!(e.msg.get("dropped"), Some(&json!(true)));
        assert_eq!(e.msg.get("dropped_count"), Some(&json!(5)));
        assert!(
            e.msg
                .get("dropped_since_ts")
                .and_then(Value::as_str)
                .map(str::is_empty)
                == Some(false)
        );
        let msg = e
            .msg
            .get("message")
            .and_then(Value::as_str)
            .expect("marker message");
        assert!(msg.contains("dropped 5 events since"), "marker msg: {msg}");
        drop(tx);
        let final_status = finish(fx, false).await;
        assert_eq!(
            final_status.dropped_count, 0,
            "the marker resets the counter"
        );
    }

    /// Two drop periods ≥ 5 s apart → TWO markers, each covering its own
    /// period (the counter reset is per-marker, so the window opens up
    /// again).
    #[tokio::test(start_paused = true)]
    async fn test_marker_emitted_per_period_spaced_over_window() {
        let (fx, tx) = start(WriterConfig::default(), false);
        fx.dropped.fetch_add(2, Ordering::SeqCst);
        sleep(Duration::from_millis(6_000)).await; // first window + margin (marker #1)
        fx.dropped.fetch_add(2, Ordering::SeqCst); // new period after
        sleep(Duration::from_millis(6_200)).await; // second window + margin (marker #2)

        let rows = fx.rows_asc();
        assert_eq!(
            rows.len(),
            2,
            "≥ 5 s between periods → one marker per period"
        );
        for e in &rows {
            assert_eq!(e.level, LogstoreLevel::WARN);
            assert_eq!(e.source.as_str(), "log-store");
            assert_eq!(e.msg.get("dropped"), Some(&json!(true)));
            assert_eq!(e.msg.get("dropped_count"), Some(&json!(2)));
        }
        assert!(rows[0].ts <= rows[1].ts, "markers in temporal order");
        assert_eq!(fx.dropped.load(Ordering::SeqCst), 0);
        drop(tx);
        let final_status = finish(fx, false).await;
        assert!(!final_status.degraded);
    }

    /// Byte cap: the collection halts at the cap — the first batch
    /// carries only the 2 rows that fit (3 would overflow), and the
    /// rest of the send goes into the ring (on the failed first
    /// insert). Without a byte cap, all 4 rows would be batched.
    #[tokio::test(start_paused = true)]
    async fn test_byte_guard_stops_collection() {
        let cfg = WriterConfig {
            byte_guard: 300, // each record ≈ 104 JSON bytes
            retry_interval: Duration::from_secs(5),
            ..Default::default()
        };
        let (fx, tx) = start(cfg, true);
        let big = "x".repeat(90);
        for i in 1..=4 {
            tx.try_send(LogRecord {
                ts: i,
                level: LogstoreLevel::WARN,
                source: Source::proxy(),
                msg: json!({ "message": big }),
            })
            .expect("send");
        }
        await_status(&fx, |s| s.degraded && s.ring_len == 2, 100, 200).await;

        let st = fx.status_rx.borrow();
        assert!(st.degraded);
        assert_eq!(
            st.ring_len, 2,
            "byte cap: first batch = the 2 rows that fit; the rest are ringed"
        );
        drop(st);
        drop(tx);
        let final_status = finish(fx, true).await;
        assert!(final_status.degraded);
    }

    /// Shutdown: 50 records in flight, cancel mid-drain → everything
    /// arrives within the 2 s flush cap, and the writer's handle joins
    /// on a non-degraded final status.
    #[tokio::test]
    async fn test_shutdown_drains_remaining_channel_records() {
        let (fx, tx) = start(WriterConfig::default(), false);
        for i in 1..=50 {
            tx.try_send(rec(i, LogstoreLevel::INFO, "m")).expect("send");
        }
        sleep(Duration::from_millis(60)).await; // a few get batched, the rest ride the channel
        let path = fx.path.clone();
        fx.token.cancel();
        let final_status = tokio::time::timeout(Duration::from_secs(5), fx.handle)
            .await
            .expect("writer joins after cancel")
            .expect("writer task completes cleanly");
        assert!(!final_status.degraded);
        let check = LogStore::open(&path).expect("check connection");
        assert_eq!(
            check.query(&LogQuery::default()).expect("query").0.len(),
            50,
            "cancel must drain the remaining channel records within the bound"
        );
    }

    /// Channel close (all senders drop) is a graceful termination: the
    /// writer flushes what remains and the handle resolves to a
    /// healthy status.
    #[tokio::test(start_paused = true)]
    async fn test_writer_exits_on_channel_close() {
        let (fx, tx) = start(WriterConfig::default(), false);
        for i in 1..=5 {
            tx.try_send(rec(i, LogstoreLevel::INFO, "m")).expect("send");
        }
        let path = fx.path.clone();
        drop(tx);
        sleep(Duration::from_millis(600)).await;
        let final_status = tokio::time::timeout(Duration::from_secs(5), fx.handle)
            .await
            .expect("writer joins after channel close")
            .expect("writer task completes cleanly");
        assert!(!final_status.degraded);
        let check = LogStore::open(&path).expect("check connection");
        assert_eq!(check.query(&LogQuery::default()).expect("query").0.len(), 5);
    }
}
