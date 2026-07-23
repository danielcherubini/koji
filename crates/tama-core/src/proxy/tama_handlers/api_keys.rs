//! API key management handlers for the Tama management API.
//!
//! Provides CRUD endpoints for managing API keys under `/tama/v1/keys`.

use axum::response::IntoResponse;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

use crate::proxy::api_keys::{self, ApiKeyStore, AuthSubject, Scope};
use crate::proxy::handlers::json_error;
use crate::proxy::ProxyState;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request body for creating a new API key.
#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub scopes: Vec<Scope>,
    pub expires_at: Option<String>, // ISO 8601 or null
}

/// Request body for updating an API key's scopes.
#[derive(Debug, Deserialize)]
pub struct UpdateApiKeyRequest {
    pub scopes: Vec<Scope>,
}

/// Response body returned when a key is created (includes plaintext key ONCE).
#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    pub id: i64,
    pub name: String,
    pub key: String, // Plaintext — returned ONCE
    pub scopes: Vec<Scope>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// Response body for listing API keys (never includes plaintext).
#[derive(Debug, Serialize)]
pub struct ListApiKeyResponse {
    pub id: i64,
    pub name: String,
    pub key_prefix: String, // Never the full key
    pub scopes: Vec<Scope>,
    pub created_by: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub expires_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the `created_by` string from an `AuthSubject`.
/// For API key subjects this is `key:{id}` (stable identifier).
fn created_by(subject: &AuthSubject) -> String {
    match subject {
        AuthSubject::User { username } => username.clone(),
        AuthSubject::Key { key_id, scopes: _ } => format!("key:{}", key_id),
    }
}

/// Check whether `granted` is a subset of `caller`'s scopes.
///
/// A caller can only grant scopes they hold. `management:write` implies
/// `management:read`, so a write-scoped caller can grant both read and write.
fn scopes_are_subset(granted: &[Scope], caller: &[Scope]) -> bool {
    let caller_has_write = caller.contains(&Scope::ManagementWrite);
    for scope in granted {
        let has = match scope {
            Scope::Inference => caller.contains(&Scope::Inference),
            Scope::ManagementRead => caller.contains(&Scope::ManagementRead) || caller_has_write,
            Scope::ManagementWrite => caller_has_write,
        };
        if !has {
            return false;
        }
    }
    true
}

/// Sync the in-memory `api_keys_enabled` config flag with the DB value.
/// Must be called after create/revoke so the auth middleware sees the change.
async fn sync_api_keys_enabled(state: &ProxyState, enabled: bool) {
    let mut config = state.config.write().await;
    config.proxy.api_keys_enabled = enabled;
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /tama/v1/keys — Create a new API key.
///
/// Returns 201 with the plaintext key (returned only once).
pub async fn handle_tama_api_keys_create(
    State(state): State<Arc<ProxyState>>,
    Extension(subject): Extension<AuthSubject>,
    Json(body): Json<CreateApiKeyRequest>,
) -> impl IntoResponse {
    let created_by = created_by(&subject);

    // Validate scopes (non-empty, known values)
    if body.scopes.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "scopes must not be empty",
            Some("ValidationError"),
        );
    }

    // Validate that caller's scopes are a superset of the scopes being granted.
    // OAuth2 users can grant any scope; API keys can only grant scopes they have.
    if let AuthSubject::Key {
        scopes: caller_scopes,
        ..
    } = &subject
    {
        if !scopes_are_subset(&body.scopes, caller_scopes) {
            return json_error(
                StatusCode::FORBIDDEN,
                "cannot grant scopes you do not have",
                Some("ForbiddenError"),
            );
        }
    }

    // Validate expires_at is valid RFC 3339 (if provided)
    if let Some(ref expires_at) = body.expires_at {
        if chrono::DateTime::parse_from_rfc3339(expires_at).is_err() {
            return json_error(
                StatusCode::BAD_REQUEST,
                "expires_at must be a valid RFC 3339 timestamp",
                Some("ValidationError"),
            );
        }
    }

    // Generate key and hash
    let raw_key = api_keys::generate_key();
    let key_prefix = api_keys::extract_prefix(&raw_key);

    // Insert into DB via spawn_blocking
    let name = body.name.clone();
    let scopes = body.scopes.clone();
    let expires_at = body.expires_at.clone();
    let raw_key_for_db = raw_key.clone();
    let created_by_for_db = created_by.clone();
    let state_for_create = state.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = state_for_create.open_db().unwrap();
        ApiKeyStore::new(&conn).create_key(
            &name,
            &raw_key_for_db,
            &scopes,
            &created_by_for_db,
            expires_at.as_deref(),
        )
    })
    .await;

    let key_id = match result {
        Ok(Ok(id)) => id,
        Ok(Err(e)) => {
            warn!(error = %e, "failed to create API key");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create API key",
                None,
            );
        }
        Err(e) => {
            warn!(error = %e, "spawn_blocking panicked creating API key");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create API key",
                None,
            );
        }
    };

    info!(key_id, key_prefix = %key_prefix, creator = %created_by, "API key created");

    // Sync in-memory config so auth_middleware picks up api_keys_enabled = true
    sync_api_keys_enabled(&state, true).await;

    // Fetch the record to get the DB-assigned created_at
    let state_for_record = state.clone();
    let record = tokio::task::spawn_blocking(move || {
        let conn = state_for_record.open_db().unwrap();
        ApiKeyStore::new(&conn).get_key(key_id)
    })
    .await;

    let created_at = match record {
        Ok(Ok(Some(r))) => r.created_at,
        _ => chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    };

    // Return 201 with plaintext key (ONCE)
    let response = CreateApiKeyResponse {
        id: key_id,
        name: body.name,
        key: raw_key,
        scopes: body.scopes,
        expires_at: body.expires_at,
        created_at,
    };

    (StatusCode::CREATED, Json(response)).into_response()
}

/// GET /tama/v1/keys — List all API keys.
///
/// Returns 200 with key metadata (no plaintext keys).
pub async fn handle_tama_api_keys_list(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.open_db().unwrap();
        ApiKeyStore::new(&conn).list_keys()
    })
    .await;

    let keys = match result {
        Ok(Ok(keys)) => keys,
        Ok(Err(e)) => {
            warn!(error = %e, "failed to list API keys");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list API keys",
                None,
            );
        }
        Err(e) => {
            warn!(error = %e, "spawn_blocking panicked listing API keys");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list API keys",
                None,
            );
        }
    };

    let responses: Vec<ListApiKeyResponse> = keys
        .into_iter()
        .map(|record| ListApiKeyResponse {
            id: record.id,
            name: record.name,
            key_prefix: record.key_prefix,
            scopes: record.scopes,
            created_by: record.created_by,
            created_at: record.created_at,
            last_used_at: record.last_used_at,
            revoked_at: record.revoked_at,
            expires_at: record.expires_at,
        })
        .collect();

    (StatusCode::OK, Json(responses)).into_response()
}

/// PATCH /tama/v1/keys/:id — Update an API key's scopes.
///
/// Returns 200 with the updated key metadata.
pub async fn handle_tama_api_keys_update(
    Path(key_id_str): Path<String>,
    State(state): State<Arc<ProxyState>>,
    Extension(subject): Extension<AuthSubject>,
    Json(body): Json<UpdateApiKeyRequest>,
) -> impl IntoResponse {
    // Parse key_id
    let key_id: i64 = match key_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid key ID format",
                Some("ValidationError"),
            );
        }
    };

    // Validate scopes
    if body.scopes.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "scopes must not be empty",
            Some("ValidationError"),
        );
    }

    // Validate that caller's scopes are a superset of the scopes being granted.
    if let AuthSubject::Key {
        scopes: caller_scopes,
        ..
    } = &subject
    {
        if !scopes_are_subset(&body.scopes, caller_scopes) {
            return json_error(
                StatusCode::FORBIDDEN,
                "cannot grant scopes you do not have",
                Some("ForbiddenError"),
            );
        }
    }

    // Validate key exists
    let state_for_check = state.clone();
    let key_exists = tokio::task::spawn_blocking(move || {
        let conn = state_for_check.open_db().unwrap();
        ApiKeyStore::new(&conn).get_key(key_id)
    })
    .await;

    match key_exists {
        Ok(Ok(Some(record))) => {
            if record.revoked_at.is_some() {
                return json_error(
                    StatusCode::NOT_FOUND,
                    "key not found",
                    Some("NotFoundError"),
                );
            }
        }
        Ok(Ok(None)) => {
            return json_error(
                StatusCode::NOT_FOUND,
                "key not found",
                Some("NotFoundError"),
            );
        }
        Ok(Err(e)) => {
            warn!(error = %e, "failed to get API key");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get API key",
                None,
            );
        }
        Err(e) => {
            warn!(error = %e, "spawn_blocking panicked getting API key");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get API key",
                None,
            );
        }
    }

    // Update scopes in DB (returns the updated record)
    let scopes = body.scopes.clone();
    let state_for_update = state.clone();
    let update_result = tokio::task::spawn_blocking(move || {
        let conn = state_for_update.open_db().unwrap();
        ApiKeyStore::new(&conn).update_key_scopes(key_id, &scopes)
    })
    .await;

    match update_result {
        Ok(Ok(record)) => {
            info!(key_id, "API key scopes updated");
            let response = ListApiKeyResponse {
                id: record.id,
                name: record.name,
                key_prefix: record.key_prefix,
                scopes: record.scopes,
                created_by: record.created_by,
                created_at: record.created_at,
                last_used_at: record.last_used_at,
                revoked_at: record.revoked_at,
                expires_at: record.expires_at,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(Err(e)) => {
            warn!(error = %e, "failed to update API key scopes");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update API key scopes",
                None,
            )
        }
        Err(e) => {
            warn!(error = %e, "spawn_blocking panicked updating API key scopes");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update API key scopes",
                None,
            )
        }
    }
}

/// DELETE /tama/v1/keys/:id — Revoke an API key (soft delete).
///
/// Idempotent — returns 204 for already-revoked keys.
pub async fn handle_tama_api_keys_revoke(
    Path(key_id_str): Path<String>,
    State(state): State<Arc<ProxyState>>,
) -> impl IntoResponse {
    // Parse key_id
    let key_id: i64 = match key_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid key ID format",
                Some("ValidationError"),
            );
        }
    };

    // Validate key exists (already-revoked keys are accepted — revoke is idempotent)
    let state_for_check = state.clone();
    let key_exists = tokio::task::spawn_blocking(move || {
        let conn = state_for_check.open_db().unwrap();
        ApiKeyStore::new(&conn).get_key(key_id)
    })
    .await;

    match key_exists {
        Ok(Ok(Some(record))) => {
            // Already revoked — idempotent, return 204
            if record.revoked_at.is_some() {
                return StatusCode::NO_CONTENT.into_response();
            }
        }
        Ok(Ok(None)) => {
            return json_error(
                StatusCode::NOT_FOUND,
                "key not found",
                Some("NotFoundError"),
            );
        }
        Ok(Err(e)) => {
            warn!(error = %e, "failed to get API key");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get API key",
                None,
            );
        }
        Err(e) => {
            warn!(error = %e, "spawn_blocking panicked getting API key");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get API key",
                None,
            );
        }
    }

    // Revoke in DB
    let state_for_revoke = state.clone();
    let revoke_result = tokio::task::spawn_blocking(move || {
        let conn = state_for_revoke.open_db().unwrap();
        ApiKeyStore::new(&conn).revoke_key(key_id)
    })
    .await;

    match revoke_result {
        Ok(Ok(enabled)) => {
            info!(key_id, "API key revoked");
            // Sync in-memory config (revoke may have cleared api_keys_enabled)
            sync_api_keys_enabled(&state, enabled).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Err(e)) => {
            warn!(error = %e, "failed to revoke API key");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to revoke API key",
                None,
            )
        }
        Err(e) => {
            warn!(error = %e, "spawn_blocking panicked revoking API key");
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to revoke API key",
                None,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware;
    use axum::routing::{get, patch};
    use axum::Router;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    /// Helper: create a temporary directory with a DB containing an API key.
    /// Returns the proxy state, the temp dir (kept alive), and the raw key.
    fn make_test_db(scopes: &[Scope]) -> (Arc<ProxyState>, tempfile::TempDir, String) {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("tama.db");
        let db_dir = temp_dir.path().to_path_buf();

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        crate::db::migrations::run(&conn).unwrap();
        crate::db::queries::seed_defaults(&conn).unwrap();

        let key = api_keys::generate_key();
        ApiKeyStore::new(&conn)
            .create_key("test-key", &key, scopes, "admin", None)
            .unwrap();

        let config = crate::config::Config {
            proxy: crate::config::ProxyConfig {
                authenticator_skip_paths: vec![
                    "/health".to_string(),
                    "/metrics".to_string(),
                    "/login".to_string(),
                    "/login/callback".to_string(),
                    "/login/error".to_string(),
                ],
                api_keys_enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let proxy_state = Arc::new(ProxyState::new(config, Some(db_dir)));

        (proxy_state, temp_dir, key)
    }

    /// Build an app with auth middleware + API key handlers.
    fn make_api_keys_app(state: Arc<ProxyState>) -> Router {
        Router::new()
            .route(
                "/tama/v1/keys",
                get(handle_tama_api_keys_list).post(handle_tama_api_keys_create),
            )
            .route(
                "/tama/v1/keys/:id",
                patch(handle_tama_api_keys_update).delete(handle_tama_api_keys_revoke),
            )
            .layer(middleware::from_fn(
                crate::proxy::scope_middleware::scope_middleware,
            ))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                crate::proxy::auth::auth_middleware,
            ))
            .with_state(state)
    }

    /// Test: POST /tama/v1/keys creates key, returns 201 with plaintext key.
    #[tokio::test]
    async fn test_create_key_returns_201_with_plaintext() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        let app = make_api_keys_app(state);

        let request_body = serde_json::json!({
            "name": "my-new-key",
            "scopes": ["inference"],
            "expires_at": null
        });

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tama/v1/keys")
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(request_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

        // Response must include the plaintext key
        assert!(
            body.get("key").is_some(),
            "response must include 'key' field"
        );
        let key_val = body["key"].as_str().unwrap();
        assert!(key_val.starts_with("tama_"), "key must start with tama_");
        assert_eq!(body["name"], "my-new-key");
        assert!(body["id"].as_i64().unwrap() > 0);
    }

    /// Test: GET /tama/v1/keys lists keys with key_prefix, no plaintext.
    #[tokio::test]
    async fn test_list_keys_excludes_plaintext() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        // Create another key first
        let app = make_api_keys_app(state.clone());
        let request_body = serde_json::json!({
            "name": "listed-key",
            "scopes": ["inference"],
            "expires_at": null
        });
        let _resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tama/v1/keys")
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(request_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Now list
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/tama/v1/keys")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: Vec<serde_json::Value> = serde_json::from_slice(&body_bytes).unwrap();

        assert!(!body.is_empty(), "should have at least one key");
        for item in &body {
            // Must have key_prefix
            assert!(
                item.get("key_prefix").is_some(),
                "response must include 'key_prefix'"
            );
            // Must NOT have plaintext key field
            assert!(
                item.get("key").is_none(),
                "response must NOT include 'key' (plaintext)"
            );
        }
    }

    /// Test: PATCH /tama/v1/keys/:id updates scopes.
    #[tokio::test]
    async fn test_update_key_scopes() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        // Create a key first
        let app = make_api_keys_app(state.clone());
        let create_body = serde_json::json!({
            "name": "updatable-key",
            "scopes": ["inference"],
            "expires_at": null
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tama/v1/keys")
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let key_id = body["id"].as_i64().unwrap();

        // Update scopes
        let update_body = serde_json::json!({
            "scopes": ["management-read", "management-write"]
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/tama/v1/keys/{}", key_id))
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let scopes: Vec<String> = body["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert!(scopes.contains(&"management-read".to_string()));
        assert!(scopes.contains(&"management-write".to_string()));
    }

    /// Test: PATCH with empty scopes returns 400.
    #[tokio::test]
    async fn test_update_key_invalid_scopes_returns_400() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        // Create a key first
        let app = make_api_keys_app(state.clone());
        let create_body = serde_json::json!({
            "name": "updatable-key",
            "scopes": ["inference"],
            "expires_at": null
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tama/v1/keys")
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let key_id = body["id"].as_i64().unwrap();

        // Update with empty scopes
        let update_body = serde_json::json!({
            "scopes": []
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/tama/v1/keys/{}", key_id))
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["error"]["type"], "ValidationError");
        assert_eq!(body["error"]["message"], "scopes must not be empty");
    }

    /// Test: DELETE /tama/v1/keys/:id revokes key, returns 204.
    #[tokio::test]
    async fn test_revoke_key_returns_204() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        // Create a key first
        let app = make_api_keys_app(state.clone());
        let create_body = serde_json::json!({
            "name": "revocable-key",
            "scopes": ["inference"],
            "expires_at": null
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tama/v1/keys")
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let key_id = body["id"].as_i64().unwrap();

        // Revoke
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tama/v1/keys/{}", key_id))
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    /// Test: DELETE nonexistent key returns 404.
    #[tokio::test]
    async fn test_revoke_nonexistent_key_returns_404() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        let app = make_api_keys_app(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/tama/v1/keys/99999")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["error"]["type"], "NotFoundError");
        assert_eq!(body["error"]["message"], "key not found");
    }

    /// Test: POST with empty scopes returns 400.
    #[tokio::test]
    async fn test_create_key_empty_scopes_returns_400() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        let app = make_api_keys_app(state);

        let request_body = serde_json::json!({
            "name": "empty-scopes-key",
            "scopes": [],
            "expires_at": null
        });

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tama/v1/keys")
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(request_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["error"]["type"], "ValidationError");
        assert_eq!(body["error"]["message"], "scopes must not be empty");
    }

    /// Test: Full CRUD flow — create → list → update → validate scopes → revoke → validate fails.
    #[tokio::test]
    async fn test_key_crud_full_flow() {
        let (state, _temp_dir, _admin_key) = make_test_db(&[Scope::ManagementWrite]);

        let app = make_api_keys_app(state);

        // 1. Create a key
        let create_body = serde_json::json!({
            "name": "full-flow-key",
            "scopes": ["inference"],
            "expires_at": null
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tama/v1/keys")
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(create_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let key_id = body["id"].as_i64().unwrap();
        let plaintext_key = body["key"].as_str().unwrap().to_string();
        assert!(plaintext_key.starts_with("tama_"));
        assert_eq!(body["name"], "full-flow-key");

        // 2. List keys — should include the new key
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/tama/v1/keys")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let list: Vec<serde_json::Value> = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let found = list
            .iter()
            .find(|k| k["id"].as_i64().unwrap() == key_id)
            .expect("key not in list");
        assert_eq!(found["name"], "full-flow-key");
        assert!(found["key_prefix"].as_str().unwrap().starts_with("tama_"));
        assert!(
            found["key"].is_null(),
            "list must not include plaintext key"
        );

        // 3. Update scopes
        let update_body = serde_json::json!({
            "scopes": ["management-read", "management-write"]
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/tama/v1/keys/{}", key_id))
                    .header("Content-Type", "application/json")
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::from(update_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let updated: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let updated_scopes: Vec<String> = updated["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert!(updated_scopes.contains(&"management-read".to_string()));
        assert!(updated_scopes.contains(&"management-write".to_string()));

        // 4. Revoke the key
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tama/v1/keys/{}", key_id))
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // 5. Revoke again — idempotent, returns 204
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/tama/v1/keys/{}", key_id))
                    .header("X-Authentik-Username", "testuser")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
}
