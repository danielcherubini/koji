//! Terminal pull result JSON crossing the proxy ↔ tamad boundary
//! (plan-191 Task 6, ADR-0010).
//!
//! The tamad downloads to its own disk, verifies the file there (SHA-256
//! against the HF LFS hash, GGUF/transformers header parse), and
//! serializes these types into the terminal job event's `result_json`.
//! The proxy deserializes them and persists the registry effects
//! (`model_files` rows, model card/configs) from that payload — it never
//! reads or re-hashes the downloaded file itself, which may not exist on
//! the proxy's machine in a remote-host layout.
//!
//! The wire shape is a cross-process contract with the host: keep field
//! names and serde behavior stable, and extend additively
//! (`serde(default)` + `skip_serializing_if`) so mixed-version hosts
//! interoperate.

use serde::{Deserialize, Serialize};

use crate::models::gguf::GgufMetadata;
use crate::models::transformers::TransformersMetadata;

/// One downloaded + verified file in a host GGUF pull result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TamadPulledFile {
    /// Path relative to the destination dir (== the requested filename).
    pub path: String,
    /// Size in bytes of the downloaded file.
    pub size: u64,
    /// Actual SHA-256 of the downloaded file (`None` when hashing failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Upstream HF LFS SHA-256 (`None` when the blobs API was
    /// unavailable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha: Option<String>,
    /// Passed verification (true when no upstream hash was available).
    #[serde(default)]
    pub verified: bool,
    /// Verification error detail (hash mismatch / hash error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_error: Option<String>,
    /// Whether this file is the primary shard of a (possibly sharded)
    /// quant.
    #[serde(default)]
    pub is_primary_shard: bool,
}

/// Terminal result JSON of a GGUF pull executed by the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TamadGgufPullResult {
    /// Destination dir as resolved on the host.
    pub dir: String,
    /// One entry per requested file.
    pub files: Vec<TamadPulledFile>,
    /// GGUF header metadata of the first parseable non-mmproj/MTP file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gguf_metadata: Option<GgufMetadata>,
    /// transformers config.json metadata (when GGUF parsing was not
    /// applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transformers_metadata: Option<TransformersMetadata>,
}

impl TamadGgufPullResult {
    /// The result entry for the requested relative file path, if any.
    pub fn file(&self, path: &str) -> Option<&TamadPulledFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// Terminal result JSON of a whole-repo pull (`hf` CLI) executed by the
/// host. Carries no per-file hashes: the `hf` CLI verifies downloads via
/// its own cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TamadRepoPullResult {
    /// Destination dir on the host.
    pub dir: String,
    /// Whether the download succeeded.
    pub ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire shape is the contract with the tamad host: the exact JSON
    /// the host emits (all optional keys present) must deserialize into
    /// the shared types unchanged.
    #[test]
    fn test_gguf_result_wire_shape_full() {
        let json = r#"{
            "dir": "/models/test/repo",
            "files": [{
                "path": "repo-Q4_K_M.gguf",
                "size": 1234,
                "sha256": "abc123",
                "expected_sha": "def456",
                "verified": false,
                "verify_error": "hash mismatch: expected abc1 got def2",
                "is_primary_shard": true
            }],
            "gguf_metadata": {
                "architecture": "llama",
                "context_length": 8192,
                "embedding_length": 4096,
                "block_count": 36,
                "head_count": 32,
                "quantization": "Q4_K_M",
                "name": "test model",
                "nextn_predict_count": 1
            },
            "transformers_metadata": {
                "architectures": ["Qwen3ForCausalLM"],
                "hidden_size": 2560,
                "num_hidden_layers": 36,
                "num_attention_heads": 32,
                "max_position_embeddings": 40960,
                "quantization_method": "fp8"
            }
        }"#;
        let r: TamadGgufPullResult = serde_json::from_str(json).expect("wire shape parses");
        assert_eq!(r.dir, "/models/test/repo");
        assert_eq!(r.files.len(), 1);
        let f = &r.files[0];
        assert_eq!(f.path, "repo-Q4_K_M.gguf");
        assert_eq!(f.size, 1234);
        assert_eq!(f.sha256.as_deref(), Some("abc123"));
        assert_eq!(f.expected_sha.as_deref(), Some("def456"));
        assert!(!f.verified);
        assert!(f
            .verify_error
            .as_deref()
            .is_some_and(|e| e.contains("hash mismatch")));
        assert!(f.is_primary_shard);
        assert_eq!(
            r.gguf_metadata
                .as_ref()
                .and_then(|m| m.architecture.as_deref()),
            Some("llama")
        );
        assert_eq!(
            r.gguf_metadata.as_ref().and_then(|m| m.context_length),
            Some(8192)
        );
        assert_eq!(
            r.transformers_metadata
                .as_ref()
                .and_then(|m| m.architectures.first().cloned()),
            Some("Qwen3ForCausalLM".to_string())
        );
        // file() looks up by relative path.
        assert!(r.file("repo-Q4_K_M.gguf").is_some());
        assert!(r.file("other.gguf").is_none());
    }

    /// Sparse payloads (optional keys omitted — the shapes the host emits
    /// when hashing failed or no metadata was parseable) must deserialize
    /// with the omitted keys as `None`/`false`.
    #[test]
    fn test_gguf_result_wire_shape_sparse() {
        let json = r#"{
            "dir": "/models/test/repo",
            "files": [{ "path": "repo-Q4_K_M.gguf", "size": 10, "verified": true }]
        }"#;
        let r: TamadGgufPullResult = serde_json::from_str(json).expect("sparse shape parses");
        let f = &r.files[0];
        assert!(f.sha256.is_none());
        assert!(f.expected_sha.is_none());
        assert!(f.verified);
        assert!(f.verify_error.is_none());
        assert!(!f.is_primary_shard);
        assert!(r.gguf_metadata.is_none());
        assert!(r.transformers_metadata.is_none());
    }

    /// Serializing `None` metadata omits the keys (host-side wire output
    /// is unchanged by adding the shared types).
    #[test]
    fn test_gguf_result_roundtrip_omits_none_metadata() {
        let r = TamadGgufPullResult {
            dir: "/models/o/r".to_string(),
            files: vec![TamadPulledFile {
                path: "a.gguf".to_string(),
                size: 1,
                sha256: None,
                expected_sha: None,
                verified: true,
                verify_error: None,
                is_primary_shard: true,
            }],
            gguf_metadata: None,
            transformers_metadata: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            !json.contains("gguf_metadata"),
            "None keys must be omitted: {json}"
        );
        assert!(
            !json.contains("transformers_metadata"),
            "None keys must be omitted: {json}"
        );
        assert!(
            !json.contains("sha256"),
            "None keys must be omitted: {json}"
        );
        // Round-trip keeps values.
        let back: TamadGgufPullResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.files[0].size, 1);
        assert!(back.files[0].verified);
        assert!(back.files[0].is_primary_shard);
    }

    /// Repo-pull result wire shape (`{"dir","ok"}`).
    #[test]
    fn test_repo_result_wire_shape() {
        let r: TamadRepoPullResult =
            serde_json::from_str(r#"{"dir": "/models/happy/repo", "ok": true}"#).unwrap();
        assert_eq!(r.dir, "/models/happy/repo");
        assert!(r.ok);
    }
}
