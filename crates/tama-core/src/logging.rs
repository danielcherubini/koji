use anyhow::{Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Get the log file path for a profile.
pub fn log_path(logs_dir: &Path, profile: &str) -> PathBuf {
    logs_dir.join(format!("{}.log", profile))
}

/// Read the last N lines from a log file.
pub fn tail_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    use std::io::{BufRead, BufReader};

    if !path.exists() {
        return Ok(vec![]);
    }

    let file =
        File::open(path).with_context(|| format!("Failed to open log file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().collect::<Result<Vec<String>, _>>()?;

    if all_lines.len() <= n {
        Ok(all_lines)
    } else {
        Ok(all_lines[all_lines.len() - n..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;

    /// Append-test-file fixture (open_log is gone with the manual
    /// rotation; the legacy tail reads are what this module serves now).
    fn append_lines(logs_dir: &Path, profile: &str, lines: &[&str]) {
        std::fs::create_dir_all(logs_dir).unwrap();
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(logs_dir, profile))
            .unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
    }

    #[test]
    fn test_log_path() {
        let path = log_path(Path::new("/tmp/logs"), "default");
        assert_eq!(path, PathBuf::from("/tmp/logs/default.log"));
    }

    #[test]
    fn test_log_path_with_special_profile() {
        let path = log_path(Path::new("/tmp/logs"), "profile-with-dashes");
        assert_eq!(path, PathBuf::from("/tmp/logs/profile-with-dashes.log"));
    }

    #[test]
    fn test_open_and_tail() {
        let tmp = tempfile::tempdir().unwrap();
        append_lines(tmp.path(), "test", &["line 1", "line 2", "line 3"]);

        let lines = tail_lines(&log_path(tmp.path(), "test"), 2).unwrap();
        assert_eq!(lines, vec!["line 2", "line 3"]);
    }

    #[test]
    fn test_tail_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = tail_lines(&log_path(tmp.path(), "nonexistent"), 10).unwrap();
        assert!(lines.is_empty());
    }

    #[test]
    fn test_tail_more_lines_than_requested() {
        let tmp = tempfile::tempdir().unwrap();
        append_lines(
            tmp.path(),
            "test",
            &[
                "line 1", "line 2", "line 3", "line 4", "line 5", "line 6", "line 7", "line 8",
                "line 9", "line 10",
            ],
        );

        // Request only 3 lines from a 10-line file
        let lines = tail_lines(&log_path(tmp.path(), "test"), 3).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line 8");
        assert_eq!(lines[1], "line 9");
        assert_eq!(lines[2], "line 10");
    }

    #[test]
    fn test_tail_fewer_lines_than_requested() {
        let tmp = tempfile::tempdir().unwrap();
        append_lines(tmp.path(), "test", &["line 1", "line 2"]);

        // Request 10 lines from a 2-line file
        let lines = tail_lines(&log_path(tmp.path(), "test"), 10).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[1], "line 2");
    }

    #[test]
    fn test_tail_zero_lines() {
        let tmp = tempfile::tempdir().unwrap();
        append_lines(tmp.path(), "test", &["line 1"]);

        let lines = tail_lines(&log_path(tmp.path(), "test"), 0).unwrap();
        assert!(lines.is_empty());
    }
}
