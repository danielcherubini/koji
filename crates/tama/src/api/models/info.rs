use crate::api::error::{error_body, error_response};
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::collections::HashMap;
use std::sync::Arc;
use tama_core::gpu::ModelState;
use tama_core::proxy::ProxyState;

use crate::api::load_config_from_state;
use crate::web_types::WebState;
use sqlx::PgPool;
use tama_core::db::queries::{ModelConfigRecord, ModelFileRecord};
use tama_core::installations::InstallationOption;

/// Build the list of available backend options by querying installed variants from the DB.
async fn build_backend_options(
    _cfg: &tama_core::config::Config,
    config_dir: &std::path::Path,
) -> Vec<InstallationOption> {
    let config_dir = config_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mgr = tama_core::installations::InstallationManager::open(&config_dir).ok()?;
        mgr.available_installations().ok()
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Resolve a model identifier string (integer DB id or config_key) to the integer DB id.
pub(crate) async fn resolve_db_id(pool: &PgPool, id_str: &str) -> anyhow::Result<Option<i64>> {
    // Try parsing as integer id first
    if let Ok(id) = id_str.parse::<i64>() {
        return Ok(Some(id));
    }
    // Otherwise treat as config_key (double-dash format) → convert to repo_id and look up
    let repo_id = tama_core::models::config_key_to_repo_id(id_str);
    let record = tama_core::db::queries::get_model_config_by_repo_id(pool, &repo_id).await?;
    Ok(record.map(|r| r.id))
}

/// Resolve `id_str` (integer id or config_key) to the model id and load its
/// config record.
///
/// Error mapping matches the historical per-handler chains:
/// unresolvable id → 400 ValidationError, unknown id → 404 NotFoundError.
pub(crate) async fn resolve_model_record(
    pool: &PgPool,
    id_str: &str,
) -> Result<(i64, ModelConfigRecord), (StatusCode, serde_json::Value)> {
    let model_id = resolve_db_id(pool, id_str)
        .await
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
    let record = tama_core::db::queries::get_model_config(pool, model_id)
        .await
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
    Ok((model_id, record))
}

/// Per-file DB metadata enrichment loaded from the `model_files` / `model_pulls`
/// SQLite tables. Layered onto the API response so the frontend can render
/// verification state, LFS hashes, and repo-level commit SHA without changing
/// the TOML schema.
#[derive(Debug, Default, Clone)]
struct RepoDbMeta {
    commit_sha: Option<String>,
    pulled_at: Option<String>,
    /// Keyed by filename (matches `QuantEntry.file`), not by quant name.
    files: std::collections::HashMap<String, ModelFileRecord>,
}

/// Load per-repo DB metadata for a model using the Postgres pool.
async fn load_repo_db_meta(pool: &PgPool, model_id: i64) -> RepoDbMeta {
    let mut meta = RepoDbMeta::default();
    // Pull metadata (commit SHA + pull timestamp)
    if let Ok(Some(pull)) = tama_core::db::queries::get_model_pull(pool, model_id).await {
        meta.commit_sha = Some(pull.commit_sha);
        meta.pulled_at = Some(pull.pulled_at);
    }
    if let Ok(files) = tama_core::db::queries::get_model_files(pool, model_id).await {
        for f in files {
            meta.files.insert(f.filename.clone(), f);
        }
    }
    meta
}

/// Build the full JSON for a model config entry, including all unified fields.
///
/// When `db_meta` is provided, each quant entry is enriched with its stored
/// LFS hash, DB-tracked size, and verification status, and the repo-level
/// commit SHA / last-pulled timestamp is surfaced at the top of the entry.
fn model_entry_json(
    id: i64,
    record: &ModelConfigRecord,
    m: &tama_core::config::ModelConfig,
    _configs_dir: &std::path::Path,
    db_meta: Option<&RepoDbMeta>,
    state: Option<ModelState>,
) -> serde_json::Value {
    // Build a per-quant JSON map, layering DB metadata onto each entry by filename.
    let quants_json: serde_json::Map<String, serde_json::Value> = m
        .quants
        .iter()
        .map(|(name, q)| {
            let mut entry = serde_json::json!({
                "file": q.file,
                "kind": q.kind,
                "size_bytes": q.size_bytes,
                "context_length": q.context_length,
            });
            if let Some(meta) = db_meta.and_then(|dm| dm.files.get(&q.file)) {
                entry["lfs_oid"] = meta.lfs_oid.clone().into();
                entry["db_size_bytes"] = meta.size_bytes.into();
                entry["last_verified_at"] = meta.last_verified_at.clone().into();
                entry["verified_ok"] = meta.verified_ok.into();
                entry["verify_error"] = meta.verify_error.clone().into();
            }
            (name.clone(), entry)
        })
        .collect();

    let mut val = serde_json::json!({
        "id": id,
        "state": state.as_ref().map_or("idle", ModelState::as_str),
        "repo_id": record.repo_id,
        "backend": record.backend,
        "gpu_variant": record.gpu_variant,
        "gpu_device": record.gpu_device,
        "model": m.model,
        "quant": m.quant,
        "mmproj": m.mmproj,
        "mtp_model": m.mtp_model,
        "args": m.args,
        "sampling": m.sampling,
        "enabled": record.enabled,
        "context_length": record.context_length,
        "num_parallel": record.num_parallel,
        "port": record.port,
        "api_name": record.api_name,
        "display_name": record.display_name,
        "kv_unified": record.kv_unified,
        "gpu_layers": record.gpu_layers,
        "cache_type_k": record.cache_type_k,
        "cache_type_v": record.cache_type_v,
        "hf_context_length": record.hf_context_length,
        "hf_format": record.hf_format,
        "hf_architecture_type": record.hf_architecture_type,
        "hf_base_model": record.hf_base_model,
        "quants": quants_json,
        "modalities": m.modalities,
        "reasoningLevels": m.reasoning_levels,
        "spec_decoding": serde_json::to_value(&m.spec_decoding).unwrap_or_default(),
        "vllm": serde_json::to_value(&m.vllm).unwrap_or_default(),
        "n_batch": m.n_batch,
        "n_ubatch": m.n_ubatch,
        "capabilities": serde_json::to_value(tama_core::models::model_capabilities(
            m,
            None, // list-time: GGUF not parsed, use heuristics only
        ))
        .unwrap_or_default(),
    });

    if let Some(meta) = db_meta {
        val["repo_commit_sha"] = meta.commit_sha.clone().into();
        val["repo_pulled_at"] = meta.pulled_at.clone().into();
    }

    val
}

/// GET /tama/v1/models — list all model configs plus available backends.
pub async fn list_models(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    match load_config_from_state(&state).await {
        Ok((cfg, config_dir)) => {
            let configs_dir = config_dir.join("configs");
            let backend_options = build_backend_options(&cfg, &config_dir).await;

            // Collect current runtime state for each model (idle/ready/etc.)
            // Keyed by db_id so we can look up by the integer ID from the DB record.
            let model_states: HashMap<i64, ModelState> = state
                .collect_model_state_snapshots()
                .await
                .into_iter()
                .filter_map(|s| s.db_id.map(|db_id| (db_id, s.state)))
                .collect();

            // Load models from the Postgres pool.
            let Some(pool) = web_state.db_pool.as_ref() else {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Postgres pool not available",
                    None,
                );
            };
            let configs_dir_clone = configs_dir.clone();
            let records = match tama_core::db::queries::get_all_model_configs(pool).await {
                Ok(r) => r,
                Err(e) => {
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
                }
            };
            let ids: Vec<i64> = records.iter().map(|r| r.id).collect();
            let all_files = match tama_core::db::queries::get_model_files_by_ids(pool, &ids).await {
                Ok(f) => f,
                Err(e) => {
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
                }
            };
            let files_by_model: HashMap<i64, Vec<ModelFileRecord>> =
                all_files.into_iter().fold(HashMap::new(), |mut map, file| {
                    map.entry(file.model_id).or_default().push(file);
                    map
                });

            let mut models = Vec::new();
            for record in &records {
                let mut meta = RepoDbMeta::default();
                if let Ok(Some(pull)) =
                    tama_core::db::queries::get_model_pull(pool, record.id).await
                {
                    meta.commit_sha = Some(pull.commit_sha);
                    meta.pulled_at = Some(pull.pulled_at);
                }
                if let Some(files) = files_by_model.get(&record.id) {
                    for f in files {
                        meta.files.insert(f.filename.clone(), f.clone());
                    }
                }
                let m = tama_core::config::ModelConfig::from_db_record(record);
                let mut model_config = m.clone();
                for f in meta.files.values() {
                    let quant_key = f.quant.clone().unwrap_or_else(|| f.filename.clone());
                    model_config.quants.insert(
                        quant_key,
                        tama_core::config::QuantEntry {
                            file: f.filename.clone(),
                            kind: tama_core::config::QuantKind::from_filename(&f.filename),
                            size_bytes: f.size_bytes.map(|s| s as u64),
                            context_length: None,
                        },
                    );
                }
                let model_state = model_states.get(&record.id).cloned();
                models.push(model_entry_json(
                    record.id,
                    record,
                    &model_config,
                    &configs_dir_clone,
                    Some(&meta),
                    model_state,
                ));
            }

            let sampling_templates: serde_json::Value =
                serde_json::to_value(&cfg.sampling_templates).unwrap_or_default();
            Json(serde_json::json!({
                "models": models,
                "backends": backend_options,
                "sampling_templates": sampling_templates
            }))
            .into_response()
        }
        Err((status, body)) => (status, Json(body)).into_response(),
    }
}

/// GET /tama/v1/models/:id — get a single model config.
/// Accepts integer id or config_key (double-dash format) for compatibility.
pub async fn get_model(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    match load_config_from_state(&state).await {
        Ok((cfg, config_dir)) => {
            let configs_dir = config_dir.join("configs");
            let backend_options = build_backend_options(&cfg, &config_dir).await;

            // Collect current runtime state for model lookup
            // Keyed by db_id so we can look up by the integer ID from the DB record.
            let model_states: HashMap<i64, ModelState> = state
                .collect_model_state_snapshots()
                .await
                .into_iter()
                .filter_map(|s| s.db_id.map(|db_id| (db_id, s.state)))
                .collect();

            // Resolve + load the model from the Postgres pool.
            let Some(pool) = web_state.db_pool.as_ref() else {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Postgres pool not available",
                    None,
                );
            };
            let configs_dir_clone = configs_dir.clone();
            let backend_options_clone = backend_options.clone();
            let (model_id, record) = match resolve_model_record(pool, &id_str).await {
                Ok(v) => v,
                Err((s, b)) => return (s, Json(b)).into_response(),
            };
            let meta = load_repo_db_meta(pool, model_id).await;
            let m = tama_core::config::ModelConfig::from_db_record(&record);
            let mut config = m.clone();
            for f in meta.files.values() {
                let quant_key = f.quant.clone().unwrap_or_else(|| f.filename.clone());
                config.quants.insert(
                    quant_key,
                    tama_core::config::QuantEntry {
                        file: f.filename.clone(),
                        kind: tama_core::config::QuantKind::from_filename(&f.filename),
                        size_bytes: f.size_bytes.map(|s| s as u64),
                        context_length: None,
                    },
                );
            }
            let model_state = model_states.get(&model_id).cloned();
            let mut val = model_entry_json(
                model_id,
                &record,
                &config,
                &configs_dir_clone,
                Some(&meta),
                model_state,
            );
            val["backends"] = serde_json::json!(backend_options_clone);
            Json(val).into_response()
        }
        Err((status, body)) => (status, Json(body)).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tama_core::config::ModelConfig;
    use tama_core::db::queries::ModelConfigRecord;

    fn make_record() -> ModelConfigRecord {
        ModelConfigRecord {
            id: 1,
            repo_id: "test/repo".to_string(),
            display_name: None,
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            gpu_device: None,
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            selected_mtp_model: None,
            context_length: None,
            num_parallel: None,
            kv_unified: false,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            spec_decoding: None,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            updated_at: "2025-01-01T00:00:00Z".to_string(),
            n_batch: None,
            n_ubatch: None,
            vllm_config: None,
            provider_name: None,
            reasoning_levels: None,
        }
    }

    fn make_config(mtp_model: Option<String>) -> ModelConfig {
        ModelConfig {
            mtp_model,
            ..Default::default()
        }
    }

    #[test]
    fn test_model_entry_json_includes_hf_fields() {
        let mut record = make_record();
        record.hf_architecture_type = Some("text-generation".to_string());
        record.hf_base_model = Some("meta-llama/Llama-3.1-8B".to_string());
        record.hf_format = Some("gguf".to_string());
        let config = make_config(None);
        let tmp = std::path::Path::new("/tmp");

        let result = model_entry_json(1, &record, &config, tmp, None, None);

        assert_eq!(
            result.get("hf_architecture_type").and_then(|v| v.as_str()),
            Some("text-generation"),
            "hf_architecture_type should be included in API JSON when set"
        );
        assert_eq!(
            result.get("hf_base_model").and_then(|v| v.as_str()),
            Some("meta-llama/Llama-3.1-8B"),
            "hf_base_model should be included in API JSON when set"
        );
        assert_eq!(
            result.get("hf_format").and_then(|v| v.as_str()),
            Some("gguf"),
            "hf_format should be included in API JSON when set"
        );

        // None case: all should be null when not set
        let record_none = make_record();
        let config_none = make_config(None);
        let result_none = model_entry_json(1, &record_none, &config_none, tmp, None, None);
        assert!(
            result_none["hf_architecture_type"].is_null(),
            "hf_architecture_type should be null when not set"
        );
        assert!(
            result_none["hf_base_model"].is_null(),
            "hf_base_model should be null when not set"
        );
        assert!(
            result_none["hf_format"].is_null(),
            "hf_format should be null when not set"
        );
    }

    #[test]
    fn test_model_entry_json_includes_mtp_model() {
        let record = make_record();
        let config = make_config(Some("mtp-test.gguf".to_string()));
        let tmp = std::path::Path::new("/tmp");

        let result = model_entry_json(1, &record, &config, tmp, None, None);

        // Some case: mtp_model should be present
        assert_eq!(
            result.get("mtp_model").and_then(|v| v.as_str()),
            Some("mtp-test.gguf"),
            "mtp_model should be included in API JSON when set"
        );

        // None case: mtp_model should be null
        let record_none = make_record();
        let config_none = make_config(None);
        let result_none = model_entry_json(1, &record_none, &config_none, tmp, None, None);
        assert!(
            result_none["mtp_model"].is_null(),
            "mtp_model should be null in API JSON when not set"
        );
    }

    #[test]
    fn test_model_entry_json_includes_capabilities() {
        let record = make_record();
        let config = make_config(None);
        let tmp = std::path::Path::new("/tmp");

        let result = model_entry_json(1, &record, &config, tmp, None, None);

        // capabilities should be present in the JSON
        assert!(
            result.get("capabilities").is_some(),
            "capabilities field should be present in model JSON"
        );

        // With no MTP/mmproj indicators, all should be false
        let caps = result["capabilities"].as_object().unwrap();
        assert_eq!(caps["supports_mtp"], false);
        assert_eq!(caps["has_mtp_draft_file"], false);
        assert_eq!(caps["has_mmproj"], false);
    }

    #[test]
    fn test_model_entry_json_state_field() {
        let record = make_record();
        let config = make_config(None);
        let tmp = std::path::Path::new("/tmp");

        // None state should produce "idle" (not null)
        let result_idle = model_entry_json(1, &record, &config, tmp, None, None);
        assert_eq!(
            result_idle.get("state").and_then(|v| v.as_str()),
            Some("idle"),
            "state should be \"idle\" when no snapshot is available"
        );

        // Some(Ready) state should produce "ready"
        let result_ready =
            model_entry_json(1, &record, &config, tmp, None, Some(ModelState::Ready));
        assert_eq!(
            result_ready.get("state").and_then(|v| v.as_str()),
            Some("ready"),
            "state should be \"ready\" when ModelState::Ready"
        );
    }

    // ── resolve_model_record integration tests ────────────────────────────────

    /// Seed a test model in Postgres and return its id.
    async fn insert_test_model(pool: &sqlx::PgPool) -> i64 {
        tama_core::db::save_model_config(
            pool,
            "org--test-model",
            &tama_core::config::ModelConfig {
                backend: "llama-cpp".into(),
                model: Some("org/test-model".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_resolve_model_record_by_config_key() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = guard.pool.clone();
        let expected_id = insert_test_model(&pool).await;

        let result = resolve_model_record(&pool, "org--test-model").await;
        assert!(
            result.is_ok(),
            "resolve_model_record should succeed for valid config_key"
        );
        let (id, record) = result.unwrap();
        assert_eq!(id, expected_id);
        assert_eq!(record.repo_id, "org/test-model");

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_resolve_model_record_by_integer_id() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = guard.pool.clone();
        let expected_id = insert_test_model(&pool).await;

        let result = resolve_model_record(&pool, &expected_id.to_string()).await;
        assert!(
            result.is_ok(),
            "resolve_model_record should succeed for valid integer id"
        );
        let (id, record) = result.unwrap();
        assert_eq!(id, expected_id);
        assert_eq!(record.repo_id, "org/test-model");

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_resolve_model_record_unknown_config_key_404() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = guard.pool.clone();
        // No models inserted — any lookup should 404.
        let result = resolve_model_record(&pool, "no--such-model").await;
        assert!(result.is_err(), "should return Err for unknown config_key");
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        guard.finish().await;
    }

    #[tokio::test]
    async fn test_resolve_model_record_unknown_integer_404() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = guard.pool.clone();
        // No models inserted — integer parse succeeds but record is missing.
        let result = resolve_model_record(&pool, "999").await;
        assert!(result.is_err(), "should return Err for unknown integer id");
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        guard.finish().await;
    }
}
