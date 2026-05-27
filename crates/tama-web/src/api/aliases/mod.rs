//! REST API endpoints for managing model aliases.

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use std::sync::Arc;

use tama_core::proxy::ProxyState;

/// GET /tama/v1/aliases
/// Returns list of all aliases (enabled and disabled)
pub async fn list_aliases(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let mgr = match state.model_mgr() {
        Some(m) => m,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database not configured"})),
            )
                .into_response();
        }
    };

    match tama_core::db::queries::get_all_aliases(mgr.conn()) {
        Ok(aliases) => Json(aliases).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /tama/v1/aliases/{id}
pub async fn get_alias(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let mgr = match state.model_mgr() {
        Some(m) => m,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database not configured"})),
            )
                .into_response();
        }
    };

    match tama_core::db::queries::get_alias_by_id(mgr.conn(), id) {
        Ok(Some(alias)) => Json(alias).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Alias not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /tama/v1/aliases
pub async fn create_alias(
    State(state): State<Arc<ProxyState>>,
    Json(payload): Json<CreateAliasRequest>,
) -> impl IntoResponse {
    let mgr = match state.model_mgr() {
        Some(m) => m,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database not configured"})),
            )
                .into_response();
        }
    };

    // Validate model_id exists
    let model_exists: bool = match mgr.conn().query_row(
        "SELECT COUNT(*) > 0 FROM model_configs WHERE id = ?",
        [payload.model_id],
        |row| row.get(0),
    ) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    if !model_exists {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Model not found"})),
        )
            .into_response();
    }

    let new_id = match tama_core::db::queries::insert_alias(
        mgr.conn(),
        &payload.name,
        payload.model_id,
        payload.description.as_deref(),
    ) {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Reload alias cache in ProxyState
    let state_clone = state.clone();
    tokio::spawn(async move {
        let _ = state_clone.reload_aliases().await;
    });

    // Return the created alias
    match tama_core::db::queries::get_alias_by_id(mgr.conn(), new_id) {
        Ok(Some(alias)) => (StatusCode::CREATED, Json(alias)).into_response(),
        Ok(None) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Failed to retrieve created alias"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// PUT /tama/v1/aliases/{id}
pub async fn update_alias(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(payload): Json<UpdateAliasRequest>,
) -> impl IntoResponse {
    let mgr = match state.model_mgr() {
        Some(m) => m,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database not configured"})),
            )
                .into_response();
        }
    };

    // Validate model_id if provided
    if let Some(ref model_id) = payload.model_id {
        let model_exists: bool = match mgr.conn().query_row(
            "SELECT COUNT(*) > 0 FROM model_configs WHERE id = ?",
            [model_id],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
                    .into_response();
            }
        };

        if !model_exists {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "Model not found"})),
            )
                .into_response();
        }
    }

    match tama_core::db::queries::update_alias(
        mgr.conn(),
        id,
        payload.name.as_deref(),
        payload.model_id,
        payload.description.as_ref().map(|d| d.as_deref()),
        payload.enabled,
    ) {
        Ok(()) => {}
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    }

    // Reload alias cache in ProxyState
    let state_clone = state.clone();
    tokio::spawn(async move {
        let _ = state_clone.reload_aliases().await;
    });

    match tama_core::db::queries::get_alias_by_id(mgr.conn(), id) {
        Ok(Some(alias)) => Json(alias).into_response(),
        Ok(None) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Failed to retrieve updated alias"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// DELETE /tama/v1/aliases/{id}
pub async fn delete_alias(
    State(state): State<Arc<ProxyState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let mgr = match state.model_mgr() {
        Some(m) => m,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Database not configured"})),
            )
                .into_response();
        }
    };

    match tama_core::db::queries::delete_alias(mgr.conn(), id) {
        Ok(()) => {}
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    }

    // Reload alias cache in ProxyState
    let state_clone = state.clone();
    tokio::spawn(async move {
        let _ = state_clone.reload_aliases().await;
    });

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
