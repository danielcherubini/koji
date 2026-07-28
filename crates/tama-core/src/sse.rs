//! Shared SSE event serialization helpers.

use axum::response::sse::Event;

/// Trait for domain events that can be serialized into an SSE event.
/// The default implementation serializes to a `serde_json::Value`,
/// extracts the `"event"` tag, and builds an `Event` with the
/// variant name as the SSE event type and the JSON as the data.
pub trait ToSseEvent: serde::Serialize {
    fn to_sse_event(&self) -> Result<Event, serde_json::Error> {
        let value = serde_json::to_value(self)?;
        let name = value
            .get("event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let json_str = serde_json::to_string(&value)?;
        Ok(Event::default().event(name).data(json_str))
    }
}
