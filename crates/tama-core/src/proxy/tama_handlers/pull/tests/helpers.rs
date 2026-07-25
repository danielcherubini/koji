use std::sync::Arc;

use axum::Router;

use crate::proxy::pull_queue::PullQueueService;
use crate::proxy::ProxyState;

/// Serializes env-var-mutating tests (repo convention — see
/// models/pull/mod.rs ENV_GUARD).
#[allow(dead_code)]
pub(crate) static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// ProxyState on a tempdir DB with a PullQueueService, mirroring
/// crates/tama/tests/downloads_api.rs create_test_state.
/// Returns (state, db TempDir — keep alive).
pub(crate) fn create_test_state() -> (Arc<ProxyState>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let db_dir = tmp.path().to_path_buf();
    let mgr = crate::models::ModelManager::open(&db_dir).unwrap();
    let svc = PullQueueService::new(mgr, 2);
    let config = crate::config::Config::default();
    let mut state = ProxyState::new(config, Some(db_dir));
    state.set_pull_queue(Some(Arc::new(svc)));
    (Arc::new(state), tmp)
}

/// Minimal router mounting the three pull handlers (routes mirror
/// crates/tama-core/src/proxy/server/router.rs:67-69).
pub(crate) fn pull_router(state: Arc<ProxyState>) -> Router {
    use crate::proxy::tama_handlers::pull::handlers;
    use axum::routing::{get, post};
    Router::new()
        .route("/tama/v1/pulls", post(handlers::handle_tama_pull_model))
        .route(
            "/tama/v1/pulls/:job_id",
            get(handlers::handle_tama_get_pull_job),
        )
        .route(
            "/tama/v1/pulls/:job_id/stream",
            get(handlers::handle_pull_job_stream),
        )
        .with_state(state)
}
