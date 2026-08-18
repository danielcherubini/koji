//! The `tama-mock` crate: a mock LLM backend HTTP server used by tests of
//! the tamad crate (plan-191 Task 10).
//!
//! The lib target exists so other crates can dev-depend on this crate and
//! obtain the `CARGO_BIN_EXE_tama-mock` env for spawning the binary; all
//! runtime behavior lives in the `tama-mock` binary (`src/main.rs`).
