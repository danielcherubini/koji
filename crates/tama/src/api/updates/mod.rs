//! Update check and apply handlers for backends and models.
//!
//! Exposes `get_updates`, `trigger_check`, `check_item_for_update`,
//! `apply_backend_update`, and `apply_model_update` via Axum route handlers,
//! plus an SSE endpoint for live update-check progress events.

mod apply;
mod check;
mod events;

#[cfg(test)]
mod tests;

pub use apply::{
    apply_backend_update, apply_model_update, ModelUpdateRequest, ModelUpdateResponse,
};
pub use check::{
    check_item_for_update, get_updates, trigger_check, CheckResponse, CheckSingleQuery,
    QuantDetailJson, UpdateCheckDto, UpdatesListResponse,
};
pub use events::update_events_sse;
