use super::*;
use crate::models::update::FileStatus;

// ── determine_update_status tests ─────────────────────────────────────

#[test]
fn test_determine_update_status_no_files() {
    let statuses: Vec<FileStatus> = vec![];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "up_to_date");
}

#[test]
fn test_determine_update_status_all_unchanged() {
    let statuses = vec![FileStatus::Unchanged, FileStatus::Unchanged];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "up_to_date");
}

#[test]
fn test_determine_update_status_has_changes() {
    let statuses = vec![
        FileStatus::Unchanged,
        FileStatus::Changed {
            old_oid: "abc".to_string(),
            new_oid: "def".to_string(),
        },
    ];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(available);
    assert_eq!(status, "update_available");
}

#[test]
fn test_determine_update_status_new_remote() {
    let statuses = vec![FileStatus::Unchanged, FileStatus::NewRemote];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(available);
    assert_eq!(status, "update_available");
}

#[test]
fn test_determine_update_status_unknown_hashes() {
    let statuses = vec![FileStatus::Unchanged, FileStatus::Unknown];
    let (available, status, error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "verification_failed");
    assert!(error.is_some());
    assert!(error.unwrap().contains("No stored hashes"));
}

#[test]
fn test_determine_update_status_unknown_overrides_changes() {
    // Unknown should take priority over changes
    let statuses = vec![
        FileStatus::Changed {
            old_oid: "a".to_string(),
            new_oid: "b".to_string(),
        },
        FileStatus::Unknown,
    ];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "verification_failed");
}

#[test]
fn test_determine_update_status_only_unknown() {
    let statuses = vec![FileStatus::Unknown];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "verification_failed");
}

#[test]
fn test_determine_update_status_removed_from_remote() {
    // RemovedFromRemote counts as a change
    let statuses = vec![FileStatus::RemovedFromRemote];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(available);
    assert_eq!(status, "update_available");
}

// ── should_check_since tests ──────────────────────────────────────────

#[test]
fn test_should_check_since_no_prior_check() {
    // No prior check → should always check
    assert!(should_check_since(None, 3600, 1000));
    assert!(should_check_since(None, 86400, 500));
}

#[test]
fn test_should_check_since_interval_elapsed() {
    // Last check was 2 hours ago, interval is 1 hour → should check
    assert!(should_check_since(Some(0), 3600, 7200));
}

#[test]
fn test_should_check_since_interval_not_elapsed() {
    // Last check was 30 minutes ago, interval is 1 hour → should not check
    assert!(!should_check_since(Some(0), 3600, 1800));
}

#[test]
fn test_should_check_since_exact_boundary() {
    // Exactly at the boundary → should check (>=)
    assert!(should_check_since(Some(0), 3600, 3600));
}

#[test]
fn test_should_check_since_one_second_over() {
    // One second over the interval → should check
    assert!(should_check_since(Some(0), 3600, 3601));
}

#[test]
fn test_should_check_since_large_interval() {
    // 24-hour interval, checked 23h ago → should not check
    assert!(!should_check_since(Some(0), 86400, 82800));
    // 24-hour interval, checked 25h ago → should check
    assert!(should_check_since(Some(0), 86400, 90000));
}

#[test]
fn test_should_check_since_zero_interval() {
    // Zero interval means always check (even with prior check)
    assert!(should_check_since(Some(1000), 0, 1000));
    assert!(should_check_since(Some(1000), 0, 2000));
}

// ── UpdateChecker construction tests ──────────────────────────────────

#[test]
fn test_update_checker_new() {
    let checker = UpdateChecker::new();
    // Just verify it constructs without panicking
    let _ = checker.clone();
}

#[test]
fn test_update_checker_default() {
    let checker = UpdateChecker::default();
    let _ = checker.clone();
}

#[test]
fn test_update_checker_clone() {
    let checker1 = UpdateChecker::new();
    let checker2 = checker1.clone();
    // Both should be usable independently
    let _ = checker1;
    let _ = checker2;
}

// ── GgufListingCache tests ────────────────────────────────────────────

#[test]
fn test_gguf_listing_cache_new() {
    let cache = GgufListingCache::new();
    // Just verify it constructs without panicking
    let _ = cache;
}

#[test]
fn test_gguf_listing_cache_clone() {
    let cache1 = GgufListingCache::new();
    let cache2 = cache1.clone();
    // Both should be usable independently
    let _ = cache1;
    let _ = cache2;
}
