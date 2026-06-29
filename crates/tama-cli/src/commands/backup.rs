//! Backup and restore CLI commands.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Instant;

/// Backup command arguments.
#[derive(clap::Parser, Debug)]
pub struct BackupArgs {
    /// Output path for the backup archive (default: tama-backup-YYYY-MM-DD.tar.gz in current dir)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Show what would be backed up without creating the archive
    #[arg(long)]
    pub dry_run: bool,
}

/// Restore command arguments.
#[derive(clap::Parser, Debug)]
pub struct RestoreArgs {
    /// Path to backup archive
    pub archive: PathBuf,

    /// Interactively select which models to restore
    #[arg(long)]
    pub select: bool,

    /// Show what would be restored without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Skip backend re-installation
    #[arg(long)]
    pub skip_backends: bool,

    /// Skip model re-downloading
    #[arg(long)]
    pub skip_models: bool,
}

/// Create a backup of the Tama configuration.
pub fn cmd_backup(
    _config: &tama_core::config::Config,
    output: Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    let config_dir = tama_core::config::Config::config_dir()?;

    let output_path = if let Some(path) = output {
        path
    } else {
        let timestamp = chrono::Utc::now().format("%Y-%m-%d");
        PathBuf::from(format!("tama-backup-{}.tar.gz", timestamp))
    };

    if dry_run {
        println!(
            "Dry run - would create backup at: {}",
            output_path.display()
        );
        println!("\nFiles to be backed up:");
        println!("  - tama.db (all settings)");
        if let Ok(entries) = std::fs::read_dir(config_dir.join("configs")) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|e| e == "toml") {
                    println!("  - configs/{}", entry.file_name().to_string_lossy());
                }
            }
        }
        println!("\nNote: Model files and backend binaries are NOT included.");
        return Ok(());
    }

    let start = Instant::now();

    let manifest = tama_core::backup::create_backup(&config_dir, &output_path)
        .context("Failed to create backup")?;

    let size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    println!("Backup created successfully: {}", output_path.display());
    println!("  Size: {:.2} MB", size as f64 / (1024.0 * 1024.0));
    println!("  Models: {}", manifest.models.len());
    println!("  Backends: {}", manifest.backends.len());
    println!("  Duration: {:.2}s", start.elapsed().as_secs_f64());

    Ok(())
}

/// Restore from a backup archive.
pub async fn cmd_restore(_config: &mut tama_core::config::Config, args: RestoreArgs) -> Result<()> {
    let config_dir = tama_core::config::Config::config_dir()?;

    if args.dry_run {
        println!("Dry run - would restore from: {}", args.archive.display());
        let manifest = tama_core::backup::extract_manifest(&args.archive)
            .context("Failed to read backup manifest")?;
        println!("\nBackup info:");
        println!("  Created: {}", manifest.created_at);
        println!("  Tama version: {}", manifest.tama_version);
        println!("\nWould restore:");
        println!("  - tama.db (all settings)");
        println!("  - {} model cards", manifest.models.len());
        return Ok(());
    }

    // Extract backup to temp directory
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    let extract_result = tama_core::backup::extract_backup(&args.archive, temp_dir.path())
        .context("Failed to extract backup")?;

    println!("Backup extracted successfully");

    // Replace the local database with the backup database
    let local_db_path = config_dir.join("tama.db");
    std::fs::copy(&extract_result.db_path, &local_db_path)
        .context("Failed to copy database from backup")?;
    println!("Database restored from backup");

    // Merge model cards (copy any that don't exist locally)
    let card_paths = tama_core::backup::merge_model_cards(
        &config_dir.join("configs"),
        &temp_dir.path().join("configs"),
    )
    .context("Failed to merge model cards")?;

    if !card_paths.is_empty() {
        println!("Model cards: {} restored", card_paths.len());
    }

    // Cleanup
    drop(temp_dir);

    println!("\nRestore complete!");
    println!("Note: Restart the proxy to load the restored configuration.");

    Ok(())
}
