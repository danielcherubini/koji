//! In-crate (in-file `#[cfg(test)]`) Postgres test harness — thin shim over
//! the `tama-test-support` workspace crate (which owns the shared-container
//! logic, schema guards, and cleanup).
//!
//! Only use it for in-file tests that genuinely need the database (e.g. the
//! repo-pull completion lifecycle); prefer the integration harness for new
//! tests.

#![allow(dead_code)]

use tama_test_support::Harness;

/// Harness for in-src tests: `tama_infile_` schema prefix and the squashed
/// tama-core migrations.
static HARNESS: Harness = Harness::new("tama_infile_", &crate::db::postgres::MIGRATIONS);

pub use tama_test_support::SchemaGuard;

/// Create an isolated test schema, scoped pool, and run the migrations.
pub async fn with_schema() -> SchemaGuard {
    HARNESS.with_schema().await
}
