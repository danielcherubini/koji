use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Metadata extracted from a transformers model directory (config.json).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransformersMetadata {
    /// Model architectures (e.g. `["Qwen2ForCausalLM"]`).
    pub architectures: Vec<String>,
    /// Embedding/hidden dimension.
    pub hidden_size: Option<u32>,
    /// Number of hidden layers (block_count / num_layers).
    pub num_hidden_layers: Option<u32>,
    /// Number of attention heads.
    pub num_attention_heads: Option<u32>,
    /// Maximum context length in tokens.
    pub max_position_embeddings: Option<u32>,
    /// Quantization method from `quantization_config.quant_method`.
    pub quantization_method: Option<String>,
}

/// Parse `config.json` from a transformers model directory.
///
/// Returns `Err` only if the file cannot be read or is invalid JSON.
/// Individual missing keys are handled gracefully (fields are `None`/empty).
pub fn parse_transformers_metadata(model_dir: &Path) -> Result<TransformersMetadata> {
    let config_path = model_dir.join("config.json");
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config.json: {}", config_path.display()))?;

    let config: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse config.json: {}", config_path.display()))?;

    Ok(TransformersMetadata {
        architectures: config
            .get("architectures")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        hidden_size: config
            .get("hidden_size")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        num_hidden_layers: config
            .get("num_hidden_layers")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        num_attention_heads: config
            .get("num_attention_heads")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        max_position_embeddings: config
            .get("max_position_embeddings")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        quantization_method: config
            .get("quantization_config")
            .and_then(|qc| qc.get("quant_method"))
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a temp directory with a config.json containing the given JSON string.
    fn create_temp_dir_with_config(json: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), json).unwrap();
        dir
    }

    /// Test: Valid config.json with all fields populated.
    #[test]
    fn test_parse_all_fields() {
        let json = r#"{
            "architectures": ["Qwen2ForCausalLM"],
            "hidden_size": 4096,
            "num_hidden_layers": 32,
            "num_attention_heads": 32,
            "max_position_embeddings": 32768,
            "quantization_config": {
                "quant_method": "bitsandbytes"
            }
        }"#;
        let dir = create_temp_dir_with_config(json);
        let meta = parse_transformers_metadata(dir.path()).unwrap();

        assert_eq!(meta.architectures, vec!["Qwen2ForCausalLM"]);
        assert_eq!(meta.hidden_size, Some(4096));
        assert_eq!(meta.num_hidden_layers, Some(32));
        assert_eq!(meta.num_attention_heads, Some(32));
        assert_eq!(meta.max_position_embeddings, Some(32768));
        assert_eq!(meta.quantization_method, Some("bitsandbytes".to_string()));
    }

    /// Test: config.json with missing fields returns graceful None/empty.
    #[test]
    fn test_parse_missing_fields_graceful() {
        let json = r#"{
            "architectures": ["LlamaForCausalLM"]
        }"#;
        let dir = create_temp_dir_with_config(json);
        let meta = parse_transformers_metadata(dir.path()).unwrap();

        assert_eq!(meta.architectures, vec!["LlamaForCausalLM"]);
        assert_eq!(meta.hidden_size, None);
        assert_eq!(meta.num_hidden_layers, None);
        assert_eq!(meta.num_attention_heads, None);
        assert_eq!(meta.max_position_embeddings, None);
        assert_eq!(meta.quantization_method, None);
    }

    /// Test: Missing config.json file returns Err.
    #[test]
    fn test_parse_missing_config_json() {
        let dir = tempfile::tempdir().unwrap();
        let result = parse_transformers_metadata(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to read config.json"));
    }

    /// Test: Invalid JSON returns Err.
    #[test]
    fn test_parse_invalid_json() {
        let dir = create_temp_dir_with_config("this is not valid json");
        let result = parse_transformers_metadata(dir.path());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to parse config.json"));
    }

    /// Test: quantization_config.quant_method extracted correctly.
    #[test]
    fn test_parse_quantization_method() {
        let json = r#"{
            "architectures": ["MistralForCausalLM"],
            "quantization_config": {
                "quant_method": "awq"
            }
        }"#;
        let dir = create_temp_dir_with_config(json);
        let meta = parse_transformers_metadata(dir.path()).unwrap();

        assert_eq!(meta.quantization_method, Some("awq".to_string()));
    }

    /// Test: quantization_config present but quant_method missing.
    #[test]
    fn test_parse_quantization_config_without_method() {
        let json = r#"{
            "architectures": ["Gemma2ForCausalLM"],
            "quantization_config": {
                "some_other_field": "value"
            }
        }"#;
        let dir = create_temp_dir_with_config(json);
        let meta = parse_transformers_metadata(dir.path()).unwrap();

        assert_eq!(meta.quantization_method, None);
    }

    /// Test: architectures is empty array when not present.
    #[test]
    fn test_parse_empty_architectures() {
        let json = r#"{}"#;
        let dir = create_temp_dir_with_config(json);
        let meta = parse_transformers_metadata(dir.path()).unwrap();

        assert!(meta.architectures.is_empty());
    }

    /// Test: multiple architectures in array.
    #[test]
    fn test_parse_multiple_architectures() {
        let json = r#"{
            "architectures": ["Qwen2ForCausalLM", "Qwen2ForSequenceClassification"],
            "hidden_size": 8192
        }"#;
        let dir = create_temp_dir_with_config(json);
        let meta = parse_transformers_metadata(dir.path()).unwrap();

        assert_eq!(meta.architectures.len(), 2);
        assert_eq!(meta.architectures[0], "Qwen2ForCausalLM");
        assert_eq!(meta.architectures[1], "Qwen2ForSequenceClassification");
    }

    /// Test: TransformersMetadata derives Default with all fields empty/None.
    #[test]
    fn test_transformers_metadata_default() {
        let meta = TransformersMetadata::default();
        assert!(meta.architectures.is_empty());
        assert_eq!(meta.hidden_size, None);
        assert_eq!(meta.num_hidden_layers, None);
        assert_eq!(meta.num_attention_heads, None);
        assert_eq!(meta.max_position_embeddings, None);
        assert_eq!(meta.quantization_method, None);
    }

    /// Test: TransformersMetadata can be cloned.
    #[test]
    fn test_transformers_metadata_clone() {
        let meta = TransformersMetadata {
            architectures: vec!["LlamaForCausalLM".to_string()],
            hidden_size: Some(4096),
            num_hidden_layers: Some(32),
            num_attention_heads: Some(32),
            max_position_embeddings: Some(8192),
            quantization_method: Some("gptq".to_string()),
        };
        let cloned = meta.clone();
        assert_eq!(cloned.architectures, meta.architectures);
        assert_eq!(cloned.hidden_size, meta.hidden_size);
        assert_eq!(cloned.num_hidden_layers, meta.num_hidden_layers);
        assert_eq!(cloned.num_attention_heads, meta.num_attention_heads);
        assert_eq!(cloned.max_position_embeddings, meta.max_position_embeddings);
        assert_eq!(cloned.quantization_method, meta.quantization_method);
    }
}
