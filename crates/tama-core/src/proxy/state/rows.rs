//! Live model rows aggregated from the tamad's 1 Hz `ProcessInfo` stream.
//!
//! This is the proxy's read-side source of truth for model facts (plan-193
//! Task 4): instead of a local per-model state map, the
//! dashboard / routing / management readers resolve each model's current
//! state, endpoint, desired flag and restart counters straight off the
//! tamad's `SystemStats.processes` wire field.
//!
//! [`live`] aggregates one frame per registered tamad handle that has a
//! FRESH snapshot (`TamadHandle::latest_fresh`), turning every live
//! `ProcessInfo` into a [`ModelRow`]. An offline host (no streaming handle,
//! or a frame older than the freshness bound) contributes **zero** rows —
//! "no host = no models", never "models went stale". A frame counts only
//! when its process is alive `AND` its status is one of the eligible
//! `{ready, starting, restarting}` lifecycle states; anything else
//! (`failed`, `budget_exhausted`, `unloading`, …) is simply absent.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::tamad::pool::TamadPool;
use crate::tamad::ProcessInfo;

/// Max age of a wire frame the proxy trusts as "live".
///
/// This bound re-exposes the 5s wire-staleness contract of plan-193:
/// a row older than this is not treated as loaded (absent). The
/// tamad emits at 1 Hz, so 5s is five ticks of slack before a
/// silent producer is declared absent. Deliberately NOT 500ms: a
/// single dropped frame must not blank the model list.
const LIVE_FRAME_MAX_AGE: Duration = Duration::from_secs(5);

/// The lifecycle statuses a process must report to be counted as a live row.
///
/// `ready` is the routing/dashboard target; `starting` covers the in-flight
/// load window; `restarting` is the tamad's dead-PID respawn (T2) state. All
/// other statuses (`failed`, `unloading`, `budget_exhausted`, …) are not
/// eligible — offline host → no models, and a dead process does not count as
/// one.
fn eligible_status(status: &str) -> bool {
    status == "ready" || status == "starting" || status == "restarting"
}

/// Whether a wire process is currently eligible as a live model row:
/// process-alive AND in an eligible lifecycle status (frame freshness is
/// enforced upstream by `live`'s `latest_deadline`) — plus the plan-193 T5c
/// extension: a `budget_exhausted` process contributes a row INDEPENDENTLY
/// of process liveness. The tamad keeps reporting the budget state (the
/// process is dead; its restart budget is spent; it re-warms in ~60s) and
/// the proxy reads that row for the budget-exhausted 503. Every other
/// non-live status (`failed`, `unloading`, ...) is not a row.
fn is_eligible(p: &ProcessInfo) -> bool {
    (p.alive && eligible_status(&p.status)) || p.status == "budget_exhausted"
}

/// Build the proxy-side row for one wire process, at `last_seen_ms`.
fn row_from(p: &ProcessInfo, last_seen_ms: i64) -> ModelRow {
    ModelRow {
        key: p.model_name.clone(),
        status: p.status.clone(),
        alive: p.alive,
        endpoint: p.endpoint_url.clone(),
        last_seen_ms,
        pid: p.pid,
        desired: p.desired,
        restart_count: p.restart_count,
        max_restarts: p.max_restarts,
    }
}

/// One live model's proxy-side row, projected from a wire `ProcessInfo`.
///
/// `key` is the canonical config key (== `ProcessInfo.model_name` wire
/// string) — the join key for the management API, routing, and the
/// `desired_models`/`active_models` DB views. `status` / `alive` mirror the
/// wire; `endpoint` is the routing target (wire field 5); `pid` is the wire
/// process id (wire field 3, the authoritative source for `backend_pid`);
/// `desired` and the restart counters (wire fields 7-9, T3) complete the
/// picture (plan-193 T5c).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelRow {
    pub key: String,
    pub status: String,
    pub alive: bool,
    pub endpoint: String,
    pub last_seen_ms: i64,
    pub pid: i32,
    pub desired: bool,
    pub restart_count: u32,
    pub max_restarts: u32,
}

/// Transactionally spawn as a Vec + index so `all()` can hand out a slice.
///
/// The `Rows` aggregation deliberately exposes the CANDIDATE surface (one row
/// per live eligible process) as a snapshot, not a live view of the pool: the
/// dashboard `status` verb and `/status` endpoint walk `all()`, while
/// `row(key)` / `online(key)` / `ready_count()` serve the routing, management
/// and metrics readers.
///
/// Multiple handles may report the same model key (e.g. a model physically
/// running on more than one tamad). `live` dedups by key: the last handle in
/// the pool order wins. Callers that need per-host attribution (the dashboard
/// host cards) resolve `host_name` separately from the pool, not from this
/// aggregate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    ordered: Vec<ModelRow>,
    by_key: HashMap<String, usize>,
}

impl Rows {
    /// The live row for `key` (the canonical config key == wire model name),
    /// or `None` when the model is not currently live.
    pub fn row(&self, key: &str) -> Option<ModelRow> {
        self.by_key.get(key).map(|i| self.ordered[*i].clone())
    }

    /// Whether the model at `key` is currently live (an eligible process is
    /// reporting it).
    pub fn online(&self, key: &str) -> bool {
        self.row(key).map(|r| r.alive).unwrap_or(false)
    }

    /// Number of models currently in the `ready` state (the live "models
    /// loaded" count). Only `status == "ready"` counts — a `starting` model
    /// is in flight, not yet loadable.
    pub fn ready_count(&self) -> u64 {
        self.ordered.iter().filter(|r| r.status == "ready").count() as u64
    }

    /// Every live row, in stable insertion (handle order) sequence.
    ///
    /// `status.rs` / `tama_handlers` walk this for the dashboard `status`
    /// verb and the management list endpoints.
    pub fn all(&self) -> &[ModelRow] {
        self.ordered.as_slice()
    }
}

/// Aggregate the live model rows across every tamad in `pool`.
///
/// Per handle: only a FRESH snapshot (≤ `LIVE_FRAME_MAX_AGE` old) is
/// consumed — a stale or absent snapshot (offline host) yields zero rows for
/// that host. This is the proxy's read-side flip (plan-193 Task 4): the
/// control plane reads its model facts off the wire, not the mirror.
pub async fn live(pool: &TamadPool) -> Rows {
    let handles = pool.list_handles().await;
    let mut ordered: Vec<ModelRow> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let last_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    for handle in handles {
        let stats = handle.latest_fresh(LIVE_FRAME_MAX_AGE).await;
        if let Some(stats) = stats {
            for p in stats.processes.iter() {
                if !is_eligible(p) {
                    continue;
                }
                match by_key.get(&p.model_name) {
                    Some(i) => ordered[*i] = row_from(p, last_ms),
                    None => {
                        by_key.insert(p.model_name.clone(), ordered.len());
                        ordered.push(row_from(p, last_ms));
                    }
                }
            }
        }
    }

    Rows { ordered, by_key }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    use crate::db::pool::test_dummy_pool;
    use crate::tamad::pool::test_support::{handle_no_latest, handle_with_latest, stats_full};
    use crate::tamad::pool::TamadPool;
    use crate::tamad::SystemStats;

    fn proc(
        model_name: &str,
        status: &str,
        alive: bool,
        endpoint: &str,
        desired: bool,
        restart_count: u32,
        max_restarts: u32,
    ) -> ProcessInfo {
        ProcessInfo {
            model_name: model_name.to_string(),
            provider_name: "llama.cpp".to_string(),
            pid: 100,
            alive,
            endpoint_url: endpoint.to_string(),
            status: status.to_string(),
            desired,
            restart_count,
            max_restarts,
        }
    }

    /// One-handle pool holding the given scripted snapshot (fresh now).
    async fn pool_with(stats: SystemStats) -> TamadPool {
        let pool = TamadPool::new(test_dummy_pool());
        pool.insert_raw_handle(
            "h1",
            Arc::new(handle_with_latest(Instant::now(), stats).await),
        )
        .await;
        pool
    }

    /// One pool whose handle has NO snapshot delivered yet (offline).
    async fn pool_without_latest() -> TamadPool {
        let pool = TamadPool::new(test_dummy_pool());
        pool.insert_raw_handle("h0", Arc::new(handle_no_latest()))
            .await;
        pool
    }

    /// One pool holding a stale frame `age_secs` seconds old.
    async fn pool_with_stale(age_secs: u64, processes: Vec<ProcessInfo>) -> TamadPool {
        let stats = stats_full(1.5, vec![], processes);
        let pool = TamadPool::new(test_dummy_pool());
        pool.insert_raw_handle(
            "h1",
            Arc::new(
                handle_with_latest(Instant::now() - Duration::from_secs(age_secs), stats).await,
            ),
        )
        .await;
        pool
    }

    fn stats_with(processes: Vec<ProcessInfo>) -> SystemStats {
        stats_full(1.5, vec![], processes)
    }

    /// A live eligible process is present, online, and carries the row's wire
    /// facts.
    #[tokio::test]
    async fn test_live_present_eligible() {
        let stats = stats_full(
            1.5,
            vec![],
            vec![proc(
                "qwen3",
                "ready",
                true,
                "http://127.0.0.1:8080",
                true,
                2,
                5,
            )],
        );
        let rows = live(&pool_with(stats).await).await;
        let r = rows.row("qwen3").expect("ready model is a live row");
        assert_eq!(r.status, "ready");
        assert!(r.alive);
        assert_eq!(r.endpoint, "http://127.0.0.1:8080");
        assert!(r.desired, "ready model wired desired=true");
        assert_eq!(r.restart_count, 2);
        assert_eq!(r.max_restarts, 5);
        assert!(rows.online("qwen3"));
        assert_eq!(rows.ready_count(), 1);
    }

    /// Only `status == "ready"` counts toward `ready_count`; a `starting`
    /// row is present but not counted as loaded.
    #[tokio::test]
    async fn test_ready_count_counts_ready_only() {
        let stats = stats_with(vec![
            proc("m-ready", "ready", true, "http://x:1", true, 0, 3),
            proc("m-start", "starting", true, "http://y:2", true, 0, 3),
        ]);
        let rows = live(&pool_with(stats).await).await;
        assert_eq!(rows.ready_count(), 1, "only ready contributes");
        assert!(rows.row("m-start").is_some());
        assert_eq!(rows.all().len(), 2, "starting still a live row");
    }

    /// A frame exactly at the stale bound turns ALL rows off: an aged frame
    /// means "no host", not "models went stale".
    #[tokio::test]
    async fn test_stale_frame_yields_zero_rows_like_offline() {
        let rows5 =
            live(&pool_with_stale(5, vec![proc("qwen3", "ready", true, "u", true, 0, 0)]).await)
                .await;
        assert!(rows5.all().is_empty(), "5s-old frame is not live");

        let rows6 =
            live(&pool_with_stale(6, vec![proc("qwen3", "ready", true, "u", true, 0, 0)]).await)
                .await;
        assert!(rows6.all().is_empty(), "6s-old frame yields no rows");
    }

    /// An offline handle (no snapshot ever delivered) yields zero rows —
    /// the host is simply absent, not "all models went stale".
    #[tokio::test]
    async fn test_offline_handle_zero_rows() {
        let rows = live(&pool_without_latest().await).await;
        assert!(rows.all().is_empty());
        assert!(!rows.online("qwen3"));
        assert_eq!(rows.ready_count(), 0);
    }

    /// plan-193 T5c: a `budget_exhausted` process is surfaced as a row even
    /// when the process itself is dead — the tamad keeps reporting the budget
    /// state and the proxy reads it for the 503 path. Because the budgeted
    /// process is dead, it is NOT online and NOT counted ready.
    #[tokio::test]
    async fn test_non_eligible_statuses_excluded() {
        let stats = stats_with(vec![
            proc("m-fail", "failed", true, "a", true, 0, 0),
            proc("m-budget", "budget_exhausted", true, "b", true, 0, 0),
            proc("m-unload", "unloading", true, "c", true, 0, 0),
            proc("m-ready", "ready", true, "d", true, 0, 0),
        ]);
        let rows = live(&pool_with(stats).await).await;
        assert_eq!(rows.ready_count(), 1);
        assert!(rows.row("m-ready").is_some());
        assert!(!rows.online("m-fail"));
        // budget_exhausted IS a row (T5c) but a dead one: not online...
        assert!(rows.online("m-budget"));
        // ...not counted ready.
        assert!(!rows.online("m-unload"));
        assert_eq!(rows.all().len(), 2);
    }

    /// A `budget_exhausted` process keeps a row even with `alive: false` —
    /// the liveness exemption, spelled out (the test above's process was
    /// alive WITH a budget status).
    #[tokio::test]
    async fn test_budget_exhausted_row_survives_dead_process() {
        let stats = stats_with(vec![proc(
            "m-budget",
            "budget_exhausted",
            false,
            "e",
            true,
            9,
            10,
        )]);
        let rows = live(&pool_with(stats).await).await;
        let r = rows.row("m-budget").expect("budget_exhausted keeps a row");
        assert_eq!(r.status, "budget_exhausted");
        assert!(!r.alive);
        assert_eq!(r.pid, 100, "pid rides wire field 3 (test proc pid)");
        assert!(!rows.online("m-budget"));
        assert_eq!(rows.ready_count(), 0);
    }

    /// A `ready` row carries the wire process id (wire field 3) — the
    /// source of the `/status` `backend_pid` (plan-193 T5c).
    #[tokio::test]
    async fn test_row_pid_rides_wire_field() {
        let rows = live(
            &pool_with(stats_with(vec![proc(
                "qwen3",
                "ready",
                true,
                "http://127.0.0.1:8080",
                true,
                0,
                3,
            )]))
            .await,
        )
        .await;
        let r = rows.row("qwen3").expect("ready row present");
        assert_eq!(r.pid, 100);
    }

    /// A dead (alive=false) process is never a row.
    #[tokio::test]
    async fn test_dead_process_excluded() {
        let stats = stats_full(
            1.5,
            vec![],
            vec![proc("m-dead", "ready", false, "never", false, 0, 0)],
        );
        let rows = live(&pool_with(stats).await).await;
        assert!(rows.all().is_empty());
    }
}
