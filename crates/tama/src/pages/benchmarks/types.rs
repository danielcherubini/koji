//! Types for the benchmarks page.

use serde::{Deserialize, Serialize};

/// A parsed model entry: (id, display_name, quant, n_batch, n_ubatch, supports_mtp).
pub type ModelEntry = (String, String, String, Option<u32>, Option<u32>, bool);

/// A model list entry: (id, display_name, quants, n_batch, n_ubatch, supports_mtp).
pub type ModelListItem = (String, String, Vec<String>, Option<u32>, Option<u32>, bool);

/// Valid benchmark type identifiers and their display labels.
pub const BENCHMARK_TYPES: &[(&str, &str)] = &[
    ("baseline", "Baseline"),
    ("pp_sweep", "PP Sweep"),
    ("kv_quant_q8", "KV Quant (q8_0)"),
    ("kv_quant_q4", "KV Quant (q4_0)"),
    ("context_test", "Context Test"),
    ("spec_scan", "Spec Scan"),
    ("spec_sweep", "Spec Sweep"),
];

/// Parse a model JSON value into (id, display_name, quant, n_batch, n_ubatch).
/// The API returns `id` as an integer (db_id), not a string.
/// Returns one tuple per quant in the "quants" map,
/// plus one for any standalone "quant" field not already in the map.
pub fn parse_model(m: &serde_json::Value) -> Option<Vec<ModelEntry>> {
    let id = m.get("id").map(|v| v.to_string()).unwrap_or_default();
    let name = m
        .get("display_name")
        .or_else(|| m.get("api_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| id.clone());
    let mut quants = Vec::new();

    // Extract quants from the "quants" map (preferred — contains all available quants)
    if let Some(quants_map) = m.get("quants").and_then(|v| v.as_object()) {
        for quant_key in quants_map.keys() {
            quants.push(quant_key.clone());
        }
    } else {
        // Fallback: single "quant" field (legacy / no quants map)
        if let Some(q) = m.get("quant").and_then(|v| v.as_str()) {
            quants.push(q.to_string());
        }
    }

    if quants.is_empty() {
        return None;
    }

    let n_batch = m.get("n_batch").and_then(|v| v.as_u64()).map(|v| v as u32);
    let n_ubatch = m.get("n_ubatch").and_then(|v| v.as_u64()).map(|v| v as u32);

    // Extract capabilities.supports_mtp from the model JSON.
    let supports_mtp = m
        .get("capabilities")
        .and_then(|v| v.get("supports_mtp"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Flatten: one tuple per quant, each with the same id, display_name, and batch values.
    Some(
        quants
            .into_iter()
            .map(|q| (id.clone(), name.clone(), q, n_batch, n_ubatch, supports_mtp))
            .collect::<Vec<_>>(),
    )
}

/// Benchmark history entry returned from `GET /tama/v1/benchmarks/history`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkHistoryEntry {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    /// Engine used for this benchmark: "llama_bench" or "llama_cli_spec".
    #[serde(default)]
    pub engine: Option<String>,
    /// Identifies what kind of benchmark was run (e.g., "baseline", "pp_sweep").
    #[serde(default)]
    pub benchmark_type: Option<String>,
    /// Suite identifier for grouping related benchmark runs.
    #[serde(default)]
    pub suite_id: Option<String>,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub results_count: usize,
    pub status: String,
    pub results: serde_json::Value,
}

/// Preset configurations — each one maps to a phase in the LLM inference
/// tuning methodology (see `llm-inference-tuning-methodology.md`). The
/// presets are ordered so running them top-to-bottom yields the
/// "measure-one-variable-at-a-time" workflow the methodology advocates:
#[derive(Debug, Clone)]
pub struct BenchmarkPresetSpec {
    pub pp_sizes: &'static str,
    pub tg_sizes: &'static str,
    pub batch_sizes: &'static str,
    pub ubatch_sizes: &'static str,
    pub kv_cache_type: &'static str,
    pub depth: &'static str,
}

/// Auto-fill presets for the LLaMA-Bench Test Type dropdown.
pub const LLAMA_BENCH_PRESETS: &[(&str, BenchmarkPresetSpec)] = &[
    (
        "baseline",
        BenchmarkPresetSpec {
            pp_sizes: "2048",
            tg_sizes: "128",
            batch_sizes: "",
            ubatch_sizes: "",
            kv_cache_type: "default",
            depth: "",
        },
    ),
    (
        "pp_sweep",
        BenchmarkPresetSpec {
            pp_sizes: "2048",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "512,1024,2048,4096",
            kv_cache_type: "default",
            depth: "",
        },
    ),
    (
        "kv_quant_q8",
        BenchmarkPresetSpec {
            pp_sizes: "0",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "2048",
            kv_cache_type: "q8_0",
            depth: "0,65536,131072",
        },
    ),
    (
        "kv_quant_q4",
        BenchmarkPresetSpec {
            pp_sizes: "0",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "2048",
            kv_cache_type: "q4_0",
            depth: "0,65536,131072",
        },
    ),
    (
        "context_test",
        BenchmarkPresetSpec {
            pp_sizes: "0",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "2048",
            kv_cache_type: "q8_0",
            depth: "131072",
        },
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_model_with_n_batch_and_n_ubatch() {
        let model_json = serde_json::json!({
            "id": 1,
            "display_name": "test-model",
            "quant": "Q4_K_M",
            "quants": {
                "Q4_K_M": {},
                "Q6_K": {}
            },
            "n_batch": 2048,
            "n_ubatch": 512,
        });

        let result = parse_model(&model_json).unwrap();

        // Should produce one entry per quant, all with same n_batch/n_ubatch
        assert_eq!(result.len(), 2);
        for (id, _name, _quant, n_batch, n_ubatch, _) in &result {
            assert_eq!(id, "1");
            assert_eq!(*n_batch, Some(2048));
            assert_eq!(*n_ubatch, Some(512));
        }
    }

    #[test]
    fn test_parse_model_without_n_batch_and_n_ubatch() {
        let model_json = serde_json::json!({
            "id": 2,
            "display_name": "legacy-model",
            "quant": "Q5_K_M",
            "quants": {
                "Q5_K_M": {},
            },
        });

        let result = parse_model(&model_json).unwrap();

        assert_eq!(result.len(), 1);
        for (id, _name, _quant, n_batch, n_ubatch, _) in &result {
            assert_eq!(id, "2");
            assert_eq!(*n_batch, None);
            assert_eq!(*n_ubatch, None);
        }
    }

    #[test]
    fn test_parse_model_n_batch_only() {
        let model_json = serde_json::json!({
            "id": 3,
            "display_name": "partial-model",
            "quant": "Q4_K_M",
            "quants": {
                "Q4_K_M": {},
            },
            "n_batch": 4096,
        });

        let result = parse_model(&model_json).unwrap();

        assert_eq!(result.len(), 1);
        for (id, _name, _quant, n_batch, n_ubatch, _) in &result {
            assert_eq!(id, "3");
            assert_eq!(*n_batch, Some(4096));
            assert_eq!(*n_ubatch, None);
        }
    }
}
