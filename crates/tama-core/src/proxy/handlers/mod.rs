pub mod chat;
pub mod compaction;
pub mod forward;
pub mod metrics;
pub mod models;
pub mod status;
pub mod tts;

pub(crate) mod helpers;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};

pub fn json_error_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "error": {
                "message": "Request body too large",
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
