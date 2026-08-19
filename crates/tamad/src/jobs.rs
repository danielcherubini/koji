//! In-memory job registry (plan-191 Task 6).
//!
//! Long-running operations (pulls now; installs and benchmarks in Tasks 7-8)
//! run as *jobs*: a runner future spawned by [`JobRegistry::start`] that
//! reports progress through a [`JobHandle`]. Every state change is broadcast
//! as a [`JobEvent`] (the same type streamed to the proxy over `StreamJob`),
//! and the latest state per job is kept in memory so late subscribers can
//! replay the terminal result.
//!
//! In addition, a bounded per-job **history ring** captures every event
//! for the job's lifetime. Emits are linearized against all subscriber
//! subscriptions (history append + broadcast send happen under the same
//! lock in total order; `subscribe` takes that same lock before creating
//! the receiver), so the returned `(receiver, history)` pair is a strict
//! partition of the event stream: each event arrives in exactly one of
//! the two halves. This is what lets a proxy which opens `StreamJob`
//! *after* the runner has already emitted early progress still
//! reconstruct the full job log.
//!
//! Jobs are in-memory only — tamad holds no database (ADR-0010). Terminal
//! jobs are pruned 1 hour after finishing (bounded memory); the proxy
//! persists all durable state from the terminal event's `result_json`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use tama_core::tamad::JobEvent;

/// Boxed future returned by a job runner.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Job status strings (wire values — match the `JobEvent.status` enum in the
/// proto).
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_SUCCEEDED: &str = "succeeded";
pub const STATUS_FAILED: &str = "failed";
/// Terminal status set by `CancelJob` (plan-191 follow-up B): the runner was
/// asked to stop and bailed out (or was aborted server-side).
pub const STATUS_CANCELLED: &str = "cancelled";

/// Broadcast capacity. Generous (low-rate job progress) so `Lagged` is
/// effectively impossible for a healthy subscriber.
const BROADCAST_CAPACITY: usize = 256;

/// Per-job replay history ring: a late `StreamJob` subscriber receives at
/// most this many buffered events (the most recent ones — the terminal
/// state always survives), so chatty runners can't grow memory unbounded.
const HISTORY_CAPACITY: usize = 512;

/// Latest state of a single job.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub progress: i32,
    pub message: String,
    pub status: String,
    pub result_json: Option<String>,
    pub error: Option<String>,
    /// Pull jobs: bytes written so far (0 for non-download jobs).
    pub bytes_downloaded: i64,
    /// Pull jobs: expected total bytes (0 = unknown).
    pub total_bytes: i64,
    /// Set when the job reaches a terminal state (drives 1h pruning).
    ended_at: Option<Instant>,
}

impl Job {
    /// Whether the job reached a terminal state (`succeeded` / `failed` /
    /// `cancelled`).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            STATUS_SUCCEEDED | STATUS_FAILED | STATUS_CANCELLED
        )
    }

    /// Convert the latest state into the wire `JobEvent` shape.
    pub fn to_event(&self) -> JobEvent {
        JobEvent {
            job_id: self.id.clone(),
            kind: self.kind.clone(),
            progress: self.progress,
            message: self.message.clone(),
            status: self.status.clone(),
            result_json: self.result_json.clone().unwrap_or_default(),
            error: self.error.clone().unwrap_or_default(),
            bytes_downloaded: self.bytes_downloaded,
            total_bytes: self.total_bytes,
        }
    }
}

/// Handle passed to a job runner: report progress or finish the job.
///
/// All methods are synchronous and idempotent — a `report` after a terminal
/// call is ignored, and an explicit `succeed`/`fail` wins over the runner's
/// return value. Cheap enough to call from sync progress callbacks.
#[derive(Clone)]
pub struct JobHandle {
    registry: Arc<JobRegistry>,
    id: String,
}

impl JobHandle {
    /// Report intermediate progress (clamped to 0-100) + a message.
    ///
    /// Ignored once the job is terminal.
    pub fn report(&self, progress: i32, message: &str) {
        self.report_with_bytes(progress, message, 0, 0)
    }

    /// Report progress + message + download byte counters (pull jobs).
    ///
    /// Ignored once the job is terminal.
    pub fn report_with_bytes(
        &self,
        progress: i32,
        message: &str,
        bytes_downloaded: i64,
        total_bytes: i64,
    ) {
        let mut map = self.registry.jobs.lock().unwrap();
        let Some(job) = map.get_mut(&self.id) else {
            return;
        };
        if job.is_terminal() {
            return;
        }
        job.progress = progress.clamp(0, 100);
        job.message = message.to_string();
        job.bytes_downloaded = bytes_downloaded.max(0);
        if total_bytes > 0 {
            job.total_bytes = total_bytes;
        }
        let event = job.to_event();
        // Append + send under the `jobs` lock: every event is linearized
        // against every `subscribe` snapshot, so replay history and the
        // live channel partition the stream (no loss, no duplicates).
        self.registry.push_history(&self.id, &event);
        let _ = self.registry.tx.send(event);
    }

    /// Mark the job succeeded with its result JSON (terminal).
    ///
    /// The runner still returns the result JSON via its future; this method
    /// exists so a runner can finalize state before a long cleanup step.
    /// (Used by tests today; install/update/benchmark runners in
    /// plan-191 Tasks 7-8.)
    #[allow(dead_code)]
    pub fn succeed(&self, result_json: &str) {
        self.registry
            .finish(&self.id, true, Some(result_json), None);
    }

    /// Mark the job failed with an error message (terminal).
    /// (Used by tests today; plan-191 Tasks 7-8 runners.)
    #[allow(dead_code)]
    pub fn fail(&self, error: &str) {
        self.registry.finish(&self.id, false, None, Some(error));
    }

    /// Resolves when the job is cancelled via [`JobRegistry::cancel`].
    ///
    /// Runners `tokio::select!` on this (next to their work future) to bail
    /// out cooperatively — e.g. a pull aborts its child download process.
    pub async fn cancelled(&self) {
        if let Some(token) = self.registry.token(&self.id) {
            token.cancelled().await;
        }
    }
}

/// Registry of in-flight and recently-finished jobs.
///
/// One shared broadcast channel carries events for ALL jobs (each
/// `JobEvent` is tagged with `job_id`; subscribers filter). The
/// latest-state map answers `get` and lets a late subscriber replay a
/// terminal result; the per-job `history` rings let any late subscriber
/// replay the full event stream (see the module docs for the partition
/// invariant). `jobs` is always locked before `history` (never the
/// reverse) so the two mutexes cannot deadlock.
pub struct JobRegistry {
    tx: broadcast::Sender<JobEvent>,
    jobs: std::sync::Mutex<HashMap<String, Job>>,
    /// Per-job event history for late-subscriber replay (keyed by job id),
    /// each ring capped at `HISTORY_CAPACITY`.
    history: std::sync::Mutex<HashMap<String, VecDeque<JobEvent>>>,
    /// Cancellation tokens for in-flight jobs (keyed by job id).
    tokens: std::sync::Mutex<HashMap<String, CancellationToken>>,
    /// Terminal jobs older than this are pruned on each insert.
    prune_age: Duration,
}

impl JobRegistry {
    /// Create a registry; `new` returns `Arc<Self>` because runners (and
    /// `JobHandle`s) hold a reference into the registry.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tx: broadcast::channel(BROADCAST_CAPACITY).0,
            jobs: std::sync::Mutex::new(HashMap::new()),
            history: std::sync::Mutex::new(HashMap::new()),
            tokens: std::sync::Mutex::new(HashMap::new()),
            prune_age: Duration::from_secs(3600),
        })
    }

    /// Override the terminal-job prune age (tests only — production uses 1h).
    #[cfg(test)]
    pub fn with_prune_age(age: Duration) -> Arc<Self> {
        Arc::new(Self {
            tx: broadcast::channel(BROADCAST_CAPACITY).0,
            jobs: std::sync::Mutex::new(HashMap::new()),
            history: std::sync::Mutex::new(HashMap::new()),
            tokens: std::sync::Mutex::new(HashMap::new()),
            prune_age: age,
        })
    }

    /// Append `event` to the job's replay history (callers hold the
    /// `jobs` lock; the `history` lock nests inside it — consistent
    /// order, so no deadlock).
    fn push_history(&self, id: &str, event: &JobEvent) {
        let mut history = self.history.lock().unwrap();
        let entry = history.entry(id.to_string()).or_default();
        if entry.len() >= HISTORY_CAPACITY {
            entry.pop_front();
        }
        entry.push_back(event.clone());
    }

    /// Start a new job of the given `kind` and return its id.
    ///
    /// The runner (called once) is invoked in a spawned task with a
    /// [`JobHandle`]. Its `Ok(result_json)` / `Err(e)` resolve the job as
    /// `succeeded` / `failed` unless the runner already called
    /// `succeed`/`fail` on the handle.
    pub async fn start(
        self: &Arc<Self>,
        kind: &str,
        runner: impl FnOnce(JobHandle) -> BoxFuture<anyhow::Result<String>> + Send + 'static,
    ) -> String {
        let id = format!("job-{}", uuid::Uuid::new_v4());
        {
            let mut map = self.jobs.lock().unwrap();
            // Prune terminal jobs older than the prune age (bounded memory).
            let now = Instant::now();
            map.retain(|_, job| {
                !job.is_terminal()
                    || job
                        .ended_at
                        .map(|t| now.duration_since(t) < self.prune_age)
                        .unwrap_or(false)
            });
            map.insert(
                id.clone(),
                Job {
                    id: id.clone(),
                    kind: kind.to_string(),
                    progress: 0,
                    message: "started".to_string(),
                    status: STATUS_RUNNING.to_string(),
                    result_json: None,
                    error: None,
                    bytes_downloaded: 0,
                    total_bytes: 0,
                    ended_at: None,
                },
            );
            // Emit the `started` event under the SAME lock — every emit
            // path appends to history and sends the event while holding
            // `jobs`, so a `subscribe` snapshot can never race an emit:
            // each event lands in exactly one half of the replay.
            let started = JobEvent {
                job_id: id.clone(),
                kind: kind.to_string(),
                progress: 0,
                message: "started".to_string(),
                status: STATUS_RUNNING.to_string(),
                result_json: String::new(),
                error: String::new(),
                bytes_downloaded: 0,
                total_bytes: 0,
            };
            self.push_history(&id, &started);
            let _ = self.tx.send(started);
            // Drop tokens of jobs pruned above (if the retain pass removed
            // anything, its token is stale). Done under the same snapshot so
            // ids and jobs stay consistent.
            let live: HashSet<String> = map.keys().cloned().collect();
            drop(map);
            self.tokens
                .lock()
                .unwrap()
                .retain(|tid, _| live.contains(tid));
            // Prune replay rings in the same pass that prunes the jobs map.
            self.history
                .lock()
                .unwrap()
                .retain(|tid, _| live.contains(tid));
        }
        // A fresh token per job; the runner's `JobHandle::cancelled()` future
        // resolves when `cancel` fires it (cooperative cancellation).
        self.tokens
            .lock()
            .unwrap()
            .insert(id.clone(), CancellationToken::new());

        let handle = JobHandle {
            registry: Arc::clone(self),
            id: id.clone(),
        };
        let registry = Arc::clone(self);
        let task_id = id.clone();
        tokio::spawn(async move {
            let result = runner(handle).await;
            match result {
                Ok(result_json) => registry.finish(&task_id, true, Some(&result_json), None),
                Err(e) => registry.finish(&task_id, false, None, Some(&e.to_string())),
            }
        });
        id
    }
    /// Subscribe to the job's event stream.
    ///
    /// Returns the live broadcast receiver (all jobs; filter by
    /// `JobEvent.job_id`) **plus a snapshot of the job's recorded
    /// history**, in emission order. The snapshot and the receiver are
    /// captured under the same lock that every emit path holds while
    /// appending to history and sending the event, so the two halves
    /// strictly partition the event stream: each event is delivered in
    /// exactly one of them — never both, never neither. This is what
    /// lets a late `StreamJob` joiner (the proxy opens the stream after
    /// the dispatch RPC returned) still receive the runner's early
    /// progress lines.
    ///
    /// Returns `None` when the job id is unknown.
    pub fn subscribe(
        self: &Arc<Self>,
        job_id: &str,
    ) -> Option<(broadcast::Receiver<JobEvent>, Vec<JobEvent>)> {
        let map = self.jobs.lock().unwrap();
        if !map.contains_key(job_id) {
            return None;
        }
        let history: Vec<JobEvent> = self
            .history
            .lock()
            .unwrap()
            .get(job_id)
            .map(|v| v.iter().cloned().collect())
            .unwrap_or_default();
        let rx = self.tx.subscribe();
        Some((rx, history))
    }

    /// The latest state for a job, if it is (or was, within the prune
    /// window) known. Test support only. `#[cfg(test)]` because production
    /// consumers use `subscribe` (with history replay) — not snapshots.
    #[cfg(test)]
    pub fn get(self: &Arc<Self>, job_id: &str) -> Option<Job> {
        self.jobs.lock().unwrap().get(job_id).cloned()
    }

    /// The cancellation token for an in-flight job, if any. Only the
    /// handle-based `cancelled()` walker uses this: `cancel()` fires the
    /// token directly on remove.
    fn token(&self, job_id: &str) -> Option<CancellationToken> {
        self.tokens.lock().unwrap().get(job_id).cloned()
    }

    /// Cancel a running job (idempotent `CancelJob` RPC, plan-191 follow-up
    /// B).
    ///
    /// Marks the job `cancelled` (terminal) immediately and broadcasts the
    /// event, then fires the job's token so the runner's `select!` leg bails
    /// out (e.g. aborting a pull's child process). Returns `true` when the
    /// job was running; `false` for unknown or already-terminal ids.
    pub fn cancel(&self, job_id: &str) -> bool {
        let mut map = self.jobs.lock().unwrap();
        let Some(job) = map.get_mut(job_id) else {
            return false;
        };
        if job.is_terminal() {
            return false;
        }
        job.status = STATUS_CANCELLED.to_string();
        job.ended_at = Some(Instant::now());
        let event = job.to_event();
        self.push_history(job_id, &event);
        let _ = self.tx.send(event);
        drop(map);
        // Remove + fire the token: `cancel()` wakes every clone the runner
        // holds through `JobHandle::cancelled()`.
        if let Some(tok) = self.tokens.lock().unwrap().remove(job_id) {
            tok.cancel();
        }
        true
    }

    /// Snapshot of all known jobs (test support only).
    #[cfg(test)]
    pub fn list(self: &Arc<Self>) -> Vec<Job> {
        self.jobs.lock().unwrap().values().cloned().collect()
    }

    /// Apply a terminal state unless the job already reached one (idempotent
    /// — an explicit `succeed`/`fail` wins over the runner's return value).
    fn finish(&self, id: &str, success: bool, result_json: Option<&str>, error: Option<&str>) {
        // Drop the job's token up front if the token went first — the job
        // still resolves below; token removal is idempotent either way.
        self.tokens.lock().unwrap().remove(id);
        let mut map = self.jobs.lock().unwrap();
        let Some(job) = map.get_mut(id) else {
            return;
        };
        if job.is_terminal() {
            return;
        }
        job.status = if success {
            STATUS_SUCCEEDED.to_string()
        } else {
            STATUS_FAILED.to_string()
        };
        if success {
            job.progress = 100;
        }
        job.result_json = result_json.map(String::from);
        job.error = error.map(String::from);
        job.ended_at = Some(Instant::now());
        let event = job.to_event();
        self.push_history(id, &event);
        let _ = self.tx.send(event);
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Poll `registry.get(id)` until terminal or deadline.
    async fn wait_terminal(registry: &Arc<JobRegistry>, id: &str) -> Job {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let job = registry.get(id).expect("job must exist");
            if job.is_terminal() {
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "job did not reach a terminal state in time"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// A runner that returns Ok resolves the job as `succeeded` with the
    /// result JSON and progress 100.
    #[tokio::test]
    async fn test_start_ok_yields_succeeded_job() {
        let registry = JobRegistry::new();
        let id = registry
            .start("pull", |_h| {
                Box::pin(async { Ok(r#"{"ok": true}"#.to_string()) })
            })
            .await;

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_SUCCEEDED);
        assert_eq!(job.progress, 100);
        assert_eq!(job.result_json.as_deref(), Some(r#"{"ok": true}"#));
        assert!(job.error.is_none());
        assert_eq!(job.kind, "pull");
    }

    /// A runner that returns Err resolves the job as `failed` with the
    /// anyhow error rendered.
    #[tokio::test]
    async fn test_start_err_yields_failed_job() {
        let registry = JobRegistry::new();
        let id = registry
            .start("pull", |_h| {
                Box::pin(async { Err(anyhow::anyhow!("download exploded: range 416")) })
            })
            .await;

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_FAILED);
        assert!(
            job.error
                .as_deref()
                .unwrap_or_default()
                .contains("download exploded"),
            "error rendered: {:?}",
            job.error
        );
        assert!(job.result_json.is_none());
    }

    /// The broadcast carries ordered events: each `report` in order (non-
    /// decreasing progress), then exactly one terminal `succeeded` event.
    #[tokio::test]
    async fn test_broadcast_receives_ordered_events() {
        let registry = JobRegistry::new();
        // The runner waits for the test to subscribe before reporting, so
        // no event can be missed.
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
        let id = registry
            .start("pull", |h| {
                Box::pin(async move {
                    let _ = ready_rx.await;
                    h.report(10, "ten");
                    h.report(50, "fifty");
                    h.report(99, "almost");
                    Ok("done".to_string())
                })
            })
            .await;
        let (rx, history) = registry.subscribe(&id).expect("job exists after start");
        // The snapshot is point-in-time: the gated runner has only emitted
        // `started` so far, and later reports will arrive on the live stream.
        assert_eq!(history.len(), 1, "history snapshot at subscribe time");
        assert_eq!(history[0].message, "started");
        let mut rx = rx;
        ready_tx.send(()).ok();

        let mut progress = Vec::new();
        let mut terminal_count = 0;
        let mut saw_ten = false;
        let mut saw_fifty = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(ev)) if ev.job_id == id => {
                    if ev.status == STATUS_RUNNING {
                        progress.push(ev.progress);
                        saw_ten = saw_ten || ev.message == "ten";
                        saw_fifty = saw_fifty || ev.message == "fifty";
                    } else {
                        terminal_count += 1;
                        assert_eq!(ev.status, STATUS_SUCCEEDED);
                        assert_eq!(ev.progress, 100);
                        break;
                    }
                }
                Ok(Ok(_)) => {} // events of other jobs
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    panic!("receiver lagged by {n}")
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => panic!("channel closed early"),
                Err(_) => continue, // recv timeout — keep waiting until deadline
            }
        }

        assert_eq!(terminal_count, 1, "exactly one terminal event");
        assert!(saw_ten && saw_fifty, "report messages must be broadcast");
        assert!(
            progress.windows(2).all(|w| w[0] <= w[1]),
            "progress must be non-decreasing: {progress:?}"
        );
        for p in [10, 50, 99] {
            assert!(progress.contains(&p), "missing progress {p}: {progress:?}");
        }
    }

    /// `report` after a terminal state is ignored — the terminal event is
    /// emitted exactly once and the latest state is untouched.
    #[tokio::test]
    async fn test_terminal_is_emitted_once() {
        let registry = JobRegistry::new();
        let id = registry
            .start("pull", |h| {
                Box::pin(async move {
                    h.report(50, "half");
                    h.succeed(r#"{"done": true}"#);
                    // Late report + late error: must be no-ops.
                    h.report(60, "late");
                    h.fail("late failure");
                    Ok(r#"{"ignored": true}"#.to_string())
                })
            })
            .await;

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_SUCCEEDED);
        assert_eq!(job.progress, 100);
        assert_eq!(job.result_json.as_deref(), Some(r#"{"done": true}"#));
        assert!(
            job.error.is_none(),
            "late fail() must not override succeed()"
        );
        assert_eq!(job.message, "half", "late report must not override state");
    }

    /// A late subscriber (the proxy opens `StreamJob` only AFTER the
    /// runner has already streamed its early progress) must still receive
    /// the full event history. The broadcast channel alone cannot provide
    /// this — a fresh receiver only sees events sent after `subscribe()`
    /// — so the registry must replay captured history alongside the
    /// receiver.
    #[tokio::test]
    async fn test_late_subscriber_replays_full_history() {
        let registry = JobRegistry::new();
        let id = registry
            .start("benchmark", |h| {
                Box::pin(async move {
                    h.report(0, "Using llama-bench: /bin/llama-bench");
                    h.report(0, "Model: test-model");
                    h.report(0, "Running: llama-bench (pp)");
                    h.report(0, "benchmark finished");
                    Ok(r#"{"ok": true}"#.to_string())
                })
            })
            .await;

        // Drain to terminal so every runner report lands before we
        // subscribe — the worst case for a late `StreamJob` joiner.
        let _ = wait_terminal(&registry, &id).await;

        let (mut rx, history) = registry.subscribe(&id).expect("job exists after start");

        // The history must carry the whole stream in order, ending in the
        // terminal event (whose message is the runner's last report).
        let messages: Vec<&str> = history.iter().map(|ev| ev.message.as_str()).collect();
        assert_eq!(
            messages,
            vec![
                "started",
                "Using llama-bench: /bin/llama-bench",
                "Model: test-model",
                "Running: llama-bench (pp)",
                "benchmark finished",
                "benchmark finished",
            ],
            "history must replay every pre-subscribe event in order"
        );
        let last = history.last().expect("history is non-empty");
        assert_eq!(last.status, STATUS_SUCCEEDED);
        assert_eq!(last.progress, 100);
        assert_eq!(last.result_json, r#"{"ok": true}"#);

        // The live channel must not DUPLICATE the replayed history: a
        // terminal job emits nothing after subscribe.
        let dup = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
        match dup {
            Err(_) => {} // no event — as expected
            Ok(Ok(ev)) => assert_ne!(ev.job_id, id, "history event replayed twice: {ev:?}"),
            Ok(other) => panic!("unexpected recv on a terminal job: {other:?}"),
        }
    }

    /// The same replay guarantee for a FAILED job: the terminal event must
    /// carry the error message, so a late joiner reconstructs the failure.
    #[tokio::test]
    async fn test_late_subscriber_replays_failed_terminal() {
        let registry = JobRegistry::new();
        let id = registry
            .start("pull", |h| {
                Box::pin(async move {
                    h.report(50, "halfway");
                    Err(anyhow::anyhow!("download exploded: range 416"))
                })
            })
            .await;

        let _ = wait_terminal(&registry, &id).await;

        let (_, history) = registry.subscribe(&id).expect("job exists after start");
        let last = history.last().expect("history is non-empty");
        assert_eq!(last.status, STATUS_FAILED);
        assert_eq!(last.error, "download exploded: range 416");
        assert_eq!(last.message, "halfway");
    }

    /// History is bounded — a chatty runner cannot grow a job's in-memory
    /// replay buffer without limit. The ring keeps the most recent
    /// `HISTORY_CAPACITY` events (always including the terminal state).
    #[tokio::test]
    async fn test_history_is_capped() {
        let registry = JobRegistry::new();
        let overflow = HISTORY_CAPACITY + 100;
        let id = registry
            .start("pull", move |h| {
                Box::pin(async move {
                    for i in 0..overflow {
                        let progress = i32::try_from(i % 100).expect("fits");
                        h.report(progress, &format!("line {i}"));
                    }
                    Ok("done".to_string())
                })
            })
            .await;

        let _ = wait_terminal(&registry, &id).await;

        let (_, history) = registry.subscribe(&id).expect("job exists after start");
        assert_eq!(history.len(), HISTORY_CAPACITY, "ring must be bounded");
        // 1 (started) + {overflow} reports + 1 (terminal) events total.
        // The ring keeps the last HISTORY_CAPACITY entries: the most
        // recent HISTORY_CAPACITY-1 report events + the terminal event.
        let first_report = overflow - HISTORY_CAPACITY + 1;
        assert_eq!(
            history.first().map(|ev| ev.message.as_str()),
            Some(format!("line {first_report}").as_str())
        );
        assert_eq!(
            history.last().map(|ev| ev.status.as_str()),
            Some(STATUS_SUCCEEDED),
            "the terminal state must always survive capping"
        );
    }

    /// Unknown job ids: `subscribe` returns `None`, `get` returns `None`.
    #[test]
    fn test_unknown_job_id() {
        let registry = JobRegistry::new();
        assert!(registry.subscribe("job-does-not-exist").is_none());
        assert!(registry.get("job-does-not-exist").is_none());
    }

    /// Terminal jobs are pruned on the next insert once older than the prune
    /// age; in-flight jobs are never pruned.
    #[tokio::test]
    async fn test_prune_terminal_jobs_after_age() {
        // prune age 50ms: a terminal job is evicted at the next insert.
        let registry = JobRegistry::with_prune_age(Duration::from_millis(50));

        let old = registry
            .start("pull", |_h| Box::pin(async { Ok("x".to_string()) }))
            .await;
        wait_terminal(&registry, &old).await;
        assert!(
            registry.get(&old).is_some(),
            "terminal job retained while fresh"
        );

        // A long-running job: must survive the pruning pass.
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
        let long_running = registry
            .start("pull", |_h| {
                Box::pin(async move {
                    let _ = gate_rx.await;
                    Ok("done".to_string())
                })
            })
            .await;

        // Age out the first job, then insert a third (triggers prune).
        tokio::time::sleep(Duration::from_millis(80)).await;
        let third = registry
            .start("pull", |_h| Box::pin(async { Ok("z".to_string()) }))
            .await;

        assert!(registry.get(&old).is_none(), "old terminal job pruned");
        assert!(
            registry.get(&long_running).is_some(),
            "in-flight job must never be pruned"
        );
        gate_tx.send(()).ok();
        wait_terminal(&registry, &long_running).await;
        wait_terminal(&registry, &third).await;
    }

    /// Cancelling a running job marks it `cancelled` (a terminal status) and
    /// fires the token so the runner's `select` leg can bail out.
    #[tokio::test]
    async fn test_cancel_running_job_marks_cancelled() {
        let registry = JobRegistry::new();
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
        let id = registry
            .start("pull", |h| {
                Box::pin(async move {
                    tokio::select! {
                        _ = gate_rx => Ok("done".to_string()),
                        _ = h.cancelled() => Err(anyhow::anyhow!("cancelled mid-pull")),
                    }
                })
            })
            .await;

        assert!(registry.cancel(&id), "cancel of a running job succeeds");

        let job = wait_terminal(&registry, &id).await;
        assert_eq!(job.status, STATUS_CANCELLED);
        assert!(job.is_terminal());
        let _ = gate_tx.send(());
    }

    /// Cancelling an unknown job id is a no-op (idempotent — the proxy may
    /// retry its cancel after a reconnect).
    #[tokio::test]
    async fn test_cancel_unknown_job() {
        let registry = JobRegistry::new();
        assert!(!registry.cancel("job-does-not-exist"));
    }

    /// Cancelling an already-terminal job reports false and does not
    /// rewrite its final status.
    #[tokio::test]
    async fn test_cancel_terminal_job_is_noop() {
        let registry = JobRegistry::new();
        let id = registry
            .start("pull", |_h| Box::pin(async { Ok("r".to_string()) }))
            .await;
        wait_terminal(&registry, &id).await;
        assert!(!registry.cancel(&id), "cancel of a terminal job is a no-op");
        assert_eq!(registry.get(&id).unwrap().status, STATUS_SUCCEEDED);
    }

    /// The handle's token lets a runner cooperate with cancellation: the
    /// `cancelled()` future resolves when `cancel` is called, whether the
    /// token fires while the runner already awaits it or before the runner
    /// first polls (both orderings must bail out).
    #[tokio::test]
    async fn test_handle_cancelled_fires_on_cancel() {
        let registry = JobRegistry::new();
        let fire = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fire_flag = fire.clone();
        // Barrier: set once the runner actually took its cancellation leg.
        let (ran_tx, ran_rx) = tokio::sync::oneshot::channel::<()>();
        let id = registry
            .start("install", move |h| {
                Box::pin(async move {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => Ok("done".to_string()),
                        _ = h.cancelled() => {
                            fire_flag
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                            let _ = ran_tx.send(());
                            Err(anyhow::anyhow!("cancelled"))
                        }
                    }
                })
            })
            .await;
        assert!(registry.cancel(&id));
        wait_terminal(&registry, &id).await;
        // Wait for the runner to actually take the cancelled leg (the job's
        // terminal state was set by `cancel` itself, not by the runner).
        ran_rx.await.expect("runner processed the cancellation");
        assert!(
            fire.load(std::sync::atomic::Ordering::SeqCst),
            "runners must observe the cancellation through the handle"
        );
    }
}
