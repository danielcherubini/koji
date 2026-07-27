//! Authentik auth middleware for Axum.
//!
//! Validates bearer tokens against Authentik's user-info API,
//! falling back to Caddy forward_auth headers for browser sessions.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::proxy::api_keys::AuthSubject;

use super::session::extract_session;

// --- Authentik user-info response ---

#[derive(Debug, Deserialize)]
struct AuthentikUserResponse {
    user: AuthentikUser,
}

#[derive(Debug, Deserialize)]
struct AuthentikUser {
    username: String,
}

// --- Middleware function ---

pub async fn auth_middleware(
    State(proxy_state): State<Arc<crate::proxy::ProxyState>>,
    req: Request,
    next: Next,
) -> Response {
    // Read auth settings from shared state (live, not snapshot)
    let config = proxy_state.config.read().await;
    let auth_url = config.proxy.authenticator_url.clone();
    let skip_paths = config.proxy.authenticator_skip_paths.clone();
    let oauth2_enabled = config.proxy.oauth2.enabled;
    let api_keys_enabled = config.proxy.api_keys_enabled;
    drop(config);

    // 1. If no auth configured at all, pass through (open mode)
    let auth_configured =
        auth_url.as_deref().is_some_and(|u| !u.is_empty()) || oauth2_enabled || api_keys_enabled;
    if !auth_configured {
        return next.run(req).await;
    }

    // 2. Check skip_paths (prefix matching: "/health" also matches "/healthcheck")
    //    The OAuth2 login flow routes are ALWAYS skipped — they're the auth entry points.
    let path = req.uri().path().to_string();
    const LOGIN_SKIP_PATHS: &[&str] = &["/login", "/login/callback", "/login/error"];
    if LOGIN_SKIP_PATHS.iter().any(|p| path.starts_with(p))
        || skip_paths.iter().any(|p| path.starts_with(p.as_str()))
    {
        return next.run(req).await;
    }

    // 3. Check session cookie (OIDC login)
    if let Some(claims) = extract_session(&req, &proxy_state) {
        debug!("Authenticated user via session cookie: {}", claims.username);
        let subject = AuthSubject::User {
            username: claims.username,
        };
        let mut req = req;
        req.extensions_mut().insert(subject);
        return next.run(req).await;
    }

    // 4. Check for bearer token
    if let Some(bearer_token) = extract_bearer_token(&req) {
        // 4a. API key bearer token (tama_ prefix, case-sensitive)
        if bearer_token.starts_with("tama_") {
            // If API keys are disabled, reject
            if !api_keys_enabled {
                warn!(
                    remote_addr = ?super::api_key::extract_remote_addr(&req),
                    "API key rejected: api_keys_enabled is false"
                );
                return (
                    StatusCode::UNAUTHORIZED,
                    super::api_key::json_unauthorized_api_keys(),
                )
                    .into_response();
            }

            // Validate against database (spawn_blocking for rusqlite)
            let raw_token = bearer_token.clone();
            let raw_token_for_db = raw_token.clone();
            let db_result = tokio::task::spawn_blocking(move || {
                let db = proxy_state.open_db();
                db.map(|conn| {
                    crate::proxy::api_keys::ApiKeyStore::new(&conn).validate_key(&raw_token_for_db)
                })
            })
            .await;

            match db_result {
                Ok(Some(Ok(Some((key_id, scopes))))) => {
                    // Successful validation
                    let key_prefix = crate::proxy::api_keys::extract_prefix(&raw_token);
                    info!(
                        key_id,
                        key_prefix = %key_prefix,
                        "API key authenticated"
                    );
                    let subject = AuthSubject::Key { key_id, scopes };
                    let mut req = req;
                    req.extensions_mut().insert(subject);
                    return next.run(req).await;
                }
                Ok(Some(Ok(None))) => {
                    // Key not found in database
                    let key_prefix_attempted = crate::proxy::api_keys::extract_prefix(&raw_token);
                    warn!(
                        key_prefix_attempted = %key_prefix_attempted,
                        reason = "key not found in database",
                        "API key validation failed"
                    );
                    return (
                        StatusCode::UNAUTHORIZED,
                        super::api_key::json_unauthorized_invalid_key(),
                    )
                        .into_response();
                }
                Ok(Some(Err(e))) => {
                    warn!(
                        error = %e,
                        reason = "database error during key validation",
                        "API key validation failed"
                    );
                    return (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response();
                }
                Ok(None) => {
                    // No database connection available
                    warn!(
                        reason = "no database connection",
                        "API key validation failed"
                    );
                    return (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response();
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        reason = "spawn_blocking panicked",
                        "API key validation failed"
                    );
                    return (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response();
                }
            }
        } else {
            // 4b. Non-tama_ bearer token — validate against Authentik
            let authenticator_url = auth_url.as_deref().unwrap_or("");
            match validate_token_against_authentik(
                &proxy_state.client,
                authenticator_url,
                &bearer_token,
            )
            .await
            {
                Ok(username) => {
                    debug!("Authenticated user via bearer token: {}", username);
                    let subject = AuthSubject::User { username };
                    let mut req = req;
                    req.extensions_mut().insert(subject);
                    return next.run(req).await;
                }
                Err(status) => {
                    return (status, json_unauthorized()).into_response();
                }
            }
        }
    }

    // 5. Check for X-Authentik-Username header (Caddy forward_auth)
    if let Some(username) = req
        .headers()
        .get("X-Authentik-Username")
        .and_then(|v| v.to_str().ok())
    {
        debug!("Authenticated user via Caddy forward_auth: {}", username);
        let subject = AuthSubject::User {
            username: username.to_string(),
        };
        let mut req = req;
        req.extensions_mut().insert(subject);
        return next.run(req).await;
    }

    // 6. No valid auth — for browser requests, redirect to /login
    // For API requests (Accept: application/json), return 401
    let is_browser = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);

    if is_browser && oauth2_enabled {
        (StatusCode::FOUND, [(header::LOCATION, "/login")]).into_response()
    } else {
        (StatusCode::UNAUTHORIZED, json_unauthorized()).into_response()
    }
}

// --- Helper functions ---

fn extract_bearer_token(req: &Request) -> Option<String> {
    let header = req.headers().get(header::AUTHORIZATION)?;
    let value = header.to_str().ok()?;
    value.strip_prefix("Bearer ").map(|s| s.to_string())
}

async fn validate_token_against_authentik(
    client: &reqwest::Client,
    authenticator_url: &str,
    token: &str,
) -> Result<String, StatusCode> {
    let url = format!("{}/api/v3/core/users/me/", authenticator_url);
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: AuthentikUserResponse = r.json().await.map_err(|e| {
                warn!("Failed to parse Authentik user response: {}", e);
                StatusCode::UNAUTHORIZED
            })?;
            Ok(body.user.username)
        }
        Ok(r) if r.status() == StatusCode::UNAUTHORIZED => Err(StatusCode::UNAUTHORIZED),
        // Any non-success non-401 (403, 500, 429) is treated as unauthorized.
        // This is intentional: if Authentik signals the token is bad, deny access.
        Ok(_) => Err(StatusCode::UNAUTHORIZED),
        Err(e) => {
            // Fail-open: if Authentik is unreachable (connection timeout, DNS failure),
            // allow the request through with a warning log.
            warn!("Authentik auth API unreachable ({}), allowing request", e);
            Ok("unknown".to_string())
        }
    }
}

pub(super) fn json_unauthorized() -> Response {
    let body = serde_json::json!({
        "error": "Authentication required",
        "detail": "Provide a valid Authorization: Bearer <token> header"
    })
    .to_string();
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .expect("build unauthorized response")
}
