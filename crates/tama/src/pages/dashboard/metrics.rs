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
    /// Average token generation speed (tok/s) over samples in this bucket.
    #[serde(default)]
    pub tps: f32,
    /// Average prompt processing speed (tok/s) over samples in this bucket.
    #[serde(default)]
    pub prompt_tps: f32,
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
/// and detail cards; `hosts` carries per-tamad host stats (plan-191
/// Task 9 — the proxy's own CPU/RAM stay in `current` for its own card).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricsSnapshot {
    #[serde(default)]
    pub buckets: Vec<MetricBucket>,
    #[serde(default)]
    pub current: MetricCurrent,
    /// One entry per registered tamad (freshest stats snapshot, ~1s fresh).
    #[serde(default)]
    pub hosts: Vec<HostStats>,
}

/// Additive per-tamad host entry in the metrics stream (plan-191 Task 4
/// shape + `version` from the cached HealthCheck).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostStats {
    #[serde(default)]
    pub tamad_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub online: bool,
    /// The tamad's self-reported version (last successful health check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub memory: HostMemory,
    #[serde(default)]
    pub gpus: Vec<HostGpu>,
}

/// Host RAM bytes (total + used).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostMemory {
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub used_bytes: u64,
}

/// One GPU on a tamad host (from its `SystemStats` stream).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HostGpu {
    #[serde(default)]
    pub index: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub driver_version: String,
    #[serde(default)]
    pub vram_total_bytes: i64,
    #[serde(default)]
    pub vram_used_bytes: i64,
    #[serde(default)]
    pub utilization_percent: f64,
    #[serde(default)]
    pub temperature_c: f64,
    #[serde(default)]
    pub power_w: f64,
    /// 0-100 fan duty cycle; 0 when unavailable.
    #[serde(default)]
    pub fan_percent: f64,
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
    /// Display-only, source tamad/provider name when resolvable. None → the
    /// model lands in the dashboard's "Unassigned" group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
}

/// Format a number with comma separators (e.g. `8460` → `"8,460"`).
#[allow(dead_code)] // Used only by unit tests in dashboard/tests.rs
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

/// Timeline of the 15-minute inference telemetry series from pre-aggregated
/// buckets, as read by the dashboard's Inference Telemetry section.
#[derive(Debug, Clone, PartialEq)]
pub struct InferenceTelemetry {
    /// Per-bucket token-generation speed (tok/s), oldest → newest.
    pub tg: Vec<f32>,
    /// Per-bucket prompt-processing speed (tok/s), oldest → newest.
    pub pp: Vec<f32>,
    /// Peak TG (tok/s) over the window (0.0 when empty).
    pub tg_peak: f32,
    /// Peak PP (tok/s) over the window (0.0 when empty).
    pub pp_peak: f32,
}

/// Build the 15-minute inference telemetry timeline from pre-aggregated
/// backend buckets. Extracted as a pure function so the view only renders
/// values — the series, their window peaks, and the chart scaling all live
/// here and are unit-tested independently of the Leptos reactive view.
pub fn build_inference_telemetry(buckets: &[MetricBucket]) -> InferenceTelemetry {
    let tg: Vec<f32> = buckets.iter().map(|b| b.tps).collect();
    let pp: Vec<f32> = buckets.iter().map(|b| b.prompt_tps).collect();
    InferenceTelemetry {
        tg_peak: tg.iter().copied().fold(0.0f32, f32::max),
        pp_peak: pp.iter().copied().fold(0.0f32, f32::max),
        tg,
        pp,
    }
}

/// Convert a throughput (tok/s) into a per-token latency in milliseconds
/// (`1000 / tps`). Returns `None` for non-positive or `NaN` throughputs
/// (the `tps > 0.0` guard rejects NaN) — there is no inter-token latency
/// to display when nothing is being generated.
pub fn ms_per_token(tps: f32) -> Option<f64> {
    (tps > 0.0).then(|| 1000.0 / tps as f64)
}

/// The single digit rule for live throughput/latency numbers:
/// `v < 1` → 2 decimals, NO trailing trim ("0.30");
/// `1 <= v < 100` → 1 decimal, trim a rendered trailing ".0" ("72.6";
/// `100.0`/`99.96` → "100"); `v >= 100` → 0 decimals ("3347").
///
/// Pure number — no unit suffix (callers append " tok/s" / " ms/tok").
pub fn format_auto(v: f64) -> String {
    if v < 1.0 {
        format!("{v:.2}")
    } else if v < 100.0 {
        let s = format!("{v:.1}");
        s.strip_suffix(".0").unwrap_or(&s).to_string()
    } else {
        format!("{v:.0}")
    }
}

/// Format a live throughput with the shared digit rule and unit suffix,
/// e.g. "72.6 tok/s", "3347 tok/s".
pub fn format_tok_s(v: f64) -> String {
    format!("{} tok/s", format_auto(v))
}

/// Format a per-token latency with the shared digit rule and unit suffix,
/// e.g. "13.8 ms/tok", "0.30 ms/tok", "25 ms/tok", "1 ms/tok" (trim).
/// Display wrapper over [`format_auto`] — same body as the tok/s numbers.
pub fn format_ms_per_token(v: f64) -> String {
    format!("{} ms/tok", format_auto(v))
}

/// Format a percentage with one decimal, for the spec-decode acceptance
/// rate — e.g. "44.5%".
pub fn format_pct(v: f64) -> String {
    format!("{v:.1}%")
}

/// Partition active models into per-host buckets keyed by host name, plus an
/// "unassigned" bucket for models whose `host_name` is `None` or matches no
/// known host. Every name in `host_names` gets a bucket (possibly empty) so
/// the dashboard can render one card per host without a missing-key branch.
///
/// Extracted as a pure function so the host-centric grouping is unit-testable
/// independently of the Leptos reactive view.
pub fn partition_models_by_host(
    models: Vec<ModelStateSnapshot>,
    host_names: &[String],
) -> (
    std::collections::HashMap<String, Vec<ModelStateSnapshot>>,
    Vec<ModelStateSnapshot>,
) {
    let mut by_host: std::collections::HashMap<String, Vec<ModelStateSnapshot>> = host_names
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();
    let mut unassigned = Vec::new();
    for m in models {
        match &m.host_name {
            Some(name) if by_host.contains_key(name) => {
                by_host.get_mut(name).expect("key checked above").push(m);
            }
            _ => unassigned.push(m),
        }
    }
    (by_host, unassigned)
}

/// Convert a tamad host's `HostGpu` list into the `GpuDeviceStats` shape the
/// GPU-allocation chip resolver (`model_gpu_label`) expects. Shared by the
/// dashboard (unassigned rows resolve against every host's GPUs) and the
/// host card (rows resolve against the card's own GPUs).
pub fn host_gpus_to_device_stats(gpus: &[HostGpu]) -> Vec<GpuDeviceStats> {
    gpus.iter()
        .map(|g| {
            let used_mib = (g.vram_used_bytes.max(0) as u64) / (1024 * 1024);
            let total_mib = (g.vram_total_bytes.max(0) as u64) / (1024 * 1024);
            GpuDeviceStats {
                device_id: format!("GPU{}", g.index),
                vendor: GpuVendor::default(),
                name: g.name.clone(),
                utilization_pct: Some(g.utilization_percent.clamp(0.0, 100.0) as u8),
                vram: (g.vram_total_bytes > 0).then_some(VramInfo {
                    used_mib,
                    total_mib,
                }),
                temperature_c: Some(g.temperature_c as u8),
                power_w: None,
                fan_pct: None,
            }
        })
        .collect()
}

/// Filter models to those currently loaded or still coming up —
/// `ModelState::Ready` or `ModelState::Starting` only.
///
/// Unlike [`active_models`], models that have started unloading are
/// excluded: once unloading begins, a model no longer belongs in the
/// dashboard's Active Models list. Extracted as a free function so it can
/// be unit-tested independently of the Leptos reactive view.
pub fn loaded_or_starting_models(models: &[ModelStateSnapshot]) -> Vec<ModelStateSnapshot> {
    models
        .iter()
        .filter(|m| matches!(m.state, ModelState::Ready | ModelState::Starting))
        .cloned()
        .collect()
}

/// Format the cluster summary line rendered under the Dashboard title.
/// The count words inflect with their value — singular for one
/// (`1 Node (1 GPU) · 1 Model Active · 53 tok/s`), plural otherwise
/// (`2 Nodes (3 GPUs) · 2 Models Active · 53 tok/s`).
///
/// `tps` is rendered with the shared digit rule ([`format_tok_s`]); when it
/// is `None` or zero the line ends with a placeholder dash (`—`) instead.
pub fn format_cluster_subtitle(
    nodes: usize,
    gpus: usize,
    active_models: usize,
    tps: Option<f32>,
) -> String {
    let tps_str = match tps {
        Some(t) if t > 0.0 => format_tok_s(t as f64),
        _ => "—".to_string(),
    };
    let node_label = if nodes == 1 { "Node" } else { "Nodes" };
    let gpu_label = if gpus == 1 { "GPU" } else { "GPUs" };
    let model_label = if active_models == 1 {
        "Model"
    } else {
        "Models"
    };
    format!("{nodes} {node_label} ({gpus} {gpu_label}) · {active_models} {model_label} Active · {tps_str}")
}

/// Format the gateway status pill text for the dashboard header, e.g.
/// `● Gateway Online (v2.1.0) · Up 2h 15m`.
///
/// Renders only what is known: when the proxy version or uptime hasn't
/// arrived yet the corresponding part is omitted, and when the stream is
/// down the pill reads `● Gateway Offline` regardless of the other inputs.
pub fn gateway_status_text(
    online: bool,
    version: Option<&str>,
    uptime_seconds: Option<f64>,
) -> String {
    if !online {
        return "● Gateway Offline".to_string();
    }
    let mut head = String::from("● Gateway Online");
    if let Some(ver) = version {
        head.push_str(&format!(" (v{ver})"));
    }
    let mut parts = vec![head];
    if let Some(up) = uptime_seconds {
        parts.push(format!("Up {}", format_uptime(up)));
    }
    parts.join(" · ")
}

/// Build the meta line parts for an Active Models row:
/// `gpu_variant · quant · {ctx}k ctx · format`, skipping missing parts so
/// the view can simply join whatever is present with ` · `.
///
/// A context length of 1000 or more is abbreviated in thousands, rounded
/// to the nearest thousand (`262144` → `262k`, `262800` → `263k`); smaller
/// values are rendered as raw numbers.
pub fn format_model_meta_parts(m: &ModelStateSnapshot) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for value in [&m.gpu_variant, &m.quant].into_iter().flatten() {
        if !value.is_empty() {
            parts.push(value.clone());
        }
    }
    if let Some(ctx) = m.context_length {
        let ctx_str = if ctx >= 1000 {
            format!("{}k", (ctx + 500) / 1000)
        } else {
            ctx.to_string()
        };
        parts.push(format!("{ctx_str} ctx"));
    }
    if let Some(file_format) = &m.hf_format {
        if !file_format.is_empty() {
            parts.push(file_format.clone());
        }
    }
    parts
}

/// Legacy broad "active" filter: models in the `Ready`, `Starting`, or
/// `Unloading` states (i.e. anything not idle/failed).
///
/// The dashboard no longer uses this for the Active Models list — it uses
/// [`loaded_or_starting_models`], which also excludes models that have
/// started unloading (and hence renders the "X active" summary count).
/// Kept because its state semantics (including `Unloading`) are what the
/// unit tests in `dashboard/tests.rs` document.
///
/// Extracted as a free function so it can be unit-tested independently of
/// the Leptos reactive view.
#[allow(dead_code)] // Referenced only by the unit tests in dashboard/tests.rs — the dashboard view uses loaded_or_starting_models instead (this filter wrongly keeps Unloading models in the Active Models list).
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

/// Format a byte count as a compact GiB string, e.g. `3.2 GiB`.
pub fn format_bytes_gib(bytes: u64) -> String {
    if bytes == 0 {
        return "0 GiB".to_string();
    }
    let gib = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    if gib >= 100.0 {
        format!("{gib:.0} GiB")
    } else {
        format!("{gib:.1} GiB")
    }
}

/// Format a byte count as GiB rounded to whole strings, e.g. `24 GiB`
/// (for card subtitles where one decimal is noise).
pub fn format_bytes_gib_rounded(bytes: u64) -> String {
    if bytes == 0 {
        return "0 GiB".to_string();
    }
    let gib = bytes as f64 / 1024.0 / 1024.0 / 1024.0;
    let rounded = (gib + 0.5) as u64;
    format!("{} GiB", rounded.max(1))
}

/// Format a duration in seconds for the proxy uptime display, e.g.
/// `3d 4h`, `2h 13m`, `45m`, `12s`.
pub fn format_uptime(total_seconds: f64) -> String {
    if total_seconds < 0.0 {
        return "0s".to_string();
    }
    let secs = total_seconds as i64;
    let (days, hours, minutes, seconds) = (
        secs / 86_400,
        (secs % 86_400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    );
    match () {
        _ if days > 0 => format!("{}d {}h", days, hours),
        _ if hours > 0 => format!("{}h {}m", hours, minutes),
        _ if minutes > 0 => format!("{}m", minutes),
        _ => format!("{}s", seconds),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1-decimal branch trims a rendered trailing ".0" so values that
    /// round up to a whole number (99.96) look clean rather than "100.0".
    #[test]
    fn test_format_tok_s_trim_mid_range() {
        assert_eq!(format_tok_s(99.96), "100 tok/s");
        assert_eq!(format_tok_s(100.0), "100 tok/s");
    }

    /// Values in [1, 100) keep their one decimal ("72.6 tok/s"); values
    /// at/above 100 render 0-decimal ("3347 tok/s") — no "3347.2".
    #[test]
    fn test_format_tok_s_digit_rules() {
        assert_eq!(format_tok_s(72.6), "72.6 tok/s");
        assert_eq!(format_tok_s(3347.2), "3347 tok/s");
    }

    /// Values below 1 keep two decimals with NO trailing trim — the false
    /// "0.0" the old `{ms:.1}` formatting produced for tiny ms/tok values
    /// now reads "0.30".
    #[test]
    fn test_format_auto_sub_one_no_trim() {
        assert_eq!(format_auto(0.2987), "0.30");
    }

    /// The ms/tok display wrapper shares the body of `format_auto` and
    /// appends the unit — "1 ms/tok" (trim), "25 ms/tok" (trim),
    /// "13.8 ms/tok" (mid range), "0.30 ms/tok" (sub-1, no trim).
    #[test]
    fn test_format_ms_per_token() {
        assert_eq!(format_ms_per_token(1.0), "1 ms/tok");
        assert_eq!(format_ms_per_token(25.0), "25 ms/tok");
        assert_eq!(format_ms_per_token(13.76), "13.8 ms/tok");
        assert_eq!(format_ms_per_token(0.02987), "0.03 ms/tok");
    }

    /// The spec-decode acceptance rate renders with one decimal ("44.5%").
    #[test]
    fn test_format_pct() {
        assert_eq!(format_pct(44.474), "44.5%");
        assert_eq!(format_pct(99.96), "100.0%");
    }
}
