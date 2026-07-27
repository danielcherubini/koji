use async_stream::stream;
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
use tama_core::updates::UpdateEvent;

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
    let mut rx = tx.subscribe();

    let event_stream = stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let sse_event = match &event {
                        UpdateEvent::CheckStarted { item_type, item_id, variant } => {
                            Event::default()
                                .event("CheckStarted")
                                .json_data(serde_json::json!({
                                    "item_type": item_type,
                                    "item_id": item_id,
                                    "variant": variant,
                                }))
                        }
                        UpdateEvent::CheckCompleted { item_type, item_id, variant, dto } => {
                            Event::default()
                                .event("CheckCompleted")
                                .json_data(serde_json::json!({
                                    "item_type": item_type,
                                    "item_id": item_id,
                                    "variant": variant,
                                    "dto": dto,
                                }))
                        }
                        UpdateEvent::CheckError { item_type, item_id, variant, error } => {
                            Event::default()
                                .event("CheckError")
                                .json_data(serde_json::json!({
                                    "item_type": item_type,
                                    "item_id": item_id,
                                    "variant": variant,
                                    "error": error,
                                }))
                        }
                        UpdateEvent::CheckSkipped { item_type, reason } => {
                            Event::default()
                                .event("CheckSkipped")
                                .json_data(serde_json::json!({
                                    "item_type": item_type,
                                    "reason": reason,
                                }))
                        }
                    };

                    match sse_event {
                        Ok(e) => yield Ok(e),
                        Err(e) => yield Err(axum::Error::new(e)),
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    yield Ok(Event::default()
                        .event("Lagged")
                        .json_data(serde_json::json!({ "lagged": n }))?);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
}
