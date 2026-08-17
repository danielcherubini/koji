//! Postgres ports of the `update_check_queries` tests (plan-190, Task 4 —
//! update check results move to Postgres).
//!
//! These mirror the former in-file SQLite tests 1:1 against the async
//! `&PgPool` API on an isolated migrated schema. The LIKE/ESCAPE semantics
//! are identical in Postgres (`ESCAPE '\'`), so the escape tests carry over
//! unchanged.

mod common;

use common::with_schema;
use tama_core::db::queries::{
    delete_update_check, delete_update_checks_by_pattern, delete_update_checks_for_backend,
    get_all_update_checks, get_oldest_check_time, get_update_check, upsert_update_check,
    UpdateCheckParams,
};

/// Helper to build upsert params for the common test rows.
fn params<'a>(item_type: &'a str, item_id: &'a str, checked_at: i64) -> UpdateCheckParams<'a> {
    UpdateCheckParams {
        item_type,
        item_id,
        current_version: None,
        latest_version: None,
        update_available: false,
        status: "unknown",
        error_message: None,
        details_json: None,
        checked_at,
    }
}

#[tokio::test]
async fn test_upsert_and_get_update_check() {
    let guard = with_schema().await;
    let item_type = "backend";
    let item_id = "llama-cpp";
    let now = 1713168000; // 2024-04-15

    // Insert
    upsert_update_check(
        &guard.pool,
        UpdateCheckParams {
            item_type,
            item_id,
            current_version: Some("v1.0.0"),
            latest_version: Some("v1.1.0"),
            update_available: true,
            status: "update_available",
            error_message: None,
            details_json: None,
            checked_at: now,
        },
    )
    .await
    .unwrap();

    let record = get_update_check(&guard.pool, item_type, item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.item_type, item_type);
    assert_eq!(record.item_id, item_id);
    assert_eq!(record.current_version.unwrap(), "v1.0.0");
    assert_eq!(record.latest_version.unwrap(), "v1.1.0");
    assert!(record.update_available);
    assert_eq!(record.status, "update_available");
    assert_eq!(record.checked_at, now);

    // Upsert (Update)
    upsert_update_check(
        &guard.pool,
        UpdateCheckParams {
            item_type,
            item_id,
            current_version: Some("v1.1.0"),
            latest_version: Some("v1.1.0"),
            update_available: false,
            status: "up_to_date",
            error_message: None,
            details_json: None,
            checked_at: now + 100,
        },
    )
    .await
    .unwrap();

    let updated = get_update_check(&guard.pool, item_type, item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.current_version.unwrap(), "v1.1.0");
    assert!(!updated.update_available);
    assert_eq!(updated.status, "up_to_date");
    assert_eq!(updated.checked_at, now + 100);

    guard.finish().await;
}

#[tokio::test]
async fn test_get_all_update_checks() {
    let guard = with_schema().await;
    let now = 1713168000;

    upsert_update_check(&guard.pool, params("backend", "b1", now))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("model", "m1", now))
        .await
        .unwrap();

    let all = get_all_update_checks(&guard.pool).await.unwrap();
    assert_eq!(all.len(), 2);

    guard.finish().await;
}

#[tokio::test]
async fn test_delete_update_check() {
    let guard = with_schema().await;
    let item_type = "backend";
    let item_id = "b1";

    upsert_update_check(&guard.pool, params(item_type, item_id, 12345))
        .await
        .unwrap();

    delete_update_check(&guard.pool, item_type, item_id)
        .await
        .unwrap();
    let record = get_update_check(&guard.pool, item_type, item_id)
        .await
        .unwrap();
    assert!(record.is_none());

    guard.finish().await;
}

#[tokio::test]
async fn test_get_oldest_check_time() {
    let guard = with_schema().await;

    assert_eq!(get_oldest_check_time(&guard.pool).await.unwrap(), None);

    upsert_update_check(&guard.pool, params("backend", "b1", 2000))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("backend", "b2", 1000))
        .await
        .unwrap();

    assert_eq!(
        get_oldest_check_time(&guard.pool).await.unwrap(),
        Some(1000)
    );

    guard.finish().await;
}

#[tokio::test]
async fn test_delete_update_checks_by_pattern() {
    let guard = with_schema().await;

    // Insert records for multiple backends with variant-style item_ids
    upsert_update_check(&guard.pool, params("backend", "llama_cpp:cpu", 1000))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("backend", "llama_cpp:cuda", 1001))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("backend", "ik_llama:cpu", 1002))
        .await
        .unwrap();

    // Delete all llama_cpp variants using LIKE pattern
    let pattern = "llama_cpp:%";
    delete_update_checks_by_pattern(&guard.pool, "backend", pattern)
        .await
        .unwrap();

    // Verify llama_cpp records are gone
    assert!(get_update_check(&guard.pool, "backend", "llama_cpp:cpu")
        .await
        .unwrap()
        .is_none());
    assert!(get_update_check(&guard.pool, "backend", "llama_cpp:cuda")
        .await
        .unwrap()
        .is_none());

    // Verify ik_llama record is unaffected
    assert!(get_update_check(&guard.pool, "backend", "ik_llama:cpu")
        .await
        .unwrap()
        .is_some());

    // Edge case: pattern that matches nothing should not error
    delete_update_checks_by_pattern(&guard.pool, "backend", "nonexistent:%")
        .await
        .unwrap();

    guard.finish().await;
}

#[tokio::test]
async fn test_delete_update_checks_by_pattern_escapes_underscore() {
    let guard = with_schema().await;

    // Insert a record with underscore in the name
    upsert_update_check(&guard.pool, params("backend", "my_backend:cpu", 1000))
        .await
        .unwrap();

    // Insert a similar record that should NOT match
    upsert_update_check(&guard.pool, params("backend", "myXbackend:cpu", 1001))
        .await
        .unwrap();

    // Escape the underscore so it matches literally, not as wildcard
    let escaped_name = "my_backend"
        .replace('\\', "\\\\")
        .replace('_', "\\_")
        .replace('%', "\\%");
    let pattern = format!("{}:%", escaped_name);
    delete_update_checks_by_pattern(&guard.pool, "backend", &pattern)
        .await
        .unwrap();

    // my_backend:cpu should be deleted
    assert!(get_update_check(&guard.pool, "backend", "my_backend:cpu")
        .await
        .unwrap()
        .is_none());

    // myXbackend:cpu should NOT be deleted (underscore was escaped)
    assert!(get_update_check(&guard.pool, "backend", "myXbackend:cpu")
        .await
        .unwrap()
        .is_some());

    guard.finish().await;
}

/// `delete_update_checks_for_backend` removes all variant rows (LIKE `name:%`)
/// and the legacy bare-name row, while leaving other backends untouched.
#[tokio::test]
async fn test_delete_update_checks_for_backend() {
    let guard = with_schema().await;

    // Insert four records: two variants + one legacy + one unrelated
    upsert_update_check(&guard.pool, params("backend", "llama_cpp:cpu", 1000))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("backend", "llama_cpp:vulkan", 1001))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("backend", "llama_cpp", 1002))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("backend", "other:cpu", 1003))
        .await
        .unwrap();

    // Act: delete all update checks for "llama_cpp"
    delete_update_checks_for_backend(&guard.pool, "llama_cpp")
        .await
        .unwrap();

    // Assert: llama_cpp variants and legacy row are gone
    assert!(get_update_check(&guard.pool, "backend", "llama_cpp:cpu")
        .await
        .unwrap()
        .is_none());
    assert!(get_update_check(&guard.pool, "backend", "llama_cpp:vulkan")
        .await
        .unwrap()
        .is_none());
    assert!(get_update_check(&guard.pool, "backend", "llama_cpp")
        .await
        .unwrap()
        .is_none());

    // Assert: other backend is untouched
    assert!(get_update_check(&guard.pool, "backend", "other:cpu")
        .await
        .unwrap()
        .is_some());

    guard.finish().await;
}

/// `delete_update_checks_for_backend` correctly escapes SQL LIKE metacharacters.
#[tokio::test]
async fn test_delete_update_checks_for_backend_escapes() {
    let guard = with_schema().await;

    // Insert records with underscore in name — one should match, one should not
    upsert_update_check(&guard.pool, params("backend", "my_backend:cpu", 1000))
        .await
        .unwrap();
    upsert_update_check(&guard.pool, params("backend", "myXbackend:cpu", 1001))
        .await
        .unwrap();

    // Act: delete for "my_backend" — the underscore should be escaped
    delete_update_checks_for_backend(&guard.pool, "my_backend")
        .await
        .unwrap();

    // Assert: my_backend:cpu is gone, myXbackend:cpu survives
    assert!(get_update_check(&guard.pool, "backend", "my_backend:cpu")
        .await
        .unwrap()
        .is_none());
    assert!(get_update_check(&guard.pool, "backend", "myXbackend:cpu")
        .await
        .unwrap()
        .is_some());

    guard.finish().await;
}

/// `UpdateChecker::get_results` reads cached check results from Postgres.
#[tokio::test]
async fn test_checker_get_results() {
    let guard = with_schema().await;

    upsert_update_check(
        &guard.pool,
        UpdateCheckParams {
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
    .await
    .unwrap();

    let checker = tama_core::updates::checker::UpdateChecker::new();
    let results = checker.get_results(&guard.pool).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].item_type, "backend");
    assert_eq!(results[0].item_id, "test-backend");
    assert_eq!(results[0].current_version.as_deref(), Some("v1"));
    assert_eq!(results[0].latest_version.as_deref(), Some("v2"));
    assert!(results[0].update_available);

    guard.finish().await;
}
