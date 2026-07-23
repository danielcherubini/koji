//! Typed model identity: the `ConfigKey` newtype.
//!
//! A model's registry/lookup key is derived from its HuggingFace `repo_id`.
//! The derivation rule lives ONLY in `ConfigKey::from_repo_id` — never
//! re-derive it inline. Model CARD filenames use a different,
//! case-preserving rule; see `crate::models::card_slug` (Task 4).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Registry key for a model config (e.g. `unsloth--gemma-4-26b-a4b-it-gguf`).
///
/// Invariant: produced by `ConfigKey::from_repo_id` (or trusted verbatim via
/// `new`/`FromStr` when read from the DB, a URL, or the registry map).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfigKey(String);

impl ConfigKey {
    /// Derive the config key for a repo id.
    ///
    /// THE ONLY derivation site for the rule:
    /// `config_key = repo_id.to_lowercase().replace('/', "--")`.
    pub fn from_repo_id(repo_id: &str) -> Self {
        Self(repo_id.to_lowercase().replace('/', "--"))
    }

    /// Wrap a string that is already a config key (read from the DB, a URL
    /// path segment, or the registry map key). Does NOT transform the input.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert back to the repo_id stored in the DB (e.g.
    /// `unsloth--gemma-4-26b-a4b-it-gguf` → `unsloth/gemma-4-26b-a4b-it-gguf`).
    ///
    /// Inverse of `from_repo_id` up to case (repo_id lookups are
    /// case-insensitive via `COLLATE NOCASE` on `model_configs.repo_id`).
    /// Only the FIRST `--` is split — repo ids have exactly one path segment.
    pub fn to_repo_id(&self) -> String {
        if let Some(idx) = self.0.find("--") {
            let (prefix, suffix) = self.0.split_at(idx);
            format!("{}/{}", prefix, &suffix[2..])
        } else {
            self.0.clone()
        }
    }
}

impl fmt::Display for ConfigKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ConfigKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for ConfigKey {
    type Err = std::convert::Infallible;
    /// Wraps the input VERBATIM (assumes it is already a config key).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigKey;

    #[test]
    fn test_from_repo_id_derives_canonical_key() {
        let key = ConfigKey::from_repo_id("Unsloth/Gemma-4-26B-A4B-IT-GGUF");
        assert_eq!(key.as_str(), "unsloth--gemma-4-26b-a4b-it-gguf");

        let key = ConfigKey::from_repo_id("owner/repo");
        assert_eq!(key.as_str(), "owner--repo");
    }

    #[test]
    fn test_from_repo_id_lowercases_and_replaces() {
        // Mixed case + slash both handled.
        let key = ConfigKey::from_repo_id("Org/Repo-Sub");
        assert_eq!(key.as_str(), "org--repo-sub");

        // Already-lowercase id unchanged.
        let key = ConfigKey::from_repo_id("org/repo-sub");
        assert_eq!(key.as_str(), "org--repo-sub");
    }

    #[test]
    fn test_from_repo_id_without_slash() {
        // No slash → lowercased only, no replacement.
        let key = ConfigKey::from_repo_id("Local-Model");
        assert_eq!(key.as_str(), "local-model");
    }

    #[test]
    fn test_to_repo_id_inverts_first_double_dash() {
        let key = ConfigKey::new("owner--repo");
        assert_eq!(key.to_repo_id(), "owner/repo");

        // No `--` → unchanged.
        let key = ConfigKey::new("local-model");
        assert_eq!(key.to_repo_id(), "local-model");
    }

    #[test]
    fn test_round_trip() {
        // Case loss is expected — the DB looks up repo_id case-insensitively
        // via COLLATE NOCASE on model_configs.repo_id.
        let key = ConfigKey::from_repo_id("Owner/Repo");
        assert_eq!(key.to_repo_id(), "owner/repo");
    }

    #[test]
    fn test_new_and_from_str_wrap_verbatim() {
        // `new` wraps VERBATIM — no transformation.
        let key = ConfigKey::new("Owner--Repo");
        assert_eq!(key.as_str(), "Owner--Repo");

        // `FromStr` also wraps verbatim.
        let key: ConfigKey = "Owner--Repo".parse().unwrap();
        assert_eq!(key.as_str(), "Owner--Repo");
    }

    #[test]
    fn test_display_and_as_ref() {
        let key = ConfigKey::from_repo_id("Org/Repo");
        assert_eq!(format!("{}", key), "org--repo");

        let key = ConfigKey::new("org--repo");
        let s: &str = key.as_ref();
        assert_eq!(s, "org--repo");
    }

    #[test]
    fn test_serde_transparent() {
        let key = ConfigKey::from_repo_id("a/b");
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "\"a--b\"");

        // Deserialization round-trips.
        let back: ConfigKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }
}
