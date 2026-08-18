//! Pure helpers around `hf` CLI whole-repo pulls (plan-191 Task 10).
//!
//! What stays here (shared by both binaries, no process spawning, ADR-0010):
//! `scan_dir_bytes` (the proxy's status DTO takes the max of the
//! relayed byte counter and this local scan) and `stderr_tail_str`
//! (rendering the capped error sink the relay fills from tamad job events).
//!
//! The `hf` CLI *execution* (`check_hf_binary`, `spawn_hf_download`,
//! `start_stderr_reader`) moved to the tamad crate
//! (`crates/tamad/src/download/hf.rs`) — the proxy never spawns it.

use std::path::Path;
use std::sync::Arc;

/// Return the captured stderr tail (trailing newlines stripped) if non-empty.
pub async fn stderr_tail_str(sink: &Arc<tokio::sync::Mutex<Vec<u8>>>) -> Option<String> {
    let tail = sink.lock().await;
    let decoded = String::from_utf8_lossy(&tail);
    let trimmed = decoded.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Recursively sum the sizes of all regular files under `dir`.
///
/// Symlinks are skipped entirely — never counted, never descended into —
/// which also makes the walk immune to symlink cycles (e.g. a directory
/// symlink pointing at an ancestor). Returns 0 if the directory does not
/// exist.
pub fn scan_dir_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            // Never follow symlinks (file_type does not): a symlinked dir to
            // an ancestor would loop forever, and `hf download` writes only
            // regular files.
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = path.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `scan_dir_bytes` sums regular files and skips symlinks.
    #[test]
    fn test_scan_dir_bytes_counts_regular_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.bin"), vec![0u8; 100]).unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("b.bin"), vec![1u8; 50]).unwrap();

        assert_eq!(scan_dir_bytes(root), 150, "regular files must be summed");

        // Symlink to a file: must not be counted (unix-only — Windows
        // symlinks require elevated privileges).
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("a.bin"), root.join("link.bin")).unwrap();
            assert_eq!(scan_dir_bytes(root), 150, "symlinks must not be counted");
        }
    }
}
