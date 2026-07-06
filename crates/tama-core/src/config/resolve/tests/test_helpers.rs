use crate::config::types::QuantEntry;
use crate::config::Config;
use crate::config::{BackendConfig, ModelConfig};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

/// Create a temp dir with a dummy model file at `models/org/repo/model-Q4_K_M.gguf`.
/// Returns the owned [`TempDir`] (must stay in scope) and the models_dir path.
pub fn temp_model_dir() -> (TempDir, PathBuf) {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let models_dir = temp_dir.path().join("models");
    let org_dir = models_dir.join("org").join("repo");
    let quant_file = org_dir.join("model-Q4_K_M.gguf");
    std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
    std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");
    (temp_dir, models_dir)
}

/// Create a default [`Config`] with `models_dir` set.
pub fn sample_config(models_dir: PathBuf) -> Config {
    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config
}

/// Create a sample [`ModelConfig`] with sensible defaults and a default
/// `Q4_K_M` quant entry. Use the `overrides` closure to customize fields.
///
/// # Example
/// ```ignore
/// let server = sample_server(|s| {
///     s.context_length = Some(4096);
///     s.num_parallel = Some(2);
/// });
/// ```
pub fn sample_server<F: FnOnce(&mut ModelConfig)>(overrides: F) -> ModelConfig {
    let mut quants = BTreeMap::new();
    quants.insert("Q4_K_M".to_string(), QuantEntry::default());

    let mut server = ModelConfig {
        backend: "llama_cpp".to_string(),
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        enabled: true,
        quants,
        ..Default::default()
    };
    overrides(&mut server);
    server
}

/// Create a default [`BackendConfig`].
pub fn sample_backend() -> BackendConfig {
    BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    }
}
