//! Shared Postgres integration test harness — thin shim over the
//! `tama-test-support` workspace crate (which owns the shared-container
//! logic, schema guards, and cleanup).

#![allow(dead_code, unused_imports)]

use tama_test_support::Harness;

/// Harness for this crate's integration tests: `tama_test_` schema prefix
/// and the squashed tama-core migrations.
static HARNESS: Harness = Harness::new("tama_test_", &tama_core::db::postgres::MIGRATIONS);

pub use tama_test_support::{container_host_port, test_pool, SchemaGuard};

/// Create an isolated test schema, scoped pool, and run the migrations.
pub async fn with_schema() -> SchemaGuard {
    HARNESS.with_schema().await
}
