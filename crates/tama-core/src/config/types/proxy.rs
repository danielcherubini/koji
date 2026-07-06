use serde::{Deserialize, Serialize};

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
    /// How often the download queue processor checks for new items (in seconds).
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
