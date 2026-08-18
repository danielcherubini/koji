use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Metadata extracted from a GGUF file header.
/// Only reads the header (~100KB), never loads tensor data.
///
/// Serialized across the proxy↔tamad boundary (plan-191 Task 6): the tamad
/// parses the header on its own disk and ships the result in the pull job's
/// terminal `result_json`; the proxy deserializes it into the model config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GgufMetadata {
    pub architecture: Option<String>, // general.architecture (e.g. "llama")
    pub context_length: Option<u64>,  // {arch}.context_length
    pub embedding_length: Option<u64>, // {arch}.embedding_length
    pub block_count: Option<u64>,     // {arch}.block_count
    pub head_count: Option<u64>,      // {arch}.attention.head_count
    pub quantization: Option<String>, // from file_type mapping (e.g. "Q4_K_M")
    pub name: Option<String>,         // general.name
    /// Number of tokens predicted per draft step (MTP models only).
    /// Set by `{arch}.nextn_predict_count` in the GGUF header.
    pub nextn_predict_count: Option<u64>,
}

/// Parse GGUF metadata from a file on disk.
///
/// Returns `Err` only if the file cannot be read or is not a valid GGUF file.
/// Individual missing metadata keys are handled gracefully (fields are `None`).
pub fn parse_gguf_metadata(path: &Path) -> Result<GgufMetadata> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open GGUF file: {}", path.display()))?;
    let mut reader = BufReader::new(file);

    let gguf = gguf_parser::GgufFile::parse(&mut reader)
        .with_context(|| format!("Failed to parse GGUF header: {}", path.display()))?;

    let nextn_predict_count = gguf.architecture().and_then(|arch| {
        let key = format!("{arch}.nextn_predict_count");
        gguf.get_metadata(&key).and_then(|v| v.as_u64())
    });

    Ok(GgufMetadata {
        architecture: gguf.architecture().map(|s| s.to_string()),
        context_length: gguf.context_length(),
        embedding_length: gguf.embedding_length(),
        block_count: gguf.block_count(),
        head_count: gguf.head_count(),
        quantization: gguf.quantization_name().map(|s| s.to_string()),
        name: gguf.name().map(|s| s.to_string()),
        nextn_predict_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_invalid_path() {
        let result = parse_gguf_metadata(Path::new("/nonexistent/file.gguf"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_non_gguf_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "this is not a GGUF file").unwrap();
        let result = parse_gguf_metadata(tmp.path());
        assert!(result.is_err());
    }

    /// Verify that GgufMetadata round-trips correctly when constructed
    /// with realistic values (e.g. a DeepSeek model with nextn_predict_count).
    #[test]
    fn test_gguf_metadata_roundtrip() {
        let meta = GgufMetadata {
            architecture: Some("deepseek2".to_string()),
            context_length: Some(131072),
            embedding_length: Some(4096),
            block_count: Some(60),
            head_count: Some(24),
            quantization: Some("Q4_K_M".to_string()),
            name: Some("DeepSeek-R1-Distill-Q4".to_string()),
            nextn_predict_count: Some(4),
        };

        // Verify all fields are accessible and correct
        assert_eq!(meta.architecture.as_deref(), Some("deepseek2"));
        assert_eq!(meta.context_length, Some(131072));
        assert_eq!(meta.embedding_length, Some(4096));
        assert_eq!(meta.block_count, Some(60));
        assert_eq!(meta.head_count, Some(24));
        assert_eq!(meta.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(meta.name.as_deref(), Some("DeepSeek-R1-Distill-Q4"));
        assert_eq!(meta.nextn_predict_count, Some(4));

        // Verify Clone round-trip
        let cloned = meta.clone();
        assert_eq!(cloned.nextn_predict_count, Some(4));
        assert_eq!(cloned.architecture.as_deref(), Some("deepseek2"));
        assert_eq!(cloned.context_length, Some(131072));

        // Verify Debug formatting (used in logs)
        let debug_str = format!("{meta:?}");
        assert!(debug_str.contains("nextn_predict_count: Some(4)"));
    }

    /// Default GgufMetadata should have all fields as None.
    #[test]
    fn test_gguf_metadata_default_all_none() {
        let meta = GgufMetadata::default();
        assert!(meta.architecture.is_none());
        assert!(meta.context_length.is_none());
        assert!(meta.embedding_length.is_none());
        assert!(meta.block_count.is_none());
        assert!(meta.head_count.is_none());
        assert!(meta.quantization.is_none());
        assert!(meta.name.is_none());
        assert!(meta.nextn_predict_count.is_none());
    }
}
