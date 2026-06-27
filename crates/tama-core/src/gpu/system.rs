use serde::{Deserialize, Serialize};
use sysinfo::System;

use super::vram::VramInfo;

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
    /// Human-readable vendor: "nvidia" | "amd".
    pub vendor: String,
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
    pub models: Vec<ModelStatus>,
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

/// Per-model loaded/idle status, embedded in `MetricSample.models`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelStatus {
    pub id: String,
    /// Integer database id of the model_configs row, if known. Emitted so the
    /// dashboard can link to the editor by id rather than by config_key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_id: Option<i64>,
    pub api_name: Option<String>,
    pub display_name: Option<String>,
    pub backend: String,
    /// Deprecated: use `state` instead. True iff the model is in the Ready state.
    #[deprecated(since = "1.45.0", note = "use state field instead")]
    pub loaded: bool,
    /// Current lifecycle state of the model's backend.
    /// One of: `idle`, `loading`, `ready`, `unloading`, `failed`.
    #[serde(default)]
    pub state: String,
    /// Quantization name (e.g. "Q4_K_M", "Q8_0"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    /// Model's configured context length in tokens. Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    /// Architecture type from HF metadata (e.g. "MoE", "Dense"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_architecture_type: Option<String>,
    /// Base model from HF metadata (e.g. "Qwen/Qwen3.6-27B"). Display-only on dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_base_model: Option<String>,
    /// GPU variant for the backend (e.g. "cpu", "cuda", "vulkan"). Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_variant: Option<String>,
    /// KV cache quant for K head (e.g. "q4_0", "f16"). Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_k: Option<String>,
    /// KV cache quant for V head (e.g. "q8_0", "f16"). Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_type_v: Option<String>,
    /// Speculative decoding types (e.g. ["draft-mtp", "ngram-simple"]). Display-only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spec_types: Vec<String>,
    /// GPU device name this model is bound to (e.g. "CUDA0", "ROCm0"),
    /// taken from `ModelConfig.gpu_device`. None if the model is idle,
    /// unconfigured, or the backend is not llama.cpp. Display-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_device: Option<String>,
    /// Error message when `state == "failed"`, surfaced on the dashboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Token generation speed for this model's backend (tokens per second).
    /// None if the model is not actively generating or no stats observed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tps: Option<f32>,
    /// Prompt processing speed for this model's backend (tokens per second).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tps: Option<f32>,
}

/// One history point for sparkline charts. Lightweight — carries only the
/// fields that need a rolling history (CPU, RAM, Network). GPU devices,
/// model statuses, and inference stats are NOT included; those live in
/// [`MetricCurrent`] and are sent once per snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricHistoryPoint {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::network::NetworkStats>,
}

/// Point-in-time current state broadcast once per snapshot (not repeated in
/// the history array). Carries GPU device stats, per-model statuses (with
/// per-model `tps`/`prompt_tps`), and aggregate inference stats.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricCurrent {
    /// Per-GPU device stats for this sample. Empty if no GPU is detected.
    #[serde(default)]
    pub gpus: Vec<GpuDeviceStats>,
    /// Per-model loaded/idle status (with per-model tps/prompt_tps).
    #[serde(default)]
    pub models: Vec<ModelStatus>,
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
    /// Split this sample into a lightweight history point (for sparklines)
    /// and a current-state snapshot (for GPU cards, model list, inference).
    pub fn split(self) -> (MetricHistoryPoint, MetricCurrent) {
        let history = MetricHistoryPoint {
            ts_unix_ms: self.ts_unix_ms,
            cpu_usage_pct: self.cpu_usage_pct,
            ram_used_mib: self.ram_used_mib,
            ram_total_mib: self.ram_total_mib,
            network: self.network,
        };
        let current = MetricCurrent {
            gpus: self.gpus,
            models: self.models,
            models_loaded: self.models_loaded,
            tps: self.tps,
            prompt_tps: self.prompt_tps,
            cache_hit_pct: self.cache_hit_pct,
            spec_accept_pct: self.spec_accept_pct,
            spec_decoding_active: self.spec_decoding_active,
            inference_last_updated_ms: self.inference_last_updated_ms,
        };
        (history, current)
    }
}

/// Full metrics snapshot broadcast over SSE every 2s. Splits a rolling
/// history of graphable fields (CPU, RAM, Network) from point-in-time state
/// (GPU devices, model statuses, inference stats) so the latter is not
/// duplicated across every history entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricsSnapshot {
    /// Rolling history of graphable fields (~450 samples) for sparklines.
    /// CPU/RAM/Network current values are also read from the last entry.
    #[serde(default)]
    pub history: Vec<MetricHistoryPoint>,
    /// Point-in-time state: GPU devices, model statuses, inference stats.
    #[serde(default)]
    pub current: MetricCurrent,
}
///
/// The caller is responsible for passing a `System` that persists across
/// calls so that `sysinfo` can compute CPU deltas correctly. This function
/// calls `refresh_cpu_usage` and `refresh_memory` once — no internal sleep.
/// It blocks on nvidia-smi subprocesses; call via `tokio::task::spawn_blocking`.
pub fn collect_system_metrics_with(sys: &mut System) -> SystemMetrics {
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let cpu_usage_pct = sys.global_cpu_info().cpu_usage();
    let ram_used_mib = sys.used_memory() / 1024 / 1024;
    let ram_total_mib = sys.total_memory() / 1024 / 1024;

    // Per-GPU device stats
    let mut gpus = query_nvidia_devices();
    gpus.extend(query_amd_devices());
    // Sort by (vendor, device_index) for stable ordering
    gpus.sort_by(|a, b| {
        a.vendor
            .cmp(&b.vendor)
            .then_with(|| a.device_id.cmp(&b.device_id))
    });
    // Assign position-based device IDs (GPU0, GPU1, ...) after sorting.
    for (i, gpu) in gpus.iter_mut().enumerate() {
        gpu.device_id = format!("GPU{i}");
    }

    // Aggregate metrics derived from per-device data
    let gpu_utilization_pct = aggregate_utilization_mean(&gpus);
    let vram = aggregate_vram_sum(&gpus);

    SystemMetrics {
        cpu_usage_pct,
        ram_used_mib,
        ram_total_mib,
        gpu_utilization_pct,
        vram,
        gpus,
        network: None,
    }
}

/// Collect a snapshot of system metrics (CPU, RAM, GPU util, VRAM).
///
/// Creates a temporary `System`, sleeps for `MINIMUM_CPU_UPDATE_INTERVAL`
/// to get a meaningful CPU reading, then returns the snapshot. Prefer
/// [`collect_system_metrics_with`] for long-running tasks to avoid the
/// per-call allocation and sleep.
///
/// This function blocks — call via `tokio::task::spawn_blocking`.
pub fn collect_system_metrics() -> SystemMetrics {
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    collect_system_metrics_with(&mut sys)
}

// ── per-GPU device stats ───────────────────────────────────────────────

/// Parse a single line of nvidia-smi CSV output into `GpuDeviceStats`.
///
/// Expected format (7 comma-separated fields, no units):
/// `index, utilization.gpu, memory.used, memory.total, temperature.gpu, power.draw, fan.speed`
///
/// Returns `None` if the line is malformed or fields cannot be parsed.
pub(crate) fn parse_nvidia_smi_csv_line(line: &str) -> Option<GpuDeviceStats> {
    let parts: Vec<&str> = line.split(",").collect();
    if parts.len() != 7 {
        return None;
    }

    let index: u32 = parts[0].trim().parse().ok()?;
    let utilization: u8 = parts[1].trim().parse().ok()?;
    let mem_used: u64 = parts[2].trim().parse().ok()?;
    let mem_total: u64 = parts[3].trim().parse().ok()?;
    let temperature: u8 = parts[4].trim().parse().ok()?;
    let power: u16 = parts[5].trim().parse().ok()?;
    let fan: u8 = parts[6].trim().parse().ok()?;

    Some(GpuDeviceStats {
        device_id: format!("nvidia{index}"),
        vendor: "nvidia".to_string(),
        utilization_pct: Some(utilization),
        vram: Some(VramInfo {
            used_mib: mem_used,
            total_mib: mem_total,
        }),
        temperature_c: Some(temperature),
        power_w: Some(power),
        fan_pct: Some(fan),
    })
}

/// Query all NVIDIA GPU devices via nvidia-smi.
/// Returns one `GpuDeviceStats` per detected GPU.
fn query_nvidia_devices() -> Vec<GpuDeviceStats> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok();

    let Some(output) = output else {
        return vec![];
    };

    if !output.status.success() {
        return vec![];
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| parse_nvidia_smi_csv_line(line.trim()))
        .collect()
}

/// Query all AMD GPU devices via sysfs.
/// Returns one `GpuDeviceStats` per detected GPU card.
fn query_amd_devices() -> Vec<GpuDeviceStats> {
    let pattern = "/sys/class/drm/card*/device";
    let Ok(paths) = glob::glob(pattern) else {
        return vec![];
    };

    let mut devices = Vec::new();

    for card_path in paths.flatten() {
        // Extract card number from the path (e.g. /sys/class/drm/card1/device → 1)
        let card_name = card_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let card_num = card_name
            .strip_prefix("card")
            .and_then(|n| n.parse::<u32>().ok());
        let Some(card_num) = card_num else {
            continue;
        };

        // Verify this is an AMD device by checking the driver
        let driver_path = card_path.join("driver");
        let is_amd = std::fs::read_link(&driver_path)
            .ok()
            .map(|p| p.to_string_lossy().contains("amdgpu"))
            .unwrap_or(false);
        if !is_amd {
            continue;
        }

        let mut stats = GpuDeviceStats {
            device_id: format!("amd{card_num}"),
            vendor: "amd".to_string(),
            utilization_pct: None,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
        };

        // GPU utilization from gpu_busy_percent
        if let Ok(contents) = std::fs::read_to_string(card_path.join("gpu_busy_percent")) {
            if let Ok(pct) = contents.trim().parse::<u8>() {
                stats.utilization_pct = Some(pct);
            }
        }

        // VRAM from mem_info_vram_used / mem_info_vram_total
        let used_bytes = std::fs::read_to_string(card_path.join("mem_info_vram_used"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        let total_bytes = std::fs::read_to_string(card_path.join("mem_info_vram_total"))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok());
        if let (Some(u), Some(t)) = (used_bytes, total_bytes) {
            stats.vram = Some(VramInfo {
                used_mib: u / (1024 * 1024),
                total_mib: t / (1024 * 1024),
            });
        }

        // Temperature: try temp1_input or temp2_input (hwmon path varies)
        // Values are in millidegrees Celsius
        let temp_path = card_path.parent().unwrap();
        for temp_file in &["temp1_input", "temp2_input"] {
            if let Ok(contents) = std::fs::read_to_string(temp_path.join(temp_file)) {
                if let Ok(temp_milli) = contents.trim().parse::<i64>() {
                    stats.temperature_c = Some((temp_milli / 1000) as u8);
                    break;
                }
            }
        }

        // Also try hwmon subdirectory for temperature
        if stats.temperature_c.is_none() {
            let hwmon_pattern = format!("{}/hwmon/hwmon*/temp*_input", card_path.display());
            if let Ok(paths) = glob::glob(&hwmon_pattern) {
                for path in paths.flatten() {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        if let Ok(temp_milli) = contents.trim().parse::<i64>() {
                            stats.temperature_c = Some((temp_milli / 1000) as u8);
                            break;
                        }
                    }
                }
            }
        }

        // Power: try power1_average (µW → W)
        if let Ok(contents) = std::fs::read_to_string(temp_path.join("power1_average")) {
            if let Ok(power_uw) = contents.trim().parse::<u64>() {
                stats.power_w = Some((power_uw / 1_000_000) as u16);
            }
        }

        // Also try hwmon for power
        if stats.power_w.is_none() {
            let hwmon_pattern = format!("{}/hwmon/hwmon*/power1_average", card_path.display());
            if let Ok(paths) = glob::glob(&hwmon_pattern) {
                for path in paths.flatten() {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        if let Ok(power_uw) = contents.trim().parse::<u64>() {
                            stats.power_w = Some((power_uw / 1_000_000) as u16);
                            break;
                        }
                    }
                }
            }
        }

        // Fan: scan hwmon for fan1_input (RPM → %)
        let fan_pattern = format!("{}/hwmon/hwmon*/fan1_input", card_path.display());
        if let Ok(paths) = glob::glob(&fan_pattern) {
            for path in paths.flatten() {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    if let Ok(rpm) = contents.trim().parse::<u16>() {
                        // Read fan1_max from the same hwmon directory for accurate %
                        let hwmon_dir = path.parent().unwrap();
                        let max_rpm = std::fs::read_to_string(hwmon_dir.join("fan1_max"))
                            .ok()
                            .and_then(|s| s.trim().parse::<u16>().ok())
                            .unwrap_or(3000);
                        let pct = if max_rpm > 0 && rpm > 0 {
                            ((rpm as u32 * 100) / max_rpm as u32).min(100) as u8
                        } else {
                            0
                        };
                        stats.fan_pct = Some(pct);
                        break;
                    }
                }
            }
        }

        devices.push(stats);
    }

    devices.sort_by_key(|d| d.device_id.clone());
    devices
}

/// Compute the mean utilization across all GPU devices.
/// Only counts devices with a non-None utilization.
fn aggregate_utilization_mean(devices: &[GpuDeviceStats]) -> Option<u8> {
    let values: Vec<u8> = devices.iter().filter_map(|d| d.utilization_pct).collect();
    if values.is_empty() {
        return None;
    }
    let sum: u32 = values.iter().map(|&v| v as u32).sum();
    Some((sum / values.len() as u32) as u8)
}

/// Sum VRAM across all GPU devices.
/// Only sums devices with non-None vram.
fn aggregate_vram_sum(devices: &[GpuDeviceStats]) -> Option<VramInfo> {
    let vrams: Vec<&VramInfo> = devices.iter().filter_map(|d| d.vram.as_ref()).collect();
    if vrams.is_empty() {
        return None;
    }
    let used: u64 = vrams.iter().map(|v| v.used_mib).sum();
    let total: u64 = vrams.iter().map(|v| v.total_mib).sum();
    Some(VramInfo {
        used_mib: used,
        total_mib: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies `collect_system_metrics` returns sane CPU and RAM values on any machine.
    #[test]
    fn test_collect_system_metrics() {
        let metrics = collect_system_metrics();
        assert!(
            metrics.cpu_usage_pct >= 0.0 && metrics.cpu_usage_pct <= 100.0,
            "cpu_usage_pct out of range: {}",
            metrics.cpu_usage_pct
        );
        assert!(metrics.ram_total_mib > 0, "ram_total_mib should be > 0");
        assert!(
            metrics.ram_used_mib <= metrics.ram_total_mib,
            "ram_used_mib ({}) > ram_total_mib ({})",
            metrics.ram_used_mib,
            metrics.ram_total_mib
        );
        // GPU fields may be None in CI — do not assert them
        println!("cpu_usage_pct: {}", metrics.cpu_usage_pct);
        println!("ram_used_mib: {}", metrics.ram_used_mib);
        println!("ram_total_mib: {}", metrics.ram_total_mib);
        println!("gpu_utilization_pct: {:?}", metrics.gpu_utilization_pct);
        println!("vram: {:?}", metrics.vram);
    }

    /// Verifies `collect_system_metrics_with` works correctly when `System` is reused across calls.
    #[test]
    fn test_collect_system_metrics_with_reuses_system() {
        // Verify collect_system_metrics_with works when System is reused across calls.
        let mut sys = System::new();
        let metrics = collect_system_metrics_with(&mut sys);
        assert!(
            metrics.cpu_usage_pct >= 0.0 && metrics.cpu_usage_pct <= 100.0,
            "cpu_usage_pct out of range: {}",
            metrics.cpu_usage_pct
        );
        assert!(metrics.ram_total_mib > 0, "ram_total_mib should be > 0");
        assert!(
            metrics.ram_used_mib <= metrics.ram_total_mib,
            "ram_used_mib ({}) > ram_total_mib ({})",
            metrics.ram_used_mib,
            metrics.ram_total_mib
        );
    }

    // ── per-GPU device stats tests ──────────────────────────────────────

    #[test]
    fn test_parse_nvidia_smi_csv_line() {
        // Simulated nvidia-smi output for one device:
        // index, utilization.gpu, memory.used, memory.total, temperature.gpu, power.draw, fan.speed
        let line = "0, 45, 4096, 8192, 62, 150, 70";
        let stats = parse_nvidia_smi_csv_line(line);
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.device_id, "nvidia0");
        assert_eq!(stats.vendor, "nvidia");
        assert_eq!(stats.utilization_pct, Some(45));
        assert_eq!(stats.temperature_c, Some(62));
        assert_eq!(stats.power_w, Some(150));
        assert_eq!(stats.fan_pct, Some(70));
        assert!(stats.vram.is_some());
        let vram = stats.vram.unwrap();
        assert_eq!(vram.used_mib, 4096);
        assert_eq!(vram.total_mib, 8192);
    }

    #[test]
    fn test_parse_nvidia_smi_csv_line_high_index() {
        let line = "12, 88, 2048, 24576, 75, 300, 85";
        let stats = parse_nvidia_smi_csv_line(line);
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.device_id, "nvidia12");
        assert_eq!(stats.utilization_pct, Some(88));
    }

    #[test]
    fn test_nvidia_device_id_format() {
        let line = "0, 10, 100, 200, 30, 50, 60";
        let stats = parse_nvidia_smi_csv_line(line).unwrap();
        assert_eq!(stats.device_id, "nvidia0");

        let line = "12, 10, 100, 200, 30, 50, 60";
        let stats = parse_nvidia_smi_csv_line(line).unwrap();
        assert_eq!(stats.device_id, "nvidia12");
    }

    #[test]
    fn test_parse_nvidia_smi_csv_malformed() {
        // Too few fields
        assert!(parse_nvidia_smi_csv_line("0, 45").is_none());
        // Empty line
        assert!(parse_nvidia_smi_csv_line("").is_none());
        // Non-numeric fields
        assert!(parse_nvidia_smi_csv_line("abc, 45, 100, 200, 30, 50, 60").is_none());
        // Extra fields
        assert!(parse_nvidia_smi_csv_line("0, 45, 100, 200, 30, 50, 60, 99").is_none());
    }

    #[test]
    fn test_aggregate_utilization_mean() {
        let devices = vec![
            build_test_device("nvidia0", Some(50)),
            build_test_device("nvidia1", Some(60)),
            build_test_device("nvidia2", Some(70)),
            build_test_device("nvidia3", Some(80)),
        ];
        assert_eq!(aggregate_utilization_mean(&devices), Some(65));
    }

    #[test]
    fn test_aggregate_utilization_empty() {
        let devices: Vec<GpuDeviceStats> = vec![];
        assert_eq!(aggregate_utilization_mean(&devices), None);
    }

    #[test]
    fn test_aggregate_utilization_single() {
        let devices = vec![build_test_device("nvidia0", Some(42))];
        assert_eq!(aggregate_utilization_mean(&devices), Some(42));
    }

    #[test]
    fn test_aggregate_utilization_with_none() {
        let devices = vec![
            build_test_device("nvidia0", Some(80)),
            build_test_device("nvidia1", None),
        ];
        assert_eq!(aggregate_utilization_mean(&devices), Some(80));
    }

    #[test]
    fn test_aggregate_vram_sum() {
        let devices = vec![
            build_test_device_with_vram("nvidia0", Some(4096), Some(8192)),
            build_test_device_with_vram("nvidia1", Some(8192), Some(16384)),
        ];
        let sum = aggregate_vram_sum(&devices);
        assert!(sum.is_some());
        let vram = sum.unwrap();
        assert_eq!(vram.used_mib, 12288); // 4096 + 8192
        assert_eq!(vram.total_mib, 24576); // 8192 + 16384
    }

    #[test]
    fn test_aggregate_vram_empty() {
        let devices: Vec<GpuDeviceStats> = vec![];
        assert_eq!(aggregate_vram_sum(&devices), None);
    }

    #[test]
    fn test_aggregate_vram_one_none() {
        let devices = vec![
            build_test_device_with_vram("nvidia0", Some(2048), Some(4096)),
            build_test_device("nvidia1", Some(30)),
        ];
        let sum = aggregate_vram_sum(&devices);
        assert!(sum.is_some());
        let vram = sum.unwrap();
        assert_eq!(vram.used_mib, 2048);
        assert_eq!(vram.total_mib, 4096);
    }

    // ── test helpers ────────────────────────────────────────────────────

    fn build_test_device(device_id: &str, util: Option<u8>) -> GpuDeviceStats {
        GpuDeviceStats {
            device_id: device_id.to_string(),
            vendor: "nvidia".to_string(),
            utilization_pct: util,
            vram: None,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
        }
    }

    fn build_test_device_with_vram(
        device_id: &str,
        used: Option<u64>,
        total: Option<u64>,
    ) -> GpuDeviceStats {
        let vram = match (used, total) {
            (Some(u), Some(t)) => Some(VramInfo {
                used_mib: u,
                total_mib: t,
            }),
            _ => None,
        };
        GpuDeviceStats {
            device_id: device_id.to_string(),
            vendor: "nvidia".to_string(),
            utilization_pct: None,
            vram,
            temperature_c: None,
            power_w: None,
            fan_pct: None,
        }
    }
}
