use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LangfuseConfig {
    pub enabled: bool,
    pub public_key: String,
    pub secret_key: String,
    pub host: String,
    pub environment: String,
    pub capture_input: bool,
    pub capture_output: bool,
    pub capture_streaming: bool,
    pub telemetry_max_bytes: usize,
    pub electricity_price_per_kwh: f64,
}

impl From<tama_core::config::LangfuseConfig> for LangfuseConfig {
    fn from(c: tama_core::config::LangfuseConfig) -> Self {
        Self {
            enabled: c.enabled,
            public_key: c.public_key,
            secret_key: c.secret_key,
            host: c.host,
            environment: c.environment,
            capture_input: c.capture_input,
            capture_output: c.capture_output,
            capture_streaming: c.capture_streaming,
            telemetry_max_bytes: c.telemetry_max_bytes,
            electricity_price_per_kwh: c.electricity_price_per_kwh,
        }
    }
}

impl From<LangfuseConfig> for tama_core::config::LangfuseConfig {
    fn from(c: LangfuseConfig) -> Self {
        Self {
            enabled: c.enabled,
            public_key: c.public_key,
            secret_key: c.secret_key,
            host: c.host,
            environment: c.environment,
            capture_input: c.capture_input,
            capture_output: c.capture_output,
            capture_streaming: c.capture_streaming,
            telemetry_max_bytes: c.telemetry_max_bytes,
            electricity_price_per_kwh: c.electricity_price_per_kwh,
        }
    }
}
