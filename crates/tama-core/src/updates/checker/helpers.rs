use crate::models::update::FileStatus;

/// Determine the update status and availability based on file comparison results.
/// Returns (update_available, status, error_message).
pub fn determine_update_status(
    file_statuses: &[FileStatus],
) -> (bool, &'static str, Option<&'static str>) {
    let has_unknown = file_statuses
        .iter()
        .any(|s| matches!(s, FileStatus::Unknown));
    let has_changes = file_statuses.iter().any(|s| {
        matches!(
            s,
            FileStatus::Changed { .. } | FileStatus::NewRemote | FileStatus::RemovedFromRemote
        )
    });

    if has_unknown {
        (
            false,
            "verification_failed",
            Some("No stored hashes — run `model update --refresh`"),
        )
    } else if has_changes {
        (true, "update_available", None)
    } else {
        (false, "up_to_date", None)
    }
}

/// Check if enough time has passed since the last check based on interval.
pub fn should_check_since(
    oldest_check_timestamp: Option<i64>,
    interval_secs: i64,
    now: i64,
) -> bool {
    match oldest_check_timestamp {
        Some(ts) => now - ts >= interval_secs,
        None => true,
    }
}
