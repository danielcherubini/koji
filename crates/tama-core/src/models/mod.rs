pub mod capabilities;
pub use capabilities::{model_capabilities, ModelCapabilities};
pub mod card;
pub mod config_key;
pub mod gguf;
pub mod manager;
pub mod metadata;
pub mod pull;
pub mod registry;
pub mod search;
pub mod transformers;
pub mod types;
pub mod update;
pub mod verify;

pub use card::{card_slug, ModelMeta, ModelToml, QuantInfo};
pub use config_key::ConfigKey;
pub use manager::ModelManager;
pub use metadata::ResolvedModelMetadata;
pub use pull::infer_quant_from_filename;
pub use registry::{InstalledModel, ModelRegistry};
pub use search::{search_models, SearchResult, SortBy};
pub use transformers::TransformersMetadata;
pub use types::ModelStateSnapshot;

#[cfg(test)]
mod manager_tests;

/// Validate a HuggingFace-style repo_id (e.g. `"unsloth/gemma-4-26b-it-GGUF"`).
///
/// Rules: split on `/`; every component must be non-empty (rejects `a//b`,
/// leading/trailing slashes), must not contain `..` (rejects `..`, `../x`,
/// `foo..bar`), and may contain only ASCII alphanumerics, `.`, `_`, `-`
/// (dots inside names are legitimate: `model.v2`). The charset whitelist
/// inherently rejects backslashes, NUL bytes, and whitespace.
pub fn is_valid_repo_id(repo_id: &str) -> bool {
    if repo_id.is_empty() {
        return false;
    }
    repo_id.split('/').all(|component| {
        !component.is_empty()
            && !component.contains("..")
            && component
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_repo_id_accepts_legitimate() {
        assert!(is_valid_repo_id("unsloth/gemma-4-26b-it-GGUF"));
        assert!(is_valid_repo_id("model.v2"));
        assert!(is_valid_repo_id("a"));
        assert!(is_valid_repo_id("Org_Name/Repo-Name.1"));
        assert!(is_valid_repo_id("a/b/c"));
    }

    #[test]
    fn test_is_valid_repo_id_rejects_traversal() {
        assert!(!is_valid_repo_id(".."));
        assert!(!is_valid_repo_id("../x"));
        assert!(!is_valid_repo_id("a/../b"));
        assert!(!is_valid_repo_id("foo..bar"));
    }

    #[test]
    fn test_is_valid_repo_id_rejects_empty_components() {
        assert!(!is_valid_repo_id(""));
        assert!(!is_valid_repo_id("a//b"));
        assert!(!is_valid_repo_id("/a"));
        assert!(!is_valid_repo_id("a/"));
    }

    #[test]
    fn test_is_valid_repo_id_rejects_backslash_nul_whitespace() {
        assert!(!is_valid_repo_id("a\\b"));
        assert!(!is_valid_repo_id("a\0b"));
        assert!(!is_valid_repo_id("a b"));
        assert!(!is_valid_repo_id("owner/repo name"));
    }
}
