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

/// Get the path to the Python entrypoint.
/// Uses config.server_path if set, otherwise uses the embedded default.
pub fn get_server_entrypoint(
    config: &crate::config::CompactionConfig,
    config_dir: &std::path::Path,
) -> anyhow::Result<PathBuf> {
    if let Some(ref path) = config.server_path {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
        tracing::warn!(
            "Configured server_path '{}' does not exist, using embedded default",
            p.display()
        );
    }
    let p = get_server_dir(config_dir)?.join("main.py");
    if p.exists() {
        Ok(p)
    } else {
        Err(anyhow::anyhow!(
            "Embedded server not found at {}",
            p.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_server_entrypoint_prefers_config() {
        // When server_path exists, it should be preferred
        let config = crate::config::CompactionConfig {
            server_path: Some("/tmp/custom_server.py".to_string()),
            ..Default::default()
        };
        // Create a temp file to simulate existing path
        std::fs::write("/tmp/custom_server.py", "# test").unwrap();
        let result = get_server_entrypoint(&config, &std::path::PathBuf::from("/tmp"));
        std::fs::remove_file("/tmp/custom_server.py").ok();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("/tmp/custom_server.py"));
    }

    #[test]
    fn test_get_server_entrypoint_falls_back_to_embedded() {
        let config = crate::config::CompactionConfig {
            server_path: Some("/nonexistent/path.py".to_string()),
            ..Default::default()
        };
        // Use a unique tempdir to avoid interference from other tests
        let tmp_dir = std::env::temp_dir().join(format!(
            "compaction_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        // Clean up any pre-existing directory (get_server_dir skips unpack if dir exists)
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let result = get_server_entrypoint(&config, &tmp_dir);
        assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
        assert!(result.unwrap().ends_with("main.py"));
        // Clean up
        let _ = std::fs::remove_dir_all(tmp_dir.parent().unwrap());
    }
}
