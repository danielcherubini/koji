//! Tests for the pull HTTP surface and download/verify orchestration.
//!
//! NOTE: tests that set HF_ENDPOINT rely on nextest's process-per-test
//! isolation (hf_api caches its endpoint in a process-wide OnceCell).
//! Run with `cargo nextest run`, never plain `cargo test`.

mod helpers;
mod jobs_stream;
mod orchestration;
mod validation;
