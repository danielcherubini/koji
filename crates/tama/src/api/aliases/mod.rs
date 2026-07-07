//! REST API endpoints for managing model aliases.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use regex::Regex;
use std::sync::{Arc, OnceLock};

use crate::api::error::{error_response, error_response_simple};
use tama_core::db::repository::Repository;
use tama_core::proxy::ProxyState;

/// Regex for valid alias names: starts with alphanumeric, then alphanumeric/underscore/hyphen,
/// max 128 characters total.
fn alias_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$").expect("invalid alias name regex")
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
            contain only letters, digits, underscores, and hyphens, and be at most 128 characters",
            name
        ));
    }
    None
}

/// GET /tama/v1/aliases
/// Returns list of all aliases (enabled and disabled)
pub async fn list_aliases(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let db_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let repo = match Repository::open(&db_dir) {
        Ok(r) => r,
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database not configured: {}", e),
            )
        }
    };

    match repo.get_all_aliases() {
        Ok(aliases) => Json(aliases).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// GET /tama/v1/aliases/{id}
pub async fn get_alias(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let db_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let repo = match Repository::open(&db_dir) {
        Ok(r) => r,
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database not configured: {}", e),
            )
        }
    };

    match repo.get_alias_by_id(id) {
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
    Json(payload): Json<CreateAliasRequest>,
) -> impl IntoResponse {
    let db_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let repo = match Repository::open(&db_dir) {
        Ok(r) => r,
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database not configured: {}", e),
            )
        }
    };

    // Validate alias name
    if let Some(err) = validate_alias_name(&payload.name) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            err,
            Some("ValidationError"),
        );
    }

    // Validate model_id exists
    let model_exists: bool = match repo.conn().query_row(
        "SELECT COUNT(*) > 0 FROM model_configs WHERE id = ?",
        [payload.model_id],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
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
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    };

    // Reload alias cache in ProxyState
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after create: {}", e);
    }

    // Return the created alias
    match repo.get_alias_by_id(new_id) {
        Ok(Some(alias)) => (StatusCode::CREATED, Json(alias)).into_response(),
        Ok(None) => error_response_simple(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve created alias",
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// PUT /tama/v1/aliases/{id}
pub async fn update_alias(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(payload): Json<UpdateAliasRequest>,
) -> impl IntoResponse {
    let db_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let repo = match Repository::open(&db_dir) {
        Ok(r) => r,
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database not configured: {}", e),
            )
        }
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

    // Validate model_id if provided
    if let Some(ref model_id) = payload.model_id {
        let model_exists: bool = match repo.conn().query_row(
            "SELECT COUNT(*) > 0 FROM model_configs WHERE id = ?",
            [model_id],
            |row| row.get(0),
        ) {
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
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }

    // Reload alias cache in ProxyState
    if let Err(e) = state.reload_aliases().await {
        tracing::warn!("Failed to reload aliases after update: {}", e);
    }

    match repo.get_alias_by_id(id) {
        Ok(Some(alias)) => Json(alias).into_response(),
        Ok(None) => error_response_simple(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to retrieve updated alias",
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
    }
}

/// DELETE /tama/v1/aliases/{id}
pub async fn delete_alias(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let db_dir = state.db_dir().clone().unwrap_or_else(|| {
        tama_core::config::Config::config_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
    });

    let repo = match Repository::open(&db_dir) {
        Ok(r) => r,
        Err(e) => {
            return error_response_simple(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database not configured: {}", e),
            )
        }
    };

    match repo.delete_alias(id) {
        Ok(()) => {}
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None),
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
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}
