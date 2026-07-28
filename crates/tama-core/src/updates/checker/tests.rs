use super::*;
use crate::models::update::FileStatus;
use crate::sse::ToSseEvent;

// ── determine_update_status tests ─────────────────────────────────────

#[test]
fn test_determine_update_status_no_files() {
    let statuses: Vec<FileStatus> = vec![];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "up_to_date");
}

/// Test with a single Unchanged file — should be up_to_date.
#[test]
fn test_determine_update_status_single_unchanged() {
    let statuses = vec![FileStatus::Unchanged];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "up_to_date");
}

/// Test with a mix of Changed and RemovedFromRemote — both count as changes.
#[test]
fn test_determine_update_status_mixed_changes() {
    let statuses = vec![
        FileStatus::Changed {
            old_oid: "a".to_string(),
            new_oid: "b".to_string(),
        },
        FileStatus::RemovedFromRemote,
    ];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(available);
    assert_eq!(status, "update_available");
}

/// Test with Unknown mixed with Unchanged — Unknown takes priority.
#[test]
fn test_determine_update_status_unknown_with_unchanged() {
    let statuses = vec![FileStatus::Unchanged, FileStatus::Unknown];
    let (available, status, error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "verification_failed");
    assert!(error.is_some());
}

/// Test with only RemovedFromRemote — counts as change.
#[test]
fn test_determine_update_status_only_removed() {
    let statuses = vec![FileStatus::RemovedFromRemote, FileStatus::RemovedFromRemote];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(available);
    assert_eq!(status, "update_available");
}

/// Test with only NewRemote — counts as change.
#[test]
fn test_determine_update_status_only_new_remote() {
    let statuses = vec![FileStatus::NewRemote];
    let (available, status, _error) = determine_update_status(&statuses);
    assert!(available);
    assert_eq!(status, "update_available");
}

/// Test with Unknown overriding both Changed and NewRemote.
#[test]
fn test_determine_update_status_unknown_overrides_all() {
    let statuses = vec![
        FileStatus::Changed {
            old_oid: "a".to_string(),
            new_oid: "b".to_string(),
        },
        FileStatus::NewRemote,
        FileStatus::RemovedFromRemote,
        FileStatus::Unknown,
    ];
    let (available, status, error) = determine_update_status(&statuses);
    assert!(!available);
    assert_eq!(status, "verification_failed");
    assert!(error.is_some());
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

/// Test that a cache miss returns None for an unknown repo_id.
#[tokio::test]
async fn test_gguf_listing_cache_miss() {
    let cache = GgufListingCache::new();
    let result = cache.get("nonexistent/repo", None).await;
    assert!(result.is_none(), "Cache miss should return None");
}

/// Test that a cache hit returns the stored entry.
#[tokio::test]
async fn test_gguf_listing_cache_hit() {
    let cache = GgufListingCache::new();
    let now = chrono::Utc::now().timestamp();
    let files = vec![crate::models::pull::RemoteGguf {
        filename: "model.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
    }];
    cache
        .insert(
            "test/repo".to_string(),
            "abc123".to_string(),
            files.clone(),
            Some(now),
        )
        .await;
    let result = cache.get("test/repo", None).await;
    assert!(result.is_some(), "Cache hit should return Some");
    let (sha, retrieved_files) = result.unwrap();
    assert_eq!(sha, "abc123");
    assert_eq!(retrieved_files, files);
}

/// Test that a cache entry is returned when accessed with the same timestamp
/// (simulates a cache hit within TTL using the same time).
#[tokio::test]
async fn test_gguf_listing_cache_hit_with_time() {
    let cache = GgufListingCache::new();
    let now = 1000i64;
    let files = vec![crate::models::pull::RemoteGguf {
        filename: "model.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
    }];
    cache
        .insert(
            "test/repo".to_string(),
            "abc123".to_string(),
            files.clone(),
            Some(now),
        )
        .await;
    // Access with the same timestamp — should still be within TTL
    let result = cache.get("test/repo", Some(now)).await;
    assert!(
        result.is_some(),
        "Cache hit with same timestamp should return Some"
    );
    let (sha, retrieved_files) = result.unwrap();
    assert_eq!(sha, "abc123");
    assert_eq!(retrieved_files, files);
}

/// Test that a cache entry expires after TTL_SECS (300 seconds).
#[tokio::test]
async fn test_gguf_listing_cache_expiry() {
    let cache = GgufListingCache::new();
    let now = 1000i64;
    let files = vec![crate::models::pull::RemoteGguf {
        filename: "model.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
    }];
    cache
        .insert(
            "test/repo".to_string(),
            "abc123".to_string(),
            files,
            Some(now),
        )
        .await;
    // Access with a timestamp 301 seconds later — should be expired
    let result = cache.get("test/repo", Some(now + 301)).await;
    assert!(
        result.is_none(),
        "Cache entry should be expired after TTL_SECS (300s)"
    );
}

/// Test that a cache entry is still valid just before TTL expiry.
#[tokio::test]
async fn test_gguf_listing_cache_valid_at_boundary() {
    let cache = GgufListingCache::new();
    let now = 1000i64;
    let files = vec![crate::models::pull::RemoteGguf {
        filename: "model.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
    }];
    cache
        .insert(
            "test/repo".to_string(),
            "abc123".to_string(),
            files,
            Some(now),
        )
        .await;
    // Access at exactly TTL_SECS — should still be valid (< 300)
    // Actually, the check is `now - epoch < TTL_SECS`, so at exactly 300 it's NOT valid
    let result = cache.get("test/repo", Some(now + 299)).await;
    assert!(
        result.is_some(),
        "Cache entry should be valid at TTL_SECS - 1"
    );
}

/// Test that a cache entry is expired at exactly TTL_SECS boundary.
#[tokio::test]
async fn test_gguf_listing_cache_expired_at_boundary() {
    let cache = GgufListingCache::new();
    let now = 1000i64;
    let files = vec![crate::models::pull::RemoteGguf {
        filename: "model.gguf".to_string(),
        quant: Some("Q4_K_M".to_string()),
    }];
    cache
        .insert(
            "test/repo".to_string(),
            "abc123".to_string(),
            files,
            Some(now),
        )
        .await;
    // Access at exactly TTL_SECS — check is `now - epoch < TTL_SECS`, so 300 < 300 is false
    let result = cache.get("test/repo", Some(now + 300)).await;
    assert!(
        result.is_none(),
        "Cache entry should be expired at exactly TTL_SECS (300)"
    );
}

#[test]
#[cfg(feature = "web-ui")]
fn test_update_event_tagged_serialization_all_variants() {
    let cases: Vec<(UpdateEvent, &str)> = vec![
        (
            UpdateEvent::CheckStarted {
                item_type: "t".into(),
                item_id: "i".into(),
                variant: None,
            },
            "CheckStarted",
        ),
        (
            UpdateEvent::CheckCompleted {
                item_type: "t".into(),
                item_id: "i".into(),
                variant: None,
                dto: serde_json::json!({ "x": 1 }),
            },
            "CheckCompleted",
        ),
        (
            UpdateEvent::CheckError {
                item_type: "t".into(),
                item_id: "i".into(),
                variant: None,
                error: "e".into(),
            },
            "CheckError",
        ),
        (
            UpdateEvent::CheckSkipped {
                item_type: "t".into(),
                reason: "r".into(),
            },
            "CheckSkipped",
        ),
    ];
    for (event, expected_name) in cases {
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["event"], expected_name);
        assert!(event.to_sse_event().is_ok());
    }
}
