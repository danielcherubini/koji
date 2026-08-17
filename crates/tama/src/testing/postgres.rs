//! In-crate (in-file `#[cfg(test)]`) Postgres test harness for the `tama`
//! crate — thin shim over the `tama-test-support` workspace crate (which
//! owns the shared-container logic, schema guards, and cleanup).
//!
//! Mirrors `tama_core::testing::postgres` for in-src unit tests that need
//! the model domain in Postgres (plan-190 Task 5). Integration tests under
//! `tests/` should keep using `tests/common/mod.rs`.

#![allow(dead_code)]

use tama_test_support::Harness;

/// Harness for in-src tests: `tama_web_` schema prefix and the squashed
/// tama-core migrations.
static HARNESS: Harness = Harness::new("tama_web_", &tama_core::db::postgres::MIGRATIONS);

pub use tama_test_support::SchemaGuard;

/// Create an isolated test schema, scoped pool, and run the migrations.
pub async fn with_schema() -> SchemaGuard {
    HARNESS.with_schema().await
}
