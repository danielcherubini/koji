//! Quantization types for models (WASM mirror).

use serde::{Deserialize, Serialize};

use tama_core::config::{QuantEntry as CoreQuantEntry, QuantKind as CoreQuantKind};

/// What kind of file a quant entry represents.
///
/// Used to distinguish regular GGUF model quants from auxiliary files like
/// vision projectors (mmproj).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    /// A regular GGUF model quantization (Q4_K_M, Q8_0, F16, etc.).
    #[default]
    Model,
    /// A vision projector (mmproj-*.gguf). Passed via `--mmproj` to llama.cpp.
    Mmproj,
    /// An MTP draft model (mtp-*.gguf). Passed via `--spec-draft-model` to llama.cpp.
    Mtp,
}

/// A quantization entry for a model.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantEntry {
    pub file: String,
    /// What kind of file this is. Defaults to `Model` for backward compat.
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

/// Convert from tama_core::config::QuantEntry to mirror type.
impl From<CoreQuantEntry> for QuantEntry {
    fn from(q: CoreQuantEntry) -> Self {
        Self {
            file: q.file,
            kind: q.kind.into(),
            size_bytes: q.size_bytes,
            context_length: q.context_length,
        }
    }
}

/// Convert from mirror QuantEntry to tama_core::config::QuantEntry.
impl From<QuantEntry> for CoreQuantEntry {
    fn from(q: QuantEntry) -> Self {
        Self {
            file: q.file,
            kind: q.kind.into(),
            size_bytes: q.size_bytes,
            context_length: q.context_length,
        }
    }
}

/// Convert from tama_core::config::QuantKind to mirror type.
impl From<CoreQuantKind> for QuantKind {
    fn from(q: CoreQuantKind) -> Self {
        match q {
            CoreQuantKind::Model => QuantKind::Model,
            CoreQuantKind::Mmproj => QuantKind::Mmproj,
            CoreQuantKind::Mtp => QuantKind::Mtp,
        }
    }
}

/// Convert from mirror QuantKind to tama_core::config::QuantKind.
impl From<QuantKind> for CoreQuantKind {
    fn from(q: QuantKind) -> Self {
        match q {
            QuantKind::Model => CoreQuantKind::Model,
            QuantKind::Mmproj => CoreQuantKind::Mmproj,
            QuantKind::Mtp => CoreQuantKind::Mtp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── QuantKind serialization tests ─────────────────────────────────────

    #[test]
    fn test_quant_kind_serialization() {
        let json_model = serde_json::to_string(&QuantKind::Model).unwrap();
        assert!(json_model.contains("model"));
        let deserialized: QuantKind = serde_json::from_str(&json_model).unwrap();
        assert_eq!(deserialized, QuantKind::Model);

        let json_mmproj = serde_json::to_string(&QuantKind::Mmproj).unwrap();
        assert!(json_mmproj.contains("mmproj"));
        let deserialized: QuantKind = serde_json::from_str(&json_mmproj).unwrap();
        assert_eq!(deserialized, QuantKind::Mmproj);
    }

    // ── QuantEntry serialization tests ────────────────────────────────────

    #[test]
    fn test_quant_entry_serialization() {
        let entry = QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(5_000_000),
            context_length: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: QuantEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.file, "model-Q4_K_M.gguf");
        assert_eq!(deserialized.kind, QuantKind::Model);
        assert_eq!(deserialized.size_bytes, Some(5_000_000));
    }

    #[test]
    fn test_quant_entry_no_size() {
        let entry = QuantEntry {
            file: "model.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: None,
            context_length: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: QuantEntry = serde_json::from_str(&json).unwrap();

        assert!(deserialized.size_bytes.is_none());
    }
}
