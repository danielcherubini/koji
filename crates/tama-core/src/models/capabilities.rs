use crate::config::ModelConfig;
use crate::types::quant::QuantKind;
use serde::{Deserialize, Serialize};

/// Computed capabilities for a model configuration.
///
/// Used by the API to expose MTP (multi-token prediction) support and vision
/// projector availability without requiring the caller to understand internal
/// config details.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model supports MTP (multi-token prediction).
    /// True when any of: GGUF nextn > 0, mtp_model is set, a quant has kind Mtp,
    /// or spec_types contains "draft-mtp".
    pub supports_mtp: bool,
    /// Whether an MTP draft file is available (Mtp quant or mtp_model config).
    pub has_mtp_draft_file: bool,
    /// Whether a vision projector (mmproj) is configured.
    pub has_mmproj: bool,
}

/// Compute model capabilities from a [`ModelConfig`] and an optional GGUF-parsed
/// `nextn_predict_count` value.
///
/// When `nextn` is `None` the function falls back entirely to heuristics (quant
/// kinds, config fields). When `Some(value)` the GGUF data takes precedence for
/// the `supports_mtp` check while heuristics still apply for `has_mtp_draft_file`
/// and `has_mmproj`.
pub fn model_capabilities(config: &ModelConfig, nextn: Option<u64>) -> ModelCapabilities {
    let has_mtp_quant = config
        .quants
        .values()
        .any(|q| matches!(q.kind, QuantKind::Mtp));

    let has_mtp_draft_file = has_mtp_quant || config.mtp_model.is_some();

    let has_mmproj = config.mmproj.is_some()
        || config
            .quants
            .values()
            .any(|q| matches!(q.kind, QuantKind::Mmproj));

    let supports_mtp = nextn.unwrap_or(0) > 0
        || has_mtp_draft_file
        || config.mtp_model.is_some()
        || config
            .spec_decoding
            .spec_types
            .iter()
            .any(|t| t == "draft-mtp");

    ModelCapabilities {
        supports_mtp,
        has_mtp_draft_file,
        has_mmproj,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{QuantEntry, SpecDecodingConfig};
    use std::collections::BTreeMap;

    fn make_config(
        mtp_model: Option<String>,
        mmproj: Option<String>,
        quants: BTreeMap<String, QuantEntry>,
        spec_types: Vec<String>,
    ) -> ModelConfig {
        ModelConfig {
            backend: "llama-cpp".to_string(),
            mtp_model,
            mmproj,
            quants,
            spec_decoding: SpecDecodingConfig {
                spec_types,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // ── supports_mtp tests ────────────────────────────────────────────────────

    #[test]
    fn test_supports_mtp_none_nextn_no_heuristics() {
        let config = make_config(None, None, BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            !caps.supports_mtp,
            "nextn=None with no heuristics should not support MTP"
        );
    }

    #[test]
    fn test_supports_mtp_nextn_some_greater_than_zero() {
        let config = make_config(None, None, BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, Some(4));
        assert!(
            caps.supports_mtp,
            "nextn=Some(4) should enable supports_mtp"
        );
    }

    #[test]
    fn test_supports_mtp_nextn_some_zero() {
        let config = make_config(None, None, BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, Some(0));
        assert!(
            !caps.supports_mtp,
            "nextn=Some(0) should not enable supports_mtp"
        );
    }

    #[test]
    fn test_supports_mtp_mtp_model_set() {
        let config = make_config(Some("mtp-draft.gguf".into()), None, BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            caps.supports_mtp,
            "mtp_model set should enable supports_mtp even with nextn=None"
        );
    }

    #[test]
    fn test_supports_mtp_mtp_quant_in_quants() {
        let mut quants = BTreeMap::new();
        quants.insert(
            "main".to_string(),
            QuantEntry {
                file: "model.gguf".into(),
                kind: QuantKind::Model,
                ..Default::default()
            },
        );
        quants.insert(
            "mtp".to_string(),
            QuantEntry {
                file: "mtp-draft.gguf".into(),
                kind: QuantKind::Mtp,
                ..Default::default()
            },
        );
        let config = make_config(None, None, quants, vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            caps.supports_mtp,
            "Mtp quant kind should enable supports_mtp"
        );
    }

    #[test]
    fn test_supports_mtp_draft_mtp_in_spec_types() {
        let config = make_config(None, None, BTreeMap::new(), vec!["draft-mtp".to_string()]);
        let caps = model_capabilities(&config, None);
        assert!(
            caps.supports_mtp,
            "spec_types containing 'draft-mtp' should enable supports_mtp"
        );
    }

    #[test]
    fn test_supports_mtp_other_spec_type_ignored() {
        let config = make_config(
            None,
            None,
            BTreeMap::new(),
            vec!["ngram-simple".to_string()],
        );
        let caps = model_capabilities(&config, None);
        assert!(
            !caps.supports_mtp,
            "spec_types without 'draft-mtp' should not enable supports_mtp"
        );
    }

    // ── has_mtp_draft_file tests ──────────────────────────────────────────────

    #[test]
    fn test_has_mtp_draft_file_no_mtp() {
        let config = make_config(None, None, BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            !caps.has_mtp_draft_file,
            "no MTP indicators should yield has_mtp_draft_file=false"
        );
    }

    #[test]
    fn test_has_mtp_draft_file_mtp_quant() {
        let mut quants = BTreeMap::new();
        quants.insert(
            "mtp".to_string(),
            QuantEntry {
                file: "mtp-draft.gguf".into(),
                kind: QuantKind::Mtp,
                ..Default::default()
            },
        );
        let config = make_config(None, None, quants, vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            caps.has_mtp_draft_file,
            "Mtp quant should set has_mtp_draft_file=true"
        );
    }

    #[test]
    fn test_has_mtp_draft_file_mtp_model_config() {
        let config = make_config(Some("mtp-draft.gguf".into()), None, BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            caps.has_mtp_draft_file,
            "mtp_model config should set has_mtp_draft_file=true"
        );
    }

    // ── has_mmproj tests ──────────────────────────────────────────────────────

    #[test]
    fn test_has_mmproj_no_mmproj() {
        let config = make_config(None, None, BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            !caps.has_mmproj,
            "no mmproj indicators should yield has_mmproj=false"
        );
    }

    #[test]
    fn test_has_mmproj_mmproj_config_set() {
        let config = make_config(None, Some("mmproj.gguf".into()), BTreeMap::new(), vec![]);
        let caps = model_capabilities(&config, None);
        assert!(caps.has_mmproj, "mmproj config should set has_mmproj=true");
    }

    #[test]
    fn test_has_mmproj_mmproj_quant_in_quants() {
        let mut quants = BTreeMap::new();
        quants.insert(
            "mmproj".to_string(),
            QuantEntry {
                file: "mmproj.gguf".into(),
                kind: QuantKind::Mmproj,
                ..Default::default()
            },
        );
        let config = make_config(None, None, quants, vec![]);
        let caps = model_capabilities(&config, None);
        assert!(caps.has_mmproj, "Mmproj quant should set has_mmproj=true");
    }

    // ── nextn: None list-time behavior ────────────────────────────────────────

    #[test]
    fn test_list_time_nextn_none_falls_back_to_heuristics() {
        // Simulates model-list time where GGUF is not parsed (nextn=None).
        // Heuristics should still detect MTP if config indicates it.
        let mut quants = BTreeMap::new();
        quants.insert(
            "mtp".to_string(),
            QuantEntry {
                file: "mtp-draft.gguf".into(),
                kind: QuantKind::Mtp,
                ..Default::default()
            },
        );
        let config = make_config(None, None, quants, vec![]);
        let caps = model_capabilities(&config, None);
        assert!(
            caps.supports_mtp,
            "heuristic should detect MTP at list time"
        );
        assert!(!caps.has_mmproj, "no mmproj indicators");
    }

    // ── combined scenarios ────────────────────────────────────────────────────

    #[test]
    fn test_full_mvp_model_all_capabilities() {
        let mut quants = BTreeMap::new();
        quants.insert(
            "main".to_string(),
            QuantEntry {
                file: "model.gguf".into(),
                kind: QuantKind::Model,
                ..Default::default()
            },
        );
        quants.insert(
            "mtp".to_string(),
            QuantEntry {
                file: "mtp-draft.gguf".into(),
                kind: QuantKind::Mtp,
                ..Default::default()
            },
        );
        quants.insert(
            "mmproj".to_string(),
            QuantEntry {
                file: "mmproj.gguf".into(),
                kind: QuantKind::Mmproj,
                ..Default::default()
            },
        );

        let config = make_config(
            Some("mtp-draft.gguf".into()),
            Some("mmproj.gguf".into()),
            quants,
            vec!["draft-mtp".to_string()],
        );
        let caps = model_capabilities(&config, Some(4));

        assert!(caps.supports_mtp, "all indicators present → supports_mtp");
        assert!(caps.has_mtp_draft_file, "MTP draft file available");
        assert!(caps.has_mmproj, "mmproj available");
    }
}
