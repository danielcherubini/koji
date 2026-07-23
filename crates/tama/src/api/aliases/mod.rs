//! REST API endpoints for managing model aliases.

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use regex::Regex;
use std::sync::{Arc, OnceLock};

use crate::api::error::{error_response, error_response_simple};
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

    // DB operations within lock scope — must not hold guard across .await
    let (_new_id, alias) = {
        let repo = repo.lock().unwrap();

        // Validate model_id exists
        let model_exists = match repo.model_exists(payload.model_id) {
            Ok(v) => v,
            Err(e) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
            }
        };

        if !model_exists {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Model not found",
                Some("ValidationError"),
            );
        }

        let new_id = match repo.insert_alias(
            &payload.name,
            payload.model_id,
            payload.description.as_deref(),
        ) {
            Ok(id) => id,
            Err(e) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
            }
        };

        let alias = repo.get_alias_by_id(new_id).ok().flatten();
        (new_id, alias)
    };

    // Reload alias cache in ProxyState — outside the lock to avoid holding
    // the MutexGuard across an .await point (which would make the future !Send).
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after create: {}", e);
    }

    // Return the created alias
    match alias {
        Some(a) => (StatusCode::CREATED, Json(a)).into_response(),
        None => error_response_simple(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve created alias",
        ),
    }
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

    // DB operations within lock scope — must not hold guard across .await
    let alias = {
        let repo = repo.lock().unwrap();

        // Validate model_id if provided
        if let Some(ref model_id) = payload.model_id {
            let model_exists = match repo.model_exists(*model_id) {
                Ok(v) => v,
                Err(e) => {
                    return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
                }
            };

            if !model_exists {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "Model not found",
                    Some("ValidationError"),
                );
            }
        }

        match repo.update_alias(
            id,
            payload.name.as_deref(),
            payload.model_id,
            payload.description.as_ref().map(|d| d.as_deref()),
            payload.enabled,
        ) {
            Ok(()) => {}
            Err(e) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
            }
        }

        repo.get_alias_by_id(id).ok().flatten()
    };

    // Reload alias cache in ProxyState — outside the lock to avoid holding
    // the MutexGuard across an .await point (which would make the future !Send).
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after update: {}", e);
    }

    match alias {
        Some(a) => Json(a).into_response(),
        None => error_response_simple(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve updated alias",
        ),
    }
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

    // DB operation within lock scope — must not hold guard across .await
    {
        let repo = repo.lock().unwrap();
        match repo.delete_alias(id) {
            Ok(()) => {}
            Err(e) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None)
            }
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
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}
