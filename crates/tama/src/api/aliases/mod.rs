//! REST API endpoints for managing model aliases.

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use regex::Regex;
use std::sync::{Arc, OnceLock};

use crate::api::error::{error_body, error_response, error_response_simple};
use crate::api::field_update::FieldUpdate;
use crate::api::helpers::shared_repository;
use crate::web_types::WebState;
use tama_core::proxy::ProxyState;

/// Regex for valid alias names: starts with alphanumeric, then alphanumeric/underscore/hyphen/period/slash,
/// max 128 characters total.
fn alias_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_.\-/]{0,127}$").expect("invalid alias name regex")
    })
}

/// Returns an error message if the alias name is invalid, or `None` if valid.
fn validate_alias_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("Alias name must not be empty".to_string());
    }
    if !alias_name_re().is_match(name) {
        return Some(format!(
            "Invalid alias name '{}': must start with a letter or digit, \
            contain only letters, digits, underscores, hyphens, periods, and forward slashes, and be at most 128 characters",
            name
        ));
    }
    None
}

/// GET /tama/v1/aliases
/// Returns list of all aliases (enabled and disabled)
pub async fn list_aliases(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let repo = repo.clone();
    let result = tokio::task::spawn_blocking(move || {
        let repo = repo.lock().unwrap();
        repo.get_all_aliases()
    })
    .await;
    match result {
        Ok(Ok(aliases)) => Json(aliases).into_response(),
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "Task panicked", None),
    }
}

/// GET /tama/v1/aliases/{id}
pub async fn get_alias(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let repo = repo.clone();
    let result = tokio::task::spawn_blocking(move || {
        let repo = repo.lock().unwrap();
        repo.get_alias_by_id(id)
    })
    .await;
    match result {
        Ok(Ok(Some(alias))) => Json(alias).into_response(),
        Ok(Ok(None)) => error_response(
            StatusCode::NOT_FOUND,
            "Alias not found",
            Some("NotFoundError"),
        ),
        Ok(Err(e)) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "Task panicked", None),
    }
}

/// POST /tama/v1/aliases
pub async fn create_alias(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Json(payload): Json<CreateAliasRequest>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    // Validate alias name
    if let Some(err) = validate_alias_name(&payload.name) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            err,
            Some("ValidationError"),
        );
    }

    // Capture payload fields for the spawn_blocking closure.
    let model_id = payload.model_id;
    let name = payload.name.clone();
    let desc = payload.description.clone();

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();

            // Validate model_id exists
            if !repo.model_exists(model_id).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(format!("Failed to check model existence: {}", e), None),
                )
            })? {
                return Err((
                    StatusCode::BAD_REQUEST,
                    error_body("Model not found", Some("ValidationError")),
                ));
            }

            let new_id = repo
                .insert_alias(&name, model_id, desc.as_deref())
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        error_body(format!("Database not configured: {}", e), None),
                    )
                })?;

            let alias = repo.get_alias_by_id(new_id).ok().flatten().ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body("Failed to retrieve created alias", None),
                )
            })?;

            Ok(alias)
        })
        .await;

    let alias = match result {
        Ok(Ok(a)) => a,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn error: {}", e),
            )
        }
    };

    // Reload alias cache in ProxyState — outside the lock to avoid holding
    // the MutexGuard across an .await point (which would make the future !Send).
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after create: {}", e);
    }

    // Return the created alias
    (StatusCode::CREATED, Json(alias)).into_response()
}

/// PUT /tama/v1/aliases/{id}
pub async fn update_alias(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(payload): Json<UpdateAliasRequest>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    // Validate alias name if provided
    if let Some(ref name) = payload.name {
        if let Some(err) = validate_alias_name(name) {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                err,
                Some("ValidationError"),
            );
        }
    }

    // Capture payload fields for the spawn_blocking closure.
    let name_for_closure = payload.name.clone();
    let model_id_for_closure = payload.model_id;
    let desc_for_closure = match &payload.description {
        FieldUpdate::Set(v) => Some(Some(v.clone())),
        FieldUpdate::Clear => Some(None),
        FieldUpdate::Unchanged => None,
    };

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();

            // Validate model_id if provided
            if let Some(ref model_id) = model_id_for_closure {
                if !repo.model_exists(*model_id).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        error_body(format!("Failed to check model existence: {}", e), None),
                    )
                })? {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        error_body("Model not found", Some("ValidationError")),
                    ));
                }
            }

            let update = tama_core::db::queries::AliasUpdate {
                name: name_for_closure.as_deref(),
                model_id: model_id_for_closure,
                description: desc_for_closure.as_ref().map(|d| d.as_deref()),
                enabled: payload.enabled,
            };
            repo.update_alias(id, update).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(format!("Database not configured: {}", e), None),
                )
            })?;

            repo.get_alias_by_id(id).ok().flatten().ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body("Failed to retrieve updated alias", None),
                )
            })
        })
        .await;

    let alias = match result {
        Ok(Ok(a)) => a,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn error: {}", e),
            )
        }
    };

    // Reload alias cache in ProxyState — outside the lock to avoid holding
    // the MutexGuard across an .await point (which would make the future !Send).
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after update: {}", e);
    }

    Json(alias).into_response()
}

/// DELETE /tama/v1/aliases/{id}
pub async fn delete_alias(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let repo = match shared_repository(&web_state) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let result =
        tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, serde_json::Value)> {
            let repo = repo.lock().unwrap();
            repo.delete_alias(id).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error_body(format!("Database not configured: {}", e), None),
                )
            })?;
            Ok(())
        })
        .await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("spawn error: {}", e),
            )
        }
    }

    // Reload alias cache in ProxyState — outside the lock to avoid holding
    // the MutexGuard across an .await point (which would make the future !Send).
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after delete: {}", e);
    }

    Json(serde_json::json!({"deleted": true})).into_response()
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateAliasRequest {
    pub name: String,
    pub model_id: i64,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateAliasRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model_id: Option<i64>,
    #[serde(default)]
    pub description: FieldUpdate<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::{Arc, Mutex};
    use tama_core::db::repository::Repository;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    fn build_test_state(
        tmp_dir: &std::path::Path,
    ) -> (Arc<ProxyState>, Arc<crate::web_types::WebState>) {
        let config = tama_core::config::Config::default();
        let state = Arc::new(ProxyState::new(config, Some(tmp_dir.to_path_buf())));

        // Re-open the repository so we have a live handle
        let repo = Repository::open(tmp_dir).unwrap();

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            repository: Some(Arc::new(Mutex::new(repo))),
        });

        (state, web_state)
    }

    /// POST → GET list → GET single → PATCH disable → DELETE → final GET empty.
    #[tokio::test]
    async fn test_alias_crud_round_trip() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");

        // Seed a model in the DB so alias creation can validate model_id.
        let conn = tama_core::db::open(tmp_dir.path()).unwrap();
        tama_core::db::queries::upsert_model_config(
            &conn.conn,
            &tama_core::db::queries::ModelConfigRecord {
                id: 0,
                repo_id: "test-org/test-model".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
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
                api_name: Some("test-model".to_string()),
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
                created_at: "2024-01-01".into(),
                updated_at: "2024-01-01".into(),
                n_batch: None,
                n_ubatch: None,
                vllm_config: None,
                provider_name: None,
                reasoning_levels: None,
            },
        )
        .unwrap();

        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create alias
        let body = serde_json::json!({"name": "my-alias", "model_id": 1}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/aliases")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);

        // GET list — should contain one alias
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/aliases")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 1);

        // GET single alias by id
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/aliases/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // PUT disable
        let body = serde_json::json!({"enabled": false}).to_string();
        let req = Request::builder()
            .method("PUT")
            .uri("/tama/v1/aliases/1")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // DELETE
        let req = Request::builder()
            .method("DELETE")
            .uri("/tama/v1/aliases/1")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // Final GET list — should be empty
        let req = Request::builder()
            .method("GET")
            .uri("/tama/v1/aliases")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body_str = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body_str).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    /// POST with invalid alias name returns 422.
    #[tokio::test]
    async fn test_create_alias_rejects_invalid_name() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");

        // Seed a model in the DB so alias creation can validate model_id.
        let conn = tama_core::db::open(tmp_dir.path()).unwrap();
        tama_core::db::queries::upsert_model_config(
            &conn.conn,
            &tama_core::db::queries::ModelConfigRecord {
                id: 0,
                repo_id: "test-org/test-model".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
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
                api_name: Some("test-model".to_string()),
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
                created_at: "2024-01-01".into(),
                updated_at: "2024-01-01".into(),
                n_batch: None,
                n_ubatch: None,
                vllm_config: None,
                provider_name: None,
                reasoning_levels: None,
            },
        )
        .unwrap();

        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST with invalid name containing space and exclamation
        let body = serde_json::json!({"name": "bad name!", "model_id": 1}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/aliases")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "invalid alias name should return 422"
        );
    }

    /// POST with a slash-containing alias name like "org/model" returns 201.
    #[tokio::test]
    async fn test_create_alias_accepts_slash_name() {
        let tmp_dir = tempfile::tempdir().expect("tempdir");

        // Seed a model in the DB so alias creation can validate model_id.
        let conn = tama_core::db::open(tmp_dir.path()).unwrap();
        tama_core::db::queries::upsert_model_config(
            &conn.conn,
            &tama_core::db::queries::ModelConfigRecord {
                id: 0,
                repo_id: "test-org/test-model".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
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
                api_name: Some("test-model".to_string()),
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
                created_at: "2024-01-01".into(),
                updated_at: "2024-01-01".into(),
                n_batch: None,
                n_ubatch: None,
                vllm_config: None,
                provider_name: None,
                reasoning_levels: None,
            },
        )
        .unwrap();

        let (state, web_state) = build_test_state(tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST with slash-containing name — should be accepted
        let body = serde_json::json!({"name": "org/model", "model_id": 1}).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/tama/v1/aliases")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::CREATED,
            "slash-containing alias name should return 201"
        );
    }
}
