//! Model-card merge for backup/restore (plan-190 Task 9).
//!
//! v3 restores no longer merge databases or global config: the archive
//! contains only `manifest.json` + config cards, and the manifest's
//! model/backend lists come from Postgres at backup time. Restore =
//! extract → merge model cards → done.

use std::path::Path;

use anyhow::{Context, Result};

/// Merge model card TOML files from backup to local.
///
/// Copies any card that doesn't exist locally.
pub fn merge_model_cards(
    local_configs_dir: &Path,
    backup_configs_dir: &Path,
) -> Result<Vec<String>> {
    let mut copied = Vec::new();

    if !backup_configs_dir.exists() {
        return Ok(copied);
    }

    // Ensure local directory exists
    std::fs::create_dir_all(local_configs_dir).with_context(|| {
        format!(
            "Failed to create local configs directory: {}",
            local_configs_dir.display()
        )
    })?;

    for entry in std::fs::read_dir(backup_configs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let local_path = local_configs_dir.join(&filename);
            if !local_path.exists() {
                std::fs::copy(&path, &local_path)
                    .with_context(|| format!("Failed to copy card: {}", filename))?;
                copied.push(filename);
            }
        }
    }

    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_model_cards_copies_missing_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("local_configs");
        let backup = temp.path().join("backup_configs");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::create_dir_all(&backup).unwrap();

        // Existing local card (should not be overwritten).
        std::fs::write(local.join("existing.toml"), "local").unwrap();
        std::fs::write(backup.join("existing.toml"), "backup").unwrap();
        std::fs::write(backup.join("new_card.toml"), "backup-new").unwrap();
        // Non-TOML files are ignored.
        std::fs::write(backup.join("notes.txt"), "ignored").unwrap();

        let copied = merge_model_cards(&local, &backup).expect("merge");
        assert_eq!(copied, vec!["new_card.toml"]);
        assert_eq!(
            std::fs::read_to_string(local.join("existing.toml")).unwrap(),
            "local"
        );
        assert_eq!(
            std::fs::read_to_string(local.join("new_card.toml")).unwrap(),
            "backup-new"
        );
        assert!(!local.join("notes.txt").exists());
    }

    #[test]
    fn test_merge_model_cards_empty_backup_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let local = temp.path().join("local_configs");
        let backup = temp.path().join("missing_configs");

        let copied = merge_model_cards(&local, &backup).expect("merge on missing dir");
        assert!(copied.is_empty());
        assert!(!local.exists());
    }
}
