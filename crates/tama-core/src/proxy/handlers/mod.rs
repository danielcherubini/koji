pub mod chat;
pub mod compaction;
pub mod forward;
pub mod metrics;
pub mod models;
pub mod status;
pub mod tts;

// Re-exports for backward compatibility (flat imports via handlers::)
#[allow(unused_imports)]
pub use chat::{handle_chat_completions, handle_stream_chat_completions};
#[allow(unused_imports)]
pub use forward::{forward_to_backend, handle_fallback, handle_forward_get, handle_forward_post};
#[allow(unused_imports)]
pub use metrics::{
    format_backend_metrics, format_system_metrics, format_tama_metrics, inject_server_label,
};
#[allow(unused_imports)]
pub use models::{handle_get_model, handle_list_models};
#[allow(unused_imports)]
pub use status::{handle_health, handle_metrics, handle_reload_configs, handle_status};

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

#[cfg(test)]
mod alias_tests;
#[cfg(test)]
mod forward_tests;
#[cfg(test)]
mod get_model_tests;
#[cfg(test)]
mod list_models_tests;
#[cfg(test)]
pub(crate) mod tests;
