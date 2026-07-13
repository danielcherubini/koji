use serde::{Deserialize, Serialize};

/// Langfuse configuration for observability and telemetry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangfuseConfig {
    /// Whether Langfuse integration is enabled.
    pub enabled: bool,
    /// Langfuse public API key.
    pub public_key: String,
    /// Langfuse secret API key.
    pub secret_key: String,
    /// Langfuse host URL (e.g., `https://cloud.langfuse.com`).
    pub host: String,
    /// Environment name for tracing.
    pub environment: String,
    /// Whether to capture input data in traces.
    pub capture_input: bool,
    /// Whether to capture output data in traces.
    pub capture_output: bool,
    /// Whether to capture streaming events.
    pub capture_streaming: bool,
    /// Maximum bytes for telemetry payloads (1 MB).
    pub telemetry_max_bytes: usize,
    /// Electricity price per kWh for cost estimation (0 = use default).
    pub electricity_price_per_kwh: f64,
}

impl Default for LangfuseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            public_key: String::new(),
            secret_key: String::new(),
            host: "https://cloud.langfuse.com".to_string(),
            environment: "default".to_string(),
            capture_input: true,
            capture_output: true,
            capture_streaming: true,
            telemetry_max_bytes: 1048576, // 1 MB
            electricity_price_per_kwh: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_langfuse_config_default_values() {
        let config = LangfuseConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.public_key, "");
        assert_eq!(config.secret_key, "");
        assert_eq!(config.host, "https://cloud.langfuse.com");
        assert_eq!(config.environment, "default");
        assert!(config.capture_input);
        assert!(config.capture_output);
        assert!(config.capture_streaming);
        assert_eq!(config.telemetry_max_bytes, 1048576); // 1 MB
        assert_eq!(config.electricity_price_per_kwh, 0.0);
    }

    #[test]
    fn test_langfuse_config_serialization() {
        let config = LangfuseConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"enabled\":false"));
        assert!(json.contains("\"host\":\"https://cloud.langfuse.com\""));

        let deserialized: LangfuseConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.enabled, false);
        assert_eq!(deserialized.host, "https://cloud.langfuse.com");
    }
}
