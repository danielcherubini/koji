//! Proxy-side convergence loop (plan-191 Task 5).
//!
//! The proxy tracks *desired* model state in the central DB
//! (`desired_models`); the *actual* state is the per-tick process snapshot
//! streamed from each tamad over `StreamStats` (Task 3/4). This reconciler
//! runs once per second and converges actual toward desired by issuing
//! `LoadModel` / `UnloadModel` RPCs — the proxy itself never spawns or
//! kills backend processes (ADR-0010).
//!
//! Restart accounting: every `LoadModel` *attempt* for a desired model
//! that is missing or dead — success and failure alike, counted at
//! dispatch time — increments an in-memory counter; when the counter
//! reaches the configured `max_restarts` within a sliding window the model
//! is left alone (logged) until the window expires or the model becomes
//! healthy again (counter reset). Counting failed attempts (not just
//! successful loads) is what bounds a persistently-failing model: without
//! it, `load_allowed` never flips and the loop re-issues LoadModel every
//! tick, spawning and killing a process on the tamad indefinitely.
//!
//! Loads never block the tick loop: a `LoadModel` RPC includes tamad-side
//! health polling of up to `proxy.startup_timeout_secs` (30–120s), so
//! awaiting one inline would starve convergence of every other model AND
//! tamad. Each allowed load is therefore dispatched on a tracked
//! background task (`in_flight` registry): the tick loop moves on, in-flight
//! models are never double-issued, and every spawned task self-terminates
//! (its outcome or a timeout cap well above the tamad-side poll bound),
//! so in-flight loads can't leak or panic on shutdown.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use tama_core::proxy::ProxyState;
use tama_core::tamad::ProcessInfo;

/// Sliding window for crash-restart counting. A model may be re-issued
/// `max_restarts` loads within this window; the window resets on expiry or
/// when the model is observed healthy.
pub const RESTART_WINDOW: Duration = Duration::from_secs(300);

/// How fresh a tamad snapshot must be for the reconciler to act on it.
/// Never act on stale data — a dropped/late snapshot must not trigger
/// spurious loads or unloads.
pub const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(5);

/// Reconcile interval.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// Safety-net slack applied on top of `proxy.startup_timeout_secs` for a
/// hung `LoadModel` RPC (process spawn + transport overhead beyond the
/// tamad-side health poll bound).
pub const LOAD_TIMEOUT_SLACK: Duration = Duration::from_secs(30);

/// A single convergence action for one model on one tamad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Desired but missing (or dead) — issue `LoadModel`.
    Load { model: String },
    /// Running but not desired — issue `UnloadModel`.
    Unload { model: String },
}

/// Per-model crash-restart counter: `(count, window_start)`.
pub type RestartMap = HashMap<String, (u32, Instant)>;

/// A `LoadModel` dispatched on a previous tick that has not returned yet.
///
/// The attempt is already counted against the model's restart budget at
/// dispatch time; the join's outcome only affects logging.
pub struct LoadTask {
    /// Model config key (the `InFlightMap` key).
    pub model: String,
    /// Name of the tamad the load was issued to (log context only).
    pub tamad: String,
    /// Spawned load task: `Ok(backend key)` / `Err(error description)`.
    pub join: tokio::task::JoinHandle<Result<String, String>>,
}

/// In-flight loads keyed by model config key. A model maps to exactly one
/// provider (→ one tamad), so the model alone is the correct de-dup key,
/// regardless of which (tamad, desired-row) pair produced the dispatch.
pub type InFlightMap = HashMap<String, LoadTask>;

/// Pure per-tick decision: given the desired model set for one tamad, the
/// actual process snapshot (`None` = no fresh snapshot — never act on
/// stale data), and the in-memory restart counters, return the actions
/// that converge actual toward desired.
///
/// - desired model missing from `actual` → `Load`
/// - desired model present but `alive == false` → `Load` (crash restart)
/// - desired and alive → no action (healthy — caller resets the counter)
/// - actual model not in `desired` → `Unload`
/// - restarts: a `Load` is bounded by `max_restarts` per `RESTART_WINDOW`; the
///   caller records every dispatched load — success and failure alike —
///   with `record_attempt`; a counter whose window has expired is
///   treated as fresh.
pub fn decide(
    desired: &[String],
    actual: Option<&[ProcessInfo]>,
    restarts: &RestartMap,
    max_restarts: u32,
    now: Instant,
) -> Vec<Action> {
    // Never act on stale/missing data.
    let Some(actual) = actual else {
        return Vec::new();
    };

    let mut actions = Vec::new();

    for model in desired {
        let present = actual
            .iter()
            .find(|p| p.model_name == *model)
            .filter(|p| p.alive);
        if present.is_none() {
            // Missing or dead — may need (re)load, bounded by restarts.
            if load_allowed(model, restarts, max_restarts, now) {
                actions.push(Action::Load {
                    model: model.clone(),
                });
            } else {
                warn!(
                    model = %model,
                    "restart budget exhausted for desired model; skipping load until window expires"
                );
            }
        }
    }

    for p in actual {
        if !desired.iter().any(|d| d == &p.model_name) {
            actions.push(Action::Unload {
                model: p.model_name.clone(),
            });
        }
    }

    actions
}

/// Whether a (re)load for `model` is within the restart budget.
fn load_allowed(model: &str, restarts: &RestartMap, max_restarts: u32, now: Instant) -> bool {
    match restarts.get(model) {
        Some((count, window_start)) if now.duration_since(*window_start) < RESTART_WINDOW => {
            *count < max_restarts
        }
        // No counter yet, or the window has expired — allow.
        _ => true,
    }
}

/// Record a load *attempt* — success and failure alike, counted at
/// dispatch time because the outcome is unknown yet — against the model's
/// restart budget: bump the counter and (re)start the window.
///
/// This is what makes a persistently-failing model behave like a crashing
/// one: `max_restarts` total attempts per `RESTART_WINDOW`, then quiet.
/// The window resets on expiry (`decide`) or when the model is observed
/// healthy (the reset loop in `tick`).
pub fn record_attempt(restarts: &mut RestartMap, model: &str, now: Instant) {
    let entry = restarts.entry(model.to_string()).or_insert((0, now));
    entry.0 = entry.0.saturating_add(1);
    entry.1 = now;
}

/// Background reconciler task: every second, for every online tamad,
/// converge the actual process snapshot to the desired set from the DB.
pub async fn run(state: Arc<ProxyState>) {
    let mut interval = tokio::time::interval(TICK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Model → (count, window_start), keyed by "tamad_id" per tamad.
    let mut restarts: HashMap<String, RestartMap> = HashMap::new();
    // Model config key → in-flight LoadModel task (see `tick`).
    let mut in_flight: InFlightMap = HashMap::new();

    loop {
        interval.tick().await;
        if let Err(e) = tick(state.clone(), &mut restarts, &mut in_flight).await {
            warn!("reconciler tick failed: {}", e);
        }
    }
}

/// One reconcile pass over all online tamads.
///
/// **Loads are dispatched, never awaited inline.** A `LoadModel` RPC runs
/// on the tamad for up to `proxy.startup_timeout_secs` (30–120s of health
/// polling after the process spawn), so awaiting one here would stall
/// convergence of every other model AND tamad for the whole time (and, with
/// an unbounded retry, a persistently-failing model would pin the loop
/// forever). Each allowed load is instead spawned on a task tracked in
/// `in_flight`: the tick loop drains finished tasks (non-blocking) and
/// keeps issuing Unloads + Loads for everything else. Two guards follow
/// from that:
///
/// - an in-flight model is never re-issued by a later tick (the
///   `in_flight` lookup in the dispatch loop);
/// - every dispatched attempt is counted with `record_attempt` — success
///   and failure alike — so a model whose loads keep failing exhausts
///   `max_restarts` within `RESTART_WINDOW` and stops churning.
///
/// The spawned task owns an `Arc<ProxyState>` clone and self-terminates
/// within `startup_timeout_secs + LOAD_TIMEOUT_SLACK` (its outcome, or the
/// timeout of a wedged RPC), so in-flight loads can't leak or block
/// shutdown; on runtime teardown it is cancelled with everything else.
///
/// Exposed (not just called from `run`) so it can be unit/integration
/// tested directly.
pub async fn tick(
    state: Arc<ProxyState>,
    restarts: &mut HashMap<String, RestartMap>,
    in_flight: &mut InFlightMap,
) -> anyhow::Result<()> {
    // Reap finished loads (non-blocking) first: log the outcome and free
    // the in-flight slot. A model still missing after a failed load can be
    // re-decided below (bounded by the budget); a model that did come up is
    // reset by the healthy loop further down.
    drain_in_flight(in_flight).await;

    let pool = state.tamad_pool();
    let db_pool = state.db_pool();
    let max_restarts = state.with_config(|c| c.lifecycle.max_restarts).await;
    // Safety-net cap for a wedged RPC: strictly above the tamad-side
    // health-poll bound (`startup_timeout_ms = startup_timeout_secs * 1000`)
    // plus spawn/transport slack, so normal slow loads are never cut off.
    let load_cap_secs = state
        .with_config(|c| {
            c.proxy
                .startup_timeout_secs
                .saturating_add(LOAD_TIMEOUT_SLACK.as_secs())
        })
        .await;
    let now = Instant::now();

    for handle in pool.list_handles().await {
        let tamad_id = handle.connection.id.clone();
        let tamad_name = handle.connection.name.clone();

        if !handle.is_online().await {
            continue;
        }
        // Never act on stale data: skip the tick for this tamad if no
        // fresh snapshot is available.
        let Some(stats) = handle.latest_fresh(SNAPSHOT_MAX_AGE).await else {
            continue;
        };

        let desired_rows = tama_core::db::queries::list_desired(&db_pool, Some(&tamad_id)).await?;
        let desired: Vec<String> = desired_rows.iter().map(|d| d.model_name.clone()).collect();

        let tracker = restarts.entry(tamad_id.clone()).or_default();
        let actions = decide(&desired, Some(&stats.processes), tracker, max_restarts, now);

        for action in &actions {
            match action {
                Action::Load { model } => {
                    // A previous load of this model is still in flight:
                    // its attempt is already counted — re-check on the next
                    // tick instead of double-issuing.
                    if in_flight.contains_key(model) {
                        continue;
                    }
                    info!(tamad = %tamad_name, model = %model, "reconciler: loading model");

                    // Count the attempt at dispatch time (success and
                    // failure alike): a model whose loads keep failing
                    // exhausts the restart budget within RESTART_WINDOW
                    // instead of being re-loaded on every tick.
                    record_attempt(tracker, model, now);

                    // Dispatch on a tracked background task, never awaited
                    // inline (see `tick` docs for the design choice).
                    let model_owned = model.clone();
                    let state_owned = state.clone();
                    let cap = Duration::from_secs(load_cap_secs);
                    let join = tokio::spawn(async move {
                        match tokio::time::timeout(
                            cap,
                            tama_core::proxy::lifecycle::spec::load_model_on_tamad(
                                state_owned.as_ref(),
                                &model_owned,
                            ),
                        )
                        .await
                        {
                            Ok(Ok(backend)) => Ok(backend),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_elapsed) => {
                                Err(format!("LoadModel timed out after {}s", cap.as_secs()))
                            }
                        }
                    });
                    in_flight.insert(
                        model.clone(),
                        LoadTask {
                            model: model.clone(),
                            tamad: tamad_name.clone(),
                            join,
                        },
                    );
                }
                Action::Unload { model } => {
                    info!(tamad = %tamad_name, model = %model, "reconciler: unloading model");
                    if let Err(e) = handle.unload_model(model).await {
                        warn!(
                            tamad = %tamad_name,
                            model = %model,
                            error = %e,
                            "reconciler: unload failed"
                        );
                    }
                    // Drop the local mirror entry (model is gone from the
                    // tamad and no longer desired).
                    state.remove_mirror_by_model(model).await;
                }
            }
        }

        // Reset the restart counter for healthy models.
        for p in &stats.processes {
            if p.alive {
                tracker.remove(&p.model_name);
            }
        }

        // Keep the local BackendState mirror in sync with the tamad's
        // process table so the forward path and the management API see
        // live endpoints (staging mirror — Task 10 removes the local
        // lifecycle entirely).
        state.sync_tamad_mirror(&stats.processes, &desired).await;
    }

    Ok(())
}

/// Join finished in-flight loads (non-blocking): log the outcome and free
/// the model's in-flight slot. Pending tasks stay tracked until they
/// return — that is what prevents double-issue on the next tick.
async fn drain_in_flight(in_flight: &mut InFlightMap) {
    let done: Vec<String> = in_flight
        .iter()
        .filter(|(_, t)| t.join.is_finished())
        .map(|(model, _)| model.clone())
        .collect();

    for model in done {
        let Some(task) = in_flight.remove(&model) else {
            continue;
        };
        match task.join.await {
            Ok(Ok(backend)) => info!(
                model = %task.model,
                tamad = %task.tamad,
                backend = %backend,
                "reconciler: load succeeded"
            ),
            Ok(Err(e)) => warn!(
                model = %task.model,
                tamad = %task.tamad,
                error = %e,
                "reconciler: load failed"
            ),
            Err(e) => warn!(
                model = %task.model,
                tamad = %task.tamad,
                error = %e,
                "reconciler: load task aborted"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tama_core::tamad::ProcessInfo;

    fn info(model: &str, alive: bool) -> ProcessInfo {
        ProcessInfo {
            model_name: model.to_string(),
            provider_name: "llama.cpp".to_string(),
            pid: 42,
            alive,
            endpoint_url: format!("http://127.0.0.1:180{}0", model.len()),
            status: "ready".to_string(),
            desired: false,
            restart_count: 0,
            max_restarts: 0,
        }
    }

    fn desired(models: &[&str]) -> Vec<String> {
        models.iter().map(|m| m.to_string()).collect()
    }

    /// Desired model missing from the snapshot → Load (and the extra
    /// actual-not-desired model → Unload).
    #[test]
    fn test_decide_missing_desired_loads() {
        let now = Instant::now();
        let actions = decide(
            &desired(&["alpha"]),
            Some(&[info("beta", true)]),
            &HashMap::new(),
            5,
            now,
        );
        assert_eq!(
            actions,
            vec![
                Action::Load {
                    model: "alpha".to_string()
                },
                Action::Unload {
                    model: "beta".to_string()
                }
            ]
        );
    }

    /// Desired model present but dead → Load (crash restart).
    #[test]
    fn test_decide_dead_desired_loads() {
        let now = Instant::now();
        let actions = decide(
            &desired(&["alpha"]),
            Some(&[info("alpha", false)]),
            &HashMap::new(),
            5,
            now,
        );
        assert_eq!(
            actions,
            vec![Action::Load {
                model: "alpha".to_string()
            }]
        );
    }

    /// Crash restarts are bounded by max_restarts within the window.
    #[test]
    fn test_decide_restart_limit_bounded() {
        let now = Instant::now();
        let mut restarts = HashMap::new();
        restarts.insert("alpha".to_string(), (3, now));
        let actions = decide(
            &desired(&["alpha"]),
            Some(&[info("alpha", false)]),
            &restarts,
            3,
            now,
        );
        assert!(actions.is_empty(), "budget exhausted: {actions:?}");
    }

    /// A counter whose window has expired is treated as fresh.
    #[test]
    fn test_decide_restart_window_expiry() {
        let now = Instant::now();
        let mut restarts = HashMap::new();
        restarts.insert(
            "alpha".to_string(),
            (5, now - RESTART_WINDOW - Duration::from_secs(1)),
        );
        let actions = decide(
            &desired(&["alpha"]),
            Some(&[info("alpha", false)]),
            &restarts,
            3,
            now,
        );
        assert_eq!(
            actions,
            vec![Action::Load {
                model: "alpha".to_string()
            }]
        );
    }

    /// A model running but not desired → Unload.
    #[test]
    fn test_decide_actual_not_desired_unloads() {
        let now = Instant::now();
        let actions = decide(
            &desired(&[]),
            Some(&[info("stray", true)]),
            &HashMap::new(),
            5,
            now,
        );
        assert_eq!(
            actions,
            vec![Action::Unload {
                model: "stray".to_string()
            }]
        );
    }

    /// Desired and alive → no actions.
    #[test]
    fn test_decide_healthy_no_actions() {
        let now = Instant::now();
        let actions = decide(
            &desired(&["alpha"]),
            Some(&[info("alpha", true)]),
            &HashMap::new(),
            5,
            now,
        );
        assert!(actions.is_empty(), "{actions:?}");
    }

    /// Stale/missing snapshot (None) → never act.
    #[test]
    fn test_decide_stale_snapshot_no_actions() {
        let now = Instant::now();
        let actions = decide(&desired(&["alpha"]), None, &HashMap::new(), 5, now);
        assert!(actions.is_empty(), "{actions:?}");
    }

    /// A model whose loads keep failing exhausts the restart budget: each
    /// attempt is counted at dispatch time (success and failure alike, via
    /// `record_attempt` — exactly what `tick` does), so after
    /// `max_restarts` attempts a `Load` is no longer re-issued even though
    /// the model is still missing/dead.
    #[test]
    fn test_decide_repeated_failed_loads_exhaust_budget() {
        let now = Instant::now();
        let mut restarts = HashMap::new();

        // Drive the dispatch loop: decide → record the dispatched attempt,
        // with the model down on every pass.
        for i in 0..3 {
            let actions = decide(
                &desired(&["alpha"]),
                Some(&[info("alpha", false)]),
                &restarts,
                3,
                now,
            );
            assert_eq!(
                actions,
                vec![Action::Load {
                    model: "alpha".to_string()
                }],
                "attempt {i} must still be decided within budget"
            );
            for action in &actions {
                if let Action::Load { model } = action {
                    record_attempt(&mut restarts, model, now);
                }
            }
        }

        // Budget exhausted: no further loads while the model is still down.
        let actions = decide(
            &desired(&["alpha"]),
            Some(&[info("alpha", false)]),
            &restarts,
            3,
            now,
        );
        assert!(actions.is_empty(), "budget exhausted: {actions:?}");
    }

    /// `record_attempt` bumps the counter and (re)starts the window; once
    /// the window expires, `decide` allows a fresh attempt even for a
    /// model whose failed loads exhausted the budget.
    #[test]
    fn test_record_attempt_counts_and_window_expiry_allows_fresh_attempt() {
        let now = Instant::now();
        let mut restarts = HashMap::new();

        record_attempt(&mut restarts, "alpha", now);
        assert_eq!(restarts.get("alpha"), Some(&(1, now)));

        let later = now + Duration::from_secs(30);
        record_attempt(&mut restarts, "alpha", later);
        assert_eq!(restarts.get("alpha"), Some(&(2, later)));

        // Window expired (last attempt was more than RESTART_WINDOW ago) →
        // a fresh attempt is allowed despite the exhausted counter.
        let now2 = later + RESTART_WINDOW + Duration::from_secs(1);
        let actions = decide(
            &desired(&["alpha"]),
            Some(&[info("alpha", false)]),
            &restarts,
            1,
            now2,
        );
        assert_eq!(
            actions,
            vec![Action::Load {
                model: "alpha".to_string()
            }],
            "window expiry must allow a fresh attempt"
        );
    }
}
