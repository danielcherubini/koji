use axum::{
    extract::{Extension, State},
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
};
use futures_util::Stream;
use std::sync::Arc;

use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// GET /tama/v1/updates/events — SSE stream of update check lifecycle events.
pub async fn update_events_sse(
    Extension(web_state): Extension<WebState>,
    State(_state): State<Arc<ProxyState>>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, axum::http::StatusCode> {
    let checker = web_state.update_checker.clone();
    let tx = checker
        .update_events_tx
        .as_ref()
        .ok_or(axum::http::StatusCode::SERVICE_UNAVAILABLE)?;
    let rx = tx.subscribe();
    let event_stream =
        crate::api::sse::broadcast_to_sse(rx, tama_core::updates::UpdateEvent::to_sse_event);
    Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
}
