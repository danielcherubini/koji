use serde::{Deserialize, Serialize};

use super::vram::VramInfo;

/// GPU vendor identifier.
///
/// `PartialOrd`/`Ord` derive is used for stable sort ordering of GPU devices
/// in `SystemMetrics` (Amd < Nvidia).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Amd,
    #[default]
    Nvidia,
}

impl GpuVendor {
    /// Convert the vendor to its string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nvidia => "nvidia",
            Self::Amd => "amd",
        }
    }
}

/// Lifecycle state of a model's backend.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelState {
    /// No model is loaded on this backend.
    #[default]
    Idle,
    /// The backend is currently starting up.
    #[serde(alias = "loading")]
    Starting,
    /// The backend is ready and accepting requests.
    Ready,
    /// The backend is unloading.
    Unloading,
    /// The backend has failed to load or crashed.
    Failed,
}

impl ModelState {
    /// Convert the state to its string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Unloading => "unloading",
            Self::Failed => "failed",
        }
    }
}

/// Per-GPU device statistics for a single tick. One entry per detected
/// device (NVIDIA or AMD). Order is stable per-tick: NVIDIA devices
/// sorted by `index`, then AMD devices by `card` number.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuDeviceStats {
    /// Position-based device identifier (e.g. "GPU0", "GPU1").
    /// Independent of vendor — the Nth detected GPU across all vendors.
    /// Used for display and model→GPU assignment in the UI.
    /// At backend launch, mapped to the llama.cpp device name
    /// (e.g. "CUDA0", "ROCm0", "Vulkan0") by the args builder.
    pub device_id: String,
    /// GPU vendor.
    pub vendor: GpuVendor,
    /// Human-readable GPU name (e.g. "Radeon AI PRO R9700", "GeForce RTX 4090").
    /// Defaults to empty string for backwards compatibility with cached samples.
    #[serde(default)]
    pub name: String,
    /// Utilization percentage (0–100), None if unavailable.
    pub utilization_pct: Option<u8>,
    /// VRAM usage in MiB, None if unavailable.
    pub vram: Option<VramInfo>,
    /// Edge temperature in °C, None if unavailable.
    pub temperature_c: Option<u8>,
    /// Power draw in watts, None if unavailable.
    pub power_w: Option<u16>,
    /// Fan speed percentage (0–100), None if unavailable.
    pub fan_pct: Option<u8>,
    /// PCI bus address (e.g. "0000:03:00.0") used for vendor-tool correlation.
    /// None for NVIDIA (uses index directly) or when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pci_bus: Option<String>,
    /// Hardware UUID for env-var GPU isolation (e.g. "GPU-4b2c1a9f-...").
    /// None when unavailable (Vulkan/Metal/no tooling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
}

/// A snapshot of system-level hardware metrics.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemMetrics {
    /// CPU utilization percentage (0.0–100.0)
    pub cpu_usage_pct: f32,
    /// RAM currently in use (MiB)
    pub ram_used_mib: u64,
    /// Total RAM (MiB)
    pub ram_total_mib: u64,
    /// GPU utilization percentage (0–100), None if not available.
    /// Derived as the mean of per-device utilization.
    pub gpu_utilization_pct: Option<u8>,
    /// VRAM usage, None if not available.
    /// Derived as the sum of per-device VRAM.
    pub vram: Option<VramInfo>,
    /// Per-GPU device stats, one entry per detected device.
    #[serde(default)]
    pub gpus: Vec<GpuDeviceStats>,
    /// Network throughput statistics, None if no interface detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::network::NetworkStats>,
}

/// A timestamped snapshot of system + proxy metrics, suitable for persistence
/// in `system_metrics_history` and broadcast over the SSE stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    pub gpu_utilization_pct: Option<u8>,
    pub vram: Option<VramInfo>,
    /// Per-GPU device stats for this sample. Empty if no GPU is detected
    /// or the backend does not support per-device queries. Always present
    /// (use `#[serde(default)]`) so older cached samples still deserialize.
    #[serde(default)]
    pub gpus: Vec<GpuDeviceStats>,
    pub models_loaded: u64,
    /// Per-model loaded/idle status, embedded in `MetricSample.models`.
    #[serde(default)]
    pub models: Vec<crate::models::ModelStateSnapshot>,
    /// Token generation speed (tokens per second), None if not yet observed.
    #[serde(default)]
    pub tps: Option<f32>,
    /// Prompt processing speed in tokens per second, None if not yet observed.
    #[serde(default)]
    pub prompt_tps: Option<f32>,
    /// KV-cache hit rate percentage, None if not yet observed.
    #[serde(default)]
    pub cache_hit_pct: Option<f32>,
    /// Speculative decoding acceptance rate, None if not yet observed.
    #[serde(default)]
    pub spec_accept_pct: Option<f32>,
    /// True if speculative decoding has been active (draft tokens accepted).
    #[serde(default)]
    pub spec_decoding_active: bool,
    /// Unix ms timestamp of the last inference update — transient, not persisted.
    #[serde(default)]
    pub inference_last_updated_ms: Option<i64>,
    /// Network throughput statistics for this sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::network::NetworkStats>,
}

/// One 30-second aggregated bucket for bar charts.
///
/// Produced by the metrics collector's bucket accumulator. Each bucket
/// averages all 2s samples that fell within a 30-second wall-clock window
/// (timestamp floored to a 30s boundary). Once a sample crosses into the
/// next window, the previous bucket is frozen (`complete = true`) and never
/// changes — only the trailing in-progress bucket (`complete = false`)
/// updates as new samples arrive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricBucket {
    /// Wall-clock start of this 30s window (floored to a 30s boundary).
    pub ts_unix_ms: i64,
    /// Average CPU usage % over samples in this bucket.
    pub cpu_usage_pct: f32,
    /// Average RAM used (MiB) over samples in this bucket.
    pub ram_used_mib: u64,
    /// RAM total (MiB) — taken from the last sample in this bucket, since
    /// total RAM is effectively constant within a 30s window.
    pub ram_total_mib: u64,
    /// Average network throughput over samples in this bucket.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::network::NetworkStats>,
    /// Average utilization % per GPU device over samples in this bucket.
    /// Index aligns with `MetricCurrent.gpus` order. Empty when no GPUs
    /// are detected (CPU-only servers, laptops).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_utils: Vec<f32>,
    /// Whether this 30s window has elapsed (frozen) or is still accumulating.
    /// The last bucket in the array is typically `false` (in-progress).
    #[serde(default)]
    pub complete: bool,
}

/// Point-in-time current state broadcast once per snapshot. Carries GPU
/// device stats, per-model statuses (with per-model `tps`/`prompt_tps`),
/// aggregate inference stats, AND the instantaneous CPU/RAM/Network values
/// for the big-number displays on the dashboard.
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
    pub network: Option<crate::network::NetworkStats>,
    /// Per-GPU device stats for this sample. Empty if no GPU is detected.
    #[serde(default)]
    pub gpus: Vec<GpuDeviceStats>,
    /// Per-model loaded/idle status (with per-model tps/prompt_tps).
    #[serde(default)]
    pub models: Vec<crate::models::ModelStateSnapshot>,
    pub models_loaded: u64,
    /// Aggregate inference stats (most-recently-updated server) for
    /// sparkline-free display. None if no inference observed yet.
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

impl MetricSample {
    /// Convert this sample into a [`MetricCurrent`] for broadcast. The
    /// bucket accumulator in the metrics task handles the 30s time-series
    /// aggregation separately — this method only extracts the
    /// point-in-time state plus the instantaneous CPU/RAM/Network values
    /// used by the dashboard's big-number displays.
    pub fn into_current(self) -> MetricCurrent {
        MetricCurrent {
            cpu_usage_pct: self.cpu_usage_pct,
            ram_used_mib: self.ram_used_mib,
            ram_total_mib: self.ram_total_mib,
            network: self.network,
            gpus: self.gpus,
            models: self.models,
            models_loaded: self.models_loaded,
            tps: self.tps,
            prompt_tps: self.prompt_tps,
            cache_hit_pct: self.cache_hit_pct,
            spec_accept_pct: self.spec_accept_pct,
            spec_decoding_active: self.spec_decoding_active,
            inference_last_updated_ms: self.inference_last_updated_ms,
        }
    }
}

/// Full metrics snapshot broadcast over SSE every 2s.
///
/// `buckets` carries ~31 pre-aggregated 30-second windows (30 frozen + 1
/// in-progress) for the bar charts — the frontend renders these directly
/// with no transformation. `current` carries the instantaneous values and
/// point-in-time state (GPU devices, model statuses, inference stats) for
/// the big-number displays and detail cards.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricsSnapshot {
    /// Pre-aggregated 30s buckets for bar charts (~31 entries: 30 frozen +
    /// 1 in-progress). Frozen buckets never change once sealed; only the
    /// trailing in-progress bucket updates as new 2s samples arrive.
    #[serde(default)]
    pub buckets: Vec<MetricBucket>,
    /// Point-in-time state + instantaneous values for big-number displays.
    #[serde(default)]
    pub current: MetricCurrent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_state_serializes_starting() {
        let json = serde_json::to_string(&ModelState::Starting).unwrap();
        assert_eq!(json, "\"starting\"");
    }

    #[test]
    fn test_model_state_deserializes_loading_as_starting() {
        let state: ModelState = serde_json::from_str("\"loading\"").unwrap();
        assert_eq!(state, ModelState::Starting);
    }

    #[test]
    fn test_model_state_serializes_and_deserializes_starting() {
        let json = serde_json::to_string(&ModelState::Starting).unwrap();
        let state: ModelState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, ModelState::Starting);
    }
}
