use sysinfo::System;

use super::amd::query_amd_devices;
use super::nvidia::query_nvidia_devices;
use super::types::{GpuDeviceStats, SystemMetrics};
use super::vram::VramInfo;

/// Sort devices by (vendor, device_index) and assign position-based IDs (GPU0, GPU1, ...).
/// Extracted so it can be tested without subprocesses.
pub(super) fn assign_position_ids(gpus: &mut [GpuDeviceStats]) {
    gpus.sort_by(|a, b| {
        let extract_index = |id: &str| {
            let numeric_part: String = id.chars().skip_while(|c| c.is_alphabetic()).collect();
            numeric_part.parse::<usize>().unwrap_or(usize::MAX)
        };
        a.vendor
            .cmp(&b.vendor)
            .then_with(|| extract_index(&a.device_id).cmp(&extract_index(&b.device_id)))
    });
    for (i, gpu) in gpus.iter_mut().enumerate() {
        gpu.device_id = format!("GPU{i}");
    }
}

/// Detect all GPU devices and assign position-based IDs (GPU0, GPU1, ...).
/// Returns devices sorted by (vendor, device_index) with UUIDs populated.
/// Blocks on subprocesses (nvidia-smi, sysfs reads); call via `tokio::task::spawn_blocking`.
pub fn detect_gpu_devices() -> Vec<GpuDeviceStats> {
    let mut gpus = query_nvidia_devices();
    gpus.extend(query_amd_devices());
    assign_position_ids(&mut gpus);
    gpus
}

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
    let gpus = detect_gpu_devices();

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

/// Compute the mean utilization across all GPU devices.
/// Only counts devices with a non-None utilization.
pub(super) fn aggregate_utilization_mean(devices: &[GpuDeviceStats]) -> Option<u8> {
    let values: Vec<u8> = devices.iter().filter_map(|d| d.utilization_pct).collect();
    if values.is_empty() {
        return None;
    }
    let sum: u32 = values.iter().map(|&v| v as u32).sum();
    Some((sum / values.len() as u32) as u8)
}

/// Sum VRAM across all GPU devices.
/// Only sums devices with non-None vram.
pub(super) fn aggregate_vram_sum(devices: &[GpuDeviceStats]) -> Option<VramInfo> {
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
