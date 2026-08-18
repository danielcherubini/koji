//! Prebuilt-release URL construction (the shared half of the installer).
//!
//! The download/extract/source-build execution moved to the tamad crate
//! (`host_installs::installer`) in plan-191 Task 10 (ADR-0010). These URL
//! helpers stay: pure string/network-metadata logic used by the update
//! checker and by the tamad's installer.

pub mod urls;
