//! Session claims and helpers for signed cookie-based authentication.

use axum::{extract::Request, http::header};
use serde::{Deserialize, Serialize};

/// Cookie name for the session token.
pub(super) const SESSION_COOKIE_NAME: &str = "tama_session";

/// Session claims stored in the signed session cookie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SessionClaims {
    /// User identifier from the OAuth2 provider.
    pub(super) sub: String,
    /// Display name for the user.
    pub(super) username: String,
    /// User email, if available.
    pub(super) email: Option<String>,
    /// Issued-at timestamp (Unix seconds).
    pub(super) iat: i64,
    /// Expiration timestamp (Unix seconds).
    pub(super) exp: i64,
}

impl SessionClaims {
    /// Create new session claims with the given username, email, and TTL.
    pub(super) fn new(username: String, email: Option<String>, ttl_secs: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            sub: username.clone(),
            username,
            email,
            iat: now,
            exp: now + ttl_secs as i64,
        }
    }

    /// Returns true if the session has not yet expired.
    pub(super) fn is_valid(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        now < self.exp
    }
}

/// Extract and validate session claims from request cookies (signature verified).
pub(super) fn extract_session(
    req: &Request,
    state: &crate::proxy::ProxyState,
) -> Option<SessionClaims> {
    let raw = req.headers().get(header::COOKIE)?.to_str().ok()?;
    let cookie_value = raw
        .split(';')
        .map(|p| p.trim())
        .find(|p| p.starts_with(&format!("{}=", SESSION_COOKIE_NAME)))
        .and_then(|p| p.split_once('=').map(|x| x.1))?;
    // Build a jar from the raw cookie and verify signature.
    // Use parse_encoded to URL-decode the value (it was set via .encoded()),
    // otherwise the HMAC signature won't verify.
    let mut jar = cookie::CookieJar::new();
    jar.add_original(
        cookie::Cookie::parse_encoded(format!("{}={}", SESSION_COOKIE_NAME, cookie_value)).ok()?,
    );
    let verified = jar.signed(&state.cookie_key).get(SESSION_COOKIE_NAME)?;
    let claims: SessionClaims = serde_json::from_str(verified.value()).ok()?;
    Some(claims).filter(|c| c.is_valid())
}

/// Build a Set-Cookie header value for the session (HMAC-signed).
pub(super) fn session_cookie(
    claims: &SessionClaims,
    is_secure: bool,
    state: &crate::proxy::ProxyState,
) -> String {
    let value = serde_json::to_string(claims).unwrap_or_default();
    let mut c = cookie::Cookie::build((SESSION_COOKIE_NAME, value))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .max_age(cookie::time::Duration::seconds(claims.exp - claims.iat));
    if is_secure {
        c = c.secure(true);
    }
    let finished = c.finish();
    // Sign the cookie so the client cannot forge session claims
    let mut jar = cookie::CookieJar::new();
    jar.signed_mut(&state.cookie_key).add(finished);
    jar.get(SESSION_COOKIE_NAME)
        .map(|c| c.encoded().to_string())
        .unwrap_or_default()
}
