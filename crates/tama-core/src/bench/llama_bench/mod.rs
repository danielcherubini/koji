//! Shared llama-bench configuration types (plan-191 Task 10).
//!
//! The runner itself (`run_llama_bench_resolved`, argument construction,
//! binary discovery, output parsing) moved to the tamad crate — benches
//! measure tamad hardware (ADR-0010). The proxy serializes these configs
//! into `RunBenchmarkRequest.config_json` and parses the result report.

use crate::bench::ModelInfo;

/// Configuration for llama-bench specific parameters.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LlamaBenchConfig {
    /// Prompt sizes to test (maps to -p)
    pub pp_sizes: Vec<u32>,
    /// Generation lengths to test (maps to -n)
    pub tg_sizes: Vec<u32>,
    /// Number of measurement runs (maps to -r)
    pub runs: u32,
    /// Warmup runs (handled by wrapper, not llama-bench itself)
    pub warmup: u32,
    /// Thread counts to test. None = auto-detect from system.
    pub threads: Option<Vec<u32>>,
    /// GPU layer range for sweet-spot sweep.
    /// Some("0-99+1") maps to --n-gpu-layers 0-99+1.
    /// None = use all layers (default).
    pub ngl_range: Option<String>,
    /// Optional context size override (maps to --fit-ctx)
    pub ctx_override: Option<u32>,
    /// Logical batch size (maps to -b). Sweep by comma-separating.
    pub batch_sizes: Vec<u32>,
    /// Physical micro-batch size (maps to -ub). Sweep by comma-separating.
    pub ubatch_sizes: Vec<u32>,
    /// KV cache type applied to BOTH -ctk and -ctv.
    /// Mismatched K/V quant falls back to CPU attention on most builds, so we
    /// only expose a single matched-pair value (e.g. "f16", "q8_0", "q4_0").
    pub kv_cache_type: Option<String>,
    /// Depth sweep (maps to -d). Tokens pre-filled into KV cache before timing.
    /// Critical for evaluating KV-cache quantization at non-trivial context.
    pub depth: Vec<u32>,
    /// Flash attention toggle (maps to -fa 0|1). None = llama-bench default.
    pub flash_attn: Option<bool>,
}

/// Per-kind `config_json` envelope for `llama_bench` benchmark requests
/// (plan-191 Task 8).
///
/// The proxy serializes this into `RunBenchmarkRequest.config_json`: the
/// bench knobs plus the project-resolved model metadata, so the tamad (no
/// DB — invariant 2) can fill the report's `ModelInfo` without the central
/// database. The tamad replaces the envelope's `gpu_variant` label with the
/// one it derives from the binary path on its host.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LlamaBenchConfigJson {
    /// The llama-bench sweep configuration.
    pub bench: LlamaBenchConfig,
    /// Model metadata resolved by the proxy (display name, HF repo id,
    /// quant, backend, context length).
    pub model_info: ModelInfo,
}
