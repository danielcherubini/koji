//! Health check configuration for models (WASM mirror).

use serde::{Deserialize, Serialize};

/// Health check configuration for a model.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HealthCheck {
    /// Health check endpoint URL. Overrides backend's health_check_url.
    #[serde(default)]
    pub url: Option<String>,
    /// Polling interval in milliseconds. Overrides lifecycle.health_check_interval_ms.
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// HTTP timeout in milliseconds per health check request (default: 3000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_check_serialization() {
        let health = HealthCheck {
            url: Some("http://localhost:8080/health".to_string()),
            interval_ms: Some(5000),
            timeout_ms: Some(3000),
        };

        let json = serde_json::to_string(&health).unwrap();
        let deserialized: HealthCheck = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.url,
            Some("http://localhost:8080/health".to_string())
        );
        assert_eq!(deserialized.interval_ms, Some(5000));
        assert_eq!(deserialized.timeout_ms, Some(3000));
    }
}
