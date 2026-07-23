pub mod card;
pub mod config_key;
pub mod gguf;
pub mod manager;
pub mod pull;
pub mod registry;
pub mod search;
pub mod update;
pub mod verify;

pub use card::{ModelCard, ModelMeta, QuantInfo};
pub use config_key::ConfigKey;
pub use manager::ModelManager;
pub use pull::infer_quant_from_filename;
pub use registry::{InstalledModel, ModelRegistry};
pub use search::{search_models, SearchResult, SortBy};

#[cfg(test)]
mod manager_tests;

/// Convert a config key (double-dash format, e.g. `unsloth--gemma-4-26b-a4b-it-gguf`)
/// back to the original repo_id stored in the DB (e.g. `unsloth/gemma-4-26b-a4b-it-gguf`).
///
/// All external IDs (URLs, JSON responses, CLI args) use the double-dash format.
/// The DB stores the original HF repo_id with a real slash.
///
/// This function was moved from `tama_core::db` to `tama_core::models` because
/// it semantically belongs to model configuration handling.
///
/// Delegates to [`ConfigKey::to_repo_id`], which is the canonical home for
/// the inverse rule (split on the FIRST `--` only).
pub fn config_key_to_repo_id(config_key: &str) -> String {
    ConfigKey::new(config_key).to_repo_id()
}

/// Append a HuggingFace `repo_id` (e.g. `"org/repo-name"`) to a base path using
/// the platform-native separator.
///
/// `PathBuf::join("org/repo")` does **not** split on `/` on Windows, producing
/// mixed-slash paths like `C:\models\org/repo`. This function splits on `/` first
/// so the result is always `C:\models\org\repo` on Windows and `/models/org/repo`
/// on Unix.
pub fn repo_path(base: impl Into<std::path::PathBuf>, repo_id: &str) -> std::path::PathBuf {
    repo_id
        .split('/')
        .fold(base.into(), |p, component| p.join(component))
}
