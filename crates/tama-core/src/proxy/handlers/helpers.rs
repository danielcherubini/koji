//! Shared helpers for proxy handlers.

use crate::proxy::{live_rows, ProxyState};

/// Get the backend URL for a backend from the live model rows
/// (plan-193 Task 4 read-side flip).
///
/// Returns `Ok(Some(url))` if the backend is loaded and has a live endpoint,
/// `Ok(None)` if the backend exists in the wire but has no endpoint yet
/// (starting) or is offline/absent (no row).
pub(crate) async fn get_backend_url(
    state: &ProxyState,
    backend_name: &str,
) -> anyhow::Result<Option<String>> {
    let rows = live_rows(state.tamad_pool().as_ref()).await;
    match rows.row(backend_name) {
        Some(r) if !r.endpoint.is_empty() => Ok(Some(r.endpoint.clone())),
        _ => Ok(None),
    }
}
