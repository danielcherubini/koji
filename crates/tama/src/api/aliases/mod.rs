//! REST API endpoints for managing model aliases (Postgres, plan-190 Task 5).

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use regex::Regex;
use std::sync::{Arc, OnceLock};

use crate::api::error::error_response;
use crate::api::field_update::FieldUpdate;
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
    let pool = web_state.db_pool.as_ref();
    match tama_core::db::queries::get_all_aliases(pool).await {
        Ok(aliases) => Json(aliases).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// GET /tama/v1/aliases/{id}
pub async fn get_alias(
    State(_state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();
    match tama_core::db::queries::get_alias_by_id(pool, id).await {
        Ok(Some(alias)) => Json(alias).into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "Alias not found",
            Some("NotFoundError"),
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// POST /tama/v1/aliases
pub async fn create_alias(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Json(payload): Json<CreateAliasRequest>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

    // Validate alias name
    if let Some(err) = validate_alias_name(&payload.name) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            err,
            Some("ValidationError"),
        );
    }

    let model_id = payload.model_id;
    let name = payload.name;
    let desc = payload.description;

    // Validate model_id exists (Postgres is the model source of truth)
    match tama_core::db::queries::get_model_config(pool, model_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Model not found",
                Some("ValidationError"),
            )
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to check model existence: {}", e),
                None,
            )
        }
    }

    let new_id =
        match tama_core::db::queries::insert_alias(pool, &name, model_id, desc.as_deref()).await {
            Ok(id) => id,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Database not configured: {}", e),
                    None,
                )
            }
        };

    let alias = match tama_core::db::queries::get_alias_by_id(pool, new_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to retrieve created alias",
                None,
            )
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to retrieve created alias: {}", e),
                None,
            )
        }
    };

    // Reload alias cache in ProxyState
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
    Path(id): Path<i64>,
    Json(payload): Json<UpdateAliasRequest>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

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

    // Validate model_id if provided
    if let Some(model_id) = payload.model_id {
        match tama_core::db::queries::get_model_config(pool, model_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Model not found",
                    Some("ValidationError"),
                )
            }
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to check model existence: {}", e),
                    None,
                )
            }
        }
    }

    let desc_owned: Option<Option<String>> = match &payload.description {
        FieldUpdate::Set(v) => Some(Some(v.clone())),
        FieldUpdate::Clear => Some(None),
        FieldUpdate::Unchanged => None,
    };
    let update = tama_core::db::queries::AliasUpdate {
        name: payload.name.as_deref(),
        model_id: payload.model_id,
        description: desc_owned.as_ref().map(|d| d.as_deref()),
        enabled: payload.enabled,
    };

    if let Err(e) = tama_core::db::queries::update_alias(pool, id, update).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database not configured: {}", e),
            None,
        );
    }

    let alias = match tama_core::db::queries::get_alias_by_id(pool, id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to retrieve updated alias",
                None,
            )
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to retrieve updated alias: {}", e),
                None,
            )
        }
    };

    // Reload alias cache in ProxyState
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after update: {}", e);
    }

    Json(alias).into_response()
}

/// DELETE /tama/v1/aliases/{id}
pub async fn delete_alias(
    State(state): State<Arc<ProxyState>>,
    Extension(web_state): Extension<WebState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let pool = web_state.db_pool.as_ref();

    if let Err(e) = tama_core::db::queries::delete_alias(pool, id).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database not configured: {}", e),
            None,
        );
    }

    // Reload alias cache in ProxyState
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
    use std::sync::Arc;
    use tama_core::proxy::ProxyState;
    use tower::ServiceExt;

    fn build_test_state(
        pool: Arc<sqlx::PgPool>,
        tmp_dir: &std::path::Path,
    ) -> (Arc<ProxyState>, Arc<crate::web_types::WebState>) {
        let config = tama_core::config::Config::default();
        let state = Arc::new(ProxyState::new(
            config,
            Some(tmp_dir.to_path_buf()),
            pool.clone(),
        ));

        let web_state = Arc::new(crate::web_types::WebState {
            jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            capabilities: None,
            update_checker: Arc::new(tama_core::updates::UpdateChecker::default()),
            binary_version: "test".to_string(),
            update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            db_pool: pool,
        });

        (state, web_state)
    }

    /// Seed a model in Postgres so alias creation can validate model_id.
    /// Returns the model_id.
    async fn seed_model(pool: &sqlx::PgPool) -> i64 {
        tama_core::db::queries::upsert_model_config(
            pool,
            &tama_core::db::queries::ModelConfigRecord {
                repo_id: "test-org/test-model".to_string(),
                backend: "llama_cpp".to_string(),
                api_name: Some("test-model".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
    }

    /// POST → GET list → GET single → PUT disable → DELETE → final GET empty.
    #[tokio::test]
    async fn test_alias_crud_round_trip() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let model_id = seed_model(&pool).await;
        let tmp_dir = tempfile::tempdir().expect("tempdir");

        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST create alias
        let body = serde_json::json!({"name": "my-alias", "model_id": model_id}).to_string();
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

        guard.finish().await;
    }

    /// POST with invalid alias name returns 422.
    #[tokio::test]
    async fn test_create_alias_rejects_invalid_name() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let model_id = seed_model(&pool).await;
        let tmp_dir = tempfile::tempdir().expect("tempdir");

        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST with invalid name containing space and exclamation
        let body = serde_json::json!({"name": "bad name!", "model_id": model_id}).to_string();
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

        guard.finish().await;
    }

    /// POST with a slash-containing alias name like "org/model" returns 201.
    #[tokio::test]
    async fn test_create_alias_accepts_slash_name() {
        let guard = crate::testing::postgres::with_schema().await;
        let pool = Arc::new(guard.pool.clone());
        let model_id = seed_model(&pool).await;
        let tmp_dir = tempfile::tempdir().expect("tempdir");

        let (state, web_state) = build_test_state(pool, tmp_dir.path());
        let router = crate::router::build_web_routes(web_state.clone())
            .with_state(state)
            .layer(axum::extract::Extension(web_state.as_ref().clone()));

        // POST with slash-containing name — should be accepted
        let body = serde_json::json!({"name": "org/model", "model_id": model_id}).to_string();
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

        guard.finish().await;
    }
}
