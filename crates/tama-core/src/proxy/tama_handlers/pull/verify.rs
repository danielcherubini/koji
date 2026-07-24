use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::config::default_num_parallel;
use crate::config::ModelConfig;
use crate::config::QuantKind;
use crate::models::card::ModelCard;
use crate::models::pull::fetch_community_card;
use crate::models::pull::infer_quant_from_filename;
use crate::models::pull::BlobInfo;
use crate::models::pull::GgufMetadata;
use crate::models::QuantInfo;
use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::pull_queue::PullQueueService;
use crate::proxy::tama_handlers::generate_display_name;
use crate::proxy::tama_handlers::QuantDownloadSpec;
use crate::proxy::ProxyState;

/// Outcome of a verification pass. Carries the hash info so the caller can
/// persist it to `model_files` *after* `setup_model_after_pull` has created
/// the matching `model_configs` row (the DB FK requires it to exist).
pub(super) struct VerificationOutcome {
    pub passed: bool,
    pub expected_sha: Option<String>,
    pub ok: Option<bool>,
    pub err: Option<String>,
    /// Whether this file is the primary shard of a sharded quant.
    /// Single-file quants are always primary. For sharded quants, the primary
    /// shard (first by sorted filename) is the one whose filename is stored
    /// in the model card's `QuantInfo.file`.
    pub is_primary_shard: bool,
}

/// Determine whether `filename` is the primary shard of a (possibly sharded)
/// quant, given the full set of blob rfilenames from the HF API.
///
/// - Single-file quants (no `/` in the filename) are always primary.
/// - For sharded quants (filename contains `/`), the primary shard is the
///   first file by sorted order within the same directory prefix.
///
/// This is a pure function so it can be unit-tested without network calls.
fn determine_primary_shard(filename: &str, blobs: &HashMap<String, BlobInfo>) -> bool {
    // Single-file quant (no directory) is always primary
    if !filename.contains('/') {
        return true;
    }
    // Extract directory prefix (everything before the last '/')
    let dir_prefix = filename.rsplit_once('/').unwrap().0;
    // Find all blobs in the same directory, sort, and check if current is first
    let mut siblings: Vec<&String> = blobs
        .keys()
        .filter(|k| k.starts_with(&format!("{}/", dir_prefix)))
        .collect();
    siblings.sort();
    siblings.first().map(|f| *f == filename).unwrap_or(true)
}

/// Run the post-pull verification phase for a pull job.
///
/// Hashes the file at `dest_path` directly (file is already pulled there),
/// then:
/// - Pass: file stays in place. Returns `passed = true`.
/// - Fail / hash error: delete `dest_path` so no corrupt data lingers.
///   Returns `passed = false`.
///
/// `None` upstream hash is treated as a pass (HF had no LFS SHA to compare).
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_verification(
    pull_jobs: Arc<tokio::sync::RwLock<HashMap<String, PullJob>>>,
    _db_dir: Option<PathBuf>,
    pull_queue: Option<Arc<PullQueueService>>,
    job_id: String,
    repo_id: String,
    filename: String,
    _quant_hint: Option<String>,
    dest_path: PathBuf,
    bytes: u64,
) -> VerificationOutcome {
    // Step 1: fetch upstream blob metadata (best-effort). Reused below to
    // determine whether this file is the primary shard of a sharded quant,
    // avoiding a redundant API call.
    let blobs_result = crate::models::pull::fetch_blob_metadata(&repo_id).await;
    let expected_sha: Option<String> = blobs_result
        .as_ref()
        .ok()
        .and_then(|blobs| blobs.get(&filename).and_then(|b| b.lfs_sha256.clone()));
    if let Err(e) = &blobs_result {
        tracing::warn!(job_id = %job_id, repo = %repo_id, error = %e,
            "Failed to fetch HF blob metadata for verification");
    }
    let is_primary_shard = match blobs_result.as_ref() {
        Ok(blobs) => determine_primary_shard(&filename, blobs),
        Err(_) => true, // fail-safe: default to primary for single-file quants
    };

    // Step 2: transition to Verifying.
    {
        let mut jobs = pull_jobs.write().await;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Verifying;
            job.verify_bytes_hashed = 0;
            job.verify_total_bytes = Some(bytes);
            tracing::info!(job_id = %job_id, "Job transitioned to Verifying");
        }
    }

    // Update DB queue item to "verifying" so Downloads Center shows progress.
    if let Some(ref svc) = pull_queue {
        let _ = svc.update_status(
            &job_id,
            "verifying",
            bytes as i64,
            Some(bytes as i64),
            None,
            None,
        );
    }

    // Step 3: hash the cached file in a blocking thread.
    // cached_path is an hf-hub snapshot symlink → blob; the OS follows it
    // automatically so we hash the real blob content without resolving manually.
    let progress = Arc::new(AtomicU64::new(0));
    let poll_progress = Arc::clone(&progress);
    let poll_jobs = Arc::clone(&pull_jobs);
    let poll_job_id = job_id.clone();
    let poll_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let hashed = poll_progress.load(Ordering::Relaxed);
            let mut jobs = poll_jobs.write().await;
            let Some(job) = jobs.get_mut(&poll_job_id) else {
                break;
            };
            if !matches!(job.status, PullJobStatus::Verifying) {
                break;
            }
            job.verify_bytes_hashed = hashed;
        }
    });

    let hash_progress = Arc::clone(&progress);
    let hash_src = dest_path.clone(); // hash the destination file directly
    let hash_expected = expected_sha.clone();

    let blocking_result = tokio::task::spawn_blocking(move || -> (Option<bool>, Option<String>) {
        let actual = match crate::models::verify::sha256_file(&hash_src, |n| {
            hash_progress.store(n, Ordering::Relaxed);
        }) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(error = %e, path = %hash_src.display(), "Hashing failed");
                None
            }
        };

        match (hash_expected.as_deref(), actual.as_deref()) {
            (None, _) => (None, None),
            (Some(_), None) => (
                Some(false),
                Some("hash error: failed to read file".to_string()),
            ),
            (Some(exp), Some(act)) if act.eq_ignore_ascii_case(exp) => (Some(true), None),
            (Some(exp), Some(act)) => (
                Some(false),
                Some(format!(
                    "hash mismatch: expected {} got {}",
                    exp.chars().take(10).collect::<String>(),
                    act.chars().take(10).collect::<String>()
                )),
            ),
        }
    })
    .await;

    poll_handle.abort();

    let (ok, err) = blocking_result.unwrap_or_else(|e| {
        tracing::error!(error = %e, "Verification blocking task panicked");
        (
            Some(false),
            Some(format!("verification task panicked: {}", e)),
        )
    });

    let passed = ok != Some(false);

    if passed {
        // Verification passed — file is already at dest_path, nothing to move.
        let mut jobs = pull_jobs.write().await;
        let map_ptr = &*jobs as *const _;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.verify_bytes_hashed = bytes;
            job.verified_ok = ok;
            job.verify_error = None;
            job.completed_at = Some(Instant::now());
            job.status = crate::proxy::pull_jobs::PullJobStatus::Completed;
            tracing::info!(
                job_id = %job_id,
                verified_ok = ?ok,
                bytes_pulled = job.bytes_pulled,
                map_addr = ?map_ptr,
                "Job completed"
            );
        }
        VerificationOutcome {
            passed: true,
            expected_sha,
            ok,
            err,
            is_primary_shard,
        }
    } else {
        // Verification failed — delete the corrupt/mismatched file so it
        // cannot be mistaken for a good pull on the next attempt.
        tokio::fs::remove_file(&dest_path).await.ok();
        tracing::error!(job_id = %job_id, error = ?err, "Verification failed — file deleted");

        let mut jobs = pull_jobs.write().await;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.verify_bytes_hashed = bytes;
            job.verified_ok = ok;
            job.verify_error = err.clone();
            job.error = err.clone();
            job.completed_at = Some(Instant::now());
            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
            tracing::error!(job_id = %job_id, "Job failed after verification");
        }
        VerificationOutcome {
            passed: false,
            expected_sha,
            ok,
            err,
            is_primary_shard,
        }
    }
}

/// Inner implementation of post-pull setup, accepting an explicit config.
/// Separated for testability — `setup_model_after_pull` delegates to this.
pub(crate) async fn _setup_model_after_pull_with_config(
    configs_dir: &std::path::Path,
    model_configs: &mut std::collections::HashMap<String, ModelConfig>,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
    gguf_metadata: Option<&GgufMetadata>,
) -> Option<String> {
    let repo_slug = crate::models::card_slug(repo_id);
    let card_path = configs_dir.join(format!("{}.toml", repo_slug));

    // Load existing or build a new card
    let mut card = ModelCard::load(&card_path).unwrap_or_else(|_| ModelCard {
        model: crate::models::card::ModelMeta {
            name: repo_id.to_string(),
            source: repo_id.to_string(),
            default_context_length: None,
            default_gpu_layers: None,
            default_gpu_device: None,
        },
        sampling: std::collections::HashMap::new(),
        quants: std::collections::HashMap::new(),
    });

    // Try community card for sampling presets and context defaults (best-effort, no network in tests).
    // We intentionally do NOT overwrite card.model.name from the community card — community cards
    // often have the GGUF suffix stripped (e.g. "OmniCoder-9B" instead of "OmniCoder-9B-GGUF"),
    // which loses information. The name is derived from the repo_id above and kept as-is.
    if let Some(community) = fetch_community_card(repo_id).await {
        for (k, v) in community.sampling {
            card.sampling.entry(k).or_insert(v);
        }
        if card.model.default_context_length.is_none() {
            card.model.default_context_length = community.model.default_context_length;
        }
        if card.model.default_gpu_layers.is_none() {
            card.model.default_gpu_layers = community.model.default_gpu_layers;
        }
    }

    // Determine the quant key
    let quant_key = spec.quant.clone().unwrap_or_else(|| {
        infer_quant_from_filename(&spec.filename).unwrap_or_else(|| {
            // Fallback: use last component after splitting by `-` or `_`
            spec.filename
                .trim_end_matches(".gguf")
                .split(|c| ['-', '_'].contains(&c))
                .next_back()
                .unwrap_or("unknown")
                .to_string()
        })
    });

    // Determine context_length: GGUF parsed value > spec value > None
    let context_length = gguf_metadata
        .and_then(|m| m.context_length.map(|v| v as u32))
        .or(spec.context_length);

    // Get actual file size from disk
    let size_bytes = std::fs::metadata(dest_dir.join(&spec.filename))
        .ok()
        .map(|m| m.len());

    // Insert/update quant entry in card. Detect mmproj files by filename so
    // they get tagged with `kind = Mmproj` and tracked separately from real
    // model quants.
    card.quants.insert(
        quant_key.clone(),
        QuantInfo {
            file: spec.filename.clone(),
            kind: QuantKind::from_filename(&spec.filename),
            size_bytes,
            context_length,
        },
    );

    // Find an existing model entry for this repo (if any), regardless of
    // its key format. This prevents creating duplicate model entries when
    // pulling additional quants for a model that's already in the config.
    // Matching is by the `model` field rather than the key, so user-renamed
    // entries are preserved.
    let existing_key: Option<String> = model_configs
        .iter()
        .find(|(_, m)| m.model.as_deref() == Some(repo_id))
        .map(|(k, _)| k.clone());

    // For mmproj files: if the parent model config already exists, turn on
    // vision by wiring the mmproj filename and adding "image" to the input
    // modalities. Downloading an mmproj is an explicit user choice, so
    // enabling vision automatically saves an extra click in the editor.
    //
    // MTP files follow the same stub-then-wire pattern (see below). Unlike
    // mmproj, MTP does NOT modify input modalities — it's a draft model for
    // speculative decoding and does not change what the model can accept.
    let file_kind = QuantKind::from_filename(&spec.filename);
    let is_mmproj = matches!(file_kind, QuantKind::Mmproj);
    let is_mtp = matches!(file_kind, QuantKind::Mtp);
    if !is_mmproj && !is_mtp {
        // Fetch pipeline_tag from HF to infer modalities (best-effort).
        let modalities = match crate::models::pull::fetch_model_pipeline_tag(repo_id).await {
            Ok(pipeline_tag) => {
                crate::models::pull::infer_modalities_from_pipeline(pipeline_tag.as_deref())
            }
            Err(e) => {
                tracing::debug!(repo = %repo_id, error = %e, "Failed to fetch pipeline_tag for modalities inference");
                None
            }
        };

        // Generate display name from HF repo name (e.g., "Unsloth: Qwen3.5 35B A3B").
        let display_name = generate_display_name(repo_id);

        // Reuse the existing model key if we found one, otherwise create a
        // new entry keyed by the bare repo slug (no per-quant suffix).
        let model_key = existing_key.unwrap_or_else(|| repo_slug.to_lowercase());
        let entry = model_configs
            .entry(model_key.clone())
            .or_insert_with(|| ModelConfig {
                backend: "llama_cpp".to_string(),
                gpu_variant: None,
                gpu_device: None,
                model: Some(repo_id.to_string()),
                quant: Some(quant_key.clone()),
                mmproj: None,
                mtp_model: None,
                context_length,
                num_parallel: default_num_parallel(),
                kv_unified: true,
                enabled: true,
                args: vec![],
                sampling: None,
                port: None,
                health_check: None,
                profile: None,
                api_name: Some(repo_id.to_string()),
                gpu_layers: None,
                cache_type_k: None,
                cache_type_v: None,
                hf_format: None,
                hf_base_model: None,
                hf_pipeline_tag: None,
                hf_total_params: None,
                hf_active_params: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                hf_last_modified: None,
                quants: std::collections::BTreeMap::new(),
                modalities: modalities.clone(),
                display_name: Some(display_name.clone()),
                db_id: None, // will be set after reload_model_configs()
                spec_decoding: Default::default(),
            });

        // Promote a stub entry (created by a prior mmproj-first pull) into a
        // real, enabled model once the main quant arrives. Without this, the
        // stub's `quant=None, enabled=false` would persist even though the
        // model file is now on disk.
        if entry.quant.is_none() {
            entry.quant = Some(quant_key);
            entry.enabled = true;
        }
        if entry.context_length.is_none() {
            entry.context_length = context_length;
        }
        if entry.modalities.is_none() {
            entry.modalities = modalities;
        }
        if entry.display_name.is_none() {
            entry.display_name = Some(display_name);
        }

        // Populate hf_* informational fields from GGUF metadata
        if let Some(meta) = gguf_metadata {
            entry.hf_architecture_type = meta.architecture.clone();
            entry.hf_context_length = meta.context_length.map(|v| v as u32);
            entry.hf_num_layers = meta.block_count.map(|v| v as u32);
        }

        // Save card (best-effort — pull is already marked Completed)
        let _ = std::fs::create_dir_all(configs_dir);
        let _ = card.save(&card_path);

        return Some(model_key);
    }

    // For mmproj / MTP, still save the card.
    let _ = std::fs::create_dir_all(configs_dir);
    let _ = card.save(&card_path);

    let key = match existing_key {
        Some(k) => k,
        None => {
            // mmproj / MTP pulled before any main quant. Create a stub entry
            // with enabled=false so the file is tracked; the next main-quant
            // pull will find this entry by `model == repo_id` and flip
            // enabled to true. Without the stub, the auxiliary file sits on
            // disk invisible to the editor.
            let display_name = generate_display_name(repo_id);
            let stub_key = repo_slug.to_lowercase();
            model_configs
                .entry(stub_key.clone())
                .or_insert_with(|| ModelConfig {
                    backend: "llama_cpp".to_string(),
                    gpu_variant: None,
                    gpu_device: None,
                    model: Some(repo_id.to_string()),
                    quant: None,
                    mmproj: None,
                    mtp_model: None,
                    context_length: None,
                    num_parallel: default_num_parallel(),
                    kv_unified: true,
                    enabled: false,
                    args: vec![],
                    sampling: None,
                    port: None,
                    health_check: None,
                    profile: None,
                    api_name: Some(repo_id.to_string()),
                    gpu_layers: None,
                    cache_type_k: None,
                    cache_type_v: None,
                    hf_format: None,
                    hf_base_model: None,
                    hf_pipeline_tag: None,
                    hf_total_params: None,
                    hf_active_params: None,
                    hf_architecture_type: None,
                    hf_context_length: None,
                    hf_num_layers: None,
                    hf_last_modified: None,
                    quants: std::collections::BTreeMap::new(),
                    modalities: None,
                    display_name: Some(display_name),
                    db_id: None,
                    spec_decoding: Default::default(),
                });
            stub_key
        }
    };

    // Wire mmproj + image modality onto the entry (stub or existing parent).
    if is_mmproj {
        if let Some(mc) = model_configs.get_mut(&key) {
            mc.mmproj = Some(spec.filename.clone());
            let modalities = mc.modalities.get_or_insert_with(Default::default);
            if !modalities
                .input
                .iter()
                .any(|m| m.eq_ignore_ascii_case("text"))
            {
                modalities.input.push("text".to_string());
            }
            if !modalities
                .input
                .iter()
                .any(|m| m.eq_ignore_ascii_case("image"))
            {
                modalities.input.push("image".to_string());
            }
            if modalities.output.is_empty() {
                modalities.output.push("text".to_string());
            }
        }
    } else if is_mtp {
        // Wire mtp_model onto the entry (stub or existing parent). MTP
        // affects speculative decoding only, so input modalities are left
        // untouched. We do NOT auto-enable `draft-mtp` in
        // `spec_decoding.spec_types` — that's a runtime decision that may
        // depend on hardware and the user can flip it from the editor.
        if let Some(mc) = model_configs.get_mut(&key) {
            mc.mtp_model = Some(spec.filename.clone());
        }
    }
    Some(key)
}

/// Post-pull: auto-create model card and config entries.
///
/// Called after a quant pull completes. Updates the model card, saves config to
/// disk, and — critically — also inserts the new model entry into the live
/// `ProxyState.config` so it appears immediately in the models list without a restart.
///
/// Returns the integer model_configs id when the row was (re)saved, so the
/// caller can persist related rows (model_files) against it without a second
/// lookup that could miss due to case or key drift.
pub(crate) async fn setup_model_after_pull(
    state: Arc<ProxyState>,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
    gguf_metadata: Option<GgufMetadata>,
) -> Option<i64> {
    let _permit = state.config_write_semaphore.acquire().await.ok()?;
    // Clone needed data from config before awaiting — don't hold the read guard
    // across an awaited call to avoid blocking other writers/readers unnecessarily.
    let configs_dir = match state.config.read().await.configs_dir() {
        Ok(d) => d,
        Err(_) => return None,
    };
    // Config read guard is dropped here automatically when it goes out of scope.

    let mut model_configs = state.model_configs.write().await;
    let model_key = _setup_model_after_pull_with_config(
        &configs_dir,
        &mut model_configs,
        repo_id,
        spec,
        dest_dir,
        gguf_metadata.as_ref(),
    )
    .await;

    let mut saved_id: Option<i64> = None;
    if let Some(key) = model_key {
        if let Some(mgr) = state.model_mgr() {
            let save_result = model_configs
                .get(&key)
                .map(|mc| mgr.save_model_config(&key, mc));
            match save_result {
                Some(Ok(id)) => {
                    saved_id = Some(id);
                    if let Some(mc_mut) = model_configs.get_mut(&key) {
                        mc_mut.db_id = Some(id);
                    }
                }
                Some(Err(e)) => {
                    tracing::error!(key = %key, error = %e, "Failed to save model config to DB after pull");
                }
                None => {}
            }
        }
    }
    saved_id
    // _guard dropped here, releasing the lock
    // config write guard also dropped here, making the new model entry visible immediately
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::pull::BlobInfo;

    fn make_blob(size: i64, sha: Option<&str>) -> BlobInfo {
        BlobInfo {
            filename: String::new(),
            blob_id: None,
            size: Some(size),
            lfs_sha256: sha.map(String::from),
        }
    }

    #[test]
    fn test_determine_primary_shard() {
        let mut blobs = HashMap::new();
        // Sharded quant: 3 shards in UD-Q4_K_XL/
        blobs.insert(
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf".to_string(),
            make_blob(100, Some("sha1")),
        );
        blobs.insert(
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00002-of-00003.gguf".to_string(),
            make_blob(200, Some("sha2")),
        );
        blobs.insert(
            "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00003-of-00003.gguf".to_string(),
            make_blob(300, Some("sha3")),
        );
        // Single-file quant (no directory)
        blobs.insert(
            "Laguna-S-2.1-Q4_K_M.gguf".to_string(),
            make_blob(500, Some("sha4")),
        );

        // Single-file quant (no '/') is always primary
        assert!(
            determine_primary_shard("Laguna-S-2.1-Q4_K_M.gguf", &blobs),
            "single-file quant should be primary"
        );

        // First shard (by sorted filename order) is primary
        assert!(
            determine_primary_shard(
                "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00001-of-00003.gguf",
                &blobs,
            ),
            "first shard should be primary"
        );

        // Non-primary shards are NOT primary
        assert!(
            !determine_primary_shard(
                "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00002-of-00003.gguf",
                &blobs,
            ),
            "second shard should not be primary"
        );
        assert!(
            !determine_primary_shard(
                "UD-Q4_K_XL/Laguna-S-2.1-UD-Q4_K_XL-00003-of-00003.gguf",
                &blobs,
            ),
            "third shard should not be primary"
        );
    }
}
