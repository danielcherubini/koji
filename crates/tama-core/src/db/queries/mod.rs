//! Typed query functions for the tama SQLite database.
//!
//! All functions take a `&Connection` — the caller owns the connection.
//! All functions are synchronous (no async).

mod active_model_queries;
mod alias_queries;
mod api_key_queries;
mod app_config_queries;
mod benchmark_queries;
mod installation_queries;
mod metrics_queries;
mod model_config_queries;
mod model_queries;
mod provider_queries;
mod pull_queue_queries;
mod tamad_queries;
mod tts_config_queries;
mod types;
mod update_check_queries;

pub use active_model_queries::*;
pub use alias_queries::*;
pub use api_key_queries::*;
pub use app_config_queries::*;
pub use benchmark_queries::*;
pub use installation_queries::*;
pub use metrics_queries::*;
pub use model_config_queries::*;
pub use model_queries::*;
pub use provider_queries::*;
pub use pull_queue_queries::*;
pub use tamad_queries::*;
pub use tts_config_queries::*;
pub use types::*;
pub use update_check_queries::*;

#[cfg(test)]
mod tests;
