//! Host-side backend execution code (plan-191 Task 10).
//!
//! Moved from `tama_core::installations`: the actual download/build of
//! backend binaries (`installer/`), docker container execution
//! (`docker/`), and the Kokoro-FastAPI TTS install (`kokoro/`).
//! ADR-0010: these run on the tamad host, never on the proxy. Shared types
//! (`InstallationType`, `InstallOptions`, `DockerConfig`, ...) and the DB
//! manager stay in `tama_core::installations`.

pub mod docker;
pub mod installer;
pub mod kokoro;
