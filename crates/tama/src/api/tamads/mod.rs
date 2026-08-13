//! REST API endpoints for managing tamad connections.

pub mod list;
pub mod manage;
pub mod register;

// Re-export public items for use in router.rs
pub use list::{get_tamad, list_tamads};
pub use manage::{delete_tamad, trigger_health_check, update_tamad};
pub use register::{create_tamad, CreateTamadRequest};
