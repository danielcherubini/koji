//! In-memory process table for the tamad daemon.
//!
//! Tracks the backend processes this tamad has spawned. The lifecycle
//! module (plan-191 Task 5) populates it; the stats stream (Task 3) reads
//! it once per tick; `RestartProvider` re-launches from the stored spec.
//! Tamad holds no database — this table is the sole record of what is
//! running and dies with the process.

use std::collections::HashMap;
use std::time::Instant;

use tama_core::tamad::LoadModelRequest;

/// A running (or recently running) backend process.
#[derive(Clone)]
pub struct ProcessEntry {
    /// Model name — the unique key in the table.
    pub model_name: String,
    /// Backend name, e.g. "llama.cpp".
    pub provider_name: String,
    /// OS process id of the spawned backend.
    pub pid: u32,
    /// Health/endpoint URL of the running backend.
    pub endpoint_url: String,
    /// One of the canonical lifecycle status words (plan-193 T2 pins
    /// the six in `lifecycle::status`):
    /// `starting` | `ready` | `restarting` | `failed` | `budget_exhausted`
    /// | `unloading`.
    /// `failed` also covers a backend that crashed after launch: the
    /// tamad's reap task is the authoritative liveness signal (a zombie
    /// pid would otherwise still answer `kill(pid, 0)`).
    pub status: String,
    /// When this entry was created (process launch requested).
    /// Used by Task 5 (idle/restart accounting).
    #[allow(dead_code)]
    pub started_at: Instant,
    /// Full launch spec (the `LoadModelRequest` that started this process) —
    /// required so `RestartProvider` can re-load without proxy involvement
    /// (plan-191 Task 5).
    #[allow(dead_code)]
    pub spec: LoadModelRequest,
    /// Respawn attempts inside the current [`lifecycle::RESTART_WINDOW_SECS`]
    /// window — kept in sync with [`window_starts`](Self::window_starts)
    /// (the derived count, published for readers that must not re-derive).
    pub restart_count: u32,
    /// Unix-millis start stamps of the respawns inside the sliding
    /// 300-second budget window (trim trailing entry to stay alive).
    pub window_starts: Vec<i64>,
    /// Mirror of the on-disk store's `user_flagged` for this key —
    /// the operator marker that stops auto-respawn for this row.
    pub user_flagged: bool,
}

/// A PID that is guaranteed dead: anything above the kernel's `pid_max`
/// can never exist.
///
/// Used by tests to simulate a crashed backend. (Do NOT use `u32::MAX`
/// — it casts to `-1` as `pid_t`, which `kill(-1, 0)` treats as "my
/// process group" and reports alive.)
#[allow(dead_code)] // consumed by tests on this task; runtime use lands in Task 5
pub(crate) fn guaranteed_dead_pid() -> u32 {
    std::fs::read_to_string("/proc/sys/kernel/pid_max")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|max| max.saturating_add(4096))
        .unwrap_or(4_194_000)
}

/// In-memory table of backend processes keyed by model name.
#[derive(Default)]
pub struct ProcessTable {
    inner: tokio::sync::RwLock<HashMap<String, ProcessEntry>>,
}

// Methods consumed by the lifecycle module (plan-191 Task 5) and by the
// stats snapshot on this task; the dead_code allow is removed in Task 5.
#[allow(dead_code)]
impl ProcessTable {
    /// Insert or replace the entry for `entry.model_name`.
    pub async fn insert(&self, entry: ProcessEntry) {
        self.inner
            .write()
            .await
            .insert(entry.model_name.clone(), entry);
    }

    /// Remove the entry for `model_name`, returning it if present.
    pub async fn remove(&self, model_name: &str) -> Option<ProcessEntry> {
        self.inner.write().await.remove(model_name)
    }

    /// Get a clone of the entry for `model_name`, if present.
    pub async fn get(&self, model_name: &str) -> Option<ProcessEntry> {
        self.inner.read().await.get(model_name).cloned()
    }

    /// Mark the entry for `model_name` as "failed" when its process
    /// exited.
    ///
    /// Called by the lifecycle's reap task's legacy arm: the row has
    /// no desired/unflagged store row to respawn from, so the death is
    /// terminal for this process. The line is only made when the row is
    /// still "ours" — a canonical status state that has not yet been
    /// marked (terminal during re-spawn, or a self-line).
    /// Missing entries (already unloaded) are a no-op.
    pub async fn mark_failed(&self, model_name: &str, pid: u32) {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.get_mut(model_name) {
            if entry.pid == pid
                && crate::lifecycle::status::is_accepted(&entry.status)
                && Self::can_move_downstream(&entry.status)
            {
                entry.status = crate::lifecycle::status::FAILED.to_string();
            }
        }
    }

    /// All entries (order unspecified).
    pub async fn list(&self) -> Vec<ProcessEntry> {
        self.inner.read().await.values().cloned().collect()
    }

    /// Record one death for `model_name`: append `now_ms` to the entry's
    /// restart window, trim the trailing entry down to the live range
    /// (counted in [`lifecycle::RESTART_WINDOW_SECS`]), and publish the
    /// survivor count as `restart_count` (the row becomes
    /// self-checked for the budget gate; the fields stay cleared on
    /// the trigger outcome — the caller decides which one follows
    /// persisting the row).
    ///
    /// Returns the post-trim count; `None` when the entry is absent
    /// (already unloaded).
    pub async fn record_restart_window(&self, model_name: &str, now_ms: i64) -> Option<u32> {
        let cutoff = now_ms - (crate::lifecycle::RESTART_WINDOW_SECS as i64) * 1000;
        let mut inner = self.inner.write().await;
        let entry = inner.get_mut(model_name)?;
        entry.window_starts.push(now_ms);
        entry.window_starts.retain(|ts| *ts >= cutoff);
        entry.restart_count = entry.window_starts.len() as u32;
        Some(entry.restart_count)
    }

    /// Return the row to `restarting` — the row is rising, defunct
    /// — before the re-spawn launch re-injects it (the reaper marks it
    /// synchronously; `load()` flips it to `starting` the moment the
    /// replacement actually launches). Guarded so a stale or racing
    /// reaper never clobbers a row that has already made a terminal
    /// decision for this death (triggered, triggered, self-clobbered).
    ///
    /// Returns `false` when the row is not at the reaper's point in
    /// such a way at all; the reaper must bail.
    pub async fn mark_restarting(&self, model_name: &str, pid: u32) -> bool {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.get_mut(model_name) {
            if entry.pid == pid
                && crate::lifecycle::status::is_accepted(&entry.status)
                && Self::can_move_downstream(&entry.status)
            {
                entry.status = crate::lifecycle::status::RESTARTING.to_string();
                return true;
            }
        }
        false
    }

    /// Trigger the restart budget for a key: set the row to
    /// `budget_exhausted`, stamp the operator's mark on the row
    /// (mirror of the on-disk `user_flagged` — the disk write
    /// happens over in the store, and the caller does it after this
    /// succeeds), and stop the automatic re-spawn for this row.
    ///
    /// Same guard as [`mark_restarting`](Self::mark_restarting).
    pub async fn mark_budget_exhausted(&self, model_name: &str, pid: u32) -> bool {
        let mut inner = self.inner.write().await;
        if let Some(entry) = inner.get_mut(model_name) {
            if entry.pid == pid
                && crate::lifecycle::status::is_accepted(&entry.status)
                && Self::can_move_downstream(&entry.status)
            {
                entry.status = crate::lifecycle::status::BUDGET_EXHAUSTED.to_string();
                entry.user_flagged = true;
                return true;
            }
        }
        false
    }

    /// The shared guard for an intent rewrite: the status is a
    /// canonical vocabulary word and the transition point is still
    /// open (not `failed` — already logged; not `restarting` — the
    /// reaper for this death is in the middle of reaping; not
    /// `budget_exhausted` — the budget trip is final; not
    /// `unloading` — an operator's unload has a live self-line).
    fn can_move_downstream(status: &str) -> bool {
        !matches!(
            status,
            crate::lifecycle::status::FAILED
                | crate::lifecycle::status::RESTARTING
                | crate::lifecycle::status::BUDGET_EXHAUSTED
                | crate::lifecycle::status::UNLOADING
        )
    }

    /// Table-pure snapshot of the entries for the stats tick (plan-193
    /// T3). The caller (`stream_stats`) applies the store-augmenting
    /// [`to_process_info`](crate::lifecycle::to_process_info) builder per
    /// entry — this accessor never touches the store, so the table stays
    /// free of `Store` imports.
    pub async fn snapshot(&self) -> Vec<ProcessEntry> {
        self.inner.read().await.values().cloned().collect()
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(model: &str, pid: u32) -> ProcessEntry {
        ProcessEntry {
            model_name: model.to_string(),
            provider_name: "llama.cpp".to_string(),
            pid,
            endpoint_url: format!("http://127.0.0.1:180{}0", model.len()),
            status: "ready".to_string(),
            started_at: Instant::now(),
            spec: LoadModelRequest::default(),
            restart_count: 0,
            window_starts: Vec::new(),
            user_flagged: false,
        }
    }

    /// A PID that is guaranteed dead (see `guaranteed_dead_pid`).
    #[cfg(test)]
    fn dead_pid() -> u32 {
        guaranteed_dead_pid()
    }

    /// insert + get + list reflect the stored entries.
    #[tokio::test]
    async fn test_insert_get_list() {
        let table = ProcessTable::default();
        table.insert(entry("alpha", 100)).await;
        table.insert(entry("beta", 200)).await;

        let got = table.get("alpha").await.expect("alpha must exist");
        assert_eq!(got.provider_name, "llama.cpp");
        assert_eq!(got.pid, 100);
        assert_eq!(got.status, "ready");

        assert!(table.get("gamma").await.is_none());
        assert_eq!(table.list().await.len(), 2);

        // Re-inserting the same model name replaces the entry.
        table.insert(entry("alpha", 111)).await;
        assert_eq!(table.get("alpha").await.unwrap().pid, 111);
        assert_eq!(table.list().await.len(), 2);
    }

    /// remove returns the entry once and nothing the second time.
    #[tokio::test]
    async fn test_remove() {
        let table = ProcessTable::default();
        table.insert(entry("alpha", 100)).await;

        let removed = table.remove("alpha").await.expect("must be removed");
        assert_eq!(removed.model_name, "alpha");
        assert!(table.get("alpha").await.is_none());
        assert!(table.remove("alpha").await.is_none());
        assert!(table.list().await.is_empty());
    }

    /// snapshot() hands back table entries (the caller computes the wired
    /// liveness + store fields): the test process itself is alive, a PID
    /// above pid_max is dead.
    #[tokio::test]
    async fn test_snapshot_liveness() {
        let table = ProcessTable::default();
        table.insert(entry("alive-model", std::process::id())).await;
        table.insert(entry("dead-model", dead_pid())).await;

        let snap = table.snapshot().await;
        assert_eq!(snap.len(), 2);

        let alive = snap
            .iter()
            .find(|e| e.model_name == "alive-model")
            .expect("alive-model in snapshot");
        assert_eq!(alive.pid, std::process::id());
        assert_eq!(alive.status, "ready");
        assert!(
            alive.status != "failed" && crate::process::is_process_alive(alive.pid),
            "own process (pid {}) must be reported alive",
            std::process::id()
        );

        let dead = snap
            .iter()
            .find(|e| e.model_name == "dead-model")
            .expect("dead-model in snapshot");
        assert!(
            !(dead.status != "failed" && crate::process::is_process_alive(dead.pid)),
            "PID above pid_max must be reported dead"
        );
        assert!(!crate::process::is_process_alive(dead_pid()));
    }

    /// Empty table → empty snapshot (no panic).
    #[tokio::test]
    async fn test_snapshot_empty() {
        let table = ProcessTable::default();
        assert!(table.snapshot().await.is_empty());
    }

    /// mark_failed flips the entry to "failed" when the pid matches, and is
    /// a no-op for a different pid (a stale wait task after a restart) or a
    /// missing entry (already unloaded).
    #[tokio::test]
    async fn test_mark_failed() {
        let table = ProcessTable::default();
        let my_pid = std::process::id();
        table.insert(entry("alpha", my_pid)).await;

        // Wrong pid → untouched (stale wait task from a previous load).
        table.mark_failed("alpha", my_pid.saturating_add(7)).await;
        assert_eq!(table.get("alpha").await.unwrap().status, "ready");

        // Missing entry → no-op.
        table.mark_failed("ghost", my_pid).await;

        // Matching pid → failed.
        table.mark_failed("alpha", my_pid).await;
        assert_eq!(table.get("alpha").await.unwrap().status, "failed");
    }

    /// A "failed" entry is reported (via the caller's alive fold) dead even
    /// if the (zombie) pid still answers `kill(pid, 0)` — the reap task that
    /// marks it failed is the authoritative liveness signal. The snapshot
    /// preserves the entry's failed status.
    #[tokio::test]
    async fn test_snapshot_failed_entry_reports_dead() {
        let table = ProcessTable::default();
        let my_pid = std::process::id(); // guaranteed-alive pid
        table.insert(entry("crashed", my_pid)).await;
        table.mark_failed("crashed", my_pid).await;

        let snap = table.snapshot().await;
        let e = snap.iter().find(|e| e.model_name == "crashed").unwrap();
        assert_eq!(e.status, "failed");
        assert!(
            e.status == "failed" || !crate::process::is_process_alive(e.pid),
            "failed entry must be reported dead"
        );
    }
}
