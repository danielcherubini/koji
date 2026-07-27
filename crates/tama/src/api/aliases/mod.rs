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

/// Regex for valid alias names: starts with alphanumeric, then alphanumeric/underscore/hyphen/period,
/// max 128 characters total.
fn alias_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_.\-]{0,127}$").expect("invalid alias name regex")
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
            contain only letters, digits, underscores, hyphens, and periods, and be at most 128 characters",
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
