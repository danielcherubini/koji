pub mod chat;
pub mod forward;
pub mod metrics;
pub mod models;
pub mod status;
pub mod tts;

// Re-exports for backward compatibility (flat imports via handlers::)
#[allow(unused_imports)]
pub use chat::{handle_chat_completions, handle_stream_chat_completions};
#[allow(unused_imports)]
pub use forward::{handle_fallback, handle_forward_get, handle_forward_post};
#[allow(unused_imports)]
pub use metrics::{
    format_backend_metrics, format_system_metrics, format_tama_metrics, inject_server_label,
};
#[allow(unused_imports)]
pub use models::{handle_get_model, handle_list_models};
#[allow(unused_imports)]
pub use status::{handle_health, handle_metrics, handle_reload_configs, handle_status};

use crate::proxy::ProxyState;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};

pub fn json_error_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": {
                "message": "Bad Request",
                "type": "BadRequestError"
            }
        })),
    )
        .into_response()
}

/// Update the last_used_model in DB. Best-effort — never fails the request.
/// Throttled: only writes if the server_name differs from what's stored.
async fn update_last_used_best_effort(state: &ProxyState, server_name: &str, model_name: &str) {
    let Some(mgr) = state.model_mgr() else {
        return;
    };
    let current = mgr.get_last_used().ok().flatten();
    if current.as_ref().map(|r| r.server_name.as_str()) == Some(server_name) {
        return; // Same model, no write needed
    }
    let _ = mgr.set_last_used(server_name, model_name);
}

#[cfg(test)]
mod tests;
