//! Archive creation and extraction for Tama backup/restore.
//!
//! **v3 format (plan-190 Task 9):** the archive contains ONLY `manifest.json`
//! and `configs/*.toml` model cards — no database file. All DB state and the
//! global app config live in Postgres (`pg_dump` is the DB backup path). The
//! manifest's model/backend lists come from Postgres queries at backup time.
//!
//! **SHA-256 Contract:** `manifest.sha256` covers all archive entries
//! **EXCEPT** `manifest.json` itself (i.e. the config-card entries). This
//! avoids the chicken-and-egg problem.
//!
//! On creation (`create_backup`):
//! 1. Stream config cards through a hasher
//! 2. Compute SHA-256
//! 3. Write tar.gz with manifest.json first (containing the hash)
//! 4. Then write the config-card entries
//!
//! On extraction (`extract_backup`):
//! 1. Read all entries except manifest.json into a hasher
//! 2. Compare computed SHA-256 against manifest.sha256
//! 3. If mismatch, delete extracted files and error
//!
//! Pre-v3 archives (containing `tama.db`) still extract: the db entry is
//! hashed (for integrity) but ignored — its data stays in Postgres.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder};

use crate::backup::manifest::{BackendEntry, BackupManifest, BackupModelEntry};
use crate::db::queries;

/// Result of extracting a backup archive.
#[derive(Debug)]
pub struct ExtractResult {
    /// Parsed manifest from the archive
    pub manifest: BackupManifest,
    /// Paths to extracted model card TOML files
    pub card_paths: Vec<PathBuf>,
}

/// Streaming SHA-256 hasher that implements `Write`.
///
/// Pipes data through a `sha2::Sha256` hasher without buffering the full
/// contents in memory. Used for streaming file hashing during backup creation
/// and extraction integrity verification.
pub struct StreamingHasher {
    inner: Sha256,
}

impl Default for StreamingHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingHasher {
    /// Create a new streaming hasher.
    pub fn new() -> Self {
        Self {
            inner: Sha256::new(),
        }
    }

    /// Finalize the hash and return the digest as hex string.
    pub fn finalize_hex(&mut self) -> String {
        let hash = self.inner.clone().finalize();
        format!("{:x}", hash)
    }

    /// Reset the hasher to its initial state for reuse.
    pub fn reset(&mut self) {
        self.inner = Sha256::new();
    }

    /// Update the hasher with raw bytes (for streaming extraction).
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }
}

impl Write for StreamingHasher {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Create a v3 backup archive: `manifest.json` (built from Postgres) +
/// config cards only.
///
/// **SHA-256 Contract:** The returned manifest's `sha256` field covers all
/// archive entries **EXCEPT** `manifest.json` itself (the config cards).
pub async fn create_backup(
    pool: &PgPool,
    config_dir: &Path,
    output_path: &Path,
) -> Result<BackupManifest> {
    if !config_dir.exists() {
        anyhow::bail!("Config directory does not exist: {}", config_dir.display());
    }

    // Manifest lists come from Postgres (plan-190 Task 9: no tama.db).
    let models = backup_model_entries(pool).await?;
    let backends = backup_backend_entries(pool).await?;

    // File streaming is blocking — keep it off the async runtime.
    let (config_dir, output_path) = (config_dir.to_path_buf(), output_path.to_path_buf());
    tokio::task::spawn_blocking(move || {
        create_backup_offline(&models, &backends, &config_dir, &output_path)
    })
    .await
    .context("backup task panicked")?
    .context("Failed to create archive")
}

/// Model entries for the backup manifest, from Postgres `model_pulls` +
/// `model_files`.
async fn backup_model_entries(pool: &PgPool) -> Result<Vec<BackupModelEntry>> {
    let pulls = queries::get_all_model_pulls(pool).await?;
    let files = queries::get_all_model_files(pool).await?;

    let mut files_by_repo: HashMap<String, Vec<(Option<String>, i64)>> = HashMap::new();
    for f in files {
        files_by_repo
            .entry(f.repo_id)
            .or_default()
            .push((f.quant, f.size_bytes.unwrap_or(0)));
    }

    Ok(pulls
        .into_iter()
        .map(|pull| {
            let repo_files = files_by_repo
                .get(&pull.repo_id)
                .cloned()
                .unwrap_or_default();
            let quants: Vec<String> = repo_files.iter().filter_map(|(q, _)| q.clone()).collect();
            let total_size_bytes: i64 = repo_files.iter().map(|(_, size)| *size).sum();
            BackupModelEntry {
                repo_id: pull.repo_id,
                quants,
                total_size_bytes,
            }
        })
        .collect())
}

/// Active backend entries for the backup manifest, from Postgres
/// `provider_installations`.
async fn backup_backend_entries(pool: &PgPool) -> Result<Vec<BackendEntry>> {
    Ok(queries::list_active_installations(pool)
        .await?
        .into_iter()
        .map(|r| BackendEntry {
            name: r.name,
            version: r.version,
            backend_type: r.backend_type,
            source: r.source,
            docker_config: r.docker_config,
        })
        .collect())
}

/// Blocking half of [`create_backup`]: hash the config cards, build the
/// manifest, write the tar.gz (manifest first, then cards).
fn create_backup_offline(
    models: &[BackupModelEntry],
    backends: &[BackendEntry],
    config_dir: &Path,
    output_path: &Path,
) -> Result<BackupManifest> {
    // Step 1: compute SHA-256 by streaming the config cards through a hasher.
    let mut hasher = StreamingHasher::new();
    let configs_dir = config_dir.join("configs");
    if configs_dir.exists() {
        let mut entries: Vec<_> = fs::read_dir(&configs_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let file = File::open(&path)
                .with_context(|| format!("Failed to open config card: {}", path.display()))?;
            let mut reader = BufReader::new(file);
            std::io::copy(&mut reader, &mut hasher)
                .with_context(|| format!("Failed to hash config card: {}", path.display()))?;
        }
    }
    let sha256_hex = hasher.finalize_hex();

    // Step 2: build the manifest (lists already fetched from Postgres).
    let mut manifest = BackupManifest::new(env!("CARGO_PKG_VERSION"));
    manifest.sha256 = sha256_hex;
    manifest.models = models.to_vec();
    manifest.backends = backends.to_vec();

    // Step 3: create the tar.gz archive.
    write_archive(config_dir, output_path, &manifest)?;
    Ok(manifest)
}

/// Write the tar.gz archive: `manifest.json` first, then the sorted config
/// cards. No database file (v3, plan-190 Task 9).
///
/// Public so tests (and future tooling) can build archives without a DB.
pub fn write_archive(
    config_dir: &Path,
    output_path: &Path,
    manifest: &BackupManifest,
) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;
    }

    let manifest_json =
        serde_json::to_string_pretty(manifest).context("Failed to serialize manifest to JSON")?;

    let file = File::create(output_path)
        .with_context(|| format!("Failed to create archive file: {}", output_path.display()))?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = Builder::new(encoder);

    let manifest_name = "manifest.json";
    let manifest_data = manifest_json.as_bytes();
    let mut header = tar::Header::new_gnu();
    header
        .set_path(manifest_name)
        .context("Failed to set manifest.json path")?;
    header.set_size(manifest_data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(chrono::Utc::now().timestamp() as u64);
    header.set_cksum();
    tar.append(&header, manifest_json.as_bytes())
        .context("Failed to append manifest.json to archive")?;

    let configs_dir = config_dir.join("configs");
    if configs_dir.exists() {
        let mut entries: Vec<_> = fs::read_dir(&configs_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            add_file_to_archive(&mut tar, &path, &format!("configs/{}", name))
                .context("Failed to add config card to archive")?;
        }
    }

    tar.into_inner()?
        .finish()
        .context("Failed to finalize archive")?;

    Ok(())
}

/// Add a file to the tar archive by streaming it directly from disk.
///
/// Uses `BufReader` + `std::io::copy()` to stream data without loading
/// the entire file into memory.
fn add_file_to_archive(
    tar: &mut Builder<flate2::write::GzEncoder<File>>,
    path: &Path,
    name: &str,
) -> Result<()> {
    let file =
        File::open(path).with_context(|| format!("Failed to open {}: {}", name, path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("Failed to read metadata for {}: {}", name, path.display()))?;

    let mut header = tar::Header::new_gnu();
    header
        .set_path(name)
        .with_context(|| format!("Failed to set path for {}: {}", name, path.display()))?;
    header.set_size(metadata.len());
    header.set_mode(0o644);
    header.set_mtime(chrono::Utc::now().timestamp() as u64);
    header.set_cksum();

    let mut reader = BufReader::new(file);
    tar.append(&header, &mut reader)
        .with_context(|| format!("Failed to append {} to archive", name))?;

    Ok(())
}

pub fn extract_manifest(archive_path: &Path) -> Result<BackupManifest> {
    let file = File::open(archive_path)
        .with_context(|| format!("Failed to open archive: {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.to_string_lossy() == "manifest.json" {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .context("Failed to read manifest.json from archive")?;
            return serde_json::from_str(&contents)
                .context("Failed to parse manifest.json from archive");
        }
    }

    anyhow::bail!("manifest.json not found in archive")
}

pub fn extract_backup(archive_path: &Path, target_dir: &Path) -> Result<ExtractResult> {
    let manifest =
        extract_manifest(archive_path).context("Failed to extract or parse manifest.json")?;

    // Validate backup format version before proceeding.
    manifest
        .validate_version()
        .context("Backup format version mismatch")?;

    fs::create_dir_all(target_dir).with_context(|| {
        format!(
            "Failed to create target directory: {}",
            target_dir.display()
        )
    })?;

    let file = File::open(archive_path)
        .with_context(|| format!("Failed to open archive: {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    let mut hasher = StreamingHasher::new();
    let mut extracted_cards: Vec<PathBuf> = Vec::new();

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let entry_name_str = entry.path()?;
        let entry_name_owned = entry_name_str.to_string_lossy().to_string();
        let needs_hashing = entry_name_owned != "manifest.json";

        if needs_hashing {
            let dest_path = target_dir.join(entry_name_owned.trim_start_matches("/"));

            // Validate path to prevent traversal attacks
            // Use target_dir directly for prefix check to avoid Windows short-path vs
            // long-path mismatches (e.g. RUNNER~1 vs DANIELCH~1)
            let canonical_target = target_dir.canonicalize().with_context(|| {
                format!(
                    "Failed to canonicalize target directory: {}",
                    target_dir.display()
                )
            })?;

            // Check for path traversal before creating directories
            if dest_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                anyhow::bail!(
                    "Path traversal detected in archive entry: {}",
                    entry_name_owned
                );
            }

            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directory: {}", parent.display())
                })?;
            }

            // Double-check the resolved path is within target_dir
            if let Ok(canonical_dest) = dest_path.canonicalize() {
                if !canonical_dest.starts_with(&canonical_target) {
                    anyhow::bail!(
                        "Extracted path escapes target directory: {}",
                        dest_path.display()
                    );
                }
            } else {
                // Path doesn't exist yet, check relative path using target_dir
                // (not canonical_target to avoid short/long path mismatches on Windows)
                let relative = dest_path.strip_prefix(target_dir).map_err(|_| {
                    anyhow::anyhow!("Path escapes target directory: {}", dest_path.display())
                })?;
                if relative
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    anyhow::bail!(
                        "Extracted path escapes target directory: {}",
                        dest_path.display()
                    );
                }
            }

            // Stream entry directly into both the hasher and the destination file
            let mut output_file = File::create(&dest_path)
                .with_context(|| format!("Failed to create file: {}", dest_path.display()))?;

            // Read chunk by chunk, updating hasher and writing to file
            const BUF_SIZE: usize = 64 * 1024; // 64KB buffer
            let mut buf = [0u8; BUF_SIZE];
            loop {
                let n = entry.read(&mut buf).with_context(|| {
                    format!("Failed to read archive entry: {}", entry_name_owned)
                })?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                output_file
                    .write_all(&buf[..n])
                    .with_context(|| format!("Failed to write file: {}", dest_path.display()))?;
            }

            // v3: only config cards matter; pre-v3 `tama.db` entries are
            // extracted + hashed (integrity) but ignored.
            if entry_name_owned.starts_with("configs/") {
                extracted_cards.push(dest_path);
            }
        } else {
            let mut _contents = Vec::new();
            entry
                .read_to_end(&mut _contents)
                .with_context(|| format!("Failed to read manifest.json: {}", entry_name_owned))?;
        }
    }

    let computed_hex = hasher.finalize_hex();
    if computed_hex != manifest.sha256 {
        fs::remove_dir_all(target_dir).ok();
        anyhow::bail!(
            "SHA-256 integrity check failed! Expected: {}, Computed: {}",
            manifest.sha256,
            computed_hex
        );
    }

    Ok(ExtractResult {
        manifest,
        card_paths: extracted_cards,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed a Postgres schema with one pulled model + one active backend.
    async fn seed_fixture(pool: &PgPool) {
        let mc = crate::config::ModelConfig {
            backend: "llama_cpp".to_string(),
            model: Some("test/repo".to_string()),
            ..Default::default()
        };
        let key = crate::models::ConfigKey::from_repo_id("test/repo");
        let model_id = crate::db::save_model_config(pool, key.as_str(), &mc)
            .await
            .unwrap();
        crate::db::queries::upsert_model_pull(pool, model_id, "test/repo", "abc123")
            .await
            .unwrap();
        crate::db::queries::upsert_model_file(
            pool,
            model_id,
            "test/repo",
            "model.gguf",
            Some("Q4_K_M"),
            None,
            Some(1000),
        )
        .await
        .unwrap();
        crate::db::queries::insert_installation(
            pool,
            &crate::db::queries::InstallationRecord {
                id: 0,
                name: "llama_cpp".to_string(),
                backend_type: "llama_cpp".to_string(),
                version: "v1.0".to_string(),
                path: "/tmp/llama".to_string(),
                installed_at: 1234567890,
                gpu_variant: "cpu".to_string(),
                source: Some("prebuilt".to_string()),
                is_active: true,
                docker_config: None,
                logical_id: String::new(),
            },
        )
        .await
        .unwrap();
    }

    /// v3 round-trip: manifest from Postgres, no db entry in the archive,
    /// config card survives create -> extract.
    #[tokio::test]
    async fn test_create_and_extract_backup_roundtrip() {
        let guard = crate::testing::postgres::with_schema().await;
        let config_dir = tempfile::tempdir().expect("tempdir");
        let config_dir = config_dir.path();
        let output_path = config_dir_dir_join(config_dir, "backup.tar.gz");
        let extract_dir = config_dir_dir_join(config_dir, "extracted");

        fs::create_dir_all(config_dir.join("configs")).expect("create dirs");
        fs::write(
            config_dir.join("configs").join("test_config.toml"),
            "[model]\nid = \"test/model\"\nname = \"Test Model\"\n",
        )
        .expect("write model card");

        seed_fixture(&guard.pool).await;

        let manifest = create_backup(&guard.pool, config_dir, &output_path)
            .await
            .expect("create_backup should succeed");
        assert!(
            manifest.sha256.len() == 64,
            "sha256 should be hex: {:?}",
            manifest
        );
        assert_eq!(
            manifest.models.len(),
            1,
            "manifest should list the pulled model"
        );
        assert_eq!(manifest.models[0].repo_id, "test/repo");
        assert_eq!(manifest.models[0].quants, vec!["Q4_K_M".to_string()]);
        assert_eq!(manifest.models[0].total_size_bytes, 1000);
        assert_eq!(
            manifest.backends.len(),
            1,
            "manifest should list the active backend"
        );
        assert_eq!(manifest.backends[0].name, "llama_cpp");

        let extracted = extract_backup(&output_path, &extract_dir).unwrap();
        // No db entry: only the model card is extracted as content.
        assert_eq!(
            extracted.card_paths.len(),
            1,
            "only the model card should extract"
        );
        let card = fs::read_to_string(&extracted.card_paths[0]).expect("read card");
        assert!(card.contains("test/model"));

        // The archive must not contain a tama.db entry.
        assert!(
            !archive_contains_entry(&output_path, "tama.db"),
            "v3 archive must not contain tama.db"
        );
        guard.finish().await;
    }

    fn config_dir_dir_join(dir: &Path, name: &str) -> PathBuf {
        dir.join(name)
    }

    fn archive_contains_entry(archive_path: &Path, name: &str) -> bool {
        let file = File::open(archive_path).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        archive.entries().unwrap().any(|e| {
            e.as_ref()
                .map(|e| {
                    e.path()
                        .map(|p| p.to_string_lossy() == name)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
    }

    /// A tampered manifest version is rejected on extraction.
    #[tokio::test]
    async fn test_backup_version_validation_rejects_incompatible() {
        let guard = crate::testing::postgres::with_schema().await;
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_dir = temp_dir.path().join("config");
        let output_path = temp_dir.path().join("backup.tar.gz");
        let extract_dir = temp_dir.path().join("extracted");

        fs::create_dir_all(config_dir.join("configs")).expect("create dirs");
        seed_fixture(&guard.pool).await;
        create_backup(&guard.pool, &config_dir, &output_path)
            .await
            .expect("create backup");

        // Re-pack the archive with the manifest version tampered to 99.
        let temp_archive = tempfile::NamedTempFile::new().expect("temp file");
        let modified_dir = tempfile::tempdir().expect("temp dir");
        {
            let mut archive = Archive::new(flate2::read::GzDecoder::new(
                File::open(&output_path).unwrap(),
            ));
            for entry_result in archive.entries().unwrap() {
                let mut entry = entry_result.unwrap();
                let path = entry.path().unwrap().into_owned();
                let path_str = path.to_string_lossy().to_string();
                if path_str == "manifest.json" {
                    let mut contents = String::new();
                    entry.read_to_string(&mut contents).unwrap();
                    let mut manifest: serde_json::Value = serde_json::from_str(&contents).unwrap();
                    manifest["version"] = serde_json::json!(99);
                    let modified_json = serde_json::to_string_pretty(&manifest).unwrap();
                    fs::write(modified_dir.path().join("manifest.json"), &modified_json).unwrap();
                } else {
                    entry.unpack(modified_dir.path().join(&path)).unwrap();
                }
            }
        }
        let packed_file = File::create(temp_archive.path()).unwrap();
        let encoder = flate2::write::GzEncoder::new(packed_file, flate2::Compression::default());
        let mut tar_builder = Builder::new(encoder);
        for entry in fs::read_dir(modified_dir.path()).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let file = File::open(&path).unwrap();
            let metadata = file.metadata().unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_path(&name).unwrap();
            header.set_size(metadata.len());
            header.set_mode(0o644);
            header.set_mtime(chrono::Utc::now().timestamp() as u64);
            header.set_cksum();
            let mut reader = BufReader::new(file);
            tar_builder.append(&header, &mut reader).unwrap();
        }
        tar_builder.into_inner().unwrap().finish().unwrap();

        let result = extract_backup(temp_archive.path(), &extract_dir);
        assert!(
            result.is_err(),
            "extract_backup should reject incompatible backup version"
        );
        let err_chain = format!("{}", result.unwrap_err());
        assert!(
            err_chain.contains("Incompatible backup format version")
                || err_chain.contains("version mismatch"),
            "error message should mention version mismatch: {}",
            err_chain
        );
        guard.finish().await;
    }

    /// create_backup with an empty DB: manifest has no models/backends,
    /// hashing still works, archive round-trips.
    #[tokio::test]
    async fn test_create_backup_empty_db() {
        let guard = crate::testing::postgres::with_schema().await;
        let config_dir = tempfile::tempdir().expect("tempdir");
        let output_path = config_dir.path().join("backup.tar.gz");
        let extract_dir = config_dir.path().join("extracted");

        let manifest = create_backup(&guard.pool, config_dir.path(), &output_path)
            .await
            .expect("create_backup should succeed with no models");
        assert!(manifest.models.is_empty());
        assert!(manifest.backends.is_empty());
        // Empty archive still verifies.
        let extracted = extract_backup(&output_path, &extract_dir).unwrap();
        assert!(extracted.card_paths.is_empty());
        guard.finish().await;
    }

    #[test]
    fn test_streaming_hasher_basic() {
        let mut hasher = StreamingHasher::new();
        hasher.write_all(b"hello world").unwrap();
        let hash = hasher.finalize_hex();
        // Known SHA-256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_streaming_hasher_reset() {
        let mut hasher = StreamingHasher::new();
        hasher.write_all(b"hello").unwrap();
        let hash1 = hasher.finalize_hex();

        hasher.reset();
        hasher.write_all(b"hello").unwrap();
        let hash2 = hasher.finalize_hex();

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_streaming_hasher_copy() {
        use std::io::Cursor;

        let data = b"the quick brown fox jumps over the lazy dog";
        let mut hasher = StreamingHasher::new();
        let mut cursor = Cursor::new(&data[..]);
        std::io::copy(&mut cursor, &mut hasher).unwrap();
        let hash = hasher.finalize_hex();

        // Known SHA-256 of "the quick brown fox jumps over the lazy dog" (no trailing newline)
        assert_eq!(
            hash,
            "05c6e08f1d9fdafa03147fcb8f82f124c76d2f70e3d989dc8aadb5e7d7450bec"
        );
    }
}
