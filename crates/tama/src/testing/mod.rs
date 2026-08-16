//! Test-only support modules for the `tama` crate (compiled under `#[cfg(test)]`).

#[cfg(feature = "ssr")]
pub mod postgres;
