//! E2E: reconciler tick-loop behavior against a stub tamad (plan-191
//! follow-up).
//!
//! The chain under test is [`tama_web::reconciler::tick`] driving real
//! `LoadModel` gRPC RPCs into the shared [`StubTamad`] (the same stub the
//! pool/dashboard tests use). `LoadModel` either succeeds after a
//! per-model scripted delay (simulating the tamad-side health poll up to
//! `startup_timeout_secs`) or fails immediately — this exercises the two
//! convergence-loop invariants:
//!
//! 1. **No tick starvation** — a slow in-flight load must not block the
//!    decision/dispatch of the other desired models in the same tick, and a
//!    load in flight must not be double-issued by a later tick.
//! 2. **Bounded failed loads** — a model whose loads keep failing exhausts
//!    the `max_restarts` budget (counted per attempt, not just on success)
//!    and stops being re-issued within `RESTART_WINDOW`.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tama_core::config::{BackendConfig, Config};
use tama_core::providers::{Protocol, TamadConnection, TamadStatus};
use tama_core::proxy::ProxyState;
use tama_core::tamad::pool::test_support::{start_stub, stub_default, wait_for, StubTamad};
use tama_web::reconciler::{self, InFlightMap, RestartMap};

use common::with_schema;

const TAMAD_ID: &str = "uuid-reconciler";

/// One `model_configs` row (+ `model_files` row) for a local llama_cpp
/// model whose repo id IS the config key (no slash). Returns the row id.
async fn insert_model(db_pool: &sqlx::PgPool, name: &str) -> i64 {
    let model_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO model_configs \
             (repo_id, display_name, backend, enabled, selected_quant, api_name) \
             VALUES ($1, $2, 'llama_cpp', true, 'Q4_K_M', $1) RETURNING id",
    )
    .bind(name)
    .bind(name)
    .fetch_one(db_pool)
    .await
    .unwrap_or_else(|e| panic!("insert model_configs for {name}: {e}"));
    tama_core::db::queries::upsert_model_file(
        db_pool,
        model_id,
        name,
        "m.gguf",
        Some("Q4_K_M"),
        None,
        Some(16),
    )
    .await
    .unwrap_or_else(|e| panic!("insert model_file for {name}: {e}"));
    model_id
}

/// ProxyState + stub tamad wired for a reconciler run:
///
/// - llama_cpp backend path + temp models dir in the config
/// - installation row for `llama_cpp`/`cuda` (binary + health URL template)
/// - one local provider row bound to the stub tamad
/// - one model config + one `desired_models` row per desired model
/// - the stub tamad in the pool (stats stream delivering fresh snapshots)
///
/// `load_delays` script a per-model delay inside the stub's `LoadModel`
/// (simulated tamad-side health poll); `load_fail` makes every `LoadModel`
/// fail immediately.
async fn setup_env(
    desired_models: &[&str],
    max_restarts: u32,
    load_delays: &[(&str, Duration)],
    load_fail: bool,
) -> (
    common::SchemaGuard,
    Arc<ProxyState>,
    StubTamad,
    tempfile::TempDir,
) {
    let guard = with_schema().await;
    let db_pool = Arc::new(guard.pool.clone());
    let models_dir = tempfile::tempdir().unwrap();

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.path().to_string_lossy().to_string());
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/llama-server".to_string()),
            version: None,
            gpu_variant: None,
        },
    );
    config.lifecycle.max_restarts = max_restarts;

    let state = Arc::new(ProxyState::new(config, None, db_pool.clone()));

    let manager = tama_core::installations::InstallationManager::new(db_pool.clone());
    manager
        .save_config(
            "llama_cpp",
            "cpu",
            &[],
            &[],
            Some("http://localhost:5801/health"),
        )
        .await
        .unwrap();

    let mut stub = stub_default();
    stub.load_delays = load_delays
        .iter()
        .map(|(m, d)| (m.to_string(), *d))
        .collect();
    if load_fail {
        *stub.load_model_fail.lock().await = true;
    }
    let addr = start_stub(stub.clone()).await;
    let url = format!("grpc://{addr}");

    tama_core::db::queries::insert_tamad(
        &db_pool,
        TAMAD_ID,
        "stub-host",
        &url,
        "grpc",
        Some("secret"),
    )
    .await
    .unwrap();
    tama_core::db::queries::insert_provider(
        &db_pool,
        "prov-reconciler",
        "local",
        "llama_cpp",
        Some(TAMAD_ID),
        None,
        None,
    )
    .await
    .unwrap();

    for model in desired_models {
        insert_model(&db_pool, model).await;
        tama_core::db::queries::set_desired(&db_pool, model, TAMAD_ID)
            .await
            .unwrap();
    }
    // DB rows → in-memory registry (the public seam used after API
    // mutations; `registry` itself is crate-private).
    state.reload_model_configs().await.unwrap();

    state
        .tamad_pool()
        .upsert_connection(&TamadConnection {
            id: TAMAD_ID.to_string(),
            name: "stub-host".to_string(),
            url,
            protocol: Protocol::Grpc,
            token: Some("secret".to_string()),
            status: TamadStatus::Unknown,
        })
        .await
        .unwrap();

    // Wait for the stats stream to come up with a fresh snapshot — the
    // reconciler never acts on stale data.
    let handle = state
        .tamad_pool()
        .get(TAMAD_ID)
        .await
        .expect("handle registered");
    assert!(
        wait_for(|| async {
            handle.is_online().await
                && handle
                    .latest_fresh(reconciler::SNAPSHOT_MAX_AGE)
                    .await
                    .is_some()
        })
        .await,
        "stub tamad never came online with a fresh snapshot"
    );

    (guard, state, stub, models_dir)
}

/// A slow `LoadModel` (4s of simulated tamad-side health poll) must not
/// block the tick loop: both desired models are decided and dispatched in
/// the same tick — the second is dispatched while the first is still in
/// flight — and the in-flight load is not double-issued by the next tick.
#[tokio::test]
async fn test_slow_load_does_not_starve_tick_loop() {
    let (guard, state, stub, _models_dir) = setup_env(
        &["slow-model", "fast-model"],
        10,
        &[
            ("slow-model", Duration::from_millis(4_000)),
            ("fast-model", Duration::from_millis(50)),
        ],
        false,
    )
    .await;

    let mut restarts: HashMap<String, RestartMap> = HashMap::new();
    let mut in_flight: InFlightMap = HashMap::new();

    // Tick 1 must dispatch BOTH loads without awaiting the 3s slow one.
    let started = Instant::now();
    reconciler::tick(state.clone(), &mut restarts, &mut in_flight)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(1_500),
        "tick waited {elapsed:?} on a 4s in-flight load — the loop is starved"
    );
    assert!(
        in_flight.contains_key("slow-model") && in_flight.contains_key("fast-model"),
        "both loads must be in flight after tick 1: {:?}",
        in_flight.keys().collect::<Vec<_>>()
    );

    // The attempt is counted at dispatch time (budget consumed before any
    // outcome is known).
    assert_eq!(
        restarts
            .get(TAMAD_ID)
            .and_then(|t| t.get("slow-model"))
            .map(|(count, _)| *count),
        Some(1),
        "dispatched attempt must be counted against the restart budget"
    );

    // Tick 2: the in-flight loads must not be re-issued.
    reconciler::tick(state.clone(), &mut restarts, &mut in_flight)
        .await
        .unwrap();
    assert_eq!(
        in_flight.len(),
        2,
        "no double-issue while in flight: {:?}",
        in_flight.keys().collect::<Vec<_>>()
    );

    // Wait for both loads to settle (the fast one queues behind the slow
    // one on the shared per-tamad client lock), each exactly once.
    for task in in_flight.drain().map(|(_, t)| t) {
        let result = tokio::time::timeout(Duration::from_secs(30), task.join)
            .await
            .expect("load task hung")
            .expect("load task did not panic");
        assert!(result.is_ok(), "load must succeed: {result:?}");
    }
    let requests = stub.load_requests.lock().await;
    let counts = |model: &str| requests.iter().filter(|r| r.model_name == model).count();
    assert_eq!(
        counts("slow-model"),
        1,
        "slow-model was double-issued: {:?}",
        requests.iter().map(|r| &r.model_name).collect::<Vec<_>>()
    );
    assert_eq!(counts("fast-model"), 1, "fast-model was double-issued");

    guard.finish().await;
    drop(_models_dir);
}

/// A model whose `LoadModel` calls all fail must exhaust the restart
/// budget: exactly `max_restarts` attempts are dispatched, and the tick
/// loop stops re-issuing while the (short) `RESTART_WINDOW` is unexpired.
///
/// (Before the fix the budget was only counted on success, so a
/// persistently-failing model was re-issued — spawning and killing a
/// process on the tamad — on every single tick.)
#[tokio::test]
async fn test_failing_loads_exhaust_restart_budget() {
    let (guard, state, stub, _models_dir) = setup_env(&["broken-model"], 2, &[], true).await;

    let mut restarts: HashMap<String, RestartMap> = HashMap::new();
    let mut in_flight: InFlightMap = HashMap::new();

    // Drive ticks past the two allowed attempts. Failing loads fail
    // immediately; the small sleep lets each dispatched task complete and
    // be reaped before the next tick re-decides.
    for _ in 0..8 {
        reconciler::tick(state.clone(), &mut restarts, &mut in_flight)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let count = restarts
        .get(TAMAD_ID)
        .and_then(|t| t.get("broken-model"))
        .map(|(count, _)| *count);
    assert_eq!(
        count,
        Some(2),
        "exactly max_restarts attempts must be counted"
    );
    assert!(
        in_flight.is_empty(),
        "no load should remain in flight after budget exhaustion"
    );
    assert_eq!(
        stub.load_requests.lock().await.len(),
        2,
        "no re-issuance after the restart budget is exhausted"
    );

    guard.finish().await;
    drop(_models_dir);
}
