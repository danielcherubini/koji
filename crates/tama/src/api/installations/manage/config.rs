use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::types::{
    BackendPatchBody, DefaultArgsQuery, DefaultEnvQuery, RenameBackendRequest,
    UpdateDefaultArgsRequest, UpdateDefaultEnvRequest,
};
use crate::api::error::error_response;
use tama_core::proxy::ProxyState;

/// POST /tama/v1/backends/:name/rename
/// Atomically rename a backend across every table that carries its display name.
/// The backend's stable `logical_id` is preserved, so `backend_configs`
/// (default args/env) and any models on that backend survive the rename.
pub async fn rename_installation(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    Json(req): Json<RenameBackendRequest>,
) -> impl IntoResponse {
    if let Err(resp) = crate::api::installations::reject_traversal(&backend_name, "backend name") {
        return resp;
    }

    let new_name = match validate_new_backend_name(&req.name) {
        Ok(name) => name,
        Err(msg) => {
            return error_response(StatusCode::BAD_REQUEST, msg, Some("ValidationError"));
        }
    };

    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool);

    let rename_to = new_name.clone();
    let renamed = match mgr.rename(&backend_name, &rename_to).await {
        Ok(renamed) => renamed,
        Err(e) => {
            return error_response(
                StatusCode::CONFLICT,
                format!("Failed to rename backend: {}", e),
                None,
            );
        }
    };

    match renamed {
        true => Json(serde_json::json!({ "success": true, "name": new_name })).into_response(),
        false => error_response(
            StatusCode::NOT_FOUND,
            "Backend not found".to_string(),
            Some("NotFoundError"),
        ),
    }
}

/// Validate and normalize a target backend name for renaming.
/// Rejects empty, whitespace-only, NUL, and path-traversal names.
/// Returns the trimmed name on success, or a user-facing error message.
fn validate_new_backend_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("New backend name is required".to_string());
    }
    if trimmed.contains('\0') || trimmed.chars().any(char::is_whitespace) {
        return Err(
            "Backend name must not be empty or contain whitespace or null characters".to_string(),
        );
    }
    if crate::api::installations::is_path_traversal(&trimmed) {
        return Err(
            "Invalid backend name: path separators or traversal sequences not allowed".to_string(),
        );
    }
    Ok(trimmed)
}

/// POST /tama/v1/backends/:name/default-args
/// Update default_args for a backend in the backend_configs DB table.
pub async fn update_installation_default_args(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultArgsQuery>,
    Json(req): Json<UpdateDefaultArgsRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if let Err(resp) = crate::api::installations::reject_traversal(&backend_name, "backend name") {
        return resp;
    }

    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool);

    let gpu_variant = query.gpu_variant.clone();
    let default_args = req.default_args.clone();

    // Preserve existing default_env when updating default_args
    let existing_env = mgr.get_default_env(&backend_name, &gpu_variant).await;
    let result = mgr
        .save_config(
            &backend_name,
            &gpu_variant,
            &default_args,
            &existing_env,
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{}", e));

    match result {
        Ok(_) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update backend config: {}", e),
            None,
        ),
    }
}

/// POST /tama/v1/backends/:name/default-env
/// Update default_env for a backend in the backend_configs DB table.
pub async fn update_installation_default_env(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultEnvQuery>,
    Json(req): Json<UpdateDefaultEnvRequest>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if let Err(resp) = crate::api::installations::reject_traversal(&backend_name, "backend name") {
        return resp;
    }

    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool);

    let gpu_variant = query.gpu_variant.clone();
    let default_env = req.default_env.clone();

    // Preserve existing default_args when updating default_env
    let existing_args = mgr.get_default_args(&backend_name, &gpu_variant).await;
    let result = mgr
        .save_config(
            &backend_name,
            &gpu_variant,
            &existing_args,
            &default_env,
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{}", e));

    match result {
        Ok(_) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update backend config: {}", e),
            None,
        ),
    }
}

/// PATCH /tama/v1/backends/:name
/// Consolidated backend config update for default_args, default_env, and health_check_url.
pub async fn patch_installation(
    State(state): State<Arc<ProxyState>>,
    Path(backend_name): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DefaultArgsQuery>,
    Json(body): Json<BackendPatchBody>,
) -> impl IntoResponse {
    // Validate path param to prevent path traversal attacks
    if let Err(resp) = crate::api::installations::reject_traversal(&backend_name, "backend name") {
        return resp;
    }

    let pool = state.db_pool();
    let mgr = tama_core::installations::InstallationManager::new(pool);

    let gpu_variant = query.gpu_variant.clone();
    let patch_args = body.default_args.clone();
    let patch_env = body.default_env.clone();
    let patch_health = body.health_check_url;

    // Load existing values to preserve unpatched fields
    let existing_args = mgr.get_default_args(&backend_name, &gpu_variant).await;
    let existing_env = mgr.get_default_env(&backend_name, &gpu_variant).await;
    let existing_health = mgr.get_health_check_url(&backend_name, &gpu_variant).await;

    // Merge: use patched value if present, otherwise preserve existing
    let default_args = patch_args.unwrap_or(existing_args);
    let default_env = patch_env.unwrap_or(existing_env);
    // health_check_url: None=preserve, Some(value)=set
    let health_check_url = patch_health.as_deref().or(existing_health.as_deref());

    let result = mgr
        .save_config(
            &backend_name,
            &gpu_variant,
            &default_args,
            &default_env,
            health_check_url,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{}", e));

    match result {
        Ok(_) => Json(serde_json::json!({"success": true})).into_response(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to update backend config: {}", e),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_new_backend_name;

    #[test]
    fn test_validate_new_backend_name_empty() {
        assert_eq!(
            validate_new_backend_name(""),
            Err("New backend name is required".to_string())
        );
        assert!(validate_new_backend_name("   ").is_err());
    }

    #[test]
    fn test_validate_new_backend_name_whitespace() {
        assert_eq!(
            validate_new_backend_name("my backend"),
            Err(
                "Backend name must not be empty or contain whitespace or null characters"
                    .to_string()
            )
        );
        assert!(validate_new_backend_name("my\tbackend").is_err());
        assert!(validate_new_backend_name("my\0backend").is_err());
    }

    #[test]
    fn test_validate_new_backend_name_path_separators() {
        assert!(validate_new_backend_name("a/b").is_err());
        assert!(validate_new_backend_name("a\\b").is_err());
    }

    #[test]
    fn test_validate_new_backend_name_traversal() {
        assert!(validate_new_backend_name("..").is_err());
        assert!(validate_new_backend_name("a..b").is_err());
    }

    #[test]
    fn test_validate_new_backend_name_valid() {
        assert_eq!(
            validate_new_backend_name("radiance"),
            Ok("radiance".to_string())
        );
        assert_eq!(
            validate_new_backend_name("  radiance  "),
            Ok("radiance".to_string())
        );
    }
}
