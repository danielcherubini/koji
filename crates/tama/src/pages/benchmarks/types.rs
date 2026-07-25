//! Types for the benchmarks page.

use serde::{Deserialize, Serialize};

/// Valid benchmark type identifiers and their display labels.
pub const BENCHMARK_TYPES: &[(&str, &str)] = &[
    ("baseline", "Baseline"),
    ("pp_sweep", "PP Sweep"),
    ("kv_quant_q8", "KV Quant (q8_0)"),
    ("kv_quant_q4", "KV Quant (q4_0)"),
    ("context_test", "Context Test"),
    ("spec_scan", "Spec Scan"),
    ("spec_sweep", "Spec Sweep"),
    ("mtp_sweep", "MTP Sweep"),
];

/// Parse a model JSON value into (id, display_name, quant).
/// The API returns `id` as an integer (db_id), not a string.
/// Parse a model entry from the API response.
/// Returns one (id, display_name, quant) tuple per quant in the "quants" map,
/// plus one for any standalone "quant" field not already in the map.
pub fn parse_model(m: &serde_json::Value) -> Option<Vec<(String, String, String)>> {
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

    // Flatten: one tuple per quant, each with the same id and display_name.
    Some(
        quants
            .into_iter()
            .map(|q| (id.clone(), name.clone(), q))
            .collect(),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
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

/// Auto-fill presets for the Spec Decoding Test Type dropdown.
#[allow(clippy::type_complexity)]
pub const SPEC_BENCH_PRESETS: &[(&str, (&[u32], &str, &str, &str))] = &[
    ("spec_scan", (&[256], "16", "12", "48")),
    (
        "spec_sweep",
        (&[8, 16, 32, 48, 64], "8,16,32,48,64", "12,16,24", "32,48"),
    ),
];
