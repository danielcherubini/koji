//! REST API endpoints for managing providers.

pub mod list;
pub mod manage;
pub mod register;

// Re-export public items for use in router.rs
pub use list::{get_provider, list_providers};
pub use manage::{delete_provider, update_provider};
pub use register::{create_provider, CreateProviderRequest};
