use serde::{Deserialize, Serialize};
use tracing;

/// OAuth2/OpenID Connect configuration for browser-based authentication.
///
/// When `enabled` is true, the proxy will redirect unauthenticated browser
/// requests to the configured authorize endpoint and exchange authorization
/// codes for bearer tokens via the token endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Config {
    /// Whether OAuth2 login is enabled.
    pub enabled: bool,
    /// OAuth2 client ID.
    pub client_id: String,
    /// Supports env var interpolation: "${ENV_VAR_NAME}" is resolved at startup.
    pub client_secret: String,
    /// Authorization endpoint URL.
    pub authorize_url: String,
    /// Token endpoint URL.
    pub token_url: String,
    /// Optional — used to fetch user claims after token exchange.
    pub userinfo_url: Option<String>,
    /// Optional — RP-initiated logout endpoint.
    pub logout_url: Option<String>,
    /// Redirect URI registered with the OAuth2 provider.
    pub redirect_uri: String,
    /// Scopes to request. Default: ["openid", "profile", "email"].
    pub scopes: Vec<String>,
    /// Session TTL in seconds. Default: 86400 (24 hours).
    pub session_ttl_secs: u64,
}

impl Default for OAuth2Config {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            client_secret: String::new(),
            authorize_url: String::new(),
            token_url: String::new(),
            userinfo_url: None,
            logout_url: None,
            redirect_uri: String::new(),
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
            ],
            session_ttl_secs: 86_400, // 24 hours
        }
    }
}

/// Resolve a `${VAR_NAME}` reference to the environment variable value.
///
/// If the env var is not set, the original string is kept with a warning.
fn resolve_env_var_ref(value: &str) -> String {
    if let Some(inner) = value.strip_prefix("${").and_then(|s| s.strip_suffix("}")) {
        std::env::var(inner).unwrap_or_else(|_| {
            tracing::warn!(
                env_var = inner,
                "Environment variable not set, using original value"
            );
            value.to_string()
        })
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default = "default_proxy_host")]
    pub host: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    /// Whether to automatically unload models after a period of inactivity.
    /// When false, models stay loaded until explicitly unloaded or evicted by LRU.
    #[serde(default)]
    pub auto_unload: bool,
    /// How long (in seconds) a model must be idle before it is automatically unloaded.
    /// Only takes effect when `auto_unload` is true. Default: 300 (5 minutes).
    #[serde(default = "default_proxy_timeout")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_secs: u64,
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    #[serde(default = "default_circuit_breaker_cooldown")]
    pub circuit_breaker_cooldown_seconds: u64,
    #[serde(default = "default_metrics_retention")]
    pub metrics_retention_secs: u64,
    /// How often the pull queue processor checks for new items (in seconds).
    /// Default is 2, minimum is 1.
    #[serde(default = "default_download_queue_poll_interval")]
    pub download_queue_poll_interval_secs: u64,
    /// Maximum number of models that can be loaded simultaneously **per GPU
    /// device**. When a new model is requested and the limit is reached for
    /// that GPU, the least-recently-used (LRU) model on that GPU is
    /// automatically unloaded first. Set to 0 for unlimited (disabled).
    /// Default: 1.
    ///
    /// For example, with `max_loaded_models = 1` and 2 GPUs (CUDA0, CUDA1),
    /// you can have 1 model on CUDA0 AND 1 model on CUDA1 simultaneously.
    #[serde(default = "default_max_loaded_models")]
    pub max_loaded_models: u32,
    /// Authentik instance URL for bearer token validation.
    /// When set, all requests require auth (except paths in skip_paths).
    /// Example: "https://auth.wizards.town"
    #[serde(default)]
    pub authenticator_url: Option<String>,
    /// Paths exempt from authentication.
    /// Default: ["/health", "/metrics"] — internal endpoints not exposed via Caddy.
    #[serde(default)]
    pub authenticator_skip_paths: Vec<String>,
    /// OAuth2/OpenID Connect configuration for browser-based authentication.
    #[serde(default)]
    pub oauth2: OAuth2Config,
    /// Whether API key authentication is enabled.
    /// When true, bearer tokens starting with "tama_" are validated against the database.
    #[serde(default)]
    pub api_keys_enabled: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            host: default_proxy_host(),
            port: default_proxy_port(),
            auto_unload: false,
            idle_timeout_secs: default_proxy_timeout(),
            startup_timeout_secs: default_startup_timeout(),
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            circuit_breaker_cooldown_seconds: default_circuit_breaker_cooldown(),
            metrics_retention_secs: default_metrics_retention(),
            download_queue_poll_interval_secs: default_download_queue_poll_interval(),
            max_loaded_models: default_max_loaded_models(),
            authenticator_url: None,
            authenticator_skip_paths: vec!["/health".to_string(), "/metrics".to_string()],
            oauth2: OAuth2Config::default(),
            api_keys_enabled: false,
        }
    }
}

impl ProxyConfig {
    /// Resolve environment variable references in OAuth2 client_secret.
    ///
    /// "${VAR_NAME}" is replaced with the value of VAR_NAME at runtime.
    /// If the env var is not set, the original string is kept (with a warning).
    pub fn resolve_env_vars(&mut self) {
        if self.oauth2.enabled {
            self.oauth2.client_secret = resolve_env_var_ref(&self.oauth2.client_secret);
        }
    }
}

fn default_proxy_host() -> String {
    "0.0.0.0".to_string()
}

pub const DEFAULT_PROXY_PORT: u16 = 11434;

fn default_proxy_port() -> u16 {
    DEFAULT_PROXY_PORT
}

fn default_proxy_timeout() -> u64 {
    300
}

fn default_startup_timeout() -> u64 {
    120
}

fn default_circuit_breaker_threshold() -> u32 {
    3
}

fn default_circuit_breaker_cooldown() -> u64 {
    60
}

fn default_metrics_retention() -> u64 {
    86_400
}

fn default_download_queue_poll_interval() -> u64 {
    2
}

fn default_max_loaded_models() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that `${VAR_NAME}` is resolved when env var is set.
    #[test]
    fn test_resolve_env_var_ref_resolves_set_var() {
        std::env::set_var("TAMA_TEST_SECRET", "my-secret-value");
        let result = resolve_env_var_ref("${TAMA_TEST_SECRET}");
        assert_eq!(result, "my-secret-value");
        std::env::remove_var("TAMA_TEST_SECRET");
    }

    /// Test that `${VAR_NAME}` keeps original value when env var is not set.
    #[test]
    fn test_resolve_env_var_ref_keeps_unset_var() {
        std::env::remove_var("TAMA_NONEXISTENT_VAR");
        let result = resolve_env_var_ref("${TAMA_NONEXISTENT_VAR}");
        assert_eq!(result, "${TAMA_NONEXISTENT_VAR}");
    }

    /// Test that a literal value (no `${}`) is returned unchanged.
    #[test]
    fn test_resolve_env_var_ref_literal_unchanged() {
        let result = resolve_env_var_ref("literal-secret-value");
        assert_eq!(result, "literal-secret-value");
    }

    /// Test that `resolve_env_vars` resolves client_secret when OAuth2 is enabled.
    #[test]
    fn test_proxy_config_resolve_env_vars_enabled() {
        std::env::set_var("TAMA_OAUTH_SECRET", "resolved-secret");
        let mut config = ProxyConfig {
            oauth2: OAuth2Config {
                enabled: true,
                client_secret: "${TAMA_OAUTH_SECRET}".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        config.resolve_env_vars();
        assert_eq!(config.oauth2.client_secret, "resolved-secret");
        std::env::remove_var("TAMA_OAUTH_SECRET");
    }

    /// Test that `resolve_env_vars` skips client_secret when OAuth2 is disabled.
    #[test]
    fn test_proxy_config_resolve_env_vars_disabled_skips() {
        std::env::set_var("TAMA_OAUTH_SECRET", "resolved-secret");
        let mut config = ProxyConfig {
            oauth2: OAuth2Config {
                enabled: false,
                client_secret: "${TAMA_OAUTH_SECRET}".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        config.resolve_env_vars();
        // Should NOT resolve because OAuth2 is disabled
        assert_eq!(config.oauth2.client_secret, "${TAMA_OAUTH_SECRET}");
        std::env::remove_var("TAMA_OAUTH_SECRET");
    }
}
