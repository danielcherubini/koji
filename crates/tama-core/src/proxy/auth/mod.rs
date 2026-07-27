//! Authentik auth middleware for Axum.
//!
//! Validates bearer tokens against Authentik's user-info API,
//! falling back to Caddy forward_auth headers for browser sessions.

mod api_key;
mod middleware;
mod oauth2;
mod session;
#[cfg(test)]
mod tests;

/// AuthConfig is no longer used; auth settings are read live from ProxyState.
/// The type is retained for backward compatibility but is empty.
#[derive(Clone, Debug)]
pub struct AuthConfig;

pub use middleware::auth_middleware;
pub use oauth2::{handle_login, handle_login_callback, handle_login_error, handle_logout};
