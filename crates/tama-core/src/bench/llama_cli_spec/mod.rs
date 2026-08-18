//! Shared spec-decoding benchmark types + sweep validation (plan-191 Task 10).
//!
//! The runner (`run_spec_bench`, llama-server lifecycle, output parsing)
//! moved to the tamad crate (ADR-0010). These types + the sweep-matrix
//! builder/validation are the shared half: the proxy validates and
//! serializes configs and parses the `SpecBenchResult` report.

use std::path::PathBuf;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Speculative decoding type (maps to --spec-type CLI flag).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum SpecType {
    NgramSimple,
    NgramMod,
    NgramMapK,
    NgramMapK4v,
    DraftMtp,
}

impl SpecType {
    /// Returns the CLI flag value for --spec-type.
    pub fn as_str(&self) -> &'static str {
        match self {
            SpecType::NgramSimple => "ngram-simple",
            SpecType::NgramMod => "ngram-mod",
            SpecType::NgramMapK => "ngram-map-k",
            SpecType::NgramMapK4v => "ngram-map-k4v",
            SpecType::DraftMtp => "draft-mtp",
        }
    }

    /// Returns the type-specific n-gram CLI flags (llama.cpp PR #22397).
    ///
    /// Each spec type has its own set of parameter flags:
    /// - `ngram-simple`: `--spec-ngram-simple-size-n`, `--spec-ngram-simple-size-m`, `--spec-ngram-simple-min-hits`
    /// - `ngram-mod`: `--spec-ngram-mod-n-match` (no size-m or min-hits)
    /// - `ngram-map-k`: `--spec-ngram-map-k-size-n`, `--spec-ngram-map-k-size-m`, `--spec-ngram-map-k-min-hits`
    /// - `ngram-map-k4v`: `--spec-ngram-map-k4v-size-n`, `--spec-ngram-map-k4v-size-m`, `--spec-ngram-map-k4v-min-hits`
    ///
    /// Returns `(size_n_flag, size_m_flag, min_hits_flag)` — empty strings for flags
    /// that don't apply to this spec type (e.g., ngram-mod has no size-m or min-hits).
    pub fn spec_ngram_flags(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            SpecType::NgramSimple => (
                "--spec-ngram-simple-size-n",
                "--spec-ngram-simple-size-m",
                "--spec-ngram-simple-min-hits",
            ),
            SpecType::NgramMod => ("--spec-ngram-mod-n-match", "", ""),
            SpecType::NgramMapK => (
                "--spec-ngram-map-k-size-n",
                "--spec-ngram-map-k-size-m",
                "--spec-ngram-map-k-min-hits",
            ),
            SpecType::NgramMapK4v => (
                "--spec-ngram-map-k4v-size-n",
                "--spec-ngram-map-k4v-size-m",
                "--spec-ngram-map-k4v-min-hits",
            ),
            SpecType::DraftMtp => ("", "", ""),
        }
    }
}

/// Configuration for a speculative decoding benchmark sweep.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecBenchConfig {
    /// Paths to the target model GGUF file.
    pub model_path: PathBuf,
    /// Spec types to test (e.g. [NgramSimple, NgramMod]).
    pub spec_types: Vec<SpecType>,
    /// Draft max values to sweep (e.g. [8, 16, 32, 64]).
    pub draft_max_values: Vec<u32>,
    /// N-gram lookup size N values for ngram-mod and ngram-map-* types.
    pub ngram_n_values: Vec<u32>,
    /// N-gram draft size M values for ngram-map-* types.
    pub ngram_m_values: Vec<u32>,
    /// N-gram minimum match values for n-gram-mod (e.g. [3, 5]).
    #[serde(default)]
    pub ngram_min_values: Vec<u32>,
    /// N-gram maximum match values for n-gram-mod (e.g. [48, 64]).
    #[serde(default)]
    pub ngram_max_values: Vec<u32>,
    /// Minimum hits for ngram-map-* types (default 1).
    #[serde(default = "default_min_hits")]
    pub ngram_min_hits: u32,
    /// Number of tokens to generate (-n flag). Default 256.
    #[serde(default = "default_gen_tokens")]
    pub gen_tokens: u32,
    /// Number of repetitions per config. Default 3.
    #[serde(default = "default_runs")]
    pub runs: u32,
    /// GPU layers (maps to --n-gpu-layers). None = use model default.
    pub ngl: Option<u32>,
    /// Flash attention toggle (maps to -fa 1|0). Default true.
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
}

fn default_min_hits() -> u32 {
    1
}
fn default_gen_tokens() -> u32 {
    256
}
fn default_runs() -> u32 {
    3
}
fn default_flash_attn() -> bool {
    true
}

/// Result of a single spec-decoding config test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecEntry {
    pub spec_type: String,
    pub draft_max: u32,
    /// N-gram lookup size (maps to `--spec-ngram-*-size-n` or `--spec-ngram-mod-n-match`). None for ngram-simple.
    pub ngram_n: Option<u32>,
    /// N-gram draft size (only for ngram-map-*). None for others.
    pub ngram_m: Option<u32>,
    /// N-gram minimum match (only for n-gram-mod). None for other types.
    pub ngram_min: Option<u32>,
    /// N-gram maximum match (only for n-gram-mod). None for other types.
    pub ngram_max: Option<u32>,
    /// Mean token generation speed (tokens/s).
    pub tg_ts_mean: f64,
    /// Stddev of token generation speed.
    pub tg_ts_stddev: f64,
    /// Percentage delta vs baseline. Positive = faster, negative = slower.
    /// Formula: ((tg_ts_mean - baseline_tg_ts) / baseline_tg_ts) * 100
    pub delta_pct: f64,
    /// Draft acceptance rate from server statistics (0.0–1.0). None if not available.
    pub acceptance_rate: Option<f64>,
    /// Status: "success", "failed", or "skipped_oom".
    pub status: String,
    /// Error message if failed. None on success.
    pub error: Option<String>,
}

/// Complete spec benchmark result with baseline and all config entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecBenchResult {
    /// Baseline TG t/s (no spec-decoding) — mean of N runs.
    pub baseline_tg_ts: f64,
    /// Baseline TG t/s stddev.
    pub baseline_tg_stddev: f64,
    /// One entry per config tested.
    pub entries: Vec<SpecEntry>,
    /// VRAM sampled on the execution host (plan-191 Task 10: the proxy
    /// never reads local hardware; the tamad fills this in).
    #[serde(default)]
    pub vram: Option<crate::gpu::VramInfo>,
}

/// A single sweep configuration to test.
#[derive(Debug, Clone)]
pub struct SweepConfig {
    pub spec_type: SpecType,
    pub draft_max: u32,
    pub ngram_n: Option<u32>,
    pub ngram_m: Option<u32>,
    pub ngram_min: Option<u32>,
    pub ngram_max: Option<u32>,
}

/// Validate a [`SpecBenchConfig`] would produce at least one sweep entry.
///
/// Checks that required dimensions (e.g. `ngram_n_values` for ngram-mod) are
/// populated for the selected spec-types, and that the sweep is not empty.
pub fn validate_sweep_config(config: &SpecBenchConfig) -> Result<()> {
    let matrix = build_sweep_matrix(config)?;
    if matrix.is_empty() {
        bail!(
            "Sweep would produce zero entries. Ensure draft_max_values is not empty and required ngram dimensions are populated."
        );
    }
    Ok(())
}

/// Build the sweep matrix of configurations to test.
///
/// Returns an error if required dimensions are not populated for the selected spec-types.
pub fn build_sweep_matrix(config: &SpecBenchConfig) -> Result<Vec<SweepConfig>> {
    let spec_types = &config.spec_types;

    let needs_n = spec_types.iter().any(|t| {
        matches!(
            t,
            SpecType::NgramMod | SpecType::NgramMapK | SpecType::NgramMapK4v
        )
    });
    let needs_m = spec_types
        .iter()
        .any(|t| matches!(t, SpecType::NgramMapK | SpecType::NgramMapK4v));
    let needs_minmax = spec_types.iter().any(|t| matches!(t, SpecType::NgramMod));

    if needs_n && config.ngram_n_values.is_empty() {
        bail!("ngram_n_values is required when testing ngram-mod or ngram-map-* types");
    }
    if needs_m && config.ngram_m_values.is_empty() {
        bail!("ngram_m_values is required when testing ngram-map-k or ngram-map-k4v types");
    }
    if needs_minmax && (config.ngram_min_values.is_empty() || config.ngram_max_values.is_empty()) {
        bail!("ngram_min_values and ngram_max_values are required when testing n-gram-mod");
    }

    let mut matrix = Vec::new();

    for &st in spec_types {
        match st {
            SpecType::NgramMod => {
                // ngram-mod draft length is controlled by n-min/n-max,
                // not --spec-draft-n-max. Sweep only n-match/n-min/n-max.
                // (Use first draft_max value as a non-binding ceiling.)
                let dm = config.draft_max_values.first().copied().unwrap_or(16);
                for &nn in &config.ngram_n_values {
                    for &nm in &config.ngram_min_values {
                        for &nxm in &config.ngram_max_values {
                            matrix.push(SweepConfig {
                                spec_type: st,
                                draft_max: dm,
                                ngram_n: Some(nn),
                                ngram_m: None,
                                ngram_min: Some(nm),
                                ngram_max: Some(nxm),
                            });
                        }
                    }
                }
            }
            _ => {
                for &dm in &config.draft_max_values {
                    match st {
                        SpecType::NgramSimple => {
                            matrix.push(SweepConfig {
                                spec_type: st,
                                draft_max: dm,
                                ngram_n: None,
                                ngram_m: None,
                                ngram_min: None,
                                ngram_max: None,
                            });
                        }
                        SpecType::NgramMapK | SpecType::NgramMapK4v => {
                            for &nn in &config.ngram_n_values {
                                for &nm in &config.ngram_m_values {
                                    matrix.push(SweepConfig {
                                        spec_type: st,
                                        draft_max: dm,
                                        ngram_n: Some(nn),
                                        ngram_m: Some(nm),
                                        ngram_min: None,
                                        ngram_max: None,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(matrix)
}

/// Format a SweepConfig as CLI-style label for log output.
/// e.g. `--spec-type ngram-mod --spec-draft-n-max 8 --spec-ngram-mod-n-match 16 --spec-ngram-mod-n-min 3 --spec-ngram-mod-n-max 48`
pub fn format_config_label(cfg: &SweepConfig) -> String {
    let (flag_n, flag_m, _flag_hits) = cfg.spec_type.spec_ngram_flags();
    let mut parts = vec![
        format!("--spec-type {}", cfg.spec_type.as_str()),
        format!("--spec-draft-n-max {}", cfg.draft_max),
    ];
    if !flag_n.is_empty() {
        if let Some(n) = cfg.ngram_n {
            parts.push(format!("{} {}", flag_n, n));
        }
    }
    if !flag_m.is_empty() {
        if let Some(m) = cfg.ngram_m {
            parts.push(format!("{} {}", flag_m, m));
        }
    }
    if matches!(cfg.spec_type, SpecType::NgramMod) {
        if let Some(nmin) = cfg.ngram_min {
            parts.push(format!("--spec-ngram-mod-n-min {}", nmin));
        }
        if let Some(nmax) = cfg.ngram_max {
            parts.push(format!("--spec-ngram-mod-n-max {}", nmax));
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that the sweep matrix produces the correct number of entries for ngram-simple.
    #[test]
    fn test_sweep_matrix_ngram_simple() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramSimple],
            draft_max_values: vec![8, 16, 32],
            ngram_n_values: vec![],
            ngram_m_values: vec![],
            ngram_min_values: vec![],
            ngram_max_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 3 draft_max = 3
        assert_eq!(matrix.len(), 3);
    }

    /// Verifies that the sweep matrix produces correct entries for ngram-mod (includes ngram_n dimension).
    #[test]
    fn test_sweep_matrix_ngram_mod() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_values: vec![3],
            ngram_max_values: vec![48],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 2 n-match × 1 n-min × 1 n-max = 2
        // (draft_max is NOT swept for ngram-mod)
        assert_eq!(matrix.len(), 2);
    }

    /// Verifies that the sweep matrix produces correct entries for ngram-map-k (includes ngram_m dimension).
    #[test]
    fn test_sweep_matrix_ngram_map_k() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMapK],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![2, 4],
            ngram_min_values: vec![],
            ngram_max_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 2 draft_max × 2 ngram_n × 2 ngram_m = 8
        assert_eq!(matrix.len(), 8);
    }

    /// Verifies that the sweep matrix correctly combines multiple spec-types.
    #[test]
    fn test_sweep_matrix_multiple_spec_types() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramSimple, SpecType::NgramMod],
            draft_max_values: vec![8, 16, 32],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_values: vec![3],
            ngram_max_values: vec![48],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // NgramSimple: 1 × 3 draft_max = 3
        // NgramMod: 1 × 2 n-match × 1 n-min × 1 n-max = 2 (no draft_max sweep)
        // Total: 5
        assert_eq!(matrix.len(), 5);
    }

    /// Verifies that build_sweep_matrix returns an error when ngram_n_values is empty but required.
    #[test]
    fn test_sweep_matrix_requires_ngram_n() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![],
            ngram_m_values: vec![],
            ngram_min_values: vec![3],
            ngram_max_values: vec![48],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ngram_n_values is required"));
    }

    /// Verifies that build_sweep_matrix returns an error when ngram_m_values is empty but required.
    #[test]
    fn test_sweep_matrix_requires_ngram_m() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMapK],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_values: vec![],
            ngram_max_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ngram_m_values is required"));
    }

    /// Verifies that SpecType::as_str() returns correct string values.
    #[test]
    fn test_spec_type_as_str() {
        assert_eq!(SpecType::NgramSimple.as_str(), "ngram-simple");
        assert_eq!(SpecType::NgramMod.as_str(), "ngram-mod");
        assert_eq!(SpecType::NgramMapK.as_str(), "ngram-map-k");
        assert_eq!(SpecType::NgramMapK4v.as_str(), "ngram-map-k4v");
        assert_eq!(SpecType::DraftMtp.as_str(), "draft-mtp");
    }

    /// Verifies that the sweep matrix produces 3D n-gram-mod entries
    /// (draft_max × n-match × n-min × n-max).
    #[test]
    fn test_sweep_matrix_ngram_mod_3d() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5, 8],
            ngram_m_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
            ngram_min_values: vec![3, 5],
            ngram_max_values: vec![48, 64],
        };

        let matrix = build_sweep_matrix(&config).unwrap();
        // 1 spec_type × 3 n-match × 2 n-min × 2 n-max = 12
        // (draft_max is NOT swept for ngram-mod — controlled by n-min/n-max)
        assert_eq!(matrix.len(), 12);

        // Verify the first entry has all fields set.
        let first = &matrix[0];
        assert_eq!(first.spec_type, SpecType::NgramMod);
        assert_eq!(first.draft_max, 8);
        assert_eq!(first.ngram_n, Some(3));
        assert_eq!(first.ngram_min, Some(3));
        assert_eq!(first.ngram_max, Some(48));
    }

    /// Verifies that build_sweep_matrix returns an error when nmin/nmax values
    /// are empty but required for n-gram-mod.
    #[test]
    fn test_sweep_matrix_ngram_mod_requires_min_max() {
        let config = SpecBenchConfig {
            model_path: PathBuf::from("/test/model.gguf"),
            spec_types: vec![SpecType::NgramMod],
            draft_max_values: vec![8, 16],
            ngram_n_values: vec![3, 5],
            ngram_m_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
            ngram_min_values: vec![],
            ngram_max_values: vec![],
        };

        let result = build_sweep_matrix(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ngram_min_values") || err_msg.contains("ngram_max_values"));
    }

    /// Verifies that SpecType::spec_ngram_flags() returns correct type-specific
    /// flag names (llama.cpp PR #22397).
    #[test]
    fn test_spec_type_ngram_flags() {
        let (sn, sm, mh) = SpecType::NgramSimple.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-simple-size-n");
        assert_eq!(sm, "--spec-ngram-simple-size-m");
        assert_eq!(mh, "--spec-ngram-simple-min-hits");

        let (sn, sm, mh) = SpecType::NgramMod.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-mod-n-match");
        assert_eq!(sm, "");
        assert_eq!(mh, "");

        let (sn, sm, mh) = SpecType::NgramMapK.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-map-k-size-n");
        assert_eq!(sm, "--spec-ngram-map-k-size-m");
        assert_eq!(mh, "--spec-ngram-map-k-min-hits");

        let (sn, sm, mh) = SpecType::NgramMapK4v.spec_ngram_flags();
        assert_eq!(sn, "--spec-ngram-map-k4v-size-n");
        assert_eq!(sm, "--spec-ngram-map-k4v-size-m");
        assert_eq!(mh, "--spec-ngram-map-k4v-min-hits");

        let (sn, sm, mh) = SpecType::DraftMtp.spec_ngram_flags();
        assert_eq!(sn, "");
        assert_eq!(sm, "");
        assert_eq!(mh, "");
    }
}
