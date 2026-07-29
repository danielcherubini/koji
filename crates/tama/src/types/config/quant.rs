//! Quantization types for models (WASM mirror).

// Re-export from core_shared — identical on both CSR and SSR.
pub use crate::core_shared::{QuantEntry, QuantKind};

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
