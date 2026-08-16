use crate::db::queries::upsert_update_check;
use crate::updates::checker::UpdateChecker;
use tempfile::tempdir;

#[tokio::test]
async fn test_new_checker() {
    let checker = UpdateChecker::new();
    // Should just work
    drop(checker);
}

#[tokio::test]
async fn test_get_results() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path().to_path_buf();

    let open = crate::db::open(&config_dir).unwrap();
    upsert_update_check(
        &open.conn,
        crate::db::queries::UpdateCheckParams {
            item_type: "backend",
            item_id: "test-backend",
            current_version: Some("v1"),
            latest_version: Some("v2"),
            update_available: true,
            status: "update_available",
            error_message: None,
            details_json: None,
            checked_at: 123456789,
        },
    )
    .unwrap();

    let checker = UpdateChecker::new();
    let results = checker.get_results(&config_dir).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].item_type, "backend");
    assert_eq!(results[0].item_id, "test-backend");
    assert_eq!(results[0].current_version.as_deref(), Some("v1"));
    assert_eq!(results[0].latest_version.as_deref(), Some("v2"));
    assert!(results[0].update_available);
}

// `should_check` reads the interval from the Postgres-backed global config
// (plan-190 Task 3) — its test lives in
// `crates/tama-core/tests/config_postgres.rs` on the testcontainer harness.
