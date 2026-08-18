//! Docker host utilities (plan-191 Task 10).
//!
//! What remains after Task 10's dead-code sweep: `docker_available`
//! (daemon probe) and `startup_reconcile` (reap `tama.managed=true`
//! containers left on this host by a crashed daemon — run at tamad
//! startup, replacing the old proxy-side call). The docker backend *engine*
//! itself is not host-installable (`installs.rs` rejects it) and no load
//! path ever spawned a docker container (`is_docker` was always false), so
//! the spawn/stop/inspect runner and image pull code did not move — only
//! the shared config types (`DockerConfig`, `DockerVolume`, in
//! `tama_core::installations`).

pub mod image;
pub mod reconcile;
pub mod runner;

// Re-export the live surface for the daemon startup path.
pub use reconcile::startup_reconcile;
