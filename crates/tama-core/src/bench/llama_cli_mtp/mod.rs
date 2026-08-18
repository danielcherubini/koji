//! Shared MTP benchmark types (plan-191 Task 10).
//!
//! The runner (`run_mtp_bench`, prompt execution) moved to the tamad crate
//! (ADR-0010). These types are the shared half: the proxy serializes the
//! config and persists the `MtpBenchResult` report.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Nine diverse prompts for MTP benchmarking (from mtp-bench.py).
pub const MTP_PROMPTS: &[(&str, &str)] = &[
    (
        "code_python",
        "Write a Python function that returns the n-th Fibonacci number using memoization. Include a docstring.",
    ),
    (
        "code_cpp",
        "Write a C++ template function `clamp(x, lo, hi)` that returns x clamped to [lo, hi]. No std::clamp.",
    ),
    (
        "explain_concept",
        "Explain how speculative decoding works in large language model inference, in three short paragraphs.",
    ),
    (
        "summarize",
        "Summarize in two sentences: The Industrial Revolution began in Britain in the late 18th century, transforming manufacturing through mechanization, steam power, and the factory system. It spread to continental Europe and North America during the 19th century.",
    ),
    (
        "qa_factual",
        "Q: What are the four fundamental forces of physics?\nA:",
    ),
    (
        "translation",
        "Translate to French: 'The quick brown fox jumps over the lazy dog.'",
    ),
    (
        "creative_short",
        "Write a four-line poem about an old lighthouse.",
    ),
    (
        "stepwise_math",
        "Solve step by step: A train leaves station A at 60 km/h. Two hours later, a second train leaves the same station on the same track at 90 km/h. How long until the second train catches the first?",
    ),
    (
        "long_code_review",
        "Review the following Python code for correctness, performance, and style. Suggest improvements:\n\n```python\ndef find_duplicates(lst):\n    duplicates = []\n    for i in range(len(lst)):\n        for j in range(i+1, len(lst)):\n            if lst[i] == lst[j] and lst[i] not in duplicates:\n                duplicates.append(lst[i])\n    return duplicates\n```",
    ),
];

/// Configuration for MTP benchmarking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpBenchConfig {
    /// Path to the target model GGUF file.
    pub model_path: PathBuf,
    /// Draft max values to sweep (e.g. [0, 1, 2, 4, 8]).
    pub draft_max_values: Vec<u32>,
    /// GPU layers (maps to --n-gpu-layers). Default Some(99).
    pub ngl: Option<u32>,
    /// Spec draft NGL (maps to --spec-draft-ngl). Default Some(99).
    pub draft_ngl: Option<u32>,
    /// Flash attention toggle (maps to -fa). Default true.
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
    /// Context size (maps to -c). Default Some(32768).
    #[serde(default = "default_context_size")]
    pub context_size: Option<u32>,
}

fn default_context_size() -> Option<u32> {
    Some(32768)
}

fn default_flash_attn() -> bool {
    true
}

/// Result of a single prompt within a given draft-n-max config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpPromptResult {
    /// Which draft-n-max config produced this result.
    pub draft_max: u32,
    /// Prompt name (e.g. "code_python").
    pub name: String,
    /// Wall clock time in seconds for this request.
    pub wall_s: f64,
    /// Number of predicted (completion) tokens.
    pub predicted_n: u32,
    /// Total draft tokens proposed.
    pub draft_n: u32,
    /// Draft tokens accepted.
    pub draft_n_accepted: u32,
    /// Acceptance rate (accepted / draft_n). None when draft_n == 0 (baseline).
    pub accept_rate: Option<f64>,
    /// Predicted tokens per second.
    pub predicted_per_second: f64,
    /// Error message if this prompt failed; all numeric fields are 0.
    pub error: Option<String>,
}

/// Complete MTP benchmark result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpBenchResult {
    /// One entry per prompt per draft-max config, in execution order.
    pub entries: Vec<MtpPromptResult>,
    /// Aggregate statistics across all entries.
    pub aggregate: MtpAggregate,
    /// VRAM sampled on the execution host (plan-191 Task 10: the proxy
    /// never reads local hardware; the tamad fills this in).
    #[serde(default)]
    pub vram: Option<crate::gpu::VramInfo>,
}

/// Aggregate statistics across all MTP benchmark entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtpAggregate {
    /// Total number of requests (successful + failed).
    pub n_requests: usize,
    /// Sum of predicted_n across all successful entries.
    pub total_predicted: u32,
    /// Sum of draft_n across all successful entries.
    pub total_draft: u32,
    /// Sum of draft_n_accepted across all successful entries.
    pub total_draft_accepted: u32,
    /// Aggregate acceptance rate (total_draft_accepted / total_draft). 0.0 if total_draft == 0.
    pub aggregate_accept_rate: f64,
    /// Sum of wall_s across all entries.
    pub wall_s_total: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that MTP_PROMPTS contains exactly 9 prompts.
    #[test]
    fn test_mtp_prompts_count() {
        assert_eq!(MTP_PROMPTS.len(), 9);
    }

    /// Verifies that each prompt has a unique name.
    #[test]
    fn test_mtp_prompts_unique_names() {
        let names: Vec<_> = MTP_PROMPTS.iter().map(|(name, _)| *name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "Duplicate prompt names found");
    }

    /// Verifies that all prompt names match expected values.
    #[test]
    fn test_mtp_prompts_names() {
        let expected = [
            "code_python",
            "code_cpp",
            "explain_concept",
            "summarize",
            "qa_factual",
            "translation",
            "creative_short",
            "stepwise_math",
            "long_code_review",
        ];
        let actual: Vec<_> = MTP_PROMPTS.iter().map(|(name, _)| *name).collect();
        assert_eq!(actual, expected);
    }

    /// Verifies that MtpAggregate correctly computes accept rate when there are drafts.
    #[test]
    fn test_aggregate_accept_rate_with_drafts() {
        let entries = [
            MtpPromptResult {
                draft_max: 0,
                name: "baseline".to_string(),
                wall_s: 1.0,
                predicted_n: 100,
                draft_n: 0,
                draft_n_accepted: 0,
                accept_rate: None,
                predicted_per_second: 100.0,
                error: None,
            },
            MtpPromptResult {
                draft_max: 4,
                name: "spec".to_string(),
                wall_s: 0.8,
                predicted_n: 100,
                draft_n: 50,
                draft_n_accepted: 30,
                accept_rate: Some(0.6),
                predicted_per_second: 125.0,
                error: None,
            },
        ];

        let total_draft: u32 = entries.iter().map(|e| e.draft_n).sum();
        let total_draft_accepted: u32 = entries.iter().map(|e| e.draft_n_accepted).sum();
        let aggregate_accept_rate = if total_draft > 0 {
            total_draft_accepted as f64 / total_draft as f64
        } else {
            0.0
        };

        assert!((aggregate_accept_rate - 0.6).abs() < 0.001);
    }

    /// Verifies that aggregate accept rate is 0.0 when no drafts exist.
    #[test]
    fn test_aggregate_accept_rate_no_drafts() {
        let entries = [MtpPromptResult {
            draft_max: 0,
            name: "baseline".to_string(),
            wall_s: 1.0,
            predicted_n: 100,
            draft_n: 0,
            draft_n_accepted: 0,
            accept_rate: None,
            predicted_per_second: 100.0,
            error: None,
        }];

        let total_draft: u32 = entries.iter().map(|e| e.draft_n).sum();
        let total_draft_accepted: u32 = entries.iter().map(|e| e.draft_n_accepted).sum();
        let aggregate_accept_rate = if total_draft > 0 {
            total_draft_accepted as f64 / total_draft as f64
        } else {
            0.0
        };

        assert_eq!(aggregate_accept_rate, 0.0);
    }

    /// Verifies that MtpPromptResult with error has all numeric fields = 0.
    #[test]
    fn test_error_result_zero_fields() {
        let result = MtpPromptResult {
            draft_max: 4,
            name: "test".to_string(),
            wall_s: 0.0,
            predicted_n: 0,
            draft_n: 0,
            draft_n_accepted: 0,
            accept_rate: None,
            predicted_per_second: 0.0,
            error: Some("test error".to_string()),
        };

        assert!(result.error.is_some());
        assert_eq!(result.predicted_n, 0);
        assert_eq!(result.draft_n, 0);
        assert_eq!(result.draft_n_accepted, 0);
        assert_eq!(result.predicted_per_second, 0.0);
        assert!(result.accept_rate.is_none());
    }

    /// Verifies that MtpBenchConfig serializes and deserializes correctly.
    #[test]
    fn test_mtp_bench_config_serde() {
        let config = MtpBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            draft_max_values: vec![0, 1, 2, 4, 8],
            ngl: Some(99),
            draft_ngl: Some(99),
            flash_attn: true,
            context_size: Some(32768),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MtpBenchConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.model_path, config.model_path);
        assert_eq!(deserialized.draft_max_values, config.draft_max_values);
        assert_eq!(deserialized.ngl, config.ngl);
        assert_eq!(deserialized.draft_ngl, config.draft_ngl);
        assert_eq!(deserialized.flash_attn, config.flash_attn);
        assert_eq!(deserialized.context_size, config.context_size);
    }

    /// Verifies that MtpBenchConfig with default flash_attn deserializes correctly.
    #[test]
    fn test_mtp_bench_config_default_flash_attn() {
        let json = r#"{"model_path":"/test/model.gguf","draft_max_values":[0,1,2]}"#;
        let config: MtpBenchConfig = serde_json::from_str(json).unwrap();
        assert!(config.flash_attn);
    }

    /// Verifies that MtpBenchResult serializes and deserializes correctly.
    #[test]
    fn test_mtp_bench_result_serde() {
        let result = MtpBenchResult {
            entries: vec![MtpPromptResult {
                draft_max: 0,
                name: "test_prompt".to_string(),
                wall_s: 1.5,
                predicted_n: 100,
                draft_n: 0,
                draft_n_accepted: 0,
                accept_rate: None,
                predicted_per_second: 66.67,
                error: None,
            }],
            aggregate: MtpAggregate {
                n_requests: 1,
                total_predicted: 100,
                total_draft: 0,
                total_draft_accepted: 0,
                aggregate_accept_rate: 0.0,
                wall_s_total: 1.5,
            },
            vram: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: MtpBenchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.entries.len(), 1);
        assert_eq!(deserialized.entries[0].name, "test_prompt");
        assert_eq!(deserialized.aggregate.n_requests, 1);
    }
}
