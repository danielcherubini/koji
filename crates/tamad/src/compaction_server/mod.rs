//! Embedded Python compaction server (LLMLingua-2).
//!
//! Server files are embedded via include_dir! and extracted to the config
//! directory on first use.

use anyhow::Context;
use include_dir::{include_dir, Dir};
use std::path::PathBuf;

// include_dir resolves paths relative to CARGO_MANIFEST_DIR (crate root).
// CARGO_MANIFEST_DIR for tama-core is crates/tama-core/.
static SERVER_FILES: Dir = include_dir!("$CARGO_MANIFEST_DIR/src/compaction_server/server");

/// Recursively extract an embedded directory to disk.
/// Overwrites existing files (unlike `Dir::extract` which fails on existing files).
fn unpack_dir(dir: &Dir, dest: &std::path::Path) -> anyhow::Result<()> {
    for entry in dir.entries() {
        let entry_name = entry
            .path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let entry_dest = dest.join(entry_name.as_str());

        if let Some(subdir) = entry.as_dir() {
            std::fs::create_dir_all(&entry_dest)
                .with_context(|| format!("Failed to create {}", entry_dest.display()))?;
            unpack_dir(subdir, &entry_dest)?;
        } else if let Some(file) = entry.as_file() {
            std::fs::write(&entry_dest, file.contents())
                .with_context(|| format!("Failed to write {}", entry_dest.display()))?;
        }
    }
    Ok(())
}

/// Extract embedded server files to the config directory.
/// Returns the path to the extracted directory.
pub fn get_server_dir(config_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let dest = config_dir.join("compaction_server");
    if !dest.exists() {
        std::fs::create_dir_all(&dest)
            .with_context(|| format!("Failed to create {}", dest.display()))?;
        unpack_dir(&SERVER_FILES, &dest)
            .with_context(|| format!("Failed to unpack server files to {}", dest.display()))?;
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `get_server_dir` unpacks the embedded server once (idempotent on a
    /// second call) and yields a dir that contains the entrypoint.
    #[test]
    fn test_get_server_dir_unpacks_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("data");
        let server = get_server_dir(&base).expect("unpack");
        assert!(server.join("main.py").exists());
        let again = get_server_dir(&base).expect("idempotent");
        assert_eq!(again, server);
    }
}
