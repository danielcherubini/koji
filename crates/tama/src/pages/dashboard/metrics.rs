use serde::{Deserialize, Serialize};

use crate::core_mirrors::{GpuVendor, ModelState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub download_mibps: f64,
    pub upload_mibps: f64,
}

/// One 30-second aggregated bucket for bar charts. Produced by the backend's
/// bucket accumulator — the frontend renders these directly with no
/// transformation. Frozen buckets (`complete = true`) never change; only the
/// trailing in-progress bucket (`complete = false`) updates as new samples
/// arrive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricBucket {
    /// Wall-clock start of this 30s window (floored to a 30s boundary).
    pub ts_unix_ms: i64,
    /// Average CPU usage % over samples in this bucket.
    pub cpu_usage_pct: f32,
    /// Average RAM used (MiB) over samples in this bucket.
    pub ram_used_mib: u64,
    /// RAM total (MiB) — from the last sample in this bucket.
    pub ram_total_mib: u64,
    /// Average network throughput over samples in this bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkStats>,
    /// Average utilization % per GPU device over samples in this bucket.
    /// Index aligns with `MetricCurrent.gpus` order. Empty when no GPUs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_utils: Vec<f32>,
    /// Whether this 30s window has elapsed (frozen) or is still accumulating.
    #[serde(default)]
    pub complete: bool,
}

/// Point-in-time current state broadcast once per snapshot. Carries GPU device
/// stats, per-model statuses (with per-model tps/prompt_tps), inference stats,
/// AND the instantaneous CPU/RAM/Network values for the big-number displays.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricCurrent {
    /// Instantaneous CPU usage % (latest 2s sample) for big-number display.
    #[serde(default)]
    pub cpu_usage_pct: f32,
    /// Instantaneous RAM used (MiB) for big-number display.
    #[serde(default)]
    pub ram_used_mib: u64,
    /// Instantaneous RAM total (MiB).
    #[serde(default)]
    pub ram_total_mib: u64,
    /// Instantaneous network throughput for big-number display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkStats>,
    #[serde(default)]
    pub gpus: Vec<GpuDeviceStats>,
    #[serde(default)]
    pub models: Vec<ModelStateSnapshot>,
    pub models_loaded: u64,
    #[serde(default)]
    pub tps: Option<f32>,
    #[serde(default)]
    pub prompt_tps: Option<f32>,
    #[serde(default)]
    pub cache_hit_pct: Option<f32>,
    #[serde(default)]
    pub spec_accept_pct: Option<f32>,
    #[serde(default)]
    pub spec_decoding_active: bool,
    #[serde(default)]
    pub inference_last_updated_ms: Option<i64>,
}

/// Full metrics snapshot broadcast over SSE every 2s. `buckets` carries
/// ~31 pre-aggregated 30s windows for the bar charts; `current` carries
/// instantaneous values + point-in-time state for the big-number displays
/// and detail cards.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricsSnapshot {
    #[serde(default)]
    pub buckets: Vec<MetricBucket>,
    #[serde(default)]
    pub current: MetricCurrent,
}

/// Frontend mirror of `tama_core::gpu::GpuDeviceStats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuDeviceStats {
    pub device_id: String,
    pub vendor: GpuVendor,
    /// Human-readable GPU name (e.g. "Radeon AI PRO R9700", "GeForce RTX 4090").
    #[serde(default)]
    pub name: String,
    pub utilization_pct: Option<u8>,
    pub vram: Option<VramInfo>,
    pub temperature_c: Option<u8>,
    pub power_w: Option<u16>,
    pub fan_pct: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VramInfo {
    pub used_mib: u64,
    pub total_mib: u64,
}

/// Frontend mirror of `tama_core::models::ModelStateSnapshot`.
///
/// Kept private to this module so the dashboard owns its wire shape; the only
/// contract with the backend is the JSON field names, which must match the
/// server-side struct exactly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelStateSnapshot {
    pub id: String,
    #[serde(default)]
    pub db_id: Option<i64>,
    #[serde(default)]
    pub api_name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    pub backend: String,
    /// Lifecycle state: idle, loading, ready, unloading, failed.
    #[serde(default)]
    pub state: ModelState,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub hf_architecture_type: Option<String>,
    #[serde(default)]
    pub hf_base_model: Option<String>,
    #[serde(default)]
    pub hf_format: Option<String>,
    #[serde(default)]
    pub gpu_variant: Option<String>,
    #[serde(default)]
    pub cache_type_k: Option<String>,
    #[serde(default)]
    pub cache_type_v: Option<String>,
    #[serde(default)]
    pub spec_types: Vec<String>,
    #[serde(default)]
    pub gpu_device: Option<String>,
    #[serde(default)]
    pub tps: Option<f32>,
    #[serde(default)]
    pub prompt_tps: Option<f32>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub is_docker: bool,
}

/// Format a number with comma separators (e.g. `8460` → `"8,460"`).
pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

/// Filter models to only those that are currently active (ready, loading, or unloading).
///
/// Used by the dashboard to render the Active Models list and by the
/// "X loaded" summary heading. Extracted as a free function so it can
/// be unit-tested independently of the Leptos reactive view.
#[allow(dead_code)] // Used only by unit tests in dashboard/tests.rs
pub fn active_models(models: &[ModelStateSnapshot]) -> Vec<ModelStateSnapshot> {
    models
        .iter()
        .filter(|m| {
            matches!(
                m.state,
                ModelState::Ready | ModelState::Starting | ModelState::Unloading
            )
        })
        .cloned()
        .collect()
}

/// Returns models whose state is NOT one of the "active" states.
/// These are models that are idle, failed, or otherwise not running.
/// Note: Models with an empty state string are treated as inactive.
/// This matches the behavior of `active_models()` which only considers
/// "ready", "loading", and "unloading" as active states.
#[allow(dead_code)] // Used only by unit tests in dashboard/tests.rs
pub fn inactive_models(models: &[ModelStateSnapshot]) -> Vec<ModelStateSnapshot> {
    models
        .iter()
        .filter(|m| {
            !matches!(
                m.state,
                ModelState::Ready | ModelState::Starting | ModelState::Unloading
            )
        })
        .cloned()
        .collect()
}

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
pub fn model_display_name(m: &ModelStateSnapshot) -> String {
    m.display_name
        .as_deref()
        .or(m.api_name.as_deref())
        .unwrap_or(m.id.as_str())
        .to_string()
}

/// Sort models by base model, then by display name as a tiebreaker.
#[allow(dead_code)] // Used only by unit tests in dashboard/tests.rs
pub fn model_sort_key(m: &ModelStateSnapshot) -> (String, String) {
    let primary = m
        .hf_base_model
        .clone()
        .unwrap_or_else(|| model_display_name(m));
    let secondary = model_display_name(m);
    (primary, secondary)
}

/// CSS color for a GPU bar series by device index. Cycles through the accent
/// palette so GPU0/GPU1/GPU2... get distinct, stable colors: blue, green,
/// purple, amber, cyan, orange, pink, red. Indices beyond the table wrap.
pub fn gpu_series_color(index: usize) -> &'static str {
    const PALETTE: [&str; 8] = [
        "var(--accent-blue)",
        "var(--accent-green)",
        "var(--accent-purple)",
        "var(--accent-yellow)",
        "var(--accent-cyan)",
        "var(--accent-orange)",
        "var(--accent-pink)",
        "var(--accent-red)",
    ];
    PALETTE[index % PALETTE.len()]
}
