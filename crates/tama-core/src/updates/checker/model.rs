use super::UpdateChecker;
#[cfg(feature = "web-ui")]
use super::UpdateEvent;
use crate::db;
use crate::db::queries::get_model_pull;
use crate::models::pull;
use crate::models::pull::BlobInfo;

impl UpdateChecker {
    /// Check a single model for updates.
    /// Uses the same two-tier strategy as `models::update::check_for_updates`:
    /// (1) commit SHA quick check, then (2) per-file LFS hash comparison so
    /// that non-GGUF repo changes don't trigger false positives.
    pub async fn check_model(
        &self,
        config_dir: &std::path::Path,
        model_id: i64,
        repo_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Frontend DTO uses config_key format "model-{id}"
        #[cfg(feature = "web-ui")]
        let model_config_key = format!("model-{}", model_id);
        #[cfg(feature = "web-ui")]
        self.emit(UpdateEvent::CheckStarted {
            item_type: "model".to_string(),
            item_id: model_config_key.clone(),
            variant: None,
        });

        let repo_id = match repo_id {
            Some(id) if !id.is_empty() => id,
            _ => {
                self.save_check_result(
                    config_dir,
                    "model",
                    &model_id.to_string(),
                    None,
                    None,
                    false,
                    "unknown",
                    Some("Model has no source repo configured"),
                    None,
                )
                .await?;

                #[cfg(feature = "web-ui")]
                self.emit(UpdateEvent::CheckError {
                    item_type: "model".to_string(),
                    item_id: model_config_key,
                    variant: None,
                    error: "Model has no source repo configured".to_string(),
                });

                return Ok(());
            }
        };

        // Phase 1 — SYNC: read DB state (no .await)
        let db_state = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let repo_id = repo_id.to_string();
            move || -> anyhow::Result<Option<(db::queries::ModelPullRecord, Vec<db::queries::ModelFileRecord>)>> {
                let open = db::open(&config_dir)?;
                let model_record =
                    match db::queries::get_model_config_by_repo_id(&open.conn, &repo_id)? {
                        Some(r) => r,
                        None => return Ok(None),
                    };
                let pull_record = get_model_pull(&open.conn, model_record.id)?;
                let file_records = db::queries::get_model_files(&open.conn, model_record.id)?;
                Ok(pull_record.map(|pr| (pr, file_records)))
            }
        })
        .await??;

        // Handle no prior record
        let Some((pull_record, file_records)) = db_state else {
            let save_result = self
                .save_check_result(
                    config_dir,
                    "model",
                    &model_id.to_string(),
                    None,
                    None,
                    false,
                    "no_prior_record",
                    None,
                    None,
                )
                .await;

            #[cfg(feature = "web-ui")]
            if save_result.is_ok() {
                let dto = serde_json::json!({
                    "item_type": "model",
                    "item_id": model_config_key,
                    "variant": null,
                    "current_version": null,
                    "latest_version": null,
                    "update_available": false,
                    "status": "no_prior_record",
                    "error_message": null,
                    "checked_at": chrono::Utc::now().timestamp(),
                    "details_json": null,
                });
                self.emit(UpdateEvent::CheckCompleted {
                    item_type: "model".to_string(),
                    item_id: model_config_key.clone(),
                    variant: None,
                    dto,
                });
            } else {
                let save_err = save_result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                self.emit(UpdateEvent::CheckError {
                    item_type: "model".to_string(),
                    item_id: model_config_key,
                    variant: None,
                    error: format!("Failed to save check result: {}", save_err),
                });
            }
            save_result?;
            return Ok(());
        };

        // Phase 2 — ASYNC: fetch remote state (conn not referenced after this point)
        // Check cache before making network call to list_gguf_files
        let remote_listing = match self.gguf_listing_cache.get(repo_id, None).await {
            Some((cached_sha, cached_files)) => {
                tracing::debug!("GGUF listing cache hit for '{}'", repo_id);
                // Use cached file list — no extra fetch needed; LFS hashes don't change for the same commit
                crate::models::pull::RepoGgufListing {
                    repo_id: repo_id.to_string(),
                    commit_sha: cached_sha,
                    files: cached_files,
                }
            }
            None => {
                let listing = pull::list_gguf_files(repo_id).await?;
                // Only insert into cache on a fresh fetch (cache-miss path).
                // Cache-hits should NOT rewrite the entry timestamp, which would
                // keep stale listings alive indefinitely when checks happen frequently.
                self.gguf_listing_cache
                    .insert(
                        repo_id.to_string(),
                        listing.commit_sha.clone(),
                        listing.files.clone(),
                        None,
                    )
                    .await;
                listing
            }
        };

        // Tier 1 — quick check: commit SHA match?
        if remote_listing.commit_sha == pull_record.commit_sha {
            let save_result = self
                .save_check_result(
                    config_dir,
                    "model",
                    &model_id.to_string(),
                    Some(&pull_record.commit_sha),
                    Some(&remote_listing.commit_sha),
                    false,
                    "up_to_date",
                    None,
                    None,
                )
                .await;

            #[cfg(feature = "web-ui")]
            if save_result.is_ok() {
                let dto = serde_json::json!({
                    "item_type": "model",
                    "item_id": model_config_key,
                    "variant": null,
                    "current_version": &pull_record.commit_sha,
                    "latest_version": &remote_listing.commit_sha,
                    "update_available": false,
                    "status": "up_to_date",
                    "error_message": null,
                    "checked_at": chrono::Utc::now().timestamp(),
                    "details_json": null,
                });
                self.emit(UpdateEvent::CheckCompleted {
                    item_type: "model".to_string(),
                    item_id: model_config_key.clone(),
                    variant: None,
                    dto,
                });
            } else {
                let save_err = save_result
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                self.emit(UpdateEvent::CheckError {
                    item_type: "model".to_string(),
                    item_id: model_config_key,
                    variant: None,
                    error: format!("Failed to save check result: {}", save_err),
                });
            }
            save_result?;
            return Ok(());
        }

        // Tier 2 — per-file LFS hash comparison
        let resolved_repo_id = &remote_listing.repo_id;
        let remote_blobs = match pull::fetch_blob_metadata(resolved_repo_id).await {
            Ok(blobs) => blobs,
            Err(e) => {
                let save_result = self
                    .save_check_result(
                        config_dir,
                        "model",
                        &model_id.to_string(),
                        Some(&pull_record.commit_sha),
                        Some(&remote_listing.commit_sha),
                        false,
                        "error",
                        Some(&format!(
                            "Commit changed but failed to fetch file details: {e}"
                        )),
                        None,
                    )
                    .await;

                #[cfg(feature = "web-ui")]
                if save_result.is_ok() {
                    let error_msg = format!("Commit changed but failed to fetch file details: {e}");
                    let dto = serde_json::json!({
                        "item_type": "model",
                        "item_id": model_config_key,
                        "variant": null,
                        "current_version": &pull_record.commit_sha,
                        "latest_version": &remote_listing.commit_sha,
                        "update_available": false,
                        "status": "error",
                        "error_message": error_msg,
                        "checked_at": chrono::Utc::now().timestamp(),
                        "details_json": null,
                    });
                    self.emit(UpdateEvent::CheckCompleted {
                        item_type: "model".to_string(),
                        item_id: model_config_key.clone(),
                        variant: None,
                        dto,
                    });
                } else {
                    let save_err = save_result
                        .as_ref()
                        .err()
                        .map(|err| err.to_string())
                        .unwrap_or_default();
                    self.emit(UpdateEvent::CheckError {
                        item_type: "model".to_string(),
                        item_id: model_config_key,
                        variant: None,
                        error: format!("Failed to save check result: {}", save_err),
                    });
                }
                save_result?;
                return Ok(());
            }
        };

        // Phase 3 — PURE: per-quant comparison (testable, no I/O)
        // Build a map of remote blobs by filename for quick lookup
        let remote_map: std::collections::HashMap<&str, &BlobInfo> =
            remote_blobs.iter().map(|(k, v)| (k.as_str(), v)).collect();

        // Track which local filenames we've seen for new-quant detection
        let mut local_filenames: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut quants_array: Vec<serde_json::Value> = Vec::new();
        let mut any_update_available = false;

        // Iterate each local file record and compare against remote
        for local in &file_records {
            local_filenames.insert(local.filename.as_str());

            match remote_map.get(local.filename.as_str()) {
                Some(remote) => {
                    let current_hash = local.lfs_oid.clone();
                    let latest_hash = remote.lfs_sha256.clone();

                    let (update_available, status_val) = match (&current_hash, &latest_hash) {
                        (Some(c), Some(l)) if c == l => (false, "up_to_date"),
                        (Some(_), Some(_)) => (true, "update_available"),
                        (None, _) => (false, "no_hash"),
                        (Some(_), None) => (false, "removed_from_remote"),
                    };

                    if update_available {
                        any_update_available = true;
                    }

                    quants_array.push(serde_json::json!({
                        "quant_name": local.quant,
                        "filename": local.filename,
                        "current_hash": current_hash,
                        "latest_hash": latest_hash,
                        "update_available": update_available,
                        "status": status_val,
                    }));
                }
                None => {
                    // File no longer exists on remote
                    quants_array.push(serde_json::json!({
                        "quant_name": local.quant,
                        "filename": local.filename,
                        "current_hash": local.lfs_oid.clone(),
                        "latest_hash": null,
                        "update_available": false,
                        "status": "removed_from_remote",
                    }));
                }
            }
        }

        // Check for new quants: remote files not in local records
        for (filename, remote) in &remote_blobs {
            if !local_filenames.contains(filename.as_str()) {
                any_update_available = true;
                quants_array.push(serde_json::json!({
                    "quant_name": None::<String>,
                    "filename": filename,
                    "current_hash": null,
                    "latest_hash": remote.lfs_sha256.clone(),
                    "update_available": true,
                    "status": "new_quant",
                }));
            }
        }

        // Determine overall status from quant-level results
        let (update_available, status) = if any_update_available {
            (true, "update_available")
        } else {
            (false, "up_to_date")
        };

        let details_json = serde_json::json!({
            "repo_id": remote_listing.repo_id,
            "commit_sha": remote_listing.commit_sha,
            "quants": quants_array,
        })
        .to_string();

        let save_result = self
            .save_check_result(
                config_dir,
                "model",
                &model_id.to_string(),
                Some(&pull_record.commit_sha),
                Some(&remote_listing.commit_sha),
                update_available,
                status,
                None,
                Some(&details_json),
            )
            .await;

        #[cfg(feature = "web-ui")]
        if save_result.is_ok() {
            let dto = serde_json::json!({
                "item_type": "model",
                "item_id": model_config_key,
                "variant": null,
                "current_version": &pull_record.commit_sha,
                "latest_version": &remote_listing.commit_sha,
                "update_available": update_available,
                "status": status,
                "error_message": null,
                "checked_at": chrono::Utc::now().timestamp(),
                "details_json": &details_json,
            });
            self.emit(UpdateEvent::CheckCompleted {
                item_type: "model".to_string(),
                item_id: model_config_key.clone(),
                variant: None,
                dto,
            });
        } else {
            let save_err = save_result
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            self.emit(UpdateEvent::CheckError {
                item_type: "model".to_string(),
                item_id: model_config_key,
                variant: None,
                error: format!("Failed to save check result: {}", save_err),
            });
        }
        save_result
    }
}
