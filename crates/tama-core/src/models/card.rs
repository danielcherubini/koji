use crate::config::QuantKind;
use crate::profiles::SamplingParams;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A model TOML document describing a model and its available quants.
/// Lives at `~/.config/tama/configs/<company>-<model>.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelToml {
    pub model: ModelMeta,
    /// Per-profile sampling overrides specific to this model.
    /// Keys are profile names: "coding", "chat", "analysis", "creative", or custom names.
    #[serde(default)]
    pub sampling: HashMap<String, SamplingParams>,
    /// Available quantizations. Keys are quant names like "Q4_K_M", "Q8_0".
    #[serde(default)]
    pub quants: HashMap<String, QuantInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelMeta {
    pub name: String,
    /// HuggingFace repo identifier, e.g. "bartowski/OmniCoder-8B-GGUF"
    pub source: String,
    #[serde(default)]
    pub default_context_length: Option<u32>,
    #[serde(default)]
    pub default_gpu_layers: Option<u32>,
    /// Default GPU device for this model (e.g. "ROCm0", "CUDA1").
    /// Passed as `--device` to llama.cpp backends when the model config
    /// does not override it.
    #[serde(default)]
    pub default_gpu_device: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct QuantInfo {
    /// Filename of the GGUF file relative to the model directory.
    pub file: String,
    /// What kind of file this is. Defaults to `Model` for backward compat.
    #[serde(default)]
    pub kind: QuantKind,
    /// File size in bytes (informational).
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Context length override for this specific quant.
    #[serde(default)]
    pub context_length: Option<u32>,
}

/// Filename slug for a model TOML (`<slug>.toml` in the configs directory).
///
/// Deliberately CASE-PRESERVING (unlike `ConfigKey::from_repo_id`): card
/// files already exist on disk with mixed-case names, and lowercasing the
/// rule would orphan them. Never "unify" this with the config_key rule.
pub fn card_slug(repo_id: &str) -> String {
    repo_id.replace('/', "--")
}

pub fn load(path: &std::path::Path) -> anyhow::Result<ModelToml> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read model TOML at {}", path.display()))?;
    let card: ModelToml = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse model TOML at {}", path.display()))?;
    Ok(card)
}

pub fn save(card: &ModelToml, path: &std::path::Path) -> anyhow::Result<()> {
    let toml_str = toml::to_string_pretty(card).context("Failed to serialize model TOML")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    std::fs::write(path, &toml_str)
        .with_context(|| format!("Failed to write model TOML to {}", path.display()))?;
    Ok(())
}

impl ModelToml {
    /// Load a model TOML from a TOML file.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        load(path)
    }

    /// Save a model TOML to a TOML file.
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        save(self, path)
    }

    /// Get the effective context length for a specific quant.
    /// Falls back to model-level default if the quant doesn't specify one.
    pub fn context_length_for(&self, quant_name: &str) -> Option<u32> {
        self.quants
            .get(quant_name)
            .and_then(|q| q.context_length)
            .or(self.model.default_context_length)
    }

    /// Get model-specific sampling overrides for a given profile name.
    pub fn sampling_for(&self, profile_name: &str) -> Option<&SamplingParams> {
        self.sampling.get(profile_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_card_toml() -> &'static str {
        r#"
[model]
name = "OmniCoder"
source = "bartowski/OmniCoder-8B-GGUF"
default_context_length = 8192
default_gpu_layers = 999

[sampling.coding]
temperature = 0.2
top_k = 40

[sampling.chat]
temperature = 0.6

[quants.Q4_K_M]
file = "OmniCoder-8B-Q4_K_M.gguf"
size_bytes = 4_200_000_000
context_length = 8192

[quants.Q8_0]
file = "OmniCoder-8B-Q8_0.gguf"
size_bytes = 8_100_000_000
context_length = 16384
"#
    }

    #[test]
    fn test_model_toml_deserialize() {
        let model: ModelToml = toml::from_str(sample_card_toml()).unwrap();
        assert_eq!(model.model.name, "OmniCoder");
        assert_eq!(model.model.source, "bartowski/OmniCoder-8B-GGUF");
        assert_eq!(model.model.default_context_length, Some(8192));
        assert_eq!(model.model.default_gpu_layers, Some(999));
        assert_eq!(model.quants.len(), 2);
        assert_eq!(model.quants["Q4_K_M"].file, "OmniCoder-8B-Q4_K_M.gguf");
        assert_eq!(model.quants["Q8_0"].size_bytes, Some(8_100_000_000));
    }

    #[test]
    fn test_model_toml_load_save() {
        let model: ModelToml = toml::from_str(sample_card_toml()).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.toml");

        super::save(&model, &path).unwrap();
        let loaded = super::load(&path).unwrap();

        // After loading, sampling parameters are populated with defaults for missing profiles
        // Compare only the explicitly provided sampling parameters by checking the original keys
        let model_explicit_keys: std::collections::HashSet<String> =
            model.sampling.keys().cloned().collect();
        let loaded_explicit_keys: std::collections::HashSet<String> = loaded
            .sampling
            .keys()
            .filter(|k| {
                let k_str = k.to_string();
                model_explicit_keys.contains(&k_str)
            })
            .cloned()
            .collect();

        // Both should have the same explicitly provided sampling parameters
        assert_eq!(model_explicit_keys, loaded_explicit_keys);
    }

    #[test]
    fn test_model_toml_sampling_overrides() {
        let model: ModelToml = toml::from_str(sample_card_toml()).unwrap();
        let coding = model.sampling_for("coding").unwrap();
        assert_eq!(coding.temperature, Some(0.2));
        assert_eq!(coding.top_k, Some(40));
        assert_eq!(coding.top_p, None);

        let chat = model.sampling_for("chat").unwrap();
        assert_eq!(chat.temperature, Some(0.6));

        assert!(model.sampling_for("nonexistent").is_none());
    }

    #[test]
    fn test_context_length_for_quant() {
        let model: ModelToml = toml::from_str(sample_card_toml()).unwrap();
        assert_eq!(model.context_length_for("Q8_0"), Some(16384));
        assert_eq!(model.context_length_for("Q4_K_M"), Some(8192));
        assert_eq!(model.context_length_for("unknown"), Some(8192)); // fallback to model default
    }

    #[test]
    fn test_minimal_model_toml() {
        let toml_str = r#"
[model]
name = "TinyModel"
source = "someone/tiny-model-GGUF"
"#;
        let model: ModelToml = toml::from_str(toml_str).unwrap();
        assert_eq!(model.model.name, "TinyModel");
        assert!(model.quants.is_empty());
        assert!(model.sampling.is_empty());
        assert_eq!(model.model.default_context_length, None);
    }

    #[test]
    fn test_model_toml_slug_preserves_case() {
        // Case-preserving: "Owner/Repo-GGUF" → "Owner--Repo-GGUF" (NOT lowercased)
        let slug = card_slug("Owner/Repo-GGUF");
        assert_eq!(slug, "Owner--Repo-GGUF");
        assert_eq!(slug.to_lowercase(), "owner--repo-gguf");
    }

    #[test]
    fn test_model_toml_slug_no_slash_unchanged() {
        // Names without a slash pass through unchanged.
        let slug = card_slug("local-model");
        assert_eq!(slug, "local-model");
    }
}
