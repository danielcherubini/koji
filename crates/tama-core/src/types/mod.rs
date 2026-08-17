//! Pure, dependency-free types shared between the server and the WASM frontend.
//!
//! Everything in this module must compile on `wasm32-unknown-unknown` with only
//! `serde` and `std` — no tokio, sqlx, axum, reqwest, sysinfo, or tracing.
//! The `tama` crate includes these exact files via `#[path]` for csr builds
//! (see `crates/tama/src/core_shared.rs`), so adding a non-wasm dependency here
//! breaks the frontend build. Keep it pure.

pub mod enums;
pub mod gpu;
pub mod quant;
