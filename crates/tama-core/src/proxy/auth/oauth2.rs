//! OAuth2 login flow handlers.

use axum::{
    extract::{Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, EndpointNotSet, EndpointSet, RedirectUrl,
    TokenResponse, TokenUrl,
};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::config::types::OAuth2Config;

use super::session::{session_cookie, SessionClaims, SESSION_COOKIE_NAME};

/// Cookie name for the OAuth2 CSRF state token.
pub(super) const CSRF_STATE_COOKIE_NAME: &str = "tama_oauth2_state";

/// Build an OAuth2 BasicClient from an OAuth2Config directly (avoids holding config lock).
pub(super) fn build_oauth2_client_from_config(
    oauth2: &OAuth2Config,
) -> Result<
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>,
    anyhow::Error,
> {
    let client = BasicClient::new(ClientId::new(oauth2.client_id.clone()))
        .set_client_secret(ClientSecret::new(oauth2.client_secret.clone()))
        .set_auth_uri(AuthUrl::new(oauth2.authorize_url.clone())?)
        .set_token_uri(TokenUrl::new(oauth2.token_url.clone())?)
        .set_redirect_uri(RedirectUrl::new(oauth2.redirect_uri.clone())?);
    Ok(client)
}

/// Fetch user info from the provider's userinfo endpoint.
///
/// Returns `(username, email)` extracted from the userinfo response.
pub(super) async fn fetch_userinfo(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
) -> (String, Option<String>) {
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Accept", "application/json")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = match r.json().await {
                Ok(v) => v,
                Err(e) => {
                    warn!(url = %url, error = %e, "Failed to parse userinfo JSON response");
                    return ("unknown".to_string(), None);
                }
            };
            let username = body
                .get("preferred_username")
                .or_else(|| body.get("nickname"))
                .or_else(|| body.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let email = body
                .get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (username, email)
        }
        Ok(r) => {
            warn!(
                url = %url,
                status = ?r.status(),
                "Userinfo endpoint returned non-success status"
            );
            ("unknown".to_string(), None)
        }
        Err(e) => {
            warn!(url = %url, error = %e, "Failed to call userinfo endpoint");
            ("unknown".to_string(), None)
        }
    }
}

/// Redirect to /login with an error query parameter.
fn redirect_to_login_error(reason: &str, description: &str) -> Response {
    let url = format!(
        "/login/error?reason={}&description={}",
        percent_encoding::utf8_percent_encode(reason, percent_encoding::NON_ALPHANUMERIC),
        percent_encoding::utf8_percent_encode(description, percent_encoding::NON_ALPHANUMERIC),
    );
    (StatusCode::FOUND, [(header::LOCATION, url)]).into_response()
}

/// Minimal HTML-escaper for the login error page.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

// ── OAuth2 handlers ────────────────────────────────────────────────────────

/// Handle GET /login/error — display an OAuth2 login error to the user.
pub async fn handle_login_error(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
    let reason = query.get("reason").map(String::as_str).unwrap_or("unknown");
    let description = query
        .get("description")
        .map(String::as_str)
        .unwrap_or("An unknown error occurred during login.");
    let body = format!(
        "<html><head><title>Login Error</title></head>\n\
         <body style=\"font-family: sans-serif; max-width: 600px; margin: 80px auto; text-align: center;\">\n\
         <h1>Login Error</h1>\n\
         <p><strong>Reason:</strong> {}</p>\n\
         <p>{}</p>\n\
         <p><a href=\"/\">Try again</a></p>\n\
         </body></html>",
        html_escape(reason),
        html_escape(description)
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
}

/// Handle GET /login — redirect to the OAuth2 provider's authorize endpoint.
pub async fn handle_login(State(state): State<Arc<crate::proxy::ProxyState>>) -> impl IntoResponse {
    let config = state.config.read().await;
    if !config.proxy.oauth2.enabled {
        drop(config);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "OAuth2 login is not configured",
        )
            .into_response();
    }
    let oauth2 = config.proxy.oauth2.clone();
    drop(config);

    let oauth2_client = match build_oauth2_client_from_config(&oauth2) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("OAuth2 misconfigured: {}", e),
            )
                .into_response();
        }
    };

    let mut auth_request = oauth2_client.authorize_url(oauth2::CsrfToken::new_random);
    for scope in &oauth2.scopes {
        auth_request = auth_request.add_scope(oauth2::Scope::new(scope.clone()));
    }
    let (url, csrf_state) = auth_request.url();

    // Store CSRF state in a short-lived signed cookie (5 min TTL)
    let state_value = csrf_state.secret().clone();
    let state_cookie_raw = cookie::Cookie::build((CSRF_STATE_COOKIE_NAME, state_value))
        .path("/login/callback")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(300))
        .finish();
    let mut state_jar = cookie::CookieJar::new();
    state_jar
        .signed_mut(&state.cookie_key)
        .add(state_cookie_raw);
    let state_cookie = state_jar
        .get(CSRF_STATE_COOKIE_NAME)
        .map(|c| c.encoded().to_string())
        .unwrap_or_default();

    let location = url.to_string();
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, location),
            (header::SET_COOKIE, state_cookie.clone()),
        ],
    )
        .into_response()
}

/// Handle GET /login/callback — verify CSRF, exchange code, set session.
pub async fn handle_login_callback(
    State(state): State<Arc<crate::proxy::ProxyState>>,
    query: Query<HashMap<String, String>>,
    req: Request,
) -> impl IntoResponse {
    // Extract and verify CSRF state from signed cookie
    let raw_cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok());
    let expected_state = raw_cookie
        .and_then(|raw| {
            raw.split(';')
                .map(|p| p.trim())
                .find(|p| p.starts_with(&format!("{}=", CSRF_STATE_COOKIE_NAME)))
                .and_then(|p| p.split_once('=').map(|x| x.1))
        })
        .and_then(|cookie_value| {
            // Build a jar from the raw cookie and verify HMAC signature.
            // Use parse_encoded to URL-decode the value (it was set via .encoded()),
            // otherwise the HMAC signature won't verify.
            let mut jar = cookie::CookieJar::new();
            jar.add_original(
                cookie::Cookie::parse_encoded(format!(
                    "{}={}",
                    CSRF_STATE_COOKIE_NAME, cookie_value
                ))
                .ok()?,
            );
            jar.signed(&state.cookie_key)
                .get(CSRF_STATE_COOKIE_NAME)
                .map(|c| c.value().to_string())
        });
    let expected_state = match expected_state {
        Some(s) => s,
        None => return redirect_to_login_error("state", "CSRF state missing"),
    };

    // Extract code and state from query params
    let code = match query.get("code") {
        Some(c) => oauth2::AuthorizationCode::new(c.clone()),
        None => return redirect_to_login_error("code", "Authorization code missing"),
    };
    let returned_state = match query.get("state") {
        Some(s) => s.clone(),
        None => return redirect_to_login_error("state", "State parameter missing"),
    };

    // Verify CSRF state matches
    if returned_state != expected_state {
        return redirect_to_login_error("state", "CSRF state mismatch");
    }

    // Clone all needed config upfront — drop lock before any async I/O
    let config = state.config.read().await;
    let oauth2_config = config.proxy.oauth2.clone();
    drop(config);

    // Exchange code for tokens
    let oauth2_client = match build_oauth2_client_from_config(&oauth2_config) {
        Ok(c) => c,
        Err(e) => return redirect_to_login_error("config", &e.to_string()),
    };
    let http_client = state.client.clone();
    let token_response = match oauth2_client
        .exchange_code(code)
        .request_async(&http_client)
        .await
    {
        Ok(t) => t,
        Err(e) => return redirect_to_login_error("token", &format!("{}", e)),
    };

    // Fetch userinfo if configured
    let (username, email) = if let Some(ref userinfo_url) = oauth2_config.userinfo_url {
        fetch_userinfo(
            &state.client,
            userinfo_url,
            token_response.access_token().secret(),
        )
        .await
    } else {
        ("unknown".to_string(), None)
    };

    // Create signed session claims and cookie
    let claims = SessionClaims::new(username, email, oauth2_config.session_ttl_secs);
    let set_cookie = session_cookie(&claims, false, &state);
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, "/tama"),
            (header::SET_COOKIE, &set_cookie),
        ],
    )
        .into_response()
}

/// Handle GET /logout — clear session cookie and redirect.
pub async fn handle_logout(
    State(state): State<Arc<crate::proxy::ProxyState>>,
) -> impl IntoResponse {
    let config = state.config.read().await;
    let logout_url = config.proxy.oauth2.logout_url.clone();
    drop(config);

    // Create expired session cookie to clear it
    let cleared = cookie::Cookie::build((SESSION_COOKIE_NAME, ""))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(0))
        .finish();

    let cleared_cookie = cleared.encoded().to_string();
    if let Some(url) = logout_url {
        (
            StatusCode::FOUND,
            [
                (header::LOCATION, url.as_str()),
                (header::SET_COOKIE, &cleared_cookie),
            ],
        )
            .into_response()
    } else {
        (
            StatusCode::FOUND,
            [
                (header::LOCATION, "/tama"),
                (header::SET_COOKIE, &cleared_cookie),
            ],
        )
            .into_response()
    }
}
