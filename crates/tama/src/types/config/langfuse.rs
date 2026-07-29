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
