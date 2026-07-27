//! Pull queue service and event bus for managing pull lifecycle.
//!
//! Provides a `PullQueueService` that wraps the database query functions
//! and emits `PullEvent`s via a broadcast channel for each state transition.

mod events;
mod recovery;
mod service;
#[cfg(test)]
mod tests;

pub use events::PullEvent;
pub(crate) use recovery::queue_processor_loop;
pub use service::PullQueueService;
