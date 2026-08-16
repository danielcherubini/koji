use crate::updates::checker::UpdateChecker;

#[tokio::test]
async fn test_new_checker() {
    let checker = UpdateChecker::new();
    // Should just work
    drop(checker);
}

// `get_results` reads the update_checks table from Postgres (plan-190 Task 4) —
// its test lives in `crates/tama-core/tests/update_check_queries.rs` on the
// testcontainer harness.

// `should_check` reads the interval from the Postgres-backed global config
// (plan-190 Task 3) — its test lives in
// `crates/tama-core/tests/config_postgres.rs` on the testcontainer harness.
