//! Backup and restore functionality for Tama.
//!
//! v3 (plan-190 Task 9): the archive contains ONLY `manifest.json`
//! (model/backend lists built from Postgres at backup time) and config
//! cards. No database file — Postgres state is backed up with `pg_dump`.
//!
//! This module provides:
//! - Archive creation (`create_backup`) - creates a .tar.gz of the manifest + cards
//! - Archive extraction (`extract_backup`) - validates and extracts with SHA-256 check
//! - Manifest reading (`extract_manifest`) - reads just the manifest for preview
//! - Model-card merging (`merge_model_cards`) - restore = extract → merge cards → done

pub mod archive;
pub mod manifest;
pub mod merge;

// Re-export main functions and types
pub use archive::{create_backup, extract_backup, extract_manifest, ExtractResult};
pub use manifest::{BackendEntry, BackupManifest, BackupModelEntry, BACKUP_FORMAT_VERSION};
pub use merge::merge_model_cards;
