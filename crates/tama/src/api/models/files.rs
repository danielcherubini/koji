use crate::api::error::{error_body, error_response};
use crate::api::helpers::shared_repository;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use tama_core::proxy::ProxyState;

use super::resolve_model_id;
use crate::api::load_config_from_state;
use crate::web_types::WebState;
use tama_core::db::queries::ModelFileRecord;

/// Serialize a `ModelFileRecord` into the same shape used by the enriched
/// quants response so refresh/verify callers get data identical to a GET.
fn file_record_json(rec: &ModelFileRecord) -> serde_json::Value {
    serde_json::json!({
        "filename": rec.filename,
        "quant": rec.quant,
        "lfs_oid": rec.lfs_oid,
        "size_bytes": rec.size_bytes,
        "downloaded_at": rec.downloaded_at,
        "last_verified_at": rec.last_verified_at,
        "verified_ok": rec.verified_ok,
        "verify_error": rec.verify_error,
    })
}

// ── Refresh / Verify ──────────────────────────────────────────────────────────

/// POST /tama/v1/models/:id/refresh — re-query HuggingFace for the current commit
/// SHA and per-file LFS hashes / sizes, and write them into the local DB.
///
/// Structured to keep `rusqlite::Connection` off `.await` points:
///   1. Load config (async, handles its own spawn_blocking)
///   2. `spawn_blocking` — resolve repo_id from DB
///   3. `.await` — fetch from HF
///   4. `spawn_blocking` — open DB, upsert pull + files, read back
pub async fn refresh_model_metadata(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    // Load config first (async, handles its own spawn_blocking)
    let (cfg, _config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };
    let repo_handle_for_write = repo_handle.clone();

    // Step 1: resolve model_id (from id_str) and repo_id (DB operations on blocking pool).
    let resolved = tokio::task::spawn_blocking(move || {
        let repo = repo_handle.lock().unwrap();
        let model_id = resolve_model_id(&id_str, &repo)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    error_body(e.to_string(), Some("ValidationError")),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body("Model not found", Some("NotFoundError")),
                )
            })?;
        let record = repo
            .get_model_config(model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body("Model not found", Some("NotFoundError")),
                )
            })?;
        let models_dir = cfg.models_dir().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_body(e.to_string(), None),
            )
        })?;
        Ok::<_, (StatusCode, serde_json::Value)>((model_id, record.repo_id, models_dir))
    })
    .await;
    let (model_id, repo_id, _models_dir) = match resolved {
        Ok(Ok(x)) => x,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    // Step 2: async HF fetches (no DB handle held).
    let listing = match tama_core::models::pull::list_gguf_files(&repo_id).await {
        Ok(l) => l,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                format!("HuggingFace listing failed: {}", e),
                None,
            )
        }
    };
    let blobs = match tama_core::models::pull::fetch_blob_metadata(&listing.repo_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("fetch_blob_metadata failed for {}: {}", listing.repo_id, e);
            std::collections::HashMap::new()
        }
    };

    // Step 3: DB writes (blocking pool, fresh connection).
    // Only update metadata for files that already exist locally — do NOT create
    // new entries for quants the user never pulled. This prevents the
    // "Check all for updates" button from polluting the model_files table.
    let repo_id_for_db = repo_id.clone();
    let commit_sha = listing.commit_sha.clone();
    let files = listing.files.clone();
    let write = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = repo_handle_for_write.lock().unwrap();
        repo.upsert_pull(model_id, &repo_id_for_db, &commit_sha)?;

        // Build a set of filenames already tracked locally.
        let local_files = repo.get_files(model_id)?;
        let local_filenames: std::collections::HashSet<&str> =
            local_files.iter().map(|f| f.filename.as_str()).collect();

        // Only upsert metadata for files that already exist in the local DB.
        for file in &files {
            if !local_filenames.contains(file.filename.as_str()) {
                // Skip remote-only files — don't pollute the DB.
                continue;
            }
            let blob = blobs.get(&file.filename);
            repo.upsert_file(
                model_id,
                &repo_id_for_db,
                &file.filename,
                file.quant.as_deref(),
                blob.and_then(|b| b.lfs_sha256.as_deref()),
                blob.and_then(|b| b.size),
            )?;
        }
        let files_out = repo.get_files(model_id)?;
        let pull_out = repo.get_pull(model_id)?;
        Ok((pull_out, files_out))
    })
    .await;

    match write {
        Ok(Ok((pull, files))) => {
            let files_json: Vec<_> = files.iter().map(file_record_json).collect();
            Json(serde_json::json!({
                "ok": true,
                "id": model_id,
                "repo_id": repo_id,
                "repo_commit_sha": pull.as_ref().map(|p| p.commit_sha.clone()),
                "repo_pulled_at": pull.as_ref().map(|p| p.pulled_at.clone()),
                "files": files_json,
            }))
            .into_response()
        }
        Ok(Err(e)) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("DB write failed: {}", e),
            None,
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// POST /tama/v1/models/:id/verify — recompute SHA-256 for every tracked file of
/// this model and compare against the stored LFS hash, persisting the result.
///
/// Sequential, CPU-bound, potentially multi-minute for large GGUFs. Runs on
/// the blocking threadpool. Per-file progress events are NOT streamed here;
/// the wizard already streams them during pulls.
pub async fn verify_model_files(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    // Load config first (async, handles its own spawn_blocking)
    let (cfg, _config_dir) = match load_config_from_state(&state).await {
        Ok(x) => x,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    let repo_handle = match shared_repository(&web_state) {
        Ok(h) => h,
        Err(resp) => return resp,
    };
    let repo_handle_for_write = repo_handle.clone();

    let resolved = tokio::task::spawn_blocking(move || {
        let repo = repo_handle.lock().unwrap();
        let model_id = resolve_model_id(&id_str, &repo)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    error_body(e.to_string(), Some("ValidationError")),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body("Model not found", Some("NotFoundError")),
                )
            })?;
        let record = repo
            .get_model_config(model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(e.to_string(), None),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    error_body("Model not found", Some("NotFoundError")),
                )
            })?;
        let models_dir = cfg.models_dir().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                error_body(e.to_string(), None),
            )
        })?;
        Ok::<_, (StatusCode, serde_json::Value)>((model_id, record.repo_id, models_dir))
    })
    .await;
    let (model_id, repo_id, models_dir) = match resolved {
        Ok(Ok(x)) => x,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    // Model files live at <models_dir>/<repo_id>/<filename>.gguf
    let model_dir = tama_core::models::repo_path(&models_dir, &repo_id);
    let repo_id_clone = repo_id.clone();

    let task = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let repo = repo_handle_for_write.lock().unwrap();
        let results =
            tama_core::models::verify::verify_model(&repo, model_id, &repo_id_clone, &model_dir)?;
        let files = repo.get_files(model_id)?;
        Ok((results, files))
    })
    .await;

    match task {
        Ok(Ok((results, files))) => {
            let all_ok = results.iter().all(|r| r.ok != Some(false));
            let any_unknown = results.iter().any(|r| r.ok.is_none());
            let summary: Vec<_> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "filename": r.filename,
                        "ok": r.ok,
                        "error": r.error,
                    })
                })
                .collect();
            let files_json: Vec<_> = files.iter().map(file_record_json).collect();
            Json(serde_json::json!({
                "ok": all_ok,
                "any_unknown": any_unknown,
                "id": model_id,
                "repo_id": repo_id,
                "results": summary,
                "files": files_json,
            }))
            .into_response()
        }
        Ok(Err(e)) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("verify failed: {}", e),
            None,
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}
