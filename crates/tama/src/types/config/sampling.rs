//! Sampling parameters (WASM mirror).

use serde::{Deserialize, Serialize};

/// Sampling parameters for LLM inference.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SamplingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sampling_params_serialization() {
        let params = SamplingParams {
            temperature: Some(0.7),
            top_k: Some(40),
            top_p: Some(0.95),
            min_p: Some(0.05),
            presence_penalty: Some(0.1),
            frequency_penalty: Some(0.2),
            repeat_penalty: Some(1.1),
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: SamplingParams = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.temperature, Some(0.7));
        assert_eq!(deserialized.top_k, Some(40));
        assert_eq!(deserialized.top_p, Some(0.95));
    }

    #[test]
    fn test_sampling_params_empty() {
        let params = SamplingParams::default();
        let json = serde_json::to_string(&params).unwrap();
        // Default should serialize to empty object or minimal JSON
        assert!(!json.is_empty());
    }
}
