//! Proxy configuration (WASM mirror).

use serde::{Deserialize, Serialize};

/// OAuth2/OpenID Connect configuration (WASM mirror).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OAuth2Config {
    /// Whether OAuth2 login is enabled.
    pub enabled: bool,
    /// OAuth2 client ID.
    pub client_id: String,
    /// OAuth2 client secret.
    pub client_secret: String,
    /// Authorization endpoint URL.
    pub authorize_url: String,
    /// Token endpoint URL.
    pub token_url: String,
    /// Optional — userinfo endpoint URL.
    pub userinfo_url: Option<String>,
    /// Optional — RP-initiated logout endpoint.
    pub logout_url: Option<String>,
    /// Redirect URI registered with the OAuth2 provider.
    pub redirect_uri: String,
    /// Scopes to request.
    pub scopes: Vec<String>,
    /// Session TTL in seconds.
    pub session_ttl_secs: u64,
}

/// Proxy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyConfig {
    #[serde(default = "default_proxy_host")]
    pub host: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default)]
    pub auto_unload: bool,
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
    /// Paths exempt from authentication. Default: empty.
    /// Example: ["/health", "/metrics"]
    #[serde(default)]
    pub authenticator_skip_paths: Vec<String>,
    /// OAuth2/OpenID Connect configuration for browser-based authentication.
    #[serde(default)]
    pub oauth2: OAuth2Config,
}

/// Default helper functions for ProxyConfig fields.
fn default_proxy_host() -> String {
    "0.0.0.0".to_string()
}

const DEFAULT_PROXY_PORT: u16 = 11434;

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

    #[test]
    fn test_proxy_config_serialization() {
        let proxy = ProxyConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            auto_unload: false,
            idle_timeout_secs: 300,
            startup_timeout_secs: 60,
            circuit_breaker_threshold: 5,
            circuit_breaker_cooldown_seconds: 300,
            metrics_retention_secs: 86400,
            download_queue_poll_interval_secs: 2,
            max_loaded_models: 1,
            authenticator_url: None,
            authenticator_skip_paths: Vec::new(),
            oauth2: OAuth2Config::default(),
        };

        let json = serde_json::to_string(&proxy).unwrap();
        let deserialized: ProxyConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.host, "0.0.0.0");
        assert_eq!(deserialized.port, 8080);
        assert!(!deserialized.auto_unload, "auto_unload should be false");
        assert_eq!(deserialized.idle_timeout_secs, 300);
        assert_eq!(deserialized.circuit_breaker_threshold, 5);
    }
}
