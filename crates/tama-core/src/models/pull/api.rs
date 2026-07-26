use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json;

use crate::config::ModelModalities;
use crate::models::pull::hf_api;
use crate::models::pull::metadata::parse_readme_metadata;
use crate::models::pull::quant::infer_quant_from_filename;
use crate::models::pull::{BlobInfo, HfModelMetadata, RemoteGguf, RepoGgufListing};

/// A quant entry after grouping sharded GGUF files.
///
/// For sharded quants (filenames containing `/`), all shards in the same
/// directory are grouped into a single entry. `filename` is the primary shard
/// (first by sorted filename), `quant` is the directory name, and `shards`
/// contains all shard filenames sorted. `size_bytes` is the sum of all shard
/// sizes.
///
/// For single-file quants (no `/` in the filename), `shards` is empty,
/// `quant` is inferred via `infer_quant_from_filename`, and `size_bytes`
/// is the individual file size.
#[derive(Debug, Clone)]
pub struct GroupedQuant {
    pub filename: String,
    pub quant: Option<String>,
    pub shards: Vec<String>,
    pub size_bytes: Option<i64>,
    pub kind: crate::config::QuantKind,
}

/// List GGUF files available in a HuggingFace model repository.
/// Returns a `RepoGgufListing` with the resolved repo_id, commit SHA, and file list.
///
/// Auto-resolves repos: if `repo_id` doesn't end with `-GGUF` and the initial
/// fetch finds no GGUF files (or the repo doesn't exist), retries with `-GGUF` appended.
pub async fn list_gguf_files(repo_id: &str) -> Result<RepoGgufListing> {
    let api = hf_api().await?;

    // Try the repo_id as given first
    let candidates = if repo_id.to_uppercase().ends_with("-GGUF") {
        vec![repo_id.to_string()]
    } else {
        vec![repo_id.to_string(), format!("{}-GGUF", repo_id)]
    };

    let mut last_error: Option<String> = None;

    for candidate in &candidates {
        let repo = api.model(candidate.clone());
        match repo.info().await {
            Ok(info) => {
                let commit_sha = info.sha.clone();
                let ggufs: Vec<RemoteGguf> = info
                    .siblings
                    .into_iter()
                    .filter(|s| s.rfilename.ends_with(".gguf"))
                    .map(|s| {
                        let quant = infer_quant_from_filename(&s.rfilename);
                        RemoteGguf {
                            filename: s.rfilename,
                            quant,
                        }
                    })
                    .collect();

                if !ggufs.is_empty() {
                    return Ok(RepoGgufListing {
                        repo_id: candidate.clone(),
                        commit_sha,
                        files: ggufs,
                    });
                }
                // Repo exists but no GGUFs — try next candidate
                last_error = Some(format!(
                    "'{}' exists but contains no .gguf files",
                    candidate
                ));
            }
            Err(e) => {
                last_error = Some(format!("'{}': {}", candidate, e));
                continue;
            }
        }
    }

    let detail = last_error.unwrap_or_else(|| "unknown error".to_string());
    anyhow::bail!(
        "No GGUF files found. Tried: {}\nLast error: {}",
        candidates.join(", "),
        detail
    )
}

/// Extract the directory prefix from a sharded filename (everything before
/// the last `/`). Returns `None` for single-file filenames that contain no `/`.
///
/// Used by both `group_sharded_quants` (for grouping) and
/// `determine_primary_shard` (for sibling matching) to ensure consistent
/// directory-prefix extraction.
pub fn directory_prefix(filename: &str) -> Option<&str> {
    filename.rfind('/').map(|pos| &filename[..pos])
}

/// Fetch per-file blob metadata from HuggingFace using the blobs API.
///
/// Uses `hf_hub`'s authenticated client to call the HF API with `?blobs=true`,
/// which returns `blobId`, `size`, and `lfs.sha256` per sibling.
/// Returns a map of filename → BlobInfo for GGUF files only.
pub async fn fetch_blob_metadata(repo_id: &str) -> Result<HashMap<String, BlobInfo>> {
    let api = hf_api().await?;
    let url = super::hf_api_model_blobs_url(repo_id);

    let response = api
        .client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch blob metadata for '{}'", repo_id))?
        .error_for_status()
        .with_context(|| {
            format!(
                "HuggingFace returned an error for blob metadata request for '{}'",
                repo_id
            )
        })?
        .json::<serde_json::Value>()
        .await
        .with_context(|| format!("Failed to parse blob metadata response for '{}'", repo_id))?;

    Ok(parse_blob_siblings(&response))
}

/// Fetch comprehensive metadata for a model from the HuggingFace API and README.
///
/// Calls the HF models API for repo-level info (tags, pipeline_tag, lastModified)
/// and then fetches the README to parse architecture details.
///
/// The API call and README fetch are independent — if the API call succeeds but
/// the README fetch fails, the API-level metadata is still returned.
pub async fn fetch_hf_metadata(repo_id: &str) -> Result<HfModelMetadata> {
    let api = hf_api().await?;

    // ── Fetch model info from HF API ────────────────────────────────────────
    let url = super::hf_api_model_url(repo_id);
    let response = api
        .client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch model metadata for '{}'", repo_id))?
        .error_for_status()
        .with_context(|| {
            format!(
                "HuggingFace returned an error for model metadata request for '{}'",
                repo_id
            )
        })?
        .json::<serde_json::Value>()
        .await
        .with_context(|| format!("Failed to parse model metadata response for '{}'", repo_id))?;

    let mut meta = HfModelMetadata {
        hf_format: Some("gguf".to_string()),
        ..Default::default()
    };

    // Extract base_model from tags
    if let Some(tags) = response.get("tags").and_then(|t| t.as_array()) {
        for tag in tags {
            if let Some(tag_str) = tag.as_str() {
                if tag_str.starts_with("base_model:")
                    && !tag_str.starts_with("base_model:quantized:")
                {
                    meta.hf_base_model = tag_str.strip_prefix("base_model:").map(|s| s.to_string());
                }
            }
        }
    }

    // Extract pipeline_tag
    meta.hf_pipeline_tag = response
        .get("pipeline_tag")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract lastModified
    meta.hf_last_modified = response
        .get("lastModified")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // ── Fetch and parse README ──────────────────────────────────────────────
    // Try `main` first, fall back to `master` (some older repos use master)
    let readme_url = super::hf_raw_url(repo_id, "main", "README.md");
    let readme_fallback = super::hf_raw_url(repo_id, "master", "README.md");
    let readme_text = match api.client().get(&readme_url).send().await {
        Ok(resp) if resp.status().is_success() => resp.text().await.ok(),
        _ => {
            // Fallback to master branch
            match api.client().get(&readme_fallback).send().await {
                Ok(resp) if resp.status().is_success() => resp.text().await.ok(),
                _ => None,
            }
        }
    };
    if let Some(markdown) = readme_text {
        let readme_meta = parse_readme_metadata(&markdown);
        // Merge README metadata into the main struct (only fill None fields)
        if meta.hf_total_params.is_none() {
            meta.hf_total_params = readme_meta.hf_total_params;
        }
        if meta.hf_active_params.is_none() {
            meta.hf_active_params = readme_meta.hf_active_params;
        }
        if meta.hf_architecture_type.is_none() {
            meta.hf_architecture_type = readme_meta.hf_architecture_type;
        }
        if meta.hf_context_length.is_none() {
            meta.hf_context_length = readme_meta.hf_context_length;
        }
        if meta.hf_num_layers.is_none() {
            meta.hf_num_layers = readme_meta.hf_num_layers;
        }
    }

    Ok(meta)
}

/// Fetch the pipeline_tag from HuggingFace model metadata API.
///
/// Returns the `pipeline_tag` field from the model metadata, which indicates
/// the model's task type (e.g., "text-generation", "image-text-to-text").
pub async fn fetch_model_pipeline_tag(repo_id: &str) -> Result<Option<String>> {
    let api = hf_api().await?;
    let url = super::hf_api_model_url(repo_id);

    let response = api
        .client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch model metadata for '{}'", repo_id))?
        .error_for_status()
        .with_context(|| {
            format!(
                "HuggingFace returned an error for model metadata request for '{}'",
                repo_id
            )
        })?
        .json::<serde_json::Value>()
        .await
        .with_context(|| format!("Failed to parse model metadata response for '{}'", repo_id))?;

    Ok(response
        .get("pipeline_tag")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}

/// Try to infer modalities from a HuggingFace pipeline tag.
///
/// Order matters: more specific checks (e.g., "text-to-speech") must come
/// before broader ones (e.g., "speech") to avoid misclassification.
pub fn infer_modalities_from_pipeline(pipeline_tag: Option<&str>) -> Option<ModelModalities> {
    let tag = pipeline_tag?.to_lowercase();

    if tag.contains("vision") || tag.contains("image-text") {
        Some(ModelModalities {
            input: vec!["text".to_string(), "image".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("text-generation")
        || tag.contains("conversational")
        || tag.contains("chat")
    {
        Some(ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("image-classification") || tag.contains("object-detection") {
        Some(ModelModalities {
            input: vec!["image".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("text-to-speech") || tag.contains("tts") {
        // Must check TTS before generic "speech"/"audio" to avoid misclassification.
        Some(ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["audio".to_string()],
        })
    } else if tag.contains("speech") || tag.contains("audio") {
        Some(ModelModalities {
            input: vec!["audio".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("embedding") || tag.contains("feature-extraction") {
        Some(ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["embedding".to_string()],
        })
    } else if tag.contains("image-generation") || tag.contains("text-to-image") {
        Some(ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["image".to_string()],
        })
    } else {
        None
    }
}

/// Parse the `siblings` array from a HuggingFace blobs API response.
///
/// This is a pure function for testability — extract from `fetch_blob_metadata`
/// so it can be unit-tested with fixture data.
pub fn parse_blob_siblings(value: &serde_json::Value) -> HashMap<String, BlobInfo> {
    let mut result = HashMap::new();

    let siblings = match value.get("siblings").and_then(|s| s.as_array()) {
        Some(s) => s,
        None => return result,
    };

    for sibling in siblings {
        let rfilename = match sibling.get("rfilename").and_then(|f| f.as_str()) {
            Some(f) => f,
            None => continue,
        };

        if !rfilename.ends_with(".gguf") {
            continue;
        }

        let blob_id = sibling
            .get("blobId")
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());

        let size = sibling.get("size").and_then(|s| s.as_i64());

        let lfs_sha256 = sibling
            .get("lfs")
            .and_then(|lfs| lfs.get("sha256"))
            .and_then(|sha| sha.as_str())
            .map(|s| s.to_string());

        result.insert(
            rfilename.to_string(),
            BlobInfo {
                filename: rfilename.to_string(),
                blob_id,
                size,
                lfs_sha256,
            },
        );
    }

    result
}

/// Group sharded GGUF blobs into `GroupedQuant` entries.
///
/// Files whose rfilename contains `/` are treated as shards of a single quant:
/// they are grouped by their directory prefix (the portion before the last `/`),
/// and the directory name becomes the quant name. Files without `/` are
/// single-file quants whose quant name is inferred via `infer_quant_from_filename`.
///
/// Non-GGUF files are already excluded by `parse_blob_siblings`, so this function
/// only receives `.gguf` entries.
///
/// This is a pure function — no network or I/O — for easy unit testing.
pub fn group_sharded_quants(blobs: HashMap<String, BlobInfo>) -> Vec<GroupedQuant> {
    // Group blobs by their directory prefix (for sharded files) or by
    // filename itself (for single-file quants).
    let mut groups: HashMap<String, Vec<BlobInfo>> = HashMap::new();
    for blob in blobs.into_values() {
        let key = directory_prefix(&blob.filename)
            .map(|p| p.to_string())
            .unwrap_or_else(|| blob.filename.clone());
        groups.entry(key).or_default().push(blob);
    }

    groups
        .into_values()
        .map(|mut group| {
            // Sort shards by filename so the primary (first) is deterministic.
            group.sort_by(|a, b| a.filename.cmp(&b.filename));

            let primary = &group[0];
            let primary_filename = primary.filename.clone();

            // Quant name: directory name for sharded, inferred for single-file.
            let quant = if primary_filename.contains('/') {
                directory_prefix(&primary_filename).map(|p| p.to_string())
            } else {
                infer_quant_from_filename(&primary_filename)
            };

            // For sharded quants (primary filename contains `/`), list all
            // shard filenames sorted. For single-file quants, leave empty.
            let shards: Vec<String> = if primary_filename.contains('/') {
                group.iter().map(|b| b.filename.clone()).collect()
            } else {
                Vec::new()
            };

            // Sum of all shard sizes (None if any shard has no size).
            let size_bytes = if group.iter().all(|b| b.size.is_some()) {
                Some(group.iter().filter_map(|b| b.size).sum::<i64>())
            } else {
                None
            };

            GroupedQuant {
                filename: primary_filename,
                quant,
                shards,
                size_bytes,
                kind: crate::config::QuantKind::from_filename(&primary.filename),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that GGUF siblings are parsed with blobId, size, and LFS SHA256,
    /// and that non-GGUF files (e.g. README.md) are excluded from the result.
    #[test]
    fn test_parse_blob_siblings_basic() {
        let json = serde_json::json!({
            "siblings": [
                {
                    "rfilename": "README.md",
                    "blobId": "blob1",
                    "size": 1000
                },
                {
                    "rfilename": "Model-Q4_K_M.gguf",
                    "blobId": "blob2",
                    "size": 4200000000_i64,
                    "lfs": {
                        "sha256": "abcdef1234567890"
                    }
                },
                {
                    "rfilename": "Model-Q8_0.gguf",
                    "blobId": "blob3",
                    "size": 8400000000_i64,
                    "lfs": {
                        "sha256": "fedcba0987654321"
                    }
                }
            ]
        });

        let result = parse_blob_siblings(&json);

        // README should be excluded
        assert!(!result.contains_key("README.md"));
        assert_eq!(result.len(), 2);

        let q4 = result.get("Model-Q4_K_M.gguf").unwrap();
        assert_eq!(q4.blob_id.as_deref(), Some("blob2"));
        assert_eq!(q4.size, Some(4200000000_i64));
        assert_eq!(q4.lfs_sha256.as_deref(), Some("abcdef1234567890"));

        let q8 = result.get("Model-Q8_0.gguf").unwrap();
        assert_eq!(q8.lfs_sha256.as_deref(), Some("fedcba0987654321"));
    }

    /// Verifies that a GGUF sibling without an `lfs` field has `lfs_sha256 = None`.
    #[test]
    fn test_parse_blob_siblings_no_lfs() {
        let json = serde_json::json!({
            "siblings": [
                {
                    "rfilename": "model.gguf",
                    "blobId": "blob1",
                    "size": 1000
                }
            ]
        });

        let result = parse_blob_siblings(&json);
        let info = result.get("model.gguf").unwrap();
        assert!(info.lfs_sha256.is_none());
        assert_eq!(info.size, Some(1000));
    }

    /// Verifies that an empty `siblings` array produces an empty map.
    #[test]
    fn test_parse_blob_siblings_empty() {
        let json = serde_json::json!({ "siblings": [] });
        let result: HashMap<_, _> = parse_blob_siblings(&json);
        assert!(result.is_empty());
    }

    /// Verifies that a response without a `siblings` key produces an empty map.
    #[test]
    fn test_parse_blob_siblings_no_siblings_key() {
        let json = serde_json::json!({});
        let result = parse_blob_siblings(&json);
        assert!(result.is_empty());
    }

    /// Verifies that sharded GGUF files (filenames containing `/`) are grouped
    /// by their directory prefix into a single `GroupedQuant` entry, with the
    /// directory name as the quant name, all shard filenames in `shards`
    /// (sorted), and `size_bytes` as the sum of all shard sizes.
    ///
    /// Single-file quants (no `/` in filename) are returned as individual
    /// entries with `shards = []`, `quant` inferred via `infer_quant_from_filename`,
    /// and `size_bytes` set to the individual file size.
    ///
    /// Fixture data matches the Laguna-S-2.1-GGUF repository structure:
    /// 3 shards under `UD-Q4_K_XL/` plus two single-file quants.
    #[test]
    fn test_group_sharded_quants() {
        let mut blobs: HashMap<String, BlobInfo> = HashMap::new();

        // ── Sharded quant: 3 shards in UD-Q4_K_XL/ directory ────────────────
        blobs.insert(
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf".to_string(),
            BlobInfo {
                filename: "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf".to_string(),
                blob_id: Some("blob_shard1".to_string()),
                size: Some(100),
                lfs_sha256: Some("sha256_shard1".to_string()),
            },
        );
        blobs.insert(
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00002-of-00003.gguf".to_string(),
            BlobInfo {
                filename: "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00002-of-00003.gguf".to_string(),
                blob_id: Some("blob_shard2".to_string()),
                size: Some(200),
                lfs_sha256: Some("sha256_shard2".to_string()),
            },
        );
        blobs.insert(
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00003-of-00003.gguf".to_string(),
            BlobInfo {
                filename: "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00003-of-00003.gguf".to_string(),
                blob_id: Some("blob_shard3".to_string()),
                size: Some(300),
                lfs_sha256: Some("sha256_shard3".to_string()),
            },
        );

        // ── Single-file quants ──────────────────────────────────────────────
        blobs.insert(
            "Laguna-S-2.1-Q4_K_M.gguf".to_string(),
            BlobInfo {
                filename: "Laguna-S-2.1-Q4_K_M.gguf".to_string(),
                blob_id: Some("blob_q4km".to_string()),
                size: Some(500),
                lfs_sha256: Some("sha256_q4km".to_string()),
            },
        );
        blobs.insert(
            "Laguna-S-2.1-Q8_0.gguf".to_string(),
            BlobInfo {
                filename: "Laguna-S-2.1-Q8_0.gguf".to_string(),
                blob_id: Some("blob_q80".to_string()),
                size: Some(600),
                lfs_sha256: Some("sha256_q80".to_string()),
            },
        );

        let result = group_sharded_quants(blobs);

        // Should produce 3 entries: 1 sharded group + 2 single-file
        assert_eq!(result.len(), 3);

        // ── Verify the sharded group ────────────────────────────────────────
        let sharded = result
            .iter()
            .find(|g| g.shards.len() == 3)
            .expect("should have a sharded group with 3 shards");

        assert_eq!(
            sharded.filename,
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf"
        );
        assert_eq!(sharded.quant.as_deref(), Some("UD-Q4_K_XL"));
        assert_eq!(sharded.shards.len(), 3);
        assert_eq!(
            sharded.shards[0],
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf"
        );
        assert_eq!(
            sharded.shards[1],
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00002-of-00003.gguf"
        );
        assert_eq!(
            sharded.shards[2],
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00003-of-00003.gguf"
        );
        // Sum of 100 + 200 + 300 = 600
        assert_eq!(sharded.size_bytes, Some(600));
        assert_eq!(sharded.kind, crate::config::QuantKind::Model);

        // ── Verify Q4_K_M single-file entry ─────────────────────────────────
        let q4km = result
            .iter()
            .find(|g| g.filename == "Laguna-S-2.1-Q4_K_M.gguf")
            .expect("should have Q4_K_M single-file entry");

        assert_eq!(q4km.quant.as_deref(), Some("Q4_K_M"));
        assert!(q4km.shards.is_empty());
        assert_eq!(q4km.size_bytes, Some(500));
        assert_eq!(q4km.kind, crate::config::QuantKind::Model);

        // ── Verify Q8_0 single-file entry ───────────────────────────────────
        let q80 = result
            .iter()
            .find(|g| g.filename == "Laguna-S-2.1-Q8_0.gguf")
            .expect("should have Q8_0 single-file entry");

        assert_eq!(q80.quant.as_deref(), Some("Q8_0"));
        assert!(q80.shards.is_empty());
        assert_eq!(q80.size_bytes, Some(600));
        assert_eq!(q80.kind, crate::config::QuantKind::Model);
    }

    /// Empty input should produce an empty result.
    #[test]
    fn test_group_sharded_quants_empty() {
        let blobs: HashMap<String, BlobInfo> = HashMap::new();
        let result = group_sharded_quants(blobs);
        assert!(result.is_empty(), "empty input should produce empty output");
    }

    /// A directory with only one file should produce a GroupedQuant with
    /// shards.len() == 1 (the file is both primary and only shard).
    #[test]
    fn test_group_sharded_quants_single_file_in_directory() {
        let mut blobs: HashMap<String, BlobInfo> = HashMap::new();
        blobs.insert(
            "Q4_K_M/model-Q4_K_M.gguf".to_string(),
            BlobInfo {
                filename: "Q4_K_M/model-Q4_K_M.gguf".to_string(),
                blob_id: None,
                size: Some(400),
                lfs_sha256: None,
            },
        );

        let result = group_sharded_quants(blobs);
        assert_eq!(result.len(), 1);
        let entry = &result[0];
        assert_eq!(entry.filename, "Q4_K_M/model-Q4_K_M.gguf");
        assert_eq!(entry.quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(entry.shards.len(), 1);
        assert_eq!(entry.size_bytes, Some(400));
    }

    /// Deeply nested paths (a/b/c/file.gguf) should group by the full
    /// directory prefix (a/b/c), with the quant name being the last
    /// directory component (c).
    #[test]
    fn test_group_sharded_quants_deeply_nested() {
        let mut blobs: HashMap<String, BlobInfo> = HashMap::new();
        blobs.insert(
            "a/b/c/model-00001-of-00002.gguf".to_string(),
            BlobInfo {
                filename: "a/b/c/model-00001-of-00002.gguf".to_string(),
                blob_id: None,
                size: Some(100),
                lfs_sha256: None,
            },
        );
        blobs.insert(
            "a/b/c/model-00002-of-00002.gguf".to_string(),
            BlobInfo {
                filename: "a/b/c/model-00002-of-00002.gguf".to_string(),
                blob_id: None,
                size: Some(200),
                lfs_sha256: None,
            },
        );

        let result = group_sharded_quants(blobs);
        assert_eq!(result.len(), 1);
        let entry = &result[0];
        // Primary shard is first by sorted filename
        assert_eq!(entry.filename, "a/b/c/model-00001-of-00002.gguf");
        // directory_prefix returns the full prefix "a/b/c"
        assert_eq!(entry.quant.as_deref(), Some("a/b/c"));
        assert_eq!(entry.shards.len(), 2);
        assert_eq!(entry.size_bytes, Some(300)); // 100 + 200
    }
}
