#[cfg(test)]
mod tests {
    use crate::config::QuantKind;

    #[test]
    fn test_quant_kind_detects_mmproj_files() {
        assert_eq!(
            QuantKind::from_filename("mmproj-F16.gguf"),
            QuantKind::Mmproj
        );
        assert_eq!(
            QuantKind::from_filename("mmproj-model-name.gguf"),
            QuantKind::Mmproj
        );
        assert_eq!(
            QuantKind::from_filename("mmproj-Q4_K_M.gguf"),
            QuantKind::Mmproj
        );
        // Case-insensitive
        assert_eq!(
            QuantKind::from_filename("MMPROJ-F16.GGUF"),
            QuantKind::Mmproj
        );
    }

    #[test]
    fn test_quant_kind_defaults_to_model_for_regular_quants() {
        assert_eq!(
            QuantKind::from_filename("model-Q4_K_M.gguf"),
            QuantKind::Model
        );
        assert_eq!(QuantKind::from_filename("mmproj.bin"), QuantKind::Model);
        assert_eq!(QuantKind::from_filename("model.gguf"), QuantKind::Model);
    }

    #[test]
    fn test_quant_kind_detects_mtp_files() {
        assert_eq!(QuantKind::from_filename("mtp-F16.gguf"), QuantKind::Mtp);
        assert_eq!(QuantKind::from_filename("MTP-test.gguf"), QuantKind::Mtp);
    }

    #[test]
    fn test_quant_kind_mmproj_takes_precedence_over_mtp() {
        // Filename starts with mmproj — must be classified as Mmproj even if
        // it contains "mtp" elsewhere in the name.
        assert_eq!(
            QuantKind::from_filename("mmproj-mtp-foo.gguf"),
            QuantKind::Mmproj
        );
    }

    #[test]
    fn test_quant_kind_mtp_requires_gguf_extension() {
        // "mtp" prefix alone is not enough — must end with .gguf.
        assert_eq!(QuantKind::from_filename("mtp-file.bin"), QuantKind::Model);
    }

    #[test]
    fn test_quant_kind_mtp_prefix_only() {
        // "proj" is not "mtp" — should not be detected as Mtp.
        assert_eq!(QuantKind::from_filename("proj-foo.gguf"), QuantKind::Model);
    }
}
