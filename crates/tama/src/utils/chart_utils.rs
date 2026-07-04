//! Shared utilities for chart components (sparkline, bar chart, etc.).

/// Format a relative time string from a Unix ms timestamp.
///
/// Returns a human-readable string like "2m 15s ago" or "45s ago"
/// based on the difference between `ts_unix_ms` and the current
/// browser time. Returns an empty string if the timestamp is 0.
pub fn format_relative_time(ts_unix_ms: i64) -> String {
    if ts_unix_ms == 0 {
        return String::new();
    }
    let now_ms = js_sys::Date::now() as i64;
    let diff_ms = now_ms - ts_unix_ms;
    if diff_ms < 0 {
        return String::new();
    }
    let secs = diff_ms / 1_000;
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3_600 {
        let mins = secs / 60;
        let remain_secs = secs % 60;
        if remain_secs == 0 {
            format!("{}m ago", mins)
        } else {
            format!("{}m {}s ago", mins, remain_secs)
        }
    } else {
        let hours = secs / 3_600;
        format!("{}h ago", hours)
    }
}

/// Format a duration given in seconds into a short label like "-3m" or "-1h".
pub fn format_duration_label(secs: i64) -> String {
    if secs < 60 {
        format!("-{}s", secs)
    } else if secs < 3_600 {
        format!("-{}m", secs / 60)
    } else {
        format!("-{}h", secs / 3_600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_label_seconds() {
        assert_eq!(format_duration_label(30), "-30s");
        assert_eq!(format_duration_label(59), "-59s");
    }

    #[test]
    fn test_format_duration_label_minutes() {
        assert_eq!(format_duration_label(60), "-1m");
        assert_eq!(format_duration_label(120), "-2m");
        assert_eq!(format_duration_label(3540), "-59m");
    }

    #[test]
    fn test_format_duration_label_hours() {
        assert_eq!(format_duration_label(3600), "-1h");
        assert_eq!(format_duration_label(7200), "-2h");
        assert_eq!(format_duration_label(86400), "-24h");
    }

    #[test]
    fn test_format_duration_label_zero() {
        assert_eq!(format_duration_label(0), "-0s");
    }

    #[test]
    fn test_format_duration_label_negative() {
        // Negative values should still produce output (though unusual)
        let result = format_duration_label(-1);
        assert!(result.contains("-"));
    }

    #[test]
    fn test_format_relative_time_zero_returns_empty() {
        assert_eq!(format_relative_time(0), "");
    }
}
