//! Shared helpers for proxy handlers.

use crate::proxy::ProxyState;

/// Get the backend URL for a backend from the models map.
///
/// Returns `Ok(Some(url))` if the backend is loaded and has a URL,
/// `Ok(None)` if the backend exists but has no URL (starting state)
/// or is not yet in the map.
pub(crate) async fn get_backend_url(
    state: &ProxyState,
    backend_name: &str,
) -> anyhow::Result<Option<String>> {
    let models = state.models.read().await;
    Ok(models
        .get(backend_name)
        .and_then(|ms| ms.backend_url())
        .map(|u| u.to_string()))
}
